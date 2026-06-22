// Copyright 2026 Appbird LLC. Licensed under "GNU Affero General Public License v3.0"

//! The JSON control-plane backend and the `Control` facade (overview in README.md,
//! "Control plane storage"). State is small encrypted files on a [`ControlStore`],
//! mutated via CAS read-modify-write ([`JsonBackend::mutate_doc`]).
#![allow(dead_code)]

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::crypto::Crypto;
use super::store::{CasResult, ControlStore, Version};

/// Data-schema version stamped into every envelope (drives migrate-on-read).
const SCHEMA_VERSION: u32 = 1;
/// CAS retry ceiling — a safety bound; real contention is a few writes/day.
const MAX_CAS_RETRIES: usize = 16;

/// A decrypted single-document snapshot held for `cache_ttl`. `value` is the
/// plaintext bytes, or `None` when the document is absent (negative cache).
struct CacheEntry {
    value: Option<Vec<u8>>,
    fetched: Instant,
}

/// Short TTL for the hot single-document read cache (JWT auth, env checks,
/// settings). `HELIOS_CONTROL_CACHE_TTL_SECS` overrides; `0` disables.
fn control_cache_ttl() -> Duration {
    let secs = std::env::var("HELIOS_CONTROL_CACHE_TTL_SECS")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(10);
    Duration::from_secs(secs)
}

/// The concrete JSON backend: a versioned object store plus the envelope cipher.
pub struct JsonBackend {
    store: Arc<dyn ControlStore>,
    crypto: Arc<Crypto>,
    /// TTL'd snapshots of the hot single-document keys, sparing shared-store round-trips.
    cache: Mutex<HashMap<String, CacheEntry>>,
    cache_ttl: Duration,
}

impl JsonBackend {
    pub fn new(store: Arc<dyn ControlStore>, crypto: Arc<Crypto>) -> Self {
        Self {
            store,
            crypto,
            cache: Mutex::new(HashMap::new()),
            cache_ttl: control_cache_ttl(),
        }
    }

    /// Fresh cached plaintext for `key`, if any. Outer `None` = no usable entry
    /// (absent or expired); inner `None` = the doc is known-absent.
    fn cache_get(&self, key: &str) -> Option<Option<Vec<u8>>> {
        if self.cache_ttl.is_zero() {
            return None;
        }
        let cache = self.cache.lock().unwrap();
        let entry = cache.get(key)?;
        (entry.fetched.elapsed() < self.cache_ttl).then(|| entry.value.clone())
    }

    /// Record a fresh snapshot for `key`.
    fn cache_store(&self, key: &str, value: Option<Vec<u8>>) {
        if self.cache_ttl.is_zero() {
            return;
        }
        self.cache.lock().unwrap().insert(
            key.to_string(),
            CacheEntry {
                value,
                fetched: Instant::now(),
            },
        );
    }

    /// Write-through after a mutation: refresh an *already-cached* doc so this
    /// node reads its own writes. No-op for uncached keys.
    fn cache_refresh_if_present(&self, key: &str, value: Option<Vec<u8>>) {
        let mut cache = self.cache.lock().unwrap();
        if let Some(entry) = cache.get_mut(key) {
            entry.value = value;
            entry.fetched = Instant::now();
        }
    }

    /// Read + decrypt + deserialize one document, with its store version.
    async fn read_doc<T: DeserializeOwned>(&self, key: &str) -> Result<Option<(T, Version)>> {
        match self.store.get_versioned(key).await? {
            None => Ok(None),
            Some((bytes, ver)) => {
                let (plain, _sv) = self.crypto.open(&bytes)?;
                let val = serde_json::from_slice(&plain)
                    .with_context(|| format!("control: parsing {key}"))?;
                Ok(Some((val, ver)))
            }
        }
    }

    /// Read a document, or `T::default()` when it doesn't exist yet (used for
    /// single-document collections that start empty).
    async fn read_or_default<T: DeserializeOwned + Default>(&self, key: &str) -> Result<T> {
        Ok(self
            .read_doc::<T>(key)
            .await?
            .map(|(v, _)| v)
            .unwrap_or_default())
    }

    /// Like [`read_or_default`], but TTL-cached. Only for hot single-document
    /// reads where seconds of cross-node staleness is acceptable; writes go through.
    async fn cached_read_or_default<T: DeserializeOwned + Default>(&self, key: &str) -> Result<T> {
        if let Some(cached) = self.cache_get(key) {
            return match cached {
                Some(plain) => serde_json::from_slice(&plain)
                    .with_context(|| format!("control: parsing cached {key}")),
                None => Ok(T::default()),
            };
        }
        let value = match self.store.get_versioned(key).await? {
            Some((bytes, _ver)) => Some(self.crypto.open(&bytes)?.0),
            None => None,
        };
        let out = match &value {
            Some(plain) => {
                serde_json::from_slice(plain).with_context(|| format!("control: parsing {key}"))?
            }
            None => T::default(),
        };
        self.cache_store(key, value);
        Ok(out)
    }

    /// CAS read-modify-write. `f` maps current value to `(next, result)` and may
    /// re-run on conflict, so it must be side-effect-free (precompute ids/timestamps).
    async fn mutate_doc<T, R, F>(&self, key: &str, f: F) -> Result<R>
    where
        T: Serialize + DeserializeOwned,
        F: Fn(Option<T>) -> Result<(T, R)>,
    {
        for _ in 0..MAX_CAS_RETRIES {
            let (expected, current) = match self.read_doc::<T>(key).await? {
                Some((v, ver)) => (Some(ver), Some(v)),
                None => (None, None),
            };
            let (next, result) = f(current)?;
            let plain = serde_json::to_vec(&next)?;
            let sealed = self.crypto.seal(&plain, SCHEMA_VERSION)?;
            match self
                .store
                .put_if_version(key, &sealed, expected.as_ref())
                .await?
            {
                CasResult::Written(_) => {
                    self.cache_refresh_if_present(key, Some(plain));
                    return Ok(result);
                }
                CasResult::Conflict => continue,
            }
        }
        bail!("control: write contention on {key} — exceeded {MAX_CAS_RETRIES} retries")
    }

    /// Like [`mutate_doc`], but `f` returns `None` to abort without writing
    /// (`Ok(None)`). Used for best-effort election: don't clobber another's fresh lease.
    async fn try_mutate_doc<T, R, F>(&self, key: &str, f: F) -> Result<Option<R>>
    where
        T: Serialize + DeserializeOwned,
        F: Fn(Option<T>) -> Result<Option<(T, R)>>,
    {
        for _ in 0..MAX_CAS_RETRIES {
            let (expected, current) = match self.read_doc::<T>(key).await? {
                Some((v, ver)) => (Some(ver), Some(v)),
                None => (None, None),
            };
            let Some((next, result)) = f(current)? else {
                return Ok(None);
            };
            let plain = serde_json::to_vec(&next)?;
            let sealed = self.crypto.seal(&plain, SCHEMA_VERSION)?;
            match self
                .store
                .put_if_version(key, &sealed, expected.as_ref())
                .await?
            {
                CasResult::Written(_) => {
                    self.cache_refresh_if_present(key, Some(plain));
                    return Ok(Some(result));
                }
                CasResult::Conflict => continue,
            }
        }
        bail!("control: write contention on {key} — exceeded {MAX_CAS_RETRIES} retries")
    }

    /// Read one document, dropping the version (most callers don't need it).
    async fn read_entity<T: DeserializeOwned>(&self, key: &str) -> Result<Option<T>> {
        Ok(self.read_doc::<T>(key).await?.map(|(v, _)| v))
    }

    /// Store-relative keys under `prefix` (no decrypt), for paging by key. Only
    /// `.json` entity files are returned — stray files never reach the decryptor.
    async fn list_keys(&self, prefix: &str) -> Result<Vec<String>> {
        let mut keys = self.store.list(prefix).await?;
        keys.retain(|k| k.ends_with(".json"));
        Ok(keys)
    }

    /// Read every document under `prefix`, skipping any that fail to decrypt/parse
    /// (a corrupt entity shouldn't sink the list). Store key order; callers sort.
    async fn list_entities<T: DeserializeOwned>(&self, prefix: &str) -> Result<Vec<T>> {
        let keys = self.list_keys(prefix).await?;
        let mut out = Vec::with_capacity(keys.len());
        for key in keys {
            match self.read_doc::<T>(&key).await {
                Ok(Some((v, _))) => out.push(v),
                Ok(None) => {}
                Err(e) => {
                    tracing::warn!(key = %key, error = %e, "control: skipping unreadable entity")
                }
            }
        }
        Ok(out)
    }

    async fn delete(&self, key: &str) -> Result<()> {
        self.store.delete(key).await?;
        self.cache_refresh_if_present(key, None);
        Ok(())
    }

    /// Write a per-entity document unconditionally (create-or-replace), where the
    /// caller already holds authority to overwrite (e.g. a fresh non-colliding id).
    async fn put_doc<T: Serialize>(&self, key: &str, value: &T) -> Result<()> {
        // Loop to absorb the (vanishingly rare) race on the same fresh key.
        for _ in 0..MAX_CAS_RETRIES {
            let expected = self.store.get_versioned(key).await?.map(|(_, v)| v);
            let plain = serde_json::to_vec(value)?;
            let sealed = self.crypto.seal(&plain, SCHEMA_VERSION)?;
            match self
                .store
                .put_if_version(key, &sealed, expected.as_ref())
                .await?
            {
                CasResult::Written(_) => {
                    self.cache_refresh_if_present(key, Some(plain));
                    return Ok(());
                }
                CasResult::Conflict => continue,
            }
        }
        bail!("control: write contention on {key} — exceeded {MAX_CAS_RETRIES} retries")
    }
}

/// Handle to the control plane. Cheap to clone — shares one backend.
#[derive(Clone)]
pub struct Control {
    backend: Arc<JsonBackend>,
}

impl Control {
    pub fn new(store: Arc<dyn ControlStore>, crypto: Arc<Crypto>) -> Self {
        Self {
            backend: Arc::new(JsonBackend::new(store, crypto)),
        }
    }
}

// Users (single-document: `users.json`) — RBAC allowlist folded in (§3).

use super::settings::EnvIndexAllow;
use super::users::{generate_password, hash_password, verify_password, User};

/// Storage shape for one user — the API-safe [`User`] plus the secrets and
/// access grants that never leave the backend.
#[derive(Clone, Serialize, serde::Deserialize)]
struct StoredUser {
    id: String,
    userid: String,
    email: String,
    display_name: String,
    password_hash: String,
    is_admin: bool,
    created_at: String,
    #[serde(default = "one")]
    credentials_version: u32,
    /// Env/index allowlist; empty = unrestricted (non-admin), admins bypass.
    #[serde(default)]
    allowed: Vec<EnvIndexAllow>,
    /// Per-account display preferences.
    #[serde(default)]
    timezone: Option<String>,
    #[serde(default)]
    theme: Option<String>,
    #[serde(default)]
    palette: Option<String>,
}

fn one() -> u32 {
    1
}

impl StoredUser {
    fn to_api(&self) -> User {
        User {
            id: self.id.clone(),
            userid: self.userid.clone(),
            email: self.email.clone(),
            display_name: self.display_name.clone(),
            is_admin: self.is_admin,
            created_at: self.created_at.clone(),
            timezone: self.timezone.clone(),
            theme: self.theme.clone(),
            palette: self.palette.clone(),
            credentials_version: self.credentials_version,
        }
    }
}

/// `users.json` document.
#[derive(Default, Serialize, serde::Deserialize)]
struct UsersDoc {
    users: Vec<StoredUser>,
}

const USERS_KEY: &str = "users.json";

impl Control {
    /// Number of user accounts. Drives the first-run admin bootstrap.
    pub async fn user_count(&self) -> Result<i64> {
        let doc: UsersDoc = self.backend.read_or_default(USERS_KEY).await?;
        Ok(doc.users.len() as i64)
    }

    /// Creates a user with a hashed password. `userid`/`email` are
    /// unique case-insensitively (in-document scan under CAS).
    pub async fn create_user(
        &self,
        userid: &str,
        email: &str,
        display_name: &str,
        password: &str,
        is_admin: bool,
    ) -> Result<User> {
        let userid = userid.trim().to_string();
        let email = email.trim().to_string();
        let display_name = display_name.trim().to_string();
        if userid.is_empty() || email.is_empty() || display_name.is_empty() {
            bail!("userid, email and display_name are required");
        }
        // Hash once, outside the CAS closure (it may re-run on conflict).
        let password_hash = hash_password(password)?;
        let stored = StoredUser {
            id: new_id("usr"),
            userid,
            email,
            display_name,
            password_hash,
            is_admin,
            created_at: chrono::Utc::now().to_rfc3339(),
            credentials_version: 1,
            allowed: Vec::new(),
            timezone: None,
            theme: None,
            palette: None,
        };
        self.backend
            .mutate_doc::<UsersDoc, User, _>(USERS_KEY, |doc| {
                let mut doc = doc.unwrap_or_default();
                if doc.users.iter().any(|u| {
                    u.userid.eq_ignore_ascii_case(&stored.userid)
                        || u.email.eq_ignore_ascii_case(&stored.email)
                }) {
                    bail!("a user with that userid or email already exists");
                }
                let api = stored.to_api();
                doc.users.push(stored.clone());
                Ok((doc, api))
            })
            .await
    }

    /// All users, oldest first (the bootstrap admin sorts to the top).
    pub async fn list_users(&self) -> Result<Vec<User>> {
        let doc: UsersDoc = self.backend.read_or_default(USERS_KEY).await?;
        let mut users: Vec<User> = doc.users.iter().map(StoredUser::to_api).collect();
        users.sort_by(|a, b| a.created_at.cmp(&b.created_at));
        Ok(users)
    }

    pub async fn get_user_by_id(&self, id: &str) -> Result<Option<User>> {
        let doc: UsersDoc = self.backend.cached_read_or_default(USERS_KEY).await?;
        Ok(doc
            .users
            .iter()
            .find(|u| u.id == id)
            .map(StoredUser::to_api))
    }

    /// Verifies `password` against the user matched by userid or email
    /// (case-insensitive); `None` on any failure.
    pub async fn authenticate(
        &self,
        userid_or_email: &str,
        password: &str,
    ) -> Result<Option<User>> {
        let doc: UsersDoc = self.backend.read_or_default(USERS_KEY).await?;
        let Some(u) = doc.users.iter().find(|u| {
            u.userid.eq_ignore_ascii_case(userid_or_email)
                || u.email.eq_ignore_ascii_case(userid_or_email)
        }) else {
            return Ok(None);
        };
        if !verify_password(password, &u.password_hash) {
            return Ok(None);
        }
        Ok(Some(u.to_api()))
    }

    /// Replaces a user's password. Tokens are left valid — the caller bumps
    /// `credentials_version` to revoke.
    pub async fn set_password(&self, user_id: &str, new_password: &str) -> Result<()> {
        let hash = hash_password(new_password)?;
        self.backend
            .mutate_doc::<UsersDoc, (), _>(USERS_KEY, |doc| {
                let mut doc = doc.unwrap_or_default();
                let u = doc
                    .users
                    .iter_mut()
                    .find(|u| u.id == user_id)
                    .ok_or_else(|| anyhow::anyhow!("user {user_id} not found"))?;
                u.password_hash = hash.clone();
                Ok((doc, ()))
            })
            .await
    }

    /// Admin lockout recovery: new random password + token revocation; returns
    /// the plaintext to hand over.
    pub async fn regenerate_password(&self, user_id: &str) -> Result<String> {
        let new = generate_password();
        let hash = hash_password(&new)?;
        self.backend
            .mutate_doc::<UsersDoc, (), _>(USERS_KEY, |doc| {
                let mut doc = doc.unwrap_or_default();
                let u = doc
                    .users
                    .iter_mut()
                    .find(|u| u.id == user_id)
                    .ok_or_else(|| anyhow::anyhow!("user {user_id} not found"))?;
                u.password_hash = hash.clone();
                u.credentials_version += 1;
                Ok((doc, ()))
            })
            .await?;
        Ok(new)
    }

    /// Increments `credentials_version`, invalidating every token the user
    /// holds. Returns the new value.
    pub async fn bump_credentials_version(&self, user_id: &str) -> Result<u32> {
        self.backend
            .mutate_doc::<UsersDoc, u32, _>(USERS_KEY, |doc| {
                let mut doc = doc.unwrap_or_default();
                let u = doc
                    .users
                    .iter_mut()
                    .find(|u| u.id == user_id)
                    .ok_or_else(|| anyhow::anyhow!("user {user_id} not found"))?;
                u.credentials_version += 1;
                let v = u.credentials_version;
                Ok((doc, v))
            })
            .await
    }

    /// Patch editable identity fields (`userid` is the immutable login key).
    /// `None` untouched; empty rejected; email stays unique case-insensitively.
    pub async fn update_user(
        &self,
        user_id: &str,
        email: Option<&str>,
        display_name: Option<&str>,
        is_admin: Option<bool>,
    ) -> Result<User> {
        let email = email.map(|e| e.trim().to_string());
        let display_name = display_name.map(|d| d.trim().to_string());
        if matches!(email.as_deref(), Some("")) {
            bail!("email cannot be empty");
        }
        if matches!(display_name.as_deref(), Some("")) {
            bail!("display_name cannot be empty");
        }
        self.backend
            .mutate_doc::<UsersDoc, User, _>(USERS_KEY, |doc| {
                let mut doc = doc.unwrap_or_default();
                if !doc.users.iter().any(|u| u.id == user_id) {
                    bail!("user {user_id} not found");
                }
                if let Some(e) = &email {
                    if doc
                        .users
                        .iter()
                        .any(|u| u.id != user_id && u.email.eq_ignore_ascii_case(e))
                    {
                        bail!("a user with that email already exists");
                    }
                }
                let u = doc.users.iter_mut().find(|u| u.id == user_id).unwrap();
                if let Some(e) = &email {
                    u.email = e.clone();
                }
                if let Some(d) = &display_name {
                    u.display_name = d.clone();
                }
                if let Some(a) = is_admin {
                    u.is_admin = a;
                }
                let api = u.to_api();
                Ok((doc, api))
            })
            .await
    }

    /// Self-service display-preference update. `None` leaves a field
    /// unchanged; an empty string clears it (fall back to the instance
    /// default). Returns the updated record.
    pub async fn set_user_preferences(
        &self,
        user_id: &str,
        timezone: Option<&str>,
        theme: Option<&str>,
        palette: Option<&str>,
    ) -> Result<User> {
        let timezone = timezone.map(|t| t.trim().to_string());
        let theme = theme.map(|t| t.trim().to_string());
        let palette = palette.map(|p| p.trim().to_string());
        if let Some(t) = &theme {
            if !t.is_empty() && t != "light" && t != "dark" {
                bail!("theme must be 'light' or 'dark'");
            }
        }
        if let Some(p) = &palette {
            if !p.is_empty() && !crate::control::settings::THEME_PALETTES.contains(&p.as_str()) {
                bail!(
                    "palette must be one of: {}",
                    crate::control::settings::THEME_PALETTES.join(", ")
                );
            }
        }
        self.backend
            .mutate_doc::<UsersDoc, User, _>(USERS_KEY, |doc| {
                let mut doc = doc.unwrap_or_default();
                let u = doc
                    .users
                    .iter_mut()
                    .find(|u| u.id == user_id)
                    .ok_or_else(|| anyhow::anyhow!("user {user_id} not found"))?;
                if let Some(tz) = &timezone {
                    u.timezone = (!tz.is_empty()).then(|| tz.clone());
                }
                if let Some(th) = &theme {
                    u.theme = (!th.is_empty()).then(|| th.clone());
                }
                if let Some(p) = &palette {
                    u.palette = (!p.is_empty()).then(|| p.clone());
                }
                let api = u.to_api();
                Ok((doc, api))
            })
            .await
    }

    /// Deletes a user. Returns `true` if a record was removed.
    pub async fn delete_user(&self, user_id: &str) -> Result<bool> {
        self.backend
            .mutate_doc::<UsersDoc, bool, _>(USERS_KEY, |doc| {
                let mut doc = doc.unwrap_or_default();
                let before = doc.users.len();
                doc.users.retain(|u| u.id != user_id);
                let removed = doc.users.len() != before;
                Ok((doc, removed))
            })
            .await
    }

    // ---- per-user env/index RBAC (folded into the user record) -------------

    /// The user's env-aware allowlist, sorted by env. Empty = unrestricted.
    /// Admins bypass — callers special-case `is_admin`.
    pub async fn user_allowed(&self, user_id: &str) -> Result<Vec<EnvIndexAllow>> {
        let doc: UsersDoc = self.backend.read_or_default(USERS_KEY).await?;
        let mut allowed = doc
            .users
            .iter()
            .find(|u| u.id == user_id)
            .map(|u| u.allowed.clone())
            .unwrap_or_default();
        allowed.sort_by(|a, b| a.env.cmp(&b.env));
        Ok(allowed)
    }

    /// True iff the user has any rule mentioning `env`.
    pub async fn user_has_env(&self, user_id: &str, env: &str) -> Result<bool> {
        Ok(self
            .user_allowed(user_id)
            .await?
            .iter()
            .any(|r| r.env == env))
    }

    /// Replaces the user's allowlist. Empty/blank rules are dropped.
    pub async fn set_user_allowed(&self, user_id: &str, rules: &[EnvIndexAllow]) -> Result<()> {
        let cleaned: Vec<EnvIndexAllow> = rules
            .iter()
            .filter(|r| !r.env.trim().is_empty())
            .filter_map(|r| {
                let indexes: Vec<String> = r
                    .indexes
                    .iter()
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .collect();
                (!indexes.is_empty()).then(|| EnvIndexAllow {
                    env: r.env.clone(),
                    indexes,
                })
            })
            .collect();
        self.backend
            .mutate_doc::<UsersDoc, (), _>(USERS_KEY, |doc| {
                let mut doc = doc.unwrap_or_default();
                let u = doc
                    .users
                    .iter_mut()
                    .find(|u| u.id == user_id)
                    .ok_or_else(|| anyhow::anyhow!("user {user_id} not found"))?;
                u.allowed = cleaned.clone();
                Ok((doc, ()))
            })
            .await
    }
}

// Envs (single-document: `envs.json`).

use super::envs::EnvRow;
use super::settings::KEY_DEFAULT_ENV;
use crate::catalog::{valid_env_name, DEFAULT_ENV, SYSTEM_ENV};

#[derive(Default, Serialize, Deserialize)]
struct EnvsDoc {
    #[serde(default)]
    envs: Vec<EnvRow>,
}

const ENVS_KEY: &str = "envs.json";

/// Next display position: one past the current max (so new envs land last).
fn next_order_index(envs: &[EnvRow]) -> i64 {
    envs.iter()
        .map(|e| e.order_index)
        .max()
        .map_or(0, |m| m + 1)
}

impl Control {
    /// Insert `name` if absent (no-op if it already exists).
    pub async fn upsert_env(&self, name: &str, is_system: bool) -> Result<()> {
        let name = name.to_string();
        let created_at = chrono::Utc::now().to_rfc3339();
        self.backend
            .mutate_doc::<EnvsDoc, (), _>(ENVS_KEY, |doc| {
                let mut doc = doc.unwrap_or_default();
                if !doc.envs.iter().any(|e| e.name.eq_ignore_ascii_case(&name)) {
                    let order_index = next_order_index(&doc.envs);
                    doc.envs.push(EnvRow {
                        name: name.clone(),
                        is_system,
                        created_at: created_at.clone(),
                        retention_days: None,
                        order_index,
                    });
                }
                Ok((doc, ()))
            })
            .await
    }

    /// Set or clear (`None`) an env's retention override. Returns the updated row.
    pub async fn set_env_retention(&self, name: &str, days: Option<i64>) -> Result<EnvRow> {
        let name = name.to_string();
        let days = days.filter(|&d| d > 0);
        self.backend
            .mutate_doc::<EnvsDoc, EnvRow, _>(ENVS_KEY, |doc| {
                let mut doc = doc.unwrap_or_default();
                let row = doc
                    .envs
                    .iter_mut()
                    .find(|e| e.name.eq_ignore_ascii_case(&name))
                    .ok_or_else(|| anyhow::anyhow!("env '{name}' does not exist"))?;
                row.retention_days = days;
                let ret = row.clone();
                Ok((doc, ret))
            })
            .await
    }

    /// Admin-driven env creation: validate, reject `_*` (reserved), reject dups.
    pub async fn create_env(&self, name: &str) -> Result<EnvRow> {
        let name = name.trim().to_string();
        if !valid_env_name(&name) {
            bail!(
                "invalid environment name {name:?}: an env becomes a folder on disk, so use \
                 only letters, digits, '-' and '_' — no spaces, slashes, dots, or other \
                 punctuation"
            );
        }
        if name.starts_with('_') {
            bail!("env names starting with '_' are reserved for the system");
        }
        let created_at = chrono::Utc::now().to_rfc3339();
        self.backend
            .mutate_doc::<EnvsDoc, EnvRow, _>(ENVS_KEY, |doc| {
                let mut doc = doc.unwrap_or_default();
                if doc.envs.iter().any(|e| e.name.eq_ignore_ascii_case(&name)) {
                    bail!("env '{name}' already exists");
                }
                let row = EnvRow {
                    name: name.clone(),
                    is_system: false,
                    created_at: created_at.clone(),
                    retention_days: None,
                    order_index: next_order_index(&doc.envs),
                };
                doc.envs.push(row.clone());
                Ok((doc, row))
            })
            .await
    }

    /// Lists every env, optionally including reserved system envs. System envs
    /// sort last; user envs follow the admin-set `order_index` (name as tiebreak).
    pub async fn list_envs(&self, include_system: bool) -> Result<Vec<EnvRow>> {
        let doc: EnvsDoc = self.backend.read_or_default(ENVS_KEY).await?;
        let mut envs: Vec<EnvRow> = doc
            .envs
            .into_iter()
            .filter(|e| include_system || !e.is_system)
            .collect();
        envs.sort_by(|a, b| {
            (a.is_system, a.order_index, &a.name).cmp(&(b.is_system, b.order_index, &b.name))
        });
        Ok(envs)
    }

    /// Rewrites env display order from `names` (ascending). Every name must exist;
    /// envs not listed keep their relative order after the listed ones.
    pub async fn reorder_envs(&self, names: &[String]) -> Result<Vec<EnvRow>> {
        let names: Vec<String> = names.iter().map(|s| s.trim().to_string()).collect();
        self.backend
            .mutate_doc::<EnvsDoc, Vec<EnvRow>, _>(ENVS_KEY, |doc| {
                let mut doc = doc.unwrap_or_default();
                for n in &names {
                    if !doc.envs.iter().any(|e| e.name.eq_ignore_ascii_case(n)) {
                        bail!("env '{n}' does not exist");
                    }
                }
                let listed: std::collections::HashMap<String, i64> = names
                    .iter()
                    .enumerate()
                    .map(|(i, n)| (n.to_ascii_lowercase(), i as i64))
                    .collect();
                // Unlisted envs sort after the listed ones, preserving current order.
                let mut rest: Vec<(i64, String)> = doc
                    .envs
                    .iter()
                    .filter(|e| !listed.contains_key(&e.name.to_ascii_lowercase()))
                    .map(|e| (e.order_index, e.name.to_ascii_lowercase()))
                    .collect();
                rest.sort();
                let base = names.len() as i64;
                let rest_pos: std::collections::HashMap<String, i64> = rest
                    .into_iter()
                    .enumerate()
                    .map(|(k, (_, n))| (n, base + k as i64))
                    .collect();
                for e in doc.envs.iter_mut() {
                    let key = e.name.to_ascii_lowercase();
                    if let Some(&i) = listed.get(&key) {
                        e.order_index = i;
                    } else if let Some(&i) = rest_pos.get(&key) {
                        e.order_index = i;
                    }
                }
                let out = doc.envs.clone();
                Ok((doc, out))
            })
            .await
    }

    pub async fn env_exists(&self, name: &str) -> Result<bool> {
        let doc: EnvsDoc = self.backend.cached_read_or_default(ENVS_KEY).await?;
        Ok(doc.envs.iter().any(|e| e.name.eq_ignore_ascii_case(name)))
    }

    /// Removes `name` unless reserved/system or still pinned by saved searches.
    /// The live-partition check stays at the HTTP layer (catalog in scope there).
    pub async fn delete_env_if_no_control_rows(&self, name: &str) -> Result<()> {
        if name == DEFAULT_ENV || name == SYSTEM_ENV {
            bail!("cannot delete reserved env '{name}'");
        }
        let doc: EnvsDoc = self.backend.read_or_default(ENVS_KEY).await?;
        match doc.envs.iter().find(|e| e.name == name) {
            None => bail!("env '{name}' does not exist"),
            Some(e) if e.is_system => bail!("cannot delete system env '{name}'"),
            _ => {}
        }
        let saved: Vec<StoredSaved> = self.backend.list_entities(SAVED_PREFIX).await?;
        let count = saved.iter().filter(|s| s.search.env == name).count();
        if count > 0 {
            bail!(
                "env '{name}' has {count} saved searches pinned to it — \
                 move or delete them first"
            );
        }
        self.backend
            .mutate_doc::<EnvsDoc, (), _>(ENVS_KEY, |doc| {
                let mut doc = doc.unwrap_or_default();
                doc.envs.retain(|e| e.name != name);
                Ok((doc, ()))
            })
            .await
    }

    /// The admin-configured login default env, or `None` if unset or pointing at
    /// an env that no longer exists (callers fall back to `DEFAULT_ENV`).
    pub async fn default_env(&self) -> Result<Option<String>> {
        let Some(name) = self
            .get_setting(KEY_DEFAULT_ENV)
            .await?
            .filter(|s| !s.trim().is_empty())
        else {
            return Ok(None);
        };
        Ok(self.env_exists(&name).await?.then_some(name))
    }

    /// Sets the login default env. The env must exist and be a user env (`_*`
    /// system envs are admin-only and never auto-selected).
    pub async fn set_default_env(&self, name: &str) -> Result<String> {
        let name = name.trim().to_string();
        if name.starts_with('_') {
            bail!("system env '{name}' cannot be the default for new users");
        }
        if !self.env_exists(&name).await? {
            bail!("env '{name}' does not exist");
        }
        self.set_setting(KEY_DEFAULT_ENV, &name).await?;
        Ok(name)
    }

    /// Clears the login default env (new users fall back to `default`).
    pub async fn clear_default_env(&self) -> Result<()> {
        self.unset_setting(KEY_DEFAULT_ENV).await
    }
}

// Settings (single-document: `settings.json` KV map) + typed MCP view.

use super::settings::{
    parse_allowed, parse_csv, parse_syslog_routes, McpSettings, SyslogSettings,
    KEY_MCP_ALLOWED_INDEXES, KEY_MCP_ENABLED, KEY_MCP_ENABLED_TOOLS, KEY_SAML_ACS_URL,
    KEY_SAML_BUTTON_LABEL, KEY_SAML_EMAIL_ATTR, KEY_SAML_ENABLED, KEY_SAML_IDP_CERT,
    KEY_SAML_IDP_ENTITY_ID, KEY_SAML_IDP_SSO_URL, KEY_SAML_LOCAL_LOGIN_DISABLED,
    KEY_SAML_SP_ENTITY_ID, KEY_SYSLOG_BIND, KEY_SYSLOG_DEFAULT_ENV, KEY_SYSLOG_DEFAULT_INDEX,
    KEY_SYSLOG_ENABLED, KEY_SYSLOG_ROUTES, KEY_SYSLOG_TCP_PORT, KEY_SYSLOG_UDP_PORT,
};

#[derive(Default, Serialize, Deserialize)]
struct SettingsDoc {
    #[serde(default)]
    map: std::collections::BTreeMap<String, String>,
}

const SETTINGS_KEY: &str = "settings.json";

impl Control {
    /// Read one setting (`None` if unset — callers apply defaults).
    pub async fn get_setting(&self, key: &str) -> Result<Option<String>> {
        let doc: SettingsDoc = self.backend.cached_read_or_default(SETTINGS_KEY).await?;
        Ok(doc.map.get(key).cloned())
    }

    /// Upsert. Empty string is "set to empty", not "unset".
    pub async fn set_setting(&self, key: &str, value: &str) -> Result<()> {
        let (key, value) = (key.to_string(), value.to_string());
        self.backend
            .mutate_doc::<SettingsDoc, (), _>(SETTINGS_KEY, |doc| {
                let mut doc = doc.unwrap_or_default();
                doc.map.insert(key.clone(), value.clone());
                Ok((doc, ()))
            })
            .await
    }

    pub async fn unset_setting(&self, key: &str) -> Result<()> {
        let key = key.to_string();
        self.backend
            .mutate_doc::<SettingsDoc, (), _>(SETTINGS_KEY, |doc| {
                let mut doc = doc.unwrap_or_default();
                doc.map.remove(&key);
                Ok((doc, ()))
            })
            .await
    }

    /// Typed MCP config, loaded fresh (mirrors the old `McpSettings::load`).
    pub async fn mcp_settings(&self) -> Result<McpSettings> {
        let mut s = McpSettings::default();
        if let Some(v) = self.get_setting(KEY_MCP_ENABLED).await? {
            if let Ok(b) = v.parse::<bool>() {
                s.enabled = b;
            }
        }
        if let Some(v) = self.get_setting(KEY_MCP_ALLOWED_INDEXES).await? {
            s.allowed = parse_allowed(&v);
        }
        if let Some(v) = self.get_setting(KEY_MCP_ENABLED_TOOLS).await? {
            s.enabled_tools = parse_csv(&v, "*");
        }
        Ok(s)
    }

    /// Typed SAML SP config (single trusted IdP), loaded fresh.
    pub async fn saml_settings(&self) -> Result<crate::saml::SamlConfig> {
        let mut s = crate::saml::SamlConfig {
            button_label: crate::saml::SamlConfig::default_button_label(),
            ..Default::default()
        };
        if let Some(v) = self.get_setting(KEY_SAML_ENABLED).await? {
            s.enabled = v.parse::<bool>().unwrap_or(false);
        }
        if let Some(v) = self.get_setting(KEY_SAML_IDP_ENTITY_ID).await? {
            s.idp_entity_id = v;
        }
        if let Some(v) = self.get_setting(KEY_SAML_IDP_SSO_URL).await? {
            s.idp_sso_url = v;
        }
        if let Some(v) = self.get_setting(KEY_SAML_IDP_CERT).await? {
            s.idp_cert_pem = v;
        }
        if let Some(v) = self.get_setting(KEY_SAML_SP_ENTITY_ID).await? {
            s.sp_entity_id = v;
        }
        if let Some(v) = self.get_setting(KEY_SAML_ACS_URL).await? {
            s.acs_url = v;
        }
        if let Some(v) = self.get_setting(KEY_SAML_EMAIL_ATTR).await? {
            let t = v.trim();
            if !t.is_empty() {
                s.email_attr = Some(t.to_string());
            }
        }
        if let Some(v) = self.get_setting(KEY_SAML_BUTTON_LABEL).await? {
            let t = v.trim();
            if !t.is_empty() {
                s.button_label = t.to_string();
            }
        }
        if let Some(v) = self.get_setting(KEY_SAML_LOCAL_LOGIN_DISABLED).await? {
            s.local_login_disabled = v.parse::<bool>().unwrap_or(false);
        }
        Ok(s)
    }

    /// Typed syslog listener config, loaded fresh (mirrors `mcp_settings`). The
    /// supervisor polls this to (re)bind sockets and rebuild the router.
    pub async fn syslog_settings(&self) -> Result<SyslogSettings> {
        let mut s = SyslogSettings::default();
        if let Some(v) = self.get_setting(KEY_SYSLOG_ENABLED).await? {
            s.enabled = v.parse::<bool>().unwrap_or(false);
        }
        if let Some(v) = self.get_setting(KEY_SYSLOG_BIND).await? {
            let t = v.trim();
            if !t.is_empty() {
                s.bind = t.to_string();
            }
        }
        if let Some(v) = self.get_setting(KEY_SYSLOG_UDP_PORT).await? {
            if let Ok(p) = v.trim().parse::<u16>() {
                s.udp_port = p;
            }
        }
        if let Some(v) = self.get_setting(KEY_SYSLOG_TCP_PORT).await? {
            if let Ok(p) = v.trim().parse::<u16>() {
                s.tcp_port = p;
            }
        }
        if let Some(v) = self.get_setting(KEY_SYSLOG_DEFAULT_ENV).await? {
            let t = v.trim();
            if !t.is_empty() {
                s.default_env = t.to_string();
            }
        }
        if let Some(v) = self.get_setting(KEY_SYSLOG_DEFAULT_INDEX).await? {
            let t = v.trim();
            if !t.is_empty() {
                s.default_index = t.to_string();
            }
        }
        if let Some(v) = self.get_setting(KEY_SYSLOG_ROUTES).await? {
            s.routes = parse_syslog_routes(&v);
        }
        Ok(s)
    }

    /// Resolve a user by userid or email (case-insensitive), WITHOUT a password
    /// check — for externally-authenticated flows (SAML). Match-only, never creates.
    pub async fn find_user_by_login(&self, userid_or_email: &str) -> Result<Option<User>> {
        let needle = userid_or_email.trim();
        let doc: UsersDoc = self.backend.cached_read_or_default(USERS_KEY).await?;
        Ok(doc
            .users
            .iter()
            .find(|u| u.email.eq_ignore_ascii_case(needle) || u.userid.eq_ignore_ascii_case(needle))
            .map(StoredUser::to_api))
    }
}

// SAML assertion replay guard (`saml_replay.json`): records consumed assertion
// IDs with expiry so a captured response can't be replayed; pruned on insert.

#[derive(Default, Serialize, Deserialize)]
struct SamlReplayDoc {
    #[serde(default)]
    seen: std::collections::BTreeMap<String, i64>,
}

const SAML_REPLAY_KEY: &str = "saml_replay.json";

impl Control {
    /// Atomically record an assertion ID: `true` = newly recorded (accept),
    /// `false` = already seen (replay). `expires_at`/`now` are epoch seconds.
    pub async fn saml_replay_check_and_record(
        &self,
        assertion_id: &str,
        expires_at: i64,
        now: i64,
    ) -> Result<bool> {
        let id = assertion_id.to_string();
        self.backend
            .mutate_doc::<SamlReplayDoc, bool, _>(SAML_REPLAY_KEY, |doc| {
                let mut doc = doc.unwrap_or_default();
                doc.seen.retain(|_, &mut exp| exp > now); // prune expired
                if doc.seen.contains_key(&id) {
                    return Ok((doc, false));
                }
                doc.seen.insert(id.clone(), expires_at);
                Ok((doc, true))
            })
            .await
    }
}

// Named leases (`<name>_lease.json`): best-effort single-writer election for
// background tasks (compactor, retention sweeper). Correctness is the manifest
// CAS's job; a lease only avoids *duplicated* work.

#[derive(Serialize, Deserialize)]
struct NamedLease {
    holder: String,
    /// RFC3339; compared against the reader's own clock under `ttl`.
    renewed_at: String,
}

fn lease_is_stale(renewed_at: &str, now: chrono::DateTime<chrono::Utc>, ttl: Duration) -> bool {
    match chrono::DateTime::parse_from_rfc3339(renewed_at) {
        Ok(t) => {
            let age = now.signed_duration_since(t.with_timezone(&chrono::Utc));
            age > chrono::Duration::from_std(ttl).unwrap_or_else(|_| chrono::Duration::seconds(90))
        }
        Err(_) => true, // unparseable timestamp → reclaim
    }
}

impl Control {
    /// Acquire/renew the named lease for `node_id` if free, stale, or ours;
    /// returns whether we hold it. Declines (no write) on another's fresh lease.
    pub async fn acquire_named_lease(
        &self,
        name: &str,
        node_id: &str,
        ttl: Duration,
    ) -> Result<bool> {
        let key = format!("{name}_lease.json");
        let now = chrono::Utc::now();
        let node_id = node_id.to_string();
        let held = self
            .backend
            .try_mutate_doc::<NamedLease, (), _>(&key, |cur| {
                let claimable = match &cur {
                    None => true,
                    Some(l) => l.holder == node_id || lease_is_stale(&l.renewed_at, now, ttl),
                };
                if !claimable {
                    return Ok(None); // someone else holds a fresh lease
                }
                Ok(Some((
                    NamedLease {
                        holder: node_id.clone(),
                        renewed_at: now.to_rfc3339(),
                    },
                    (),
                )))
            })
            .await?;
        Ok(held.is_some())
    }

    pub async fn acquire_compactor_lease(&self, node_id: &str, ttl: Duration) -> Result<bool> {
        self.acquire_named_lease("compactor", node_id, ttl).await
    }
}

// Saved searches (per-entity, flat: `saved/<id>.json`). Env-scoped lists.

use super::saved::{SavedSearch, SavedSearchInput, SavedSearchPatch};

/// Stored saved-search: the wire shape plus the owner (the old `user_id`
/// column). Flattened so the file reads as a single object.
#[derive(Serialize, Deserialize)]
struct StoredSaved {
    #[serde(flatten)]
    search: SavedSearch,
    owner_user_id: String,
}

const SAVED_PREFIX: &str = "saved/";

fn saved_key(id: &str) -> String {
    format!("saved/{id}.json")
}

/// Owner display label from the user map, falling back to the raw id for users
/// that no longer exist. Shared by the saved/dashboard/monitor owner columns.
fn label_for(names: &std::collections::HashMap<String, String>, owner_user_id: String) -> String {
    names.get(&owner_user_id).cloned().unwrap_or(owner_user_id)
}

impl Control {
    /// Searches in `env`, newest-updated first, with owner label. `visible_to =
    /// Some(user)` restricts to own + public; `None` returns every row (admin).
    async fn collect_saved(&self, env: &str, visible_to: Option<&str>) -> Result<Vec<SavedSearch>> {
        let names = self.owner_label_map().await?;
        let mut items: Vec<SavedSearch> = self
            .backend
            .list_entities::<StoredSaved>(SAVED_PREFIX)
            .await?
            .into_iter()
            .filter(|s| s.search.env == env)
            .filter(|s| match visible_to {
                None => true,
                Some(uid) => s.owner_user_id == uid || s.search.public,
            })
            .map(|s| {
                let mut search = s.search;
                search.owner = Some(label_for(&names, s.owner_user_id));
                search
            })
            .collect();
        items.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
        Ok(items)
    }

    /// Searches in `env` visible to `user_id`: own rows + every public row.
    /// Most-recently-updated first; owner label attached.
    pub async fn saved_list(&self, user_id: &str, env: &str) -> Result<Vec<SavedSearch>> {
        self.collect_saved(env, Some(user_id)).await
    }

    /// user_id → display label (display name, or userid when blank). Drives the
    /// owner column on every list.
    async fn owner_label_map(&self) -> Result<std::collections::HashMap<String, String>> {
        Ok(self
            .list_users()
            .await?
            .into_iter()
            .map(|u| {
                let label = if u.display_name.trim().is_empty() {
                    u.userid
                } else {
                    u.display_name
                };
                (u.id, label)
            })
            .collect())
    }

    /// Admin-only: every saved search in `env` regardless of owner/visibility,
    /// with the owner label attached. Newest-updated first.
    pub async fn saved_list_all(&self, env: &str) -> Result<Vec<SavedSearch>> {
        self.collect_saved(env, None).await
    }

    pub async fn saved_create(
        &self,
        user_id: &str,
        env: &str,
        input: SavedSearchInput,
    ) -> Result<SavedSearch> {
        if input.name.trim().is_empty() {
            bail!("name is required");
        }
        let now = chrono::Utc::now().to_rfc3339();
        let s = SavedSearch {
            id: new_id("ss"),
            name: input.name.trim().to_string(),
            q: input.q,
            index: input.index.filter(|s| !s.is_empty()),
            range: input.range,
            start: input.start.filter(|s| !s.is_empty()),
            end: input.end.filter(|s| !s.is_empty()),
            follow: input.follow,
            public: input.public,
            env: env.to_string(),
            created_at: now.clone(),
            updated_at: now,
            owner: None,
        };
        let stored = StoredSaved {
            search: s.clone(),
            owner_user_id: user_id.to_string(),
        };
        self.backend.put_doc(&saved_key(&s.id), &stored).await?;
        Ok(s)
    }

    /// Patch a search. Owner-only, or any user if already public. "Not found"
    /// covers unknown ids and others' private rows.
    pub async fn saved_update(
        &self,
        user_id: &str,
        id: &str,
        patch: SavedSearchPatch,
        as_admin: bool,
    ) -> Result<SavedSearch> {
        if let Some(n) = &patch.name {
            if n.trim().is_empty() {
                bail!("name cannot be empty");
            }
        }
        let now = chrono::Utc::now().to_rfc3339();
        let user_id = user_id.to_string();
        self.backend
            .mutate_doc::<StoredSaved, SavedSearch, _>(&saved_key(id), |cur| {
                let mut st = cur.ok_or_else(|| anyhow::anyhow!("saved search {id} not found"))?;
                if st.owner_user_id != user_id && !st.search.public && !as_admin {
                    bail!("saved search {id} not found");
                }
                if let Some(n) = &patch.name {
                    st.search.name = n.trim().to_string();
                }
                if let Some(q) = &patch.q {
                    st.search.q = q.clone();
                }
                if let Some(s) = &patch.index {
                    st.search.index = s.clone().filter(|x| !x.is_empty());
                }
                if let Some(r) = &patch.range {
                    st.search.range = r.clone();
                }
                if let Some(s) = &patch.start {
                    st.search.start = s.clone().filter(|x| !x.is_empty());
                }
                if let Some(e) = &patch.end {
                    st.search.end = e.clone().filter(|x| !x.is_empty());
                }
                if let Some(f) = patch.follow {
                    st.search.follow = f;
                }
                if let Some(p) = patch.public {
                    st.search.public = p;
                }
                st.search.updated_at = now.clone();
                let ret = st.search.clone();
                Ok((st, ret))
            })
            .await
    }

    /// Delete: owner-only, or any user on a public row, or any admin.
    pub async fn saved_delete(&self, user_id: &str, id: &str, as_admin: bool) -> Result<()> {
        let key = saved_key(id);
        let Some(st): Option<StoredSaved> = self.backend.read_entity(&key).await? else {
            bail!("saved search {id} not found");
        };
        if st.owner_user_id != user_id && !st.search.public && !as_admin {
            bail!("saved search {id} not found");
        }
        self.backend.delete(&key).await
    }

    /// Bulk import (one-shot legacy JSON migration). Skips ids that already
    /// exist so a partial import is safe to re-run.
    pub async fn saved_import_bulk(&self, user_id: &str, items: &[SavedSearch]) -> Result<usize> {
        let mut inserted = 0;
        for s in items {
            let key = saved_key(&s.id);
            if self
                .backend
                .read_entity::<StoredSaved>(&key)
                .await?
                .is_some()
            {
                continue;
            }
            let mut s2 = s.clone();
            if s2.env.is_empty() {
                s2.env = DEFAULT_ENV.to_string();
            }
            let stored = StoredSaved {
                search: s2,
                owner_user_id: user_id.to_string(),
            };
            self.backend.put_doc(&key, &stored).await?;
            inserted += 1;
        }
        Ok(inserted)
    }
}

// Dashboards (per-entity, flat: `dashboards/<id>.json`). NOT env-scoped —
// widgets follow the caller's active env at view time, so lists span all envs.

use super::dashboards::{Dashboard, DashboardInput, DashboardPatch};

#[derive(Serialize, Deserialize)]
struct StoredDashboard {
    #[serde(flatten)]
    dashboard: Dashboard,
    owner_user_id: String,
}

const DASHBOARDS_PREFIX: &str = "dashboards/";

fn dashboard_key(id: &str) -> String {
    format!("dashboards/{id}.json")
}

impl Control {
    /// Dashboards, newest-updated first, with owner label. `visible_to = Some(user)`
    /// restricts to own + public; `None` returns every row (admin "view all").
    async fn collect_dashboards(&self, visible_to: Option<&str>) -> Result<Vec<Dashboard>> {
        let names = self.owner_label_map().await?;
        let mut items: Vec<Dashboard> = self
            .backend
            .list_entities::<StoredDashboard>(DASHBOARDS_PREFIX)
            .await?
            .into_iter()
            .filter(|d| match visible_to {
                None => true,
                Some(uid) => d.owner_user_id == uid || d.dashboard.public,
            })
            .map(|d| {
                let mut dash = d.dashboard;
                dash.owner = Some(label_for(&names, d.owner_user_id));
                dash
            })
            .collect();
        items.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
        Ok(items)
    }

    /// Dashboards visible to `user_id`: own rows + every public row.
    /// Most-recently-updated first; owner label attached.
    pub async fn dashboard_list(&self, user_id: &str) -> Result<Vec<Dashboard>> {
        self.collect_dashboards(Some(user_id)).await
    }

    /// Admin-only: every dashboard regardless of owner/visibility, with the
    /// owner label attached. Newest-updated first.
    pub async fn dashboard_list_all(&self) -> Result<Vec<Dashboard>> {
        self.collect_dashboards(None).await
    }

    /// One dashboard by id, if visible to `user_id` (own/public) or `as_admin`.
    /// "Not found" also covers others' private rows for non-admins.
    pub async fn dashboard_get(
        &self,
        user_id: &str,
        id: &str,
        as_admin: bool,
    ) -> Result<Dashboard> {
        let Some(st): Option<StoredDashboard> =
            self.backend.read_entity(&dashboard_key(id)).await?
        else {
            bail!("dashboard {id} not found");
        };
        if st.owner_user_id != user_id && !st.dashboard.public && !as_admin {
            bail!("dashboard {id} not found");
        }
        Ok(st.dashboard)
    }

    pub async fn dashboard_create(
        &self,
        user_id: &str,
        input: DashboardInput,
    ) -> Result<Dashboard> {
        if input.name.trim().is_empty() {
            bail!("name is required");
        }
        let now = chrono::Utc::now().to_rfc3339();
        let d = Dashboard {
            id: new_id("dash"),
            name: input.name.trim().to_string(),
            description: input.description,
            spec: input.spec,
            public: input.public,
            created_at: now.clone(),
            updated_at: now,
            owner: None,
        };
        let stored = StoredDashboard {
            dashboard: d.clone(),
            owner_user_id: user_id.to_string(),
        };
        self.backend.put_doc(&dashboard_key(&d.id), &stored).await?;
        Ok(d)
    }

    /// Patch a dashboard. Owner-only, or any user if already public. "Not found"
    /// covers unknown ids and others' private rows.
    pub async fn dashboard_update(
        &self,
        user_id: &str,
        id: &str,
        patch: DashboardPatch,
        as_admin: bool,
    ) -> Result<Dashboard> {
        if let Some(n) = &patch.name {
            if n.trim().is_empty() {
                bail!("name cannot be empty");
            }
        }
        let now = chrono::Utc::now().to_rfc3339();
        let user_id = user_id.to_string();
        self.backend
            .mutate_doc::<StoredDashboard, Dashboard, _>(&dashboard_key(id), |cur| {
                let mut st = cur.ok_or_else(|| anyhow::anyhow!("dashboard {id} not found"))?;
                if st.owner_user_id != user_id && !st.dashboard.public && !as_admin {
                    bail!("dashboard {id} not found");
                }
                if let Some(n) = &patch.name {
                    st.dashboard.name = n.trim().to_string();
                }
                if let Some(d) = &patch.description {
                    st.dashboard.description = d.clone();
                }
                if let Some(s) = &patch.spec {
                    st.dashboard.spec = s.clone();
                }
                if let Some(p) = patch.public {
                    st.dashboard.public = p;
                }
                st.dashboard.updated_at = now.clone();
                let ret = st.dashboard.clone();
                Ok((st, ret))
            })
            .await
    }

    /// Delete: owner-only, or any user on a public row, or any admin.
    pub async fn dashboard_delete(&self, user_id: &str, id: &str, as_admin: bool) -> Result<()> {
        let key = dashboard_key(id);
        let Some(st): Option<StoredDashboard> = self.backend.read_entity(&key).await? else {
            bail!("dashboard {id} not found");
        };
        if st.owner_user_id != user_id && !st.dashboard.public && !as_admin {
            bail!("dashboard {id} not found");
        }
        self.backend.delete(&key).await
    }
}

// Monitors (per-entity, flat: `monitors/<id>.json`). NOT env-scoped — `env` is
// the agent run target, but lists span all envs.

use super::monitors::{
    Monitor, MonitorInput, MonitorKind, MonitorPatch, ThresholdConfig, DEFAULT_INTERVAL_SECONDS,
    DEFAULT_WINDOW_SECONDS, MIN_INTERVAL_SECONDS, MIN_WINDOW_SECONDS, STUCK_LEASE_SECS,
};

/// Normalize + validate a threshold monitor's config: default/clamp the window,
/// reject a negative threshold, normalize severity to low/medium/high.
fn validate_threshold(cfg: Option<ThresholdConfig>) -> Result<ThresholdConfig> {
    let mut cfg =
        cfg.ok_or_else(|| anyhow::anyhow!("threshold config is required for a threshold monitor"))?;
    if cfg.threshold < 0 {
        bail!("threshold must be zero or greater");
    }
    if cfg.window_seconds <= 0 {
        cfg.window_seconds = DEFAULT_WINDOW_SECONDS;
    }
    cfg.window_seconds = cfg.window_seconds.max(MIN_WINDOW_SECONDS);
    if cfg.query.trim().is_empty() {
        cfg.query = "*".to_string();
    } else {
        cfg.query = cfg.query.trim().to_string();
    }
    cfg.index = cfg
        .index
        .filter(|s| !s.trim().is_empty())
        .map(|s| s.trim().to_string());
    cfg.severity = super::alerts::normalize_severity(&cfg.severity);
    Ok(cfg)
}

#[derive(Serialize, Deserialize)]
struct StoredMonitor {
    #[serde(flatten)]
    monitor: Monitor,
    owner_user_id: String,
}

const MONITORS_PREFIX: &str = "monitors/";

fn monitor_key(id: &str) -> String {
    format!("monitors/{id}.json")
}

impl Control {
    /// Monitors (all envs), newest-updated first, with owner label. `visible_to =
    /// Some(user)` restricts to own + public; `None` returns every row (admin).
    async fn collect_monitors(&self, visible_to: Option<&str>) -> Result<Vec<Monitor>> {
        let names = self.owner_label_map().await?;
        let mut items: Vec<Monitor> = self
            .backend
            .list_entities::<StoredMonitor>(MONITORS_PREFIX)
            .await?
            .into_iter()
            .filter(|m| match visible_to {
                None => true,
                Some(uid) => m.owner_user_id == uid || m.monitor.public,
            })
            .map(|m| {
                let mut mon = m.monitor;
                mon.owner = Some(label_for(&names, m.owner_user_id));
                mon
            })
            .collect();
        items.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
        Ok(items)
    }

    /// Monitors visible to `user_id`: own rows + every public row. Owner label
    /// attached.
    pub async fn monitor_list(&self, user_id: &str) -> Result<Vec<Monitor>> {
        self.collect_monitors(Some(user_id)).await
    }

    /// Admin-only: every monitor regardless of owner, with the owner label
    /// attached. Newest-updated first.
    pub async fn monitor_list_admin(&self) -> Result<Vec<Monitor>> {
        self.collect_monitors(None).await
    }

    pub async fn monitor_get(
        &self,
        user_id: &str,
        id: &str,
        as_admin: bool,
    ) -> Result<Option<Monitor>> {
        let st: Option<StoredMonitor> = self.backend.read_entity(&monitor_key(id)).await?;
        Ok(st
            .filter(|m| as_admin || m.owner_user_id == user_id || m.monitor.public)
            .map(|m| m.monitor))
    }

    pub async fn monitor_create(
        &self,
        user_id: &str,
        env: &str,
        input: MonitorInput,
    ) -> Result<Monitor> {
        if input.name.trim().is_empty() {
            bail!("name is required");
        }
        let threshold = match input.kind {
            MonitorKind::Ai => {
                if input.prompt.trim().is_empty() {
                    bail!("prompt is required for an AI monitor");
                }
                None
            }
            MonitorKind::Threshold => Some(validate_threshold(input.threshold)?),
        };
        let interval = input
            .interval_seconds
            .unwrap_or(DEFAULT_INTERVAL_SECONDS)
            .max(MIN_INTERVAL_SECONDS);
        let now = chrono::Utc::now().to_rfc3339();
        let m = Monitor {
            id: new_id("mon"),
            name: input.name.trim().to_string(),
            description: input.description.trim().to_string(),
            prompt: input.prompt.trim().to_string(),
            kind: input.kind,
            threshold,
            notify: input.notify.filter(|n| !n.webhook_url.trim().is_empty()),
            interval_seconds: interval,
            enabled: input.enabled,
            last_run_at: None,
            last_status: None,
            last_error: None,
            last_conversation_id: None,
            last_breaching: None,
            running: false,
            running_since: None,
            public: input.public,
            env: env.to_string(),
            created_at: now.clone(),
            updated_at: now,
            owner: None,
        };
        let stored = StoredMonitor {
            monitor: m.clone(),
            owner_user_id: user_id.to_string(),
        };
        self.backend.put_doc(&monitor_key(&m.id), &stored).await?;
        Ok(m)
    }

    pub async fn monitor_update(
        &self,
        user_id: &str,
        id: &str,
        patch: MonitorPatch,
        as_admin: bool,
    ) -> Result<Monitor> {
        if let Some(n) = &patch.name {
            if n.trim().is_empty() {
                bail!("name cannot be empty");
            }
        }
        if let Some(p) = &patch.prompt {
            if p.trim().is_empty() {
                bail!("prompt cannot be empty");
            }
        }
        let now = chrono::Utc::now().to_rfc3339();
        let user_id = user_id.to_string();
        self.backend
            .mutate_doc::<StoredMonitor, Monitor, _>(&monitor_key(id), |cur| {
                let mut st = cur
                    .filter(|m| as_admin || m.owner_user_id == user_id || m.monitor.public)
                    .ok_or_else(|| anyhow::anyhow!("monitor {id} not found"))?;
                if let Some(n) = &patch.name {
                    st.monitor.name = n.trim().to_string();
                }
                if let Some(d) = &patch.description {
                    st.monitor.description = d.trim().to_string();
                }
                if let Some(p) = &patch.prompt {
                    st.monitor.prompt = p.trim().to_string();
                }
                if let Some(k) = patch.kind {
                    st.monitor.kind = k;
                }
                if let Some(t) = &patch.threshold {
                    // Config change re-arms edge-triggered alerting.
                    st.monitor.threshold = Some(validate_threshold(Some(t.clone()))?);
                    st.monitor.last_breaching = None;
                }
                if let Some(n) = &patch.notify {
                    // Empty webhook_url clears the override.
                    st.monitor.notify = if n.webhook_url.trim().is_empty() {
                        None
                    } else {
                        Some(n.clone())
                    };
                }
                if let Some(i) = patch.interval_seconds {
                    st.monitor.interval_seconds = i.max(MIN_INTERVAL_SECONDS);
                }
                if let Some(e) = patch.enabled {
                    st.monitor.enabled = e;
                }
                if let Some(p) = patch.public {
                    st.monitor.public = p;
                }
                // Keep kind and its required payload consistent.
                match st.monitor.kind {
                    MonitorKind::Ai => {
                        if st.monitor.prompt.trim().is_empty() {
                            bail!("prompt is required for an AI monitor");
                        }
                    }
                    MonitorKind::Threshold => {
                        if st.monitor.threshold.is_none() {
                            bail!("threshold config is required for a threshold monitor");
                        }
                    }
                }
                st.monitor.updated_at = now.clone();
                let ret = st.monitor.clone();
                Ok((st, ret))
            })
            .await
    }

    pub async fn monitor_delete(&self, user_id: &str, id: &str, as_admin: bool) -> Result<()> {
        let key = monitor_key(id);
        match self.backend.read_entity::<StoredMonitor>(&key).await? {
            Some(m) if as_admin || m.owner_user_id == user_id || m.monitor.public => {
                self.backend.delete(&key).await
            }
            _ => bail!("monitor {id} not found"),
        }
    }

    // ---- scheduler-facing (no user check) ----------------------------------

    /// Record a threshold monitor's latest breaching state so the next tick fires
    /// only on the not-breaching → breaching edge. No owner check (scheduler-internal).
    pub async fn monitor_set_breaching(&self, id: &str, breaching: bool) -> Result<()> {
        self.backend
            .mutate_doc::<StoredMonitor, (), _>(&monitor_key(id), |cur| {
                let mut st = cur
                    .ok_or_else(|| anyhow::anyhow!("monitor {id} vanished before set_breaching"))?;
                st.monitor.last_breaching = Some(breaching);
                Ok((st, ()))
            })
            .await
    }

    /// Every monitor with its owner, id-sorted. The scheduler reads what it
    /// needs and ignores the rest.
    pub async fn monitor_list_all(&self) -> Result<Vec<(String, Monitor)>> {
        let mut out: Vec<(String, Monitor)> = self
            .backend
            .list_entities::<StoredMonitor>(MONITORS_PREFIX)
            .await?
            .into_iter()
            .map(|m| (m.owner_user_id, m.monitor))
            .collect();
        out.sort_by(|a, b| a.1.id.cmp(&b.1.id));
        Ok(out)
    }

    /// Lease the monitor for a run via CAS (`true` = acquired). Stale leases
    /// (older than [`STUCK_LEASE_SECS`]) are stolen; the losing racer gets `false`.
    pub async fn monitor_try_lease(&self, id: &str) -> Result<bool> {
        let now = chrono::Utc::now().timestamp_millis();
        let cutoff = now - (STUCK_LEASE_SECS * 1000);
        let key = monitor_key(id);
        if self
            .backend
            .read_entity::<StoredMonitor>(&key)
            .await?
            .is_none()
        {
            return Ok(false);
        }
        self.backend
            .mutate_doc::<StoredMonitor, bool, _>(&key, |cur| {
                let mut st = cur.ok_or_else(|| anyhow::anyhow!("monitor {id} vanished"))?;
                let leasable = !st.monitor.running
                    || st.monitor.running_since.map(|s| s < cutoff).unwrap_or(true);
                if leasable {
                    st.monitor.running = true;
                    st.monitor.running_since = Some(now);
                    Ok((st, true))
                } else {
                    Ok((st, false))
                }
            })
            .await
    }

    /// Clear `last_run_at` so the next scheduler tick treats the monitor as
    /// overdue ("run now"). Ownership checked by caller; no-op if gone.
    pub async fn monitor_clear_last_run(&self, id: &str) -> Result<()> {
        let key = monitor_key(id);
        if self
            .backend
            .read_entity::<StoredMonitor>(&key)
            .await?
            .is_none()
        {
            return Ok(());
        }
        let now = chrono::Utc::now().to_rfc3339();
        self.backend
            .mutate_doc::<StoredMonitor, (), _>(&key, |cur| {
                let mut st = cur.ok_or_else(|| anyhow::anyhow!("monitor {id} gone"))?;
                st.monitor.last_run_at = None;
                st.monitor.updated_at = now.clone();
                Ok((st, ()))
            })
            .await
    }

    /// Release the lease and record the run outcome. No-op if the monitor was
    /// deleted mid-run.
    pub async fn monitor_finish_run(
        &self,
        id: &str,
        status: &str,
        error: Option<&str>,
        conversation_id: Option<String>,
    ) -> Result<()> {
        let key = monitor_key(id);
        if self
            .backend
            .read_entity::<StoredMonitor>(&key)
            .await?
            .is_none()
        {
            return Ok(());
        }
        let now = chrono::Utc::now().timestamp_millis();
        let status = status.to_string();
        let error = error.map(|e| e.to_string());
        self.backend
            .mutate_doc::<StoredMonitor, (), _>(&key, |cur| {
                let mut st = cur.ok_or_else(|| anyhow::anyhow!("monitor {id} deleted mid-run"))?;
                st.monitor.running = false;
                st.monitor.running_since = None;
                st.monitor.last_run_at = Some(now);
                st.monitor.last_status = Some(status.clone());
                st.monitor.last_error = error.clone();
                st.monitor.last_conversation_id = conversation_id.clone();
                Ok((st, ()))
            })
            .await
    }
}

// Ingestion sources (per-entity, flat: `sources/<id>.json`). Env-scoped lists;
// checkpoints at `source_checkpoints/<id>.json`; supervisor uses `*_all` + leases.

use super::sources::{Source, SourceCheckpoint, SourceInput, SourcePatch};

#[derive(Serialize, Deserialize)]
struct StoredSource {
    #[serde(flatten)]
    source: Source,
    owner_user_id: String,
}

const SOURCES_PREFIX: &str = "sources/";

fn source_key(id: &str) -> String {
    format!("sources/{id}.json")
}

fn source_checkpoint_key(id: &str) -> String {
    format!("source_checkpoints/{id}.json")
}

impl Control {
    /// Sources in `env` owned by `user_id`, newest-updated first.
    pub async fn source_list(&self, user_id: &str, env: &str) -> Result<Vec<Source>> {
        let mut items: Vec<Source> = self
            .backend
            .list_entities::<StoredSource>(SOURCES_PREFIX)
            .await?
            .into_iter()
            .filter(|s| s.source.env == env && s.owner_user_id == user_id)
            .map(|s| s.source)
            .collect();
        items.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
        Ok(items)
    }

    /// Every source owned by `user_id`, across all envs, newest-updated first.
    /// Backs the admin Source-management view (all envs at once).
    pub async fn source_list_all_user(&self, user_id: &str) -> Result<Vec<Source>> {
        let mut items: Vec<Source> = self
            .backend
            .list_entities::<StoredSource>(SOURCES_PREFIX)
            .await?
            .into_iter()
            .filter(|s| s.owner_user_id == user_id)
            .map(|s| s.source)
            .collect();
        items.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
        Ok(items)
    }

    pub async fn source_get(&self, user_id: &str, id: &str) -> Result<Option<Source>> {
        let st: Option<StoredSource> = self.backend.read_entity(&source_key(id)).await?;
        Ok(st.filter(|s| s.owner_user_id == user_id).map(|s| s.source))
    }

    pub async fn source_create(
        &self,
        user_id: &str,
        env: &str,
        input: SourceInput,
    ) -> Result<Source> {
        if input.name.trim().is_empty() {
            bail!("name is required");
        }
        if input.path.trim().is_empty() {
            bail!("path is required");
        }
        if input.index.trim().is_empty() {
            bail!("index is required");
        }
        let index = crate::catalog::index_or_default(Some(input.index.trim()), "default")?;
        let kind = input.kind.as_deref().unwrap_or("fs").to_string();
        let mode = input.mode.as_deref().unwrap_or("pull").to_string();
        if kind != "fs" && kind != "s3" {
            bail!("only 'fs' and 's3' sources are supported currently (got '{kind}')");
        }
        if kind == "s3" && !input.path.trim().starts_with("s3://") {
            bail!("s3 source path must be an s3:// URL (e.g. s3://bucket/logs/**/*.gz)");
        }
        if mode != "pull" {
            bail!("only 'pull' mode is supported currently (got '{mode}')");
        }
        let interval = input
            .interval_seconds
            .unwrap_or(super::sources::DEFAULT_INTERVAL_SECONDS)
            .max(super::sources::MIN_INTERVAL_SECONDS);
        let now = chrono::Utc::now().to_rfc3339();
        let s = Source {
            id: new_id("src"),
            name: input.name.trim().to_string(),
            env: env.to_string(),
            index,
            kind,
            mode,
            path: input.path.trim().to_string(),
            exclude: input.exclude,
            format: input.format.unwrap_or_else(|| "auto".to_string()),
            compression: input.compression.unwrap_or_else(|| "auto".to_string()),
            multiline_pattern: input.multiline_pattern.filter(|s| !s.is_empty()),
            multiline_max_lines: input.multiline_max_lines,
            grok_pattern: input.grok_pattern.filter(|s| !s.is_empty()),
            interval_seconds: interval,
            source_tag: input.source_tag.filter(|s| !s.is_empty()),
            enabled: input.enabled.unwrap_or(true),
            last_run_at: None,
            last_status: None,
            last_error: None,
            total_ingested: 0,
            running: false,
            running_since: None,
            progress_ingested: 0,
            progress_file: None,
            created_at: now.clone(),
            updated_at: now,
        };
        let stored = StoredSource {
            source: s.clone(),
            owner_user_id: user_id.to_string(),
        };
        self.backend.put_doc(&source_key(&s.id), &stored).await?;
        Ok(s)
    }

    pub async fn source_update(
        &self,
        user_id: &str,
        id: &str,
        patch: SourcePatch,
    ) -> Result<Source> {
        if let Some(n) = &patch.name {
            if n.trim().is_empty() {
                bail!("name cannot be empty");
            }
        }
        if let Some(m) = &patch.mode {
            if m != "pull" {
                bail!("only 'pull' mode is supported currently");
            }
        }
        let index = match &patch.index {
            Some(i) if !i.trim().is_empty() => {
                Some(crate::catalog::index_or_default(Some(i.trim()), "default")?)
            }
            Some(_) => bail!("index cannot be empty"),
            None => None,
        };
        let now = chrono::Utc::now().to_rfc3339();
        let user_id = user_id.to_string();
        self.backend
            .mutate_doc::<StoredSource, Source, _>(&source_key(id), |cur| {
                let mut st = cur
                    .filter(|s| s.owner_user_id == user_id)
                    .ok_or_else(|| anyhow::anyhow!("source {id} not found"))?;
                if let Some(n) = &patch.name {
                    st.source.name = n.trim().to_string();
                }
                if let Some(e) = &patch.env {
                    if !e.trim().is_empty() {
                        st.source.env = e.trim().to_string();
                    }
                }
                if let Some(i) = &index {
                    st.source.index = i.clone();
                }
                if let Some(m) = &patch.mode {
                    st.source.mode = m.clone();
                }
                if let Some(p) = &patch.path {
                    st.source.path = p.trim().to_string();
                }
                if let Some(e) = &patch.exclude {
                    st.source.exclude = e.clone();
                }
                if let Some(f) = &patch.format {
                    st.source.format = f.clone();
                }
                if let Some(c) = &patch.compression {
                    st.source.compression = c.clone();
                }
                if let Some(mp) = &patch.multiline_pattern {
                    st.source.multiline_pattern = mp.clone().filter(|s| !s.is_empty());
                }
                if let Some(ml) = &patch.multiline_max_lines {
                    st.source.multiline_max_lines = *ml;
                }
                if let Some(gp) = &patch.grok_pattern {
                    st.source.grok_pattern = gp.clone().filter(|s| !s.is_empty());
                }
                if let Some(i) = patch.interval_seconds {
                    st.source.interval_seconds = i.max(super::sources::MIN_INTERVAL_SECONDS);
                }
                if let Some(t) = &patch.source_tag {
                    st.source.source_tag = t.clone().filter(|s| !s.is_empty());
                }
                if let Some(en) = patch.enabled {
                    st.source.enabled = en;
                    // Leave the lease alone on disable: the in-flight run stops
                    // itself and clears it, blocking a disable→re-enable double-run.
                }
                st.source.updated_at = now.clone();
                let ret = st.source.clone();
                Ok((st, ret))
            })
            .await
    }

    /// Wipe a source's ingestion state (checkpoint + run counters) for a clean
    /// re-ingest. Refuses while running; leaves `enabled` untouched.
    pub async fn source_reset(&self, user_id: &str, id: &str) -> Result<Source> {
        let user_id = user_id.to_string();
        let now = chrono::Utc::now().to_rfc3339();
        let updated = self
            .backend
            .mutate_doc::<StoredSource, Source, _>(&source_key(id), |cur| {
                let mut st = cur
                    .filter(|s| s.owner_user_id == user_id)
                    .ok_or_else(|| anyhow::anyhow!("source {id} not found"))?;
                if st.source.running {
                    bail!(
                        "source is running — disable it and wait for it to stop before resetting"
                    );
                }
                st.source.total_ingested = 0;
                st.source.progress_ingested = 0;
                st.source.progress_file = None;
                st.source.last_run_at = None;
                st.source.last_status = None;
                st.source.last_error = None;
                st.source.updated_at = now.clone();
                let ret = st.source.clone();
                Ok((st, ret))
            })
            .await?;
        // Drop the fishbucket so the next run starts from offset 0 everywhere.
        let _ = self.backend.delete(&source_checkpoint_key(id)).await;
        Ok(updated)
    }

    pub async fn source_delete(&self, user_id: &str, id: &str) -> Result<()> {
        let key = source_key(id);
        match self.backend.read_entity::<StoredSource>(&key).await? {
            Some(s) if s.owner_user_id == user_id => {
                self.backend.delete(&key).await?;
                // Best-effort checkpoint cleanup — a leftover is harmless.
                let _ = self.backend.delete(&source_checkpoint_key(id)).await;
                Ok(())
            }
            _ => bail!("source {id} not found"),
        }
    }

    /// Clear `last_run_at` so the next supervisor tick treats the source as due.
    /// Ownership-checked.
    pub async fn source_run_now(&self, user_id: &str, id: &str) -> Result<()> {
        let now = chrono::Utc::now().to_rfc3339();
        let user_id = user_id.to_string();
        self.backend
            .mutate_doc::<StoredSource, (), _>(&source_key(id), |cur| {
                let mut st = cur
                    .filter(|s| s.owner_user_id == user_id)
                    .ok_or_else(|| anyhow::anyhow!("source {id} not found"))?;
                st.source.last_run_at = None;
                st.source.updated_at = now.clone();
                Ok((st, ()))
            })
            .await
    }

    // ---- supervisor-facing (no user check) ---------------------------------

    /// Every source with its owner, id-sorted.
    pub async fn source_list_all(&self) -> Result<Vec<(String, Source)>> {
        let mut out: Vec<(String, Source)> = self
            .backend
            .list_entities::<StoredSource>(SOURCES_PREFIX)
            .await?
            .into_iter()
            .map(|s| (s.owner_user_id, s.source))
            .collect();
        out.sort_by(|a, b| a.1.id.cmp(&b.1.id));
        Ok(out)
    }

    /// Lease a source for a run via CAS. Stale leases are stolen. Mirrors
    /// [`Control::monitor_try_lease`].
    pub async fn source_try_lease(&self, id: &str) -> Result<bool> {
        let now = chrono::Utc::now().timestamp_millis();
        let cutoff = now - (super::sources::STUCK_LEASE_SECS * 1000);
        let key = source_key(id);
        if self
            .backend
            .read_entity::<StoredSource>(&key)
            .await?
            .is_none()
        {
            return Ok(false);
        }
        self.backend
            .mutate_doc::<StoredSource, bool, _>(&key, |cur| {
                let mut st = cur.ok_or_else(|| anyhow::anyhow!("source {id} vanished"))?;
                let leasable = !st.source.running
                    || st.source.running_since.map(|s| s < cutoff).unwrap_or(true);
                if leasable {
                    st.source.running = true;
                    st.source.running_since = Some(now);
                    // Fresh run — clear the prior run's live progress.
                    st.source.progress_ingested = 0;
                    st.source.progress_file = None;
                    Ok((st, true))
                } else {
                    Ok((st, false))
                }
            })
            .await
    }

    /// Release the lease, record the outcome, and add to the lifetime ingest
    /// counter. No-op if the source was deleted mid-run.
    pub async fn source_finish_run(
        &self,
        id: &str,
        status: &str,
        error: Option<&str>,
        ingested_delta: u64,
    ) -> Result<()> {
        let key = source_key(id);
        if self
            .backend
            .read_entity::<StoredSource>(&key)
            .await?
            .is_none()
        {
            return Ok(());
        }
        let now = chrono::Utc::now().timestamp_millis();
        let status = status.to_string();
        let error = error.map(|e| e.to_string());
        self.backend
            .mutate_doc::<StoredSource, (), _>(&key, |cur| {
                let mut st = cur.ok_or_else(|| anyhow::anyhow!("source {id} deleted mid-run"))?;
                st.source.running = false;
                st.source.running_since = None;
                st.source.last_run_at = Some(now);
                st.source.last_status = Some(status.clone());
                st.source.last_error = error.clone();
                st.source.total_ingested = st.source.total_ingested.saturating_add(ingested_delta);
                // Run done — fold the live counter into the lifetime total above
                // and clear the per-run progress.
                st.source.progress_ingested = 0;
                st.source.progress_file = None;
                Ok((st, ()))
            })
            .await
    }

    /// Update live per-run progress after each file. Returns whether the source
    /// is still enabled — the pull uses a `false`/error as a cooperative-cancel signal.
    pub async fn source_progress_update(
        &self,
        id: &str,
        ingested: u64,
        current_file: Option<&str>,
    ) -> Result<bool> {
        let key = source_key(id);
        let current = current_file.map(|s| s.to_string());
        self.backend
            .mutate_doc::<StoredSource, bool, _>(&key, |cur| {
                let mut st = cur.ok_or_else(|| anyhow::anyhow!("source {id} vanished"))?;
                let still_enabled = st.source.enabled;
                st.source.progress_ingested = ingested;
                st.source.progress_file = current.clone();
                Ok((st, still_enabled))
            })
            .await
    }

    pub async fn source_checkpoint_get(&self, id: &str) -> Result<Option<SourceCheckpoint>> {
        self.backend.read_entity(&source_checkpoint_key(id)).await
    }

    pub async fn source_checkpoint_put(&self, id: &str, ckpt: &SourceCheckpoint) -> Result<()> {
        self.backend.put_doc(&source_checkpoint_key(id), ckpt).await
    }
}

// Ingest auth (single document `ingest_auth.json`: require switch + scoped push
// tokens). Single-doc so the hot-path token lookup is one cached read.

use super::ingest_tokens::{IngestAuth, PushToken};

const INGEST_AUTH_KEY: &str = "ingest_auth.json";

impl Control {
    /// The full ingest-auth config (cached — a few seconds of cross-node
    /// staleness is fine for token checks). Default = open, no tokens.
    pub async fn ingest_auth(&self) -> Result<IngestAuth> {
        self.backend.cached_read_or_default(INGEST_AUTH_KEY).await
    }

    /// Find an enabled token by its secret value (for request authorization).
    pub async fn ingest_token_find(&self, token: &str) -> Result<Option<PushToken>> {
        Ok(self
            .ingest_auth()
            .await?
            .tokens
            .into_iter()
            .find(|t| t.enabled && t.token == token))
    }

    pub async fn ingest_set_require(&self, require: bool) -> Result<()> {
        self.backend
            .mutate_doc::<IngestAuth, (), _>(INGEST_AUTH_KEY, |cur| {
                let mut a = cur.unwrap_or_default();
                a.require = require;
                Ok((a, ()))
            })
            .await
    }

    /// Enable/disable whole HTTP ingestion classes; each `None` leaves it as-is.
    pub async fn ingest_set_endpoints(
        &self,
        api_enabled: Option<bool>,
        shims_enabled: Option<bool>,
    ) -> Result<()> {
        self.backend
            .mutate_doc::<IngestAuth, (), _>(INGEST_AUTH_KEY, |cur| {
                let mut a = cur.unwrap_or_default();
                if let Some(v) = api_enabled {
                    a.api_enabled = v;
                }
                if let Some(v) = shims_enabled {
                    a.shims_enabled = v;
                }
                Ok((a, ()))
            })
            .await
    }

    /// Mint a token. The secret is generated before the CAS closure (which may
    /// retry), and returned once — callers must surface it to the user now.
    pub async fn ingest_token_create(
        &self,
        name: &str,
        env: &str,
        indexes: Vec<String>,
    ) -> Result<PushToken> {
        if name.trim().is_empty() {
            bail!("name is required");
        }
        if env.trim().is_empty() {
            bail!("env is required");
        }
        let now = chrono::Utc::now().to_rfc3339();
        let secret = crate::crypto::rand::bytes::<24>();
        let token = PushToken {
            id: new_id("itok"),
            name: name.trim().to_string(),
            token: format!("hli_{}", hex::encode(secret)),
            env: env.to_string(),
            indexes: indexes
                .into_iter()
                .filter(|i| !i.trim().is_empty())
                .collect(),
            enabled: true,
            last_used_at: None,
            created_at: now.clone(),
            updated_at: now,
        };
        let created = token.clone();
        self.backend
            .mutate_doc::<IngestAuth, (), _>(INGEST_AUTH_KEY, |cur| {
                let mut a = cur.unwrap_or_default();
                a.tokens.push(created.clone());
                Ok((a, ()))
            })
            .await?;
        Ok(token)
    }

    pub async fn ingest_token_set_enabled(&self, id: &str, enabled: bool) -> Result<()> {
        let now = chrono::Utc::now().to_rfc3339();
        self.backend
            .mutate_doc::<IngestAuth, (), _>(INGEST_AUTH_KEY, |cur| {
                let mut a = cur.unwrap_or_default();
                if let Some(t) = a.tokens.iter_mut().find(|t| t.id == id) {
                    t.enabled = enabled;
                    t.updated_at = now.clone();
                }
                Ok((a, ()))
            })
            .await
    }

    pub async fn ingest_token_delete(&self, id: &str) -> Result<()> {
        self.backend
            .mutate_doc::<IngestAuth, (), _>(INGEST_AUTH_KEY, |cur| {
                let mut a = cur.unwrap_or_default();
                a.tokens.retain(|t| t.id != id);
                Ok((a, ()))
            })
            .await
    }
}

// REST API keys (single document `api_keys.json`). Single-doc so the hot-path
// bearer lookup is one cached read, like ingest auth above.

use super::api_keys::{ApiKey, ApiKeyScopes, ApiKeyStore};

const API_KEYS_KEY: &str = "api_keys.json";

impl Control {
    /// Every API key (cached — a few seconds of cross-node staleness is fine).
    pub async fn api_key_list(&self) -> Result<Vec<ApiKey>> {
        let store: ApiKeyStore = self.backend.cached_read_or_default(API_KEYS_KEY).await?;
        Ok(store.keys)
    }

    /// Find an enabled key by its secret value (for request authorization). The
    /// caller checks expiry so it can log/answer distinctly from "unknown".
    pub async fn api_key_find(&self, token: &str) -> Result<Option<ApiKey>> {
        let store: ApiKeyStore = self.backend.cached_read_or_default(API_KEYS_KEY).await?;
        Ok(store
            .keys
            .into_iter()
            .find(|k| k.enabled && k.token == token))
    }

    /// Mint a key. The secret is generated before the CAS closure (which may
    /// retry), and returned once — callers must surface it to the user now.
    pub async fn api_key_create(
        &self,
        name: &str,
        description: &str,
        scopes: ApiKeyScopes,
        expires_at: Option<i64>,
        created_by: &str,
    ) -> Result<ApiKey> {
        if name.trim().is_empty() {
            bail!("name is required");
        }
        if !scopes.any() {
            bail!("at least one scope is required");
        }
        let now = chrono::Utc::now().to_rfc3339();
        let secret = crate::crypto::rand::bytes::<24>();
        let key = ApiKey {
            id: new_id("akey"),
            name: name.trim().to_string(),
            description: description.trim().to_string(),
            token: format!("hlk_{}", hex::encode(secret)),
            scopes,
            enabled: true,
            created_by: created_by.to_string(),
            created_at: now,
            last_used_at: None,
            expires_at,
        };
        let created = key.clone();
        self.backend
            .mutate_doc::<ApiKeyStore, (), _>(API_KEYS_KEY, |cur| {
                let mut s = cur.unwrap_or_default();
                s.keys.push(created.clone());
                Ok((s, ()))
            })
            .await?;
        Ok(key)
    }

    pub async fn api_key_set_enabled(&self, id: &str, enabled: bool) -> Result<()> {
        self.backend
            .mutate_doc::<ApiKeyStore, (), _>(API_KEYS_KEY, |cur| {
                let mut s = cur.unwrap_or_default();
                if let Some(k) = s.keys.iter_mut().find(|k| k.id == id) {
                    k.enabled = enabled;
                }
                Ok((s, ()))
            })
            .await
    }

    pub async fn api_key_delete(&self, id: &str) -> Result<()> {
        self.backend
            .mutate_doc::<ApiKeyStore, (), _>(API_KEYS_KEY, |cur| {
                let mut s = cur.unwrap_or_default();
                s.keys.retain(|k| k.id != id);
                Ok((s, ()))
            })
            .await
    }

    /// Best-effort last-used stamp (epoch ms). Called off the request path and
    /// throttled by the caller, so a lost CAS race here is harmless.
    pub async fn api_key_touch(&self, id: &str, now_ms: i64) -> Result<()> {
        self.backend
            .mutate_doc::<ApiKeyStore, (), _>(API_KEYS_KEY, |cur| {
                let mut s = cur.unwrap_or_default();
                if let Some(k) = s.keys.iter_mut().find(|k| k.id == id) {
                    k.last_used_at = Some(now_ms);
                }
                Ok((s, ()))
            })
            .await
    }
}

// Conversations (per-entity, user-scoped: `conversations/<owner>/<id>.json`).
// NOT env-scoped. IDs are timestamp-prefixed so S3 prefix listings sort by time.

/// Conversation summary (the wire shape for the chat list).
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct ConvMeta {
    pub id: String,
    pub title: String,
    pub created_at: i64,
    pub updated_at: i64,
}

/// One persisted turn. `payload` is opaque JSON (tool calls, reasoning, view
/// context) so the shape can evolve without a migration.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct ConvTurn {
    pub id: String,
    pub turn_idx: i64,
    pub role: String,
    pub payload: Value,
    pub created_at: i64,
}

/// Conversation meta + turns, as returned to the API (`meta` flattened in).
#[derive(Serialize, Clone, Debug)]
pub struct ConvDetail {
    #[serde(flatten)]
    pub meta: ConvMeta,
    pub turns: Vec<ConvTurn>,
}

/// On-disk conversation: meta + owner + kind + turns in one file.
#[derive(Serialize, Deserialize)]
struct StoredConversation {
    id: String,
    owner_user_id: String,
    title: String,
    /// `chat` (shows in the human list) or `monitor` (a scheduled-run trace,
    /// hidden from the chat list, reachable from the alert inbox).
    #[serde(default = "chat_kind")]
    kind: String,
    created_at: i64,
    updated_at: i64,
    #[serde(default)]
    turns: Vec<ConvTurn>,
}

fn chat_kind() -> String {
    "chat".to_string()
}

impl StoredConversation {
    fn meta(&self) -> ConvMeta {
        ConvMeta {
            id: self.id.clone(),
            title: self.title.clone(),
            created_at: self.created_at,
            updated_at: self.updated_at,
        }
    }
}

fn conv_key(user_id: &str, conv_id: &str) -> String {
    format!("conversations/{user_id}/{conv_id}.json")
}

fn conv_prefix(user_id: &str) -> String {
    format!("conversations/{user_id}/")
}

impl Control {
    /// Create a chat conversation.
    pub async fn conv_create(&self, user_id: &str, title: &str) -> Result<ConvMeta> {
        self.conv_create_with_kind(user_id, title, "chat").await
    }

    /// Create a conversation with an explicit `kind` (`chat` or `monitor`).
    pub async fn conv_create_with_kind(
        &self,
        user_id: &str,
        title: &str,
        kind: &str,
    ) -> Result<ConvMeta> {
        let now = chrono::Utc::now().timestamp_millis();
        let stored = StoredConversation {
            id: conv_id(now),
            owner_user_id: user_id.to_string(),
            title: title.to_string(),
            kind: kind.to_string(),
            created_at: now,
            updated_at: now,
            turns: Vec::new(),
        };
        let meta = stored.meta();
        self.backend
            .put_doc(&conv_key(user_id, &stored.id), &stored)
            .await?;
        Ok(meta)
    }

    /// `user_id`'s chat conversations, newest-updated first. Monitor-kind rows
    /// are hidden.
    pub async fn conv_list(&self, user_id: &str) -> Result<Vec<ConvMeta>> {
        let mut metas: Vec<(i64, ConvMeta)> = self
            .backend
            .list_entities::<StoredConversation>(&conv_prefix(user_id))
            .await?
            .into_iter()
            .filter(|c| c.kind == "chat")
            .map(|c| (c.updated_at, c.meta()))
            .collect();
        metas.sort_by_key(|b| std::cmp::Reverse(b.0));
        Ok(metas.into_iter().map(|(_, m)| m).collect())
    }

    /// Meta + ordered turns; `None` if missing (path-implicit ownership). Works
    /// for monitor-kind too, for click-into-trace from the inbox.
    pub async fn conv_get(&self, user_id: &str, conv_id: &str) -> Result<Option<ConvDetail>> {
        let Some(stored): Option<StoredConversation> = self
            .backend
            .read_entity(&conv_key(user_id, conv_id))
            .await?
        else {
            return Ok(None);
        };
        Ok(Some(ConvDetail {
            meta: stored.meta(),
            turns: stored.turns,
        }))
    }

    pub async fn conv_rename(&self, user_id: &str, conv_id: &str, title: &str) -> Result<bool> {
        let key = conv_key(user_id, conv_id);
        if self
            .backend
            .read_entity::<StoredConversation>(&key)
            .await?
            .is_none()
        {
            return Ok(false);
        }
        let now = chrono::Utc::now().timestamp_millis();
        let title = title.to_string();
        self.backend
            .mutate_doc::<StoredConversation, (), _>(&key, |cur| {
                let mut c = cur.ok_or_else(|| anyhow::anyhow!("conversation gone"))?;
                c.title = title.clone();
                c.updated_at = now;
                Ok((c, ()))
            })
            .await?;
        Ok(true)
    }

    pub async fn conv_delete(&self, user_id: &str, conv_id: &str) -> Result<bool> {
        let key = conv_key(user_id, conv_id);
        let existed = self
            .backend
            .read_entity::<StoredConversation>(&key)
            .await?
            .is_some();
        if existed {
            self.backend.delete(&key).await?;
        }
        Ok(existed)
    }

    /// Ownership check before an expensive stream. Ownership is path-implicit,
    /// so this is just an existence check under the user's prefix.
    pub async fn conv_owns(&self, user_id: &str, conv_id: &str) -> Result<bool> {
        Ok(self
            .backend
            .read_entity::<StoredConversation>(&conv_key(user_id, conv_id))
            .await?
            .is_some())
    }

    /// Append a turn with the next `turn_idx`, bumping `updated_at`.
    pub async fn conv_append_turn(
        &self,
        user_id: &str,
        conv_id: &str,
        role: &str,
        payload: &Value,
    ) -> Result<ConvTurn> {
        let now = chrono::Utc::now().timestamp_millis();
        let role = role.to_string();
        let payload = payload.clone();
        let conv_id_owned = conv_id.to_string();
        self.backend
            .mutate_doc::<StoredConversation, ConvTurn, _>(&conv_key(user_id, conv_id), |cur| {
                let mut c =
                    cur.ok_or_else(|| anyhow::anyhow!("conversation {conv_id_owned} not found"))?;
                let turn_idx = c.turns.last().map(|t| t.turn_idx + 1).unwrap_or(0);
                let turn = ConvTurn {
                    id: format!("{conv_id_owned}_t{turn_idx}"),
                    turn_idx,
                    role: role.clone(),
                    payload: payload.clone(),
                    created_at: now,
                };
                c.turns.push(turn.clone());
                c.updated_at = now;
                Ok((c, turn))
            })
            .await
    }
}

// Alerts (per-entity: `alerts/<owner>/<id>.json`). Owner + monitor name are
// denormalized in at create time, so the alert file is self-contained.

use super::alerts::{normalize_severity, Alert, AlertInput, ManualAlertInput};

/// Alerts live in one flat namespace so a public alert is listable for every
/// user. Visibility + per-user toast dismissal are wrapper fields, resolved on read.
#[derive(Serialize, Deserialize)]
struct StoredAlert {
    #[serde(flatten)]
    alert: Alert,
    /// Creator (the raising monitor's owner).
    #[serde(default)]
    owner_user_id: String,
    /// User ids that have dismissed this alert's toast. Per-user so one user
    /// closing their notification doesn't hide it from others.
    #[serde(default)]
    dismissed_by: Vec<String>,
}

impl StoredAlert {
    /// A user sees an alert if they created it or it's public.
    fn visible_to(&self, user_id: &str) -> bool {
        self.alert.public || self.owner_user_id == user_id
    }

    /// Project to the wire `Alert`, stamping the per-user `dismissed` flag.
    fn project(mut self, user_id: &str) -> Alert {
        self.alert.dismissed = self.dismissed_by.iter().any(|u| u == user_id);
        self.alert
    }
}

const ALERTS_PREFIX: &str = "alerts/";

fn alert_key(id: &str) -> String {
    format!("alerts/{id}.json")
}

impl Control {
    /// Alerts visible to `user_id`, newest first, capped to `limit`. `only_unacked`
    /// = inbox; `search` is a case-insensitive substring; `monitor` restricts to one.
    pub async fn alert_list(
        &self,
        user_id: &str,
        only_unacked: bool,
        monitor: Option<&str>,
        search: Option<&str>,
        limit: usize,
    ) -> Result<Vec<Alert>> {
        let needle = search
            .map(|q| q.trim().to_lowercase())
            .filter(|q| !q.is_empty());

        let mut items: Vec<Alert> = self
            .backend
            .list_entities::<StoredAlert>(ALERTS_PREFIX)
            .await?
            .into_iter()
            .filter(|st| st.visible_to(user_id))
            .filter(|st| !only_unacked || !st.alert.acknowledged)
            .filter(|st| monitor.is_none_or(|m| st.alert.monitor_id == m))
            .filter(|st| match &needle {
                None => true,
                Some(n) => {
                    let a = &st.alert;
                    a.title.to_lowercase().contains(n)
                        || a.summary.to_lowercase().contains(n)
                        || a.monitor_name.to_lowercase().contains(n)
                        || a.env.to_lowercase().contains(n)
                }
            })
            .map(|st| st.project(user_id))
            .collect();
        items.sort_by_key(|b| std::cmp::Reverse(b.created_at));
        items.truncate(limit);
        Ok(items)
    }

    /// Unacknowledged count (own + public) for the nav badge.
    pub async fn alert_unacked_count(&self, user_id: &str) -> Result<i64> {
        let items: Vec<StoredAlert> = self.backend.list_entities(ALERTS_PREFIX).await?;
        Ok(items
            .iter()
            .filter(|st| st.visible_to(user_id) && !st.alert.acknowledged)
            .count() as i64)
    }

    /// Alerts raised by one monitor, visible to `user_id`. Newest first, capped
    /// at 200.
    pub async fn alert_list_for_monitor(
        &self,
        user_id: &str,
        monitor_id: &str,
    ) -> Result<Vec<Alert>> {
        let mut items: Vec<Alert> = self
            .backend
            .list_entities::<StoredAlert>(ALERTS_PREFIX)
            .await?
            .into_iter()
            .filter(|st| st.alert.monitor_id == monitor_id && st.visible_to(user_id))
            .map(|st| st.project(user_id))
            .collect();
        items.sort_by_key(|b| std::cmp::Reverse(b.created_at));
        items.truncate(200);
        Ok(items)
    }

    /// Recent alert titles for a monitor — fed to the agent so it can dedupe
    /// repeat findings. Newest first, capped at `limit`.
    pub async fn alert_recent_titles_for_monitor(
        &self,
        user_id: &str,
        monitor_id: &str,
        limit: usize,
    ) -> Result<Vec<String>> {
        Ok(self
            .alert_list_for_monitor(user_id, monitor_id)
            .await?
            .into_iter()
            .take(limit)
            .map(|a| a.title)
            .collect())
    }

    /// Create an alert from a `raise_alert` tool call. Resolves owner + monitor
    /// name + visibility from the monitor file and denormalizes them in.
    pub async fn alert_create(&self, input: AlertInput) -> Result<Alert> {
        if input.title.trim().is_empty() {
            bail!("alert title is required");
        }
        let mon: StoredMonitor = self
            .backend
            .read_entity(&monitor_key(&input.monitor_id))
            .await?
            .ok_or_else(|| anyhow::anyhow!("monitor {} not found for alert", input.monitor_id))?;
        let now = chrono::Utc::now().timestamp_millis();
        let alert = Alert {
            id: format!("alt_{now:013}_{:08x}", crate::crypto::rand::u32()),
            monitor_id: input.monitor_id.clone(),
            monitor_name: mon.monitor.name.clone(),
            env: mon.monitor.env.clone(),
            conversation_id: input.conversation_id,
            severity: normalize_severity(&input.severity),
            title: input.title.trim().to_string(),
            summary: input.summary.trim().to_string(),
            evidence: input.evidence.clone(),
            public: mon.monitor.public,
            acknowledged: false,
            acknowledged_at: None,
            dismissed: false,
            created_at: now,
        };
        let stored = StoredAlert {
            alert: alert.clone(),
            owner_user_id: mon.owner_user_id.clone(),
            dismissed_by: Vec::new(),
        };
        self.backend.put_doc(&alert_key(&alert.id), &stored).await?;
        // Best-effort webhook delivery; never blocks or fails alert creation.
        crate::notify::spawn_dispatch(self.clone(), alert.clone(), mon.monitor.notify.clone());
        Ok(alert)
    }

    /// Create an alert with no backing monitor (agent chat / MCP tools). Owner
    /// and visibility come from the caller; `monitor_id` stays empty.
    pub async fn alert_create_manual(
        &self,
        owner_user_id: &str,
        input: ManualAlertInput,
    ) -> Result<Alert> {
        if input.title.trim().is_empty() {
            bail!("alert title is required");
        }
        let now = chrono::Utc::now().timestamp_millis();
        let alert = Alert {
            id: format!("alt_{now:013}_{:08x}", crate::crypto::rand::u32()),
            monitor_id: String::new(),
            monitor_name: input.source,
            env: input.env,
            conversation_id: None,
            severity: normalize_severity(&input.severity),
            title: input.title.trim().to_string(),
            summary: input.summary.trim().to_string(),
            evidence: input.evidence,
            public: input.public,
            acknowledged: false,
            acknowledged_at: None,
            dismissed: false,
            created_at: now,
        };
        let stored = StoredAlert {
            alert: alert.clone(),
            owner_user_id: owner_user_id.to_string(),
            dismissed_by: Vec::new(),
        };
        self.backend.put_doc(&alert_key(&alert.id), &stored).await?;
        // Same best-effort webhook path as monitor alerts, default target only.
        crate::notify::spawn_dispatch(self.clone(), alert.clone(), None);
        Ok(alert)
    }

    /// Mark acknowledged. Shared for public alerts — any user who can see it may
    /// ack it for everyone. `false` if it doesn't exist or isn't visible.
    pub async fn alert_acknowledge(&self, user_id: &str, id: &str) -> Result<bool> {
        let key = alert_key(id);
        match self.backend.read_entity::<StoredAlert>(&key).await? {
            Some(st) if st.visible_to(user_id) => {}
            _ => return Ok(false),
        }
        let now = chrono::Utc::now().timestamp_millis();
        self.backend
            .mutate_doc::<StoredAlert, (), _>(&key, |cur| {
                let mut st = cur.ok_or_else(|| anyhow::anyhow!("alert gone"))?;
                st.alert.acknowledged = true;
                st.alert.acknowledged_at = Some(now);
                Ok((st, ()))
            })
            .await?;
        Ok(true)
    }

    /// Add `user_id` to the alert's per-user toast-dismissal set. Idempotent.
    /// `false` if it doesn't exist or isn't visible to the user.
    pub async fn alert_dismiss(&self, user_id: &str, id: &str) -> Result<bool> {
        let key = alert_key(id);
        match self.backend.read_entity::<StoredAlert>(&key).await? {
            Some(st) if st.visible_to(user_id) => {}
            _ => return Ok(false),
        }
        let uid = user_id.to_string();
        self.backend
            .mutate_doc::<StoredAlert, (), _>(&key, |cur| {
                let mut st = cur.ok_or_else(|| anyhow::anyhow!("alert gone"))?;
                if !st.dismissed_by.contains(&uid) {
                    st.dismissed_by.push(uid.clone());
                }
                Ok((st, ()))
            })
            .await?;
        Ok(true)
    }

    /// Dismiss every visible, unacknowledged alert the user hasn't already
    /// dismissed (the "dismiss all toasts" action). Returns how many flipped.
    pub async fn alert_dismiss_all(&self, user_id: &str) -> Result<u64> {
        let pending: Vec<String> = self
            .backend
            .list_entities::<StoredAlert>(ALERTS_PREFIX)
            .await?
            .into_iter()
            .filter(|st| {
                st.visible_to(user_id)
                    && !st.alert.acknowledged
                    && !st.dismissed_by.iter().any(|u| u == user_id)
            })
            .map(|st| st.alert.id)
            .collect();
        let mut n = 0u64;
        for id in pending {
            if self.alert_dismiss(user_id, &id).await? {
                n += 1;
            }
        }
        Ok(n)
    }
}

/// `conv_<zero-padded-millis>_<rand>` — timestamp-prefixed so lexicographic key
/// order (S3 prefix listing) is chronological order.
fn conv_id(now_millis: i64) -> String {
    format!("conv_{now_millis:013}_{:08x}", crate::crypto::rand::u32())
}

/// `<prefix>_{micros_hex}{rand_hex}` — time-ordered + collision-resistant at the
/// human-driven write rate.
fn new_id(prefix: &str) -> String {
    let ts = chrono::Utc::now().timestamp_micros();
    format!("{prefix}_{ts:x}{:08x}", crate::crypto::rand::u32())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::control::store::FsControlStore;
    use tempfile::TempDir;

    fn control(dir: &TempDir) -> Control {
        let store = Arc::new(FsControlStore::new(dir.path()));
        let crypto = Arc::new(Crypto::Disabled);
        Control::new(store, crypto)
    }

    #[tokio::test]
    async fn manual_alert_create_list_ack() {
        let dir = TempDir::new().unwrap();
        let c = control(&dir);
        let a = c
            .alert_create_manual(
                "u1",
                ManualAlertInput {
                    source: "agent".into(),
                    env: "default".into(),
                    public: true,
                    severity: "critical".into(),
                    title: "  api 5xx spike  ".into(),
                    summary: "errors tripled".into(),
                    evidence: None,
                },
            )
            .await
            .unwrap();
        assert_eq!(a.severity, "high"); // normalized
        assert_eq!(a.title, "api 5xx spike"); // trimmed
        assert_eq!(a.monitor_id, "");
        assert_eq!(a.monitor_name, "agent");

        // Public → visible to another user, who may ack it for everyone.
        let listed = c.alert_list("u2", true, None, None, 50).await.unwrap();
        assert_eq!(listed.len(), 1);
        assert!(c.alert_acknowledge("u2", &a.id).await.unwrap());
        assert_eq!(c.alert_unacked_count("u1").await.unwrap(), 0);

        // Private manual alert: invisible to (and not ackable by) others.
        let b = c
            .alert_create_manual(
                "u1",
                ManualAlertInput {
                    source: "mcp".into(),
                    env: "default".into(),
                    public: false,
                    severity: "low".into(),
                    title: "t".into(),
                    summary: String::new(),
                    evidence: None,
                },
            )
            .await
            .unwrap();
        assert!(!c.alert_acknowledge("u2", &b.id).await.unwrap());
        assert!(c
            .alert_list("u2", false, None, None, 50)
            .await
            .unwrap()
            .iter()
            .all(|x| x.id != b.id));
    }

    #[tokio::test]
    async fn saml_replay_records_once_and_prunes() {
        let dir = TempDir::new().unwrap();
        let c = control(&dir);
        let now = 1_000i64;
        // First consume → accepted; immediate re-use → rejected.
        assert!(c
            .saml_replay_check_and_record("_a1", 2_000, now)
            .await
            .unwrap());
        assert!(!c
            .saml_replay_check_and_record("_a1", 2_000, now)
            .await
            .unwrap());
        // A different assertion is independent.
        assert!(c
            .saml_replay_check_and_record("_a2", 2_000, now)
            .await
            .unwrap());
        // After both expire, a later insert prunes them and the id is reusable.
        assert!(c
            .saml_replay_check_and_record("_a1", 9_999, 3_000)
            .await
            .unwrap());
    }

    #[tokio::test]
    async fn saml_settings_roundtrip_and_find_user_by_login() {
        let dir = TempDir::new().unwrap();
        let c = control(&dir);
        c.set_setting(KEY_SAML_ENABLED, "true").await.unwrap();
        c.set_setting(KEY_SAML_SP_ENTITY_ID, "https://sp/meta")
            .await
            .unwrap();
        let cfg = c.saml_settings().await.unwrap();
        assert!(cfg.enabled);
        assert_eq!(cfg.sp_entity_id, "https://sp/meta");
        assert_eq!(
            cfg.button_label,
            crate::saml::SamlConfig::default_button_label()
        );

        // Password-less lookup matches by email then userid, no creation.
        let u = c
            .create_user("alice", "alice@example.com", "Alice", "pw", false)
            .await
            .unwrap();
        let by_email = c.find_user_by_login("ALICE@example.com").await.unwrap();
        assert_eq!(by_email.unwrap().id, u.id);
        let by_userid = c.find_user_by_login("alice").await.unwrap();
        assert_eq!(by_userid.unwrap().id, u.id);
        assert!(c
            .find_user_by_login("nobody@example.com")
            .await
            .unwrap()
            .is_none());
    }

    #[tokio::test]
    async fn compactor_lease_single_holder() {
        let dir = TempDir::new().unwrap();
        let c = control(&dir);
        let ttl = Duration::from_secs(60);
        // A free lease is acquired, then renewed by the same node.
        assert!(c.acquire_compactor_lease("node-a", ttl).await.unwrap());
        assert!(c.acquire_compactor_lease("node-a", ttl).await.unwrap());
        // Another node can't steal a fresh lease — and the holder keeps it.
        assert!(!c.acquire_compactor_lease("node-b", ttl).await.unwrap());
        assert!(c.acquire_compactor_lease("node-a", ttl).await.unwrap());
    }

    #[tokio::test]
    async fn compactor_lease_reclaimed_when_stale() {
        let dir = TempDir::new().unwrap();
        let c = control(&dir);
        let ttl = Duration::from_millis(1); // goes stale almost immediately
        assert!(c.acquire_compactor_lease("node-a", ttl).await.unwrap());
        tokio::time::sleep(Duration::from_millis(10)).await;
        // node-a's lease has lapsed → another node reclaims it.
        assert!(c.acquire_compactor_lease("node-b", ttl).await.unwrap());
    }

    #[test]
    fn lease_staleness_boundary() {
        let now = chrono::Utc::now();
        let ttl = Duration::from_secs(30);
        let fresh = (now - chrono::Duration::seconds(10)).to_rfc3339();
        let old = (now - chrono::Duration::seconds(45)).to_rfc3339();
        assert!(!lease_is_stale(&fresh, now, ttl));
        assert!(lease_is_stale(&old, now, ttl));
        assert!(lease_is_stale("not-a-timestamp", now, ttl)); // unparseable → reclaim
    }

    #[tokio::test]
    async fn create_authenticate_and_enforce_uniqueness() {
        let dir = TempDir::new().unwrap();
        let c = control(&dir);
        assert_eq!(c.user_count().await.unwrap(), 0);

        let admin = c
            .create_user(
                "admin",
                "admin@localhost",
                "Administrator",
                "s3cret-pw",
                true,
            )
            .await
            .unwrap();
        assert!(admin.is_admin);
        assert_eq!(c.user_count().await.unwrap(), 1);

        // userid + email are unique, case-insensitively.
        assert!(c
            .create_user("ADMIN", "other@localhost", "Dup", "pw", false)
            .await
            .is_err());
        assert!(c
            .create_user("other", "Admin@LOCALHOST", "Dup", "pw", false)
            .await
            .is_err());
        assert_eq!(c.user_count().await.unwrap(), 1);

        // auth by userid or email, case-insensitive, wrong password rejected.
        assert!(c
            .authenticate("admin", "s3cret-pw")
            .await
            .unwrap()
            .is_some());
        assert!(c
            .authenticate("ADMIN", "s3cret-pw")
            .await
            .unwrap()
            .is_some());
        assert!(c
            .authenticate("admin@localhost", "s3cret-pw")
            .await
            .unwrap()
            .is_some());
        assert!(c.authenticate("admin", "wrong").await.unwrap().is_none());
        assert!(c.authenticate("ghost", "x").await.unwrap().is_none());
    }

    #[tokio::test]
    async fn regenerate_password_revokes_and_replaces() {
        let dir = TempDir::new().unwrap();
        let c = control(&dir);
        let u = c
            .create_user("eve", "eve@example.com", "Eve", "old-password", false)
            .await
            .unwrap();
        let cv0 = c
            .get_user_by_id(&u.id)
            .await
            .unwrap()
            .unwrap()
            .credentials_version;
        let new = c.regenerate_password(&u.id).await.unwrap();
        assert!(c
            .authenticate("eve", "old-password")
            .await
            .unwrap()
            .is_none());
        assert!(c.authenticate("eve", &new).await.unwrap().is_some());
        let cv1 = c
            .get_user_by_id(&u.id)
            .await
            .unwrap()
            .unwrap()
            .credentials_version;
        assert_eq!(cv1, cv0 + 1, "credentials_version must bump");
    }

    #[tokio::test]
    async fn update_list_delete_and_rbac() {
        let dir = TempDir::new().unwrap();
        let c = control(&dir);
        let a = c
            .create_user("a", "a@x.com", "A", "pw", true)
            .await
            .unwrap();
        let b = c
            .create_user("b", "b@x.com", "B", "pw", false)
            .await
            .unwrap();

        // list is oldest-first.
        let ids: Vec<String> = c
            .list_users()
            .await
            .unwrap()
            .into_iter()
            .map(|u| u.id)
            .collect();
        assert_eq!(ids, vec![a.id.clone(), b.id.clone()]);

        // email uniqueness on update.
        assert!(c
            .update_user(&b.id, Some("a@x.com"), None, None)
            .await
            .is_err());
        let b2 = c
            .update_user(&b.id, Some("b2@x.com"), Some("Bravo"), Some(true))
            .await
            .unwrap();
        assert_eq!(b2.email, "b2@x.com");
        assert_eq!(b2.display_name, "Bravo");
        assert!(b2.is_admin);

        // RBAC allowlist round-trips, empty/blank rules dropped.
        c.set_user_allowed(
            &b.id,
            &[
                EnvIndexAllow {
                    env: "prod".into(),
                    indexes: vec!["orders-*".into()],
                },
                EnvIndexAllow {
                    env: "  ".into(),
                    indexes: vec!["x".into()],
                },
                EnvIndexAllow {
                    env: "dev".into(),
                    indexes: vec![" ".into()],
                },
            ],
        )
        .await
        .unwrap();
        let allowed = c.user_allowed(&b.id).await.unwrap();
        assert_eq!(allowed.len(), 1);
        assert_eq!(allowed[0].env, "prod");
        assert_eq!(allowed[0].indexes, vec!["orders-*"]);
        assert!(c.user_has_env(&b.id, "prod").await.unwrap());
        assert!(!c.user_has_env(&b.id, "dev").await.unwrap());

        // delete.
        assert!(c.delete_user(&b.id).await.unwrap());
        assert!(!c.delete_user(&b.id).await.unwrap());
        assert_eq!(c.user_count().await.unwrap(), 1);
    }

    #[tokio::test]
    async fn survives_reopen_and_encrypts_at_rest() {
        let dir = TempDir::new().unwrap();
        // Encrypted store this time.
        let crypto = Arc::new(Crypto::Aes(
            crate::crypto::AeadKey::new(&[3u8; 32]).unwrap(),
        ));
        {
            let store = Arc::new(FsControlStore::new(dir.path()));
            let c = Control::new(store, crypto.clone());
            c.create_user("bob", "bob@x.com", "Bob", "hunter2", false)
                .await
                .unwrap();
        }
        // The on-disk file must not contain the plaintext password hash marker.
        let raw = std::fs::read_to_string(dir.path().join(USERS_KEY)).unwrap();
        assert!(raw.contains("aes-256-gcm"));
        assert!(!raw.contains("bob@x.com"));
        // Reopen with the same key and read back.
        let store = Arc::new(FsControlStore::new(dir.path()));
        let c = Control::new(store, crypto);
        assert!(c.authenticate("bob", "hunter2").await.unwrap().is_some());
    }

    #[tokio::test]
    async fn envs_create_list_and_guarded_delete() {
        let dir = TempDir::new().unwrap();
        let c = control(&dir);
        c.upsert_env(DEFAULT_ENV, false).await.unwrap();
        c.upsert_env(SYSTEM_ENV, true).await.unwrap();
        c.upsert_env(DEFAULT_ENV, false).await.unwrap(); // idempotent

        let dev = c.create_env("dev").await.unwrap();
        assert_eq!(dev.name, "dev");
        assert!(c.create_env("dev").await.is_err()); // dup
        assert!(c.create_env("_reserved").await.is_err()); // reserved prefix
        assert!(c.create_env("bad name").await.is_err()); // invalid

        // include_system toggles visibility; user envs sort before system.
        assert_eq!(c.list_envs(false).await.unwrap().len(), 2); // default, dev
        assert_eq!(c.list_envs(true).await.unwrap().len(), 3);
        assert!(c.env_exists("dev").await.unwrap());
        assert!(!c.env_exists("ghost").await.unwrap());

        // reserved/system can't be deleted; empty env can.
        assert!(c.delete_env_if_no_control_rows(DEFAULT_ENV).await.is_err());
        assert!(c.delete_env_if_no_control_rows(SYSTEM_ENV).await.is_err());
        c.delete_env_if_no_control_rows("dev").await.unwrap();
        assert!(!c.env_exists("dev").await.unwrap());

        // a saved search pins its env and blocks deletion.
        c.create_env("prod").await.unwrap();
        c.saved_create(
            "u1",
            "prod",
            SavedSearchInput {
                name: "s".into(),
                ..Default::default()
            },
        )
        .await
        .unwrap();
        assert!(c.delete_env_if_no_control_rows("prod").await.is_err());
    }

    #[tokio::test]
    async fn settings_kv_and_mcp_view() {
        let dir = TempDir::new().unwrap();
        let c = control(&dir);
        assert!(c.get_setting("x").await.unwrap().is_none());
        c.set_setting("x", "1").await.unwrap();
        c.set_setting("y", "").await.unwrap();
        assert_eq!(c.get_setting("x").await.unwrap().as_deref(), Some("1"));
        assert_eq!(c.get_setting("y").await.unwrap().as_deref(), Some("")); // empty != unset
        c.unset_setting("x").await.unwrap();
        assert!(c.get_setting("x").await.unwrap().is_none());

        // MCP typed view defaults, then reflects writes.
        let s = c.mcp_settings().await.unwrap();
        assert!(!s.enabled);
        assert!(s.indexes_unrestricted());
        c.set_setting(KEY_MCP_ENABLED, "true").await.unwrap();
        c.set_setting(
            KEY_MCP_ALLOWED_INDEXES,
            r#"[{"env":"prod","indexes":["orders-*"]}]"#,
        )
        .await
        .unwrap();
        let s = c.mcp_settings().await.unwrap();
        assert!(s.enabled);
        assert!(s.allows("prod", "orders-api"));
        assert!(!s.allows("dev", "orders-api"));
    }

    #[tokio::test]
    async fn saved_visibility_and_ownership() {
        let dir = TempDir::new().unwrap();
        let c = control(&dir);
        let a_priv = c
            .saved_create(
                "alice",
                "default",
                SavedSearchInput {
                    name: "a".into(),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        c.saved_create(
            "alice",
            "default",
            SavedSearchInput {
                name: "pub".into(),
                public: true,
                ..Default::default()
            },
        )
        .await
        .unwrap();
        // env isolation.
        c.saved_create(
            "alice",
            "prod",
            SavedSearchInput {
                name: "p".into(),
                ..Default::default()
            },
        )
        .await
        .unwrap();

        // bob sees only the public row in `default`.
        let bob = c.saved_list("bob", "default").await.unwrap();
        assert_eq!(bob.len(), 1);
        assert_eq!(bob[0].name, "pub");
        // alice sees both of her default-env rows.
        assert_eq!(c.saved_list("alice", "default").await.unwrap().len(), 2);
        assert_eq!(c.saved_list("alice", "prod").await.unwrap().len(), 1);

        // bob can't update/delete alice's private row (looks "not found").
        assert!(c
            .saved_update(
                "bob",
                &a_priv.id,
                SavedSearchPatch {
                    name: Some("x".into()),
                    ..Default::default()
                },
                false,
            )
            .await
            .is_err());
        assert!(c.saved_delete("bob", &a_priv.id, false).await.is_err());
        // an admin can edit + delete anyone's private row.
        let adm = c
            .saved_update(
                "bob",
                &a_priv.id,
                SavedSearchPatch {
                    name: Some("by-admin".into()),
                    ..Default::default()
                },
                true,
            )
            .await
            .unwrap();
        assert_eq!(adm.name, "by-admin");
        // alice can.
        let upd = c
            .saved_update(
                "alice",
                &a_priv.id,
                SavedSearchPatch {
                    q: Some("level:error".into()),
                    ..Default::default()
                },
                false,
            )
            .await
            .unwrap();
        assert_eq!(upd.q, "level:error");
        c.saved_delete("alice", &a_priv.id, false).await.unwrap();
        assert_eq!(c.saved_list("alice", "default").await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn list_skips_non_json_files_like_ds_store() {
        let dir = TempDir::new().unwrap();
        let c = control(&dir);
        let m = c
            .monitor_create(
                "alice",
                "default",
                MonitorInput {
                    name: "w".into(),
                    prompt: "p".into(),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        let a = c
            .alert_create(AlertInput {
                monitor_id: m.id.clone(),
                title: "boom".into(),
                ..Default::default()
            })
            .await
            .unwrap();

        // Drop a non-.json copy (like a macOS .DS_Store): the `.json` filter must
        // ignore it rather than read it as a duplicate alert.
        let alerts_dir = dir.path().join("alerts");
        std::fs::copy(
            alerts_dir.join(format!("{}.json", a.id)),
            alerts_dir.join(".DS_Store"),
        )
        .unwrap();

        let listed = c.alert_list("alice", true, None, None, 100).await.unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(c.alert_unacked_count("alice").await.unwrap(), 1);
    }

    #[tokio::test]
    async fn admin_view_all_lists_every_owner_with_label() {
        let dir = TempDir::new().unwrap();
        let c = control(&dir);
        // A real user so the owner column resolves to a display name.
        let alice = c
            .create_user("alice", "alice@x.io", "Alice A", "pw123456", false)
            .await
            .unwrap();
        // Alice's PRIVATE search; bob's normal list shouldn't see it.
        c.saved_create(
            &alice.id,
            "default",
            SavedSearchInput {
                name: "secret".into(),
                ..Default::default()
            },
        )
        .await
        .unwrap();
        // A search owned by an id with no user record → label falls back to the id.
        c.saved_create(
            "ghost",
            "default",
            SavedSearchInput {
                name: "orphan".into(),
                ..Default::default()
            },
        )
        .await
        .unwrap();

        assert_eq!(c.saved_list("bob", "default").await.unwrap().len(), 0);
        let all = c.saved_list_all("default").await.unwrap();
        assert_eq!(all.len(), 2);
        let secret = all.iter().find(|s| s.name == "secret").unwrap();
        assert_eq!(secret.owner.as_deref(), Some("Alice A"));
        let orphan = all.iter().find(|s| s.name == "orphan").unwrap();
        assert_eq!(orphan.owner.as_deref(), Some("ghost"));
        // env scoping still applies to the admin view.
        assert_eq!(c.saved_list_all("prod").await.unwrap().len(), 0);
    }

    #[tokio::test]
    async fn dashboard_crud_visibility_and_ownership() {
        let dir = TempDir::new().unwrap();
        let c = control(&dir);
        let priv_d = c
            .dashboard_create(
                "alice",
                DashboardInput {
                    name: "ops".into(),
                    spec: serde_json::json!({ "time_range": "-24h", "widgets": [] }),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        c.dashboard_create(
            "alice",
            DashboardInput {
                name: "shared".into(),
                public: true,
                ..Default::default()
            },
        )
        .await
        .unwrap();

        // alice sees both; bob sees only the public one (across all envs).
        assert_eq!(c.dashboard_list("alice").await.unwrap().len(), 2);
        let bob = c.dashboard_list("bob").await.unwrap();
        assert_eq!(bob.len(), 1);
        assert_eq!(bob[0].name, "shared");

        // get respects ownership; bob can't see alice's private one, but an
        // admin can open + patch it.
        assert_eq!(
            c.dashboard_get("alice", &priv_d.id, false)
                .await
                .unwrap()
                .name,
            "ops"
        );
        assert!(c.dashboard_get("bob", &priv_d.id, false).await.is_err());
        assert_eq!(
            c.dashboard_get("bob", &priv_d.id, true).await.unwrap().name,
            "ops"
        );

        // bob can't patch/delete alice's private row (looks "not found").
        assert!(c
            .dashboard_update(
                "bob",
                &priv_d.id,
                DashboardPatch {
                    name: Some("x".into()),
                    ..Default::default()
                },
                false,
            )
            .await
            .is_err());
        assert!(c.dashboard_delete("bob", &priv_d.id, false).await.is_err());
        // …but an admin can.
        c.dashboard_update(
            "bob",
            &priv_d.id,
            DashboardPatch {
                name: Some("renamed-by-admin".into()),
                ..Default::default()
            },
            true,
        )
        .await
        .unwrap();

        // alice can patch the spec, then delete.
        let upd = c
            .dashboard_update(
                "alice",
                &priv_d.id,
                DashboardPatch {
                    spec: Some(
                        serde_json::json!({ "time_range": "-7d", "widgets": [{"id":"w1"}] }),
                    ),
                    ..Default::default()
                },
                false,
            )
            .await
            .unwrap();
        assert_eq!(upd.spec["time_range"], "-7d");
        c.dashboard_delete("alice", &priv_d.id, false)
            .await
            .unwrap();
        assert_eq!(c.dashboard_list("alice").await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn monitors_crud_and_lease() {
        let dir = TempDir::new().unwrap();
        let c = control(&dir);
        let m = c
            .monitor_create(
                "u1",
                "prod",
                MonitorInput {
                    name: "watch".into(),
                    prompt: "look".into(),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        // interval clamps to the floor.
        assert_eq!(
            m.interval_seconds,
            MIN_INTERVAL_SECONDS.max(DEFAULT_INTERVAL_SECONDS)
        );

        // not env-scoped: u1 sees it regardless of env.
        assert_eq!(c.monitor_list("u1").await.unwrap().len(), 1);
        assert_eq!(c.monitor_list("u2").await.unwrap().len(), 0);
        assert!(c.monitor_get("u1", &m.id, false).await.unwrap().is_some());
        assert!(c.monitor_get("u2", &m.id, false).await.unwrap().is_none()); // ownership
        assert!(c.monitor_get("u2", &m.id, true).await.unwrap().is_some()); // admin bypass

        // lease is exclusive; second attempt fails until finished.
        assert!(c.monitor_try_lease(&m.id).await.unwrap());
        assert!(!c.monitor_try_lease(&m.id).await.unwrap());
        c.monitor_finish_run(&m.id, "ok", None, Some("conv_1".into()))
            .await
            .unwrap();
        let after = c.monitor_get("u1", &m.id, false).await.unwrap().unwrap();
        assert!(!after.running);
        assert_eq!(after.last_status.as_deref(), Some("ok"));
        assert_eq!(after.last_conversation_id.as_deref(), Some("conv_1"));
        // leasable again after finish.
        assert!(c.monitor_try_lease(&m.id).await.unwrap());

        // list_all exposes owner; ownership enforced on delete.
        let all = c.monitor_list_all().await.unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].0, "u1");
        assert!(c.monitor_delete("u2", &m.id, false).await.is_err());
        // admin can update + delete another user's monitor.
        c.monitor_update(
            "u2",
            &m.id,
            MonitorPatch {
                enabled: Some(false),
                ..Default::default()
            },
            true,
        )
        .await
        .unwrap();
        c.monitor_delete("u2", &m.id, true).await.unwrap();
        assert_eq!(c.monitor_list("u1").await.unwrap().len(), 0);
    }

    #[tokio::test]
    async fn ingest_token_crud_and_auth_lookup() {
        let dir = TempDir::new().unwrap();
        let c = control(&dir);

        // Defaults: open ingest, no tokens.
        let auth = c.ingest_auth().await.unwrap();
        assert!(!auth.require);
        assert!(auth.tokens.is_empty());

        // Validation.
        assert!(c.ingest_token_create("", "prod", vec![]).await.is_err());
        assert!(c.ingest_token_create("ok", "  ", vec![]).await.is_err());

        // Create mints a prefixed secret and drops blank index entries.
        let tok = c
            .ingest_token_create("shipper", "prod", vec!["orders".into(), "  ".into()])
            .await
            .unwrap();
        assert!(tok.token.starts_with("hli_"));
        assert_eq!(tok.indexes, vec!["orders".to_string()]);

        // Lookup by secret returns the enabled token.
        assert_eq!(
            c.ingest_token_find(&tok.token).await.unwrap().map(|t| t.id),
            Some(tok.id.clone())
        );
        assert!(c.ingest_token_find("hli_nope").await.unwrap().is_none());

        // Disabling hides it from the auth lookup.
        c.ingest_token_set_enabled(&tok.id, false).await.unwrap();
        assert!(c.ingest_token_find(&tok.token).await.unwrap().is_none());

        // Require toggle persists.
        c.ingest_set_require(true).await.unwrap();
        assert!(c.ingest_auth().await.unwrap().require);

        // Delete removes it.
        c.ingest_token_delete(&tok.id).await.unwrap();
        assert!(c.ingest_auth().await.unwrap().tokens.is_empty());
    }

    #[tokio::test]
    async fn monitor_breaching_state_and_clear_last_run() {
        let dir = TempDir::new().unwrap();
        let c = control(&dir);
        let m = c
            .monitor_create(
                "u1",
                "prod",
                MonitorInput {
                    name: "threshold".into(),
                    kind: MonitorKind::Threshold,
                    prompt: String::new(),
                    threshold: Some(ThresholdConfig {
                        query: "status:>=500".into(),
                        index: None,
                        window_seconds: 900,
                        comparison: crate::control::monitors::Comparison::Gt,
                        threshold: 50,
                        severity: "high".into(),
                    }),
                    ..Default::default()
                },
            )
            .await
            .unwrap();

        // Edge-trigger state round-trips through the store.
        c.monitor_set_breaching(&m.id, true).await.unwrap();
        let got = c.monitor_get("u1", &m.id, false).await.unwrap().unwrap();
        assert_eq!(got.last_breaching, Some(true));
        c.monitor_set_breaching(&m.id, false).await.unwrap();
        let got = c.monitor_get("u1", &m.id, false).await.unwrap().unwrap();
        assert_eq!(got.last_breaching, Some(false));

        // A finished run stamps last_run_at; clear_last_run resets it so the
        // next tick treats the monitor as due ("run now").
        c.monitor_finish_run(&m.id, "ok", None, None).await.unwrap();
        assert!(c
            .monitor_get("u1", &m.id, false)
            .await
            .unwrap()
            .unwrap()
            .last_run_at
            .is_some());
        c.monitor_clear_last_run(&m.id).await.unwrap();
        assert!(c
            .monitor_get("u1", &m.id, false)
            .await
            .unwrap()
            .unwrap()
            .last_run_at
            .is_none());
    }

    #[tokio::test]
    async fn public_monitor_is_fully_shared() {
        let dir = TempDir::new().unwrap();
        let c = control(&dir);
        let m = c
            .monitor_create(
                "u1",
                "default",
                MonitorInput {
                    name: "shared".into(),
                    prompt: "look".into(),
                    public: true,
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        // u2 sees it in their list and can fetch it (no admin flag needed).
        assert_eq!(c.monitor_list("u2").await.unwrap().len(), 1);
        assert!(c.monitor_get("u2", &m.id, false).await.unwrap().is_some());
        // u2 can edit it…
        let upd = c
            .monitor_update(
                "u2",
                &m.id,
                MonitorPatch {
                    name: Some("renamed".into()),
                    ..Default::default()
                },
                false,
            )
            .await
            .unwrap();
        assert_eq!(upd.name, "renamed");
        // …and delete it.
        c.monitor_delete("u2", &m.id, false).await.unwrap();
        assert_eq!(c.monitor_list("u1").await.unwrap().len(), 0);
    }

    #[tokio::test]
    async fn conversations_user_scoped_with_turns() {
        let dir = TempDir::new().unwrap();
        let c = control(&dir);
        let chat = c.conv_create("alice", "First chat").await.unwrap();
        assert!(chat.id.starts_with("conv_"));
        // monitor-kind conversation is hidden from the chat list.
        let mon = c
            .conv_create_with_kind("alice", "run trace", "monitor")
            .await
            .unwrap();

        let list = c.conv_list("alice").await.unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].id, chat.id);
        // bob can't see alice's conversation (path-scoped ownership).
        assert!(c.conv_get("bob", &chat.id).await.unwrap().is_none());
        assert!(!c.conv_owns("bob", &chat.id).await.unwrap());
        assert!(c.conv_owns("alice", &chat.id).await.unwrap());
        // monitor trace still fetchable by id.
        assert!(c.conv_get("alice", &mon.id).await.unwrap().is_some());

        // append turns get sequential idx; updated_at advances.
        let t0 = c
            .conv_append_turn(
                "alice",
                &chat.id,
                "user",
                &serde_json::json!({"content":"hi"}),
            )
            .await
            .unwrap();
        let t1 = c
            .conv_append_turn(
                "alice",
                &chat.id,
                "assistant",
                &serde_json::json!({"content":"yo"}),
            )
            .await
            .unwrap();
        assert_eq!(t0.turn_idx, 0);
        assert_eq!(t1.turn_idx, 1);
        let detail = c.conv_get("alice", &chat.id).await.unwrap().unwrap();
        assert_eq!(detail.turns.len(), 2);
        assert_eq!(detail.meta.updated_at, t1.created_at);

        // rename + delete return existence bools.
        assert!(c.conv_rename("alice", &chat.id, "Renamed").await.unwrap());
        assert!(!c.conv_rename("bob", &chat.id, "x").await.unwrap());
        assert_eq!(
            c.conv_get("alice", &chat.id)
                .await
                .unwrap()
                .unwrap()
                .meta
                .title,
            "Renamed"
        );
        assert!(c.conv_delete("alice", &chat.id).await.unwrap());
        assert!(!c.conv_delete("alice", &chat.id).await.unwrap());
        assert_eq!(c.conv_list("alice").await.unwrap().len(), 0);
    }

    #[tokio::test]
    async fn alerts_owner_scoped_and_ack() {
        let dir = TempDir::new().unwrap();
        let c = control(&dir);
        // an alert resolves its owner + monitor name from the monitor.
        let m = c
            .monitor_create(
                "alice",
                "prod",
                MonitorInput {
                    name: "errwatch".into(),
                    prompt: "p".into(),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        // unknown monitor → error.
        assert!(c
            .alert_create(AlertInput {
                monitor_id: "nope".into(),
                title: "t".into(),
                ..Default::default()
            })
            .await
            .is_err());

        let a = c
            .alert_create(AlertInput {
                monitor_id: m.id.clone(),
                severity: "critical".into(),
                title: "disk full".into(),
                ..Default::default()
            })
            .await
            .unwrap();
        assert!(a.id.starts_with("alt_"));
        assert_eq!(a.monitor_name, "errwatch");
        assert_eq!(a.severity, "high"); // normalized
        assert!(!a.acknowledged);

        // owner sees it; another user's prefix is empty.
        assert_eq!(
            c.alert_list("alice", true, None, None, 100)
                .await
                .unwrap()
                .len(),
            1
        );
        assert_eq!(c.alert_unacked_count("alice").await.unwrap(), 1);
        assert_eq!(
            c.alert_list("bob", true, None, None, 100)
                .await
                .unwrap()
                .len(),
            0
        );
        assert_eq!(
            c.alert_list_for_monitor("alice", &m.id)
                .await
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            c.alert_recent_titles_for_monitor("alice", &m.id, 10)
                .await
                .unwrap(),
            vec!["disk full"]
        );

        // acknowledge drops it from the inbox + count.
        assert!(c.alert_acknowledge("alice", &a.id).await.unwrap());
        assert!(!c.alert_acknowledge("bob", &a.id).await.unwrap()); // not under bob
        assert_eq!(
            c.alert_list("alice", true, None, None, 100)
                .await
                .unwrap()
                .len(),
            0
        );
        assert_eq!(
            c.alert_list("alice", false, None, None, 100)
                .await
                .unwrap()
                .len(),
            1
        ); // history
        assert_eq!(c.alert_unacked_count("alice").await.unwrap(), 0);
    }

    #[tokio::test]
    async fn alert_dismiss_is_independent_of_ack() {
        let dir = TempDir::new().unwrap();
        let c = control(&dir);
        let m = c
            .monitor_create(
                "alice",
                "prod",
                MonitorInput {
                    name: "errwatch".into(),
                    prompt: "p".into(),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        let mk = |title: &str| AlertInput {
            monitor_id: m.id.clone(),
            title: title.into(),
            ..Default::default()
        };
        let a = c.alert_create(mk("one")).await.unwrap();
        let _b = c.alert_create(mk("two")).await.unwrap();
        assert!(!a.dismissed);

        // Dismiss flips only the per-user dismissed flag, not ack.
        assert!(c.alert_dismiss("alice", &a.id).await.unwrap());
        let after = c.alert_list("alice", false, None, None, 100).await.unwrap();
        let a_now = after.iter().find(|x| x.id == a.id).unwrap();
        assert!(a_now.dismissed);
        assert!(!a_now.acknowledged); // dismiss != ack — still in the inbox
        assert_eq!(c.alert_unacked_count("alice").await.unwrap(), 2);

        // Dismiss-all flips the rest (and is idempotent for already-dismissed).
        assert_eq!(c.alert_dismiss_all("alice").await.unwrap(), 1);
        assert_eq!(c.alert_dismiss_all("alice").await.unwrap(), 0);
        let after = c.alert_list("alice", true, None, None, 100).await.unwrap();
        assert!(after.iter().all(|x| x.dismissed));
    }

    #[tokio::test]
    async fn public_alert_visible_to_all_with_shared_ack_per_user_dismiss() {
        let dir = TempDir::new().unwrap();
        let c = control(&dir);
        // Alice owns a public monitor; its alert must reach Bob too.
        let m = c
            .monitor_create(
                "alice",
                "prod",
                MonitorInput {
                    name: "pub".into(),
                    prompt: "p".into(),
                    public: true,
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        let a = c
            .alert_create(AlertInput {
                monitor_id: m.id.clone(),
                title: "shared".into(),
                ..Default::default()
            })
            .await
            .unwrap();
        assert!(a.public);
        // Both users see it; both see it unacked.
        assert_eq!(
            c.alert_list("bob", true, None, None, 100)
                .await
                .unwrap()
                .len(),
            1
        );
        assert_eq!(c.alert_unacked_count("bob").await.unwrap(), 1);
        assert_eq!(c.alert_unacked_count("alice").await.unwrap(), 1);

        // Bob dismisses the toast — only his view; Alice's stays visible.
        assert!(c.alert_dismiss("bob", &a.id).await.unwrap());
        let bob = &c.alert_list("bob", true, None, None, 100).await.unwrap()[0];
        let alice = &c.alert_list("alice", true, None, None, 100).await.unwrap()[0];
        assert!(bob.dismissed);
        assert!(!alice.dismissed);

        // Bob (a non-owner) acknowledges — shared, so it clears for everyone.
        assert!(c.alert_acknowledge("bob", &a.id).await.unwrap());
        assert_eq!(c.alert_unacked_count("alice").await.unwrap(), 0);
        assert_eq!(c.alert_unacked_count("bob").await.unwrap(), 0);

        // A private monitor's alert stays owner-only.
        let pm = c
            .monitor_create(
                "alice",
                "prod",
                MonitorInput {
                    name: "priv".into(),
                    prompt: "p".into(),
                    public: false,
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        c.alert_create(AlertInput {
            monitor_id: pm.id.clone(),
            title: "secret".into(),
            ..Default::default()
        })
        .await
        .unwrap();
        assert_eq!(c.alert_unacked_count("bob").await.unwrap(), 0); // bob can't see it
        assert_eq!(c.alert_unacked_count("alice").await.unwrap(), 1);
    }

    #[tokio::test]
    async fn alert_list_server_side_filters_and_limits() {
        let dir = TempDir::new().unwrap();
        let c = control(&dir);
        let mon = |name: &str| MonitorInput {
            name: name.into(),
            prompt: "p".into(),
            ..Default::default()
        };
        let m1 = c
            .monitor_create("alice", "prod", mon("errwatch"))
            .await
            .unwrap();
        let m2 = c
            .monitor_create("alice", "prod", mon("latency"))
            .await
            .unwrap();

        for (mid, title, summary) in [
            (&m1.id, "disk full", "volume at 98%"),
            (&m1.id, "cpu spike", "load high"),
            (&m2.id, "slow request", "p99 elevated"),
        ] {
            c.alert_create(AlertInput {
                monitor_id: mid.clone(),
                title: title.into(),
                summary: summary.into(),
                ..Default::default()
            })
            .await
            .unwrap();
        }

        let n = |unacked, monitor, search, limit| {
            let c = &c;
            async move {
                c.alert_list("alice", unacked, monitor, search, limit)
                    .await
                    .unwrap()
                    .len()
            }
        };
        // No filter → all three (fast key-only path); limit caps the result.
        assert_eq!(n(false, None, None, 100).await, 3);
        assert_eq!(n(false, None, None, 2).await, 2);
        // Search matches title / summary (case-insensitive) / monitor name.
        assert_eq!(n(false, None, Some("disk"), 100).await, 1);
        assert_eq!(n(false, None, Some("P99"), 100).await, 1);
        assert_eq!(n(false, None, Some("errwatch"), 100).await, 2);
        // Monitor filter, alone and combined with search.
        assert_eq!(n(false, Some(m2.id.as_str()), None, 100).await, 1);
        assert_eq!(n(false, Some(m1.id.as_str()), Some("cpu"), 100).await, 1);
        // No match.
        assert_eq!(n(false, None, Some("nonsense"), 100).await, 0);
    }
}
