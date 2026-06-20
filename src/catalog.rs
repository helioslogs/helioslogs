// Copyright 2026 Appbird LLC. Licensed under "GNU Affero General Public License v3.0"

//! Per-(env, index, day) partition catalog over `data/<env>/<index>/<day>/`:
//! discovers partitions, prunes by time, manages envs. Block storage itself
//! lives in [`crate::engine::block`].

use anyhow::{anyhow, bail, Context, Result};
use chrono::{DateTime, NaiveDate, Utc};
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

const DAY_FMT: &str = "%Y-%m-%d";

/// Reserved env for user data while the env picker doesn't exist yet.
/// Every ingest / search defaults here when no explicit env is supplied.
pub const DEFAULT_ENV: &str = "default";

/// Reserved env for backend self-observability partitions (`_helioslogs`,
/// `_helioshttp`, `_heliosmcp`). Excluded from the user-facing env picker.
pub const SYSTEM_ENV: &str = "_system";

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct PartitionKey {
    pub env: String,
    pub index: String,
    pub day: NaiveDate,
}

impl PartitionKey {
    pub fn new(env: impl Into<String>, index: impl Into<String>, day: NaiveDate) -> Self {
        PartitionKey {
            env: env.into(),
            index: index.into(),
            day,
        }
    }

    pub fn day_string(&self) -> String {
        self.day.format(DAY_FMT).to_string()
    }
}

/// Partition catalog: a thin layer over the data-dir layout (root path,
/// partition discovery, env management). Block storage lives in `crate::engine::block`.
#[derive(Clone)]
pub struct Catalog {
    root: Arc<PathBuf>,
}

impl Catalog {
    pub fn open(root: PathBuf) -> Result<Self> {
        fs::create_dir_all(&root)
            .with_context(|| format!("creating data root {}", root.display()))?;
        Ok(Catalog {
            root: Arc::new(root),
        })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Walks the data dir and returns every recognized (env, index, day)
    /// tuple. Layout is `root/<env>/<index>/<day>/`.
    pub fn list_partitions(&self) -> Vec<PartitionKey> {
        let mut out = Vec::new();
        let Ok(envs) = fs::read_dir(self.root.as_ref()) else {
            return out;
        };
        for env_entry in envs.flatten() {
            let Ok(t) = env_entry.file_type() else {
                continue;
            };
            if !t.is_dir() {
                continue;
            }
            let env = env_entry.file_name().to_string_lossy().to_string();
            if !valid_env_name(&env) {
                continue;
            }
            let Ok(indexes) = fs::read_dir(env_entry.path()) else {
                continue;
            };
            for idx_entry in indexes.flatten() {
                let Ok(t) = idx_entry.file_type() else {
                    continue;
                };
                if !t.is_dir() {
                    continue;
                }
                let index = idx_entry.file_name().to_string_lossy().to_string();
                if !valid_index_name(&index) {
                    continue;
                }
                let Ok(days) = fs::read_dir(idx_entry.path()) else {
                    continue;
                };
                for day_entry in days.flatten() {
                    let Ok(t) = day_entry.file_type() else {
                        continue;
                    };
                    if !t.is_dir() {
                        continue;
                    }
                    let name = day_entry.file_name().to_string_lossy().to_string();
                    if let Ok(day) = NaiveDate::parse_from_str(&name, DAY_FMT) {
                        out.push(PartitionKey {
                            env: env.clone(),
                            index: index.clone(),
                            day,
                        });
                    }
                }
            }
        }
        out.sort_by(|a, b| {
            a.env
                .cmp(&b.env)
                .then(a.index.cmp(&b.index))
                .then(a.day.cmp(&b.day))
        });
        out
    }

    /// All distinct env names currently present on disk. Unused until
    /// the env picker UI (Phase 2) starts asking which envs exist.
    #[allow(dead_code)]
    pub fn list_envs(&self) -> Vec<String> {
        let mut s: HashSet<String> = HashSet::new();
        for p in self.list_partitions() {
            s.insert(p.env);
        }
        let mut v: Vec<String> = s.into_iter().collect();
        v.sort();
        v
    }

    /// Selects partitions overlapping `[start, end]`, optionally by env + index.
    /// Unused — kept as a friendlier single-filter API for the eventual MCP server.
    #[allow(dead_code)]
    pub fn select(
        &self,
        env: Option<&str>,
        index: Option<&str>,
        start: Option<DateTime<Utc>>,
        end: Option<DateTime<Utc>>,
    ) -> Vec<PartitionKey> {
        let patterns: Vec<String> = index.map(|s| vec![s.to_string()]).unwrap_or_default();
        self.select_with_patterns(env, &patterns, start, end)
    }

    /// Selects partitions whose index matches a glob pattern (empty = all; `*`/`?`
    /// supported). `env` is a hard exact scope; `None` means all envs.
    pub fn select_with_patterns(
        &self,
        env: Option<&str>,
        index_patterns: &[String],
        start: Option<DateTime<Utc>>,
        end: Option<DateTime<Utc>>,
    ) -> Vec<PartitionKey> {
        filter_partitions(self.list_partitions(), env, index_patterns, start, end)
    }

    /// Filesystem path for a partition. Public so admin/MCP callers that size
    /// or list files don't reach into the layout convention themselves.
    pub fn partition_path(&self, k: &PartitionKey) -> PathBuf {
        self.root.join(&k.env).join(&k.index).join(k.day_string())
    }
}

/// True if `name` matches any case-insensitive glob (empty = match all). Shared
/// `pub(crate)` so scatter and MCP allowlist enforcement use one matcher.
pub(crate) fn index_matches(patterns: &[String], name: &str) -> bool {
    if patterns.is_empty() {
        return true;
    }
    let name_lc = name.to_lowercase();
    patterns
        .iter()
        .any(|p| glob_match(&p.to_lowercase(), &name_lc))
}

/// Filter + sort a partition list by env (hard scope), index globs, and date
/// range. Sorted env/index asc, day desc (newest first).
pub fn filter_partitions(
    mut all: Vec<PartitionKey>,
    env: Option<&str>,
    index_patterns: &[String],
    start: Option<DateTime<Utc>>,
    end: Option<DateTime<Utc>>,
) -> Vec<PartitionKey> {
    let lo_date = start.map(|t| t.date_naive());
    let hi_date = end.map(|t| t.date_naive());
    all.retain(|k| {
        env.is_none_or(|e| k.env == e)
            && index_matches(index_patterns, &k.index)
            && lo_date.is_none_or(|lo| k.day >= lo)
            && hi_date.is_none_or(|hi| k.day <= hi)
    });
    all.sort_by(|a, b| {
        a.env
            .cmp(&b.env)
            .then(a.index.cmp(&b.index))
            .then(b.day.cmp(&a.day))
    });
    all
}

/// Minimal `*` / `?` glob matcher (backtracking). No special handling for
/// `[]` character classes — keep it simple; index names rarely need them.
fn glob_match(pattern: &str, s: &str) -> bool {
    let p = pattern.as_bytes();
    let s = s.as_bytes();
    let mut pi = 0usize;
    let mut si = 0usize;
    let mut star: Option<usize> = None;
    let mut star_si = 0usize;
    while si < s.len() {
        if pi < p.len() && (p[pi] == b'?' || p[pi] == s[si]) {
            pi += 1;
            si += 1;
        } else if pi < p.len() && p[pi] == b'*' {
            star = Some(pi);
            star_si = si;
            pi += 1;
        } else if let Some(sp) = star {
            pi = sp + 1;
            star_si += 1;
            si = star_si;
        } else {
            return false;
        }
    }
    while pi < p.len() && p[pi] == b'*' {
        pi += 1;
    }
    pi == p.len()
}

#[cfg(test)]
mod glob_tests {
    use super::glob_match;
    #[test]
    fn exact() {
        assert!(glob_match("stripe-webhooks", "stripe-webhooks"));
        assert!(!glob_match("stripe-webhooks", "stripe"));
    }
    #[test]
    fn suffix() {
        assert!(glob_match("*webhooks", "stripe-webhooks"));
        assert!(glob_match("*webhooks", "github-webhooks"));
        assert!(!glob_match("*webhooks", "nginx"));
    }
    #[test]
    fn prefix() {
        assert!(glob_match("stripe*", "stripe-webhooks"));
        assert!(!glob_match("stripe*", "github-webhooks"));
    }
    #[test]
    fn contains() {
        assert!(glob_match("*webhook*", "stripe-webhooks"));
        assert!(glob_match("*webhook*", "github-webhooks"));
        assert!(glob_match("*webhook*", "webhook-stuff"));
        assert!(!glob_match("*webhook*", "nginx"));
    }
    #[test]
    fn question() {
        assert!(glob_match("ng?nx", "nginx"));
        assert!(!glob_match("ng?nx", "ngxinx"));
    }
}

/// Validates that an index name is a safe directory component. Allows
/// `[A-Za-z0-9_-]+` and rejects empty / dotfile / slash-containing strings.
pub fn valid_index_name(s: &str) -> bool {
    !s.is_empty()
        && !s.starts_with('.')
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

/// Validates an env name as a safe dir component (same rules as index names;
/// leading `_` allowed, reserved for system envs).
pub fn valid_env_name(s: &str) -> bool {
    !s.is_empty()
        && !s.starts_with('.')
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

/// Validates an index template: literals use the [`valid_index_name`] charset
/// with `{{field}}` placeholders allowed (sanitized per event at ingest).
pub fn valid_index_template(s: &str) -> bool {
    if s.is_empty() || s.starts_with('.') {
        return false;
    }
    // Drop `{{ ... }}` spans; the remaining literal must be safe (or empty).
    let mut literal = String::new();
    let mut rest = s;
    while let Some(start) = rest.find("{{") {
        literal.push_str(&rest[..start]);
        match rest[start + 2..].find("}}") {
            Some(end) => rest = &rest[start + 2 + end + 2..],
            None => return false, // unterminated placeholder
        }
    }
    literal.push_str(rest);
    literal
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

/// Parses `?index=...`: the trimmed value if it's a valid name or template,
/// else the supplied default.
pub fn index_or_default(input: Option<&str>, default: &str) -> Result<String> {
    let s = input.map(|s| s.trim()).filter(|s| !s.is_empty());
    match s {
        Some(s) if valid_index_name(s) || valid_index_template(s) => Ok(s.to_string()),
        Some(s) => bail!(
            "invalid index name: {s:?}; allowed: [A-Za-z0-9_-]+ (or a {{{{field}}}} template)"
        ),
        None => Ok(default.to_string()),
    }
}

/// Parses a `?env=...` query param: returns the trimmed value if it
/// passes [`valid_env_name`], else [`DEFAULT_ENV`] when missing.
pub fn env_or_default(input: Option<&str>) -> Result<String> {
    let s = input.map(|s| s.trim()).filter(|s| !s.is_empty());
    match s {
        Some(s) if valid_env_name(s) => Ok(s.to_string()),
        Some(s) => bail!("invalid env name: {s:?}; allowed: [A-Za-z0-9_-]+"),
        None => Ok(DEFAULT_ENV.to_string()),
    }
}

/// Maps an event's timestamp to the (UTC) date that owns its partition.
pub fn day_for(ts: DateTime<Utc>) -> NaiveDate {
    ts.date_naive()
}

/// Stable `env/index/day` label for logs and error messages.
pub fn partition_label(k: &PartitionKey) -> String {
    format!("{}/{}/{}", k.env, k.index, k.day_string())
}

/// One-shot migration: move old single-index data into a today-named partition
/// when the default partition is empty (only the partition assignment is approximated).
pub fn migrate_legacy_index(
    legacy_index: &Path,
    catalog: &Catalog,
    default_index: &str,
) -> Result<Option<PartitionKey>> {
    if !legacy_index.exists() {
        return Ok(None);
    }
    let has_files = fs::read_dir(legacy_index)
        .map(|mut e| e.next().is_some())
        .unwrap_or(false);
    if !has_files {
        return Ok(None);
    }
    // Don't clobber an existing migrated partition.
    let existing_for_index: Vec<PartitionKey> = catalog
        .list_partitions()
        .into_iter()
        .filter(|p| p.env == DEFAULT_ENV && p.index == default_index)
        .collect();
    if !existing_for_index.is_empty() {
        return Ok(None);
    }
    let key = PartitionKey {
        env: DEFAULT_ENV.to_string(),
        index: default_index.to_string(),
        day: Utc::now().date_naive(),
    };
    let dest = catalog.partition_path(&key);
    fs::create_dir_all(dest.parent().ok_or_else(|| anyhow!("bad dest"))?)?;
    fs::rename(legacy_index, &dest)
        .with_context(|| format!("moving {} -> {}", legacy_index.display(), dest.display()))?;
    Ok(Some(key))
}

/// One-shot, idempotent migration to the env-aware layout: legacy `data/<index>/<day>/`
/// dirs (detected by a date child) move under [`SYSTEM_ENV`] (`_`-prefixed) or [`DEFAULT_ENV`].
pub fn migrate_to_env_layout(root: &Path) -> Result<Vec<(String, String)>> {
    let mut moved: Vec<(String, String)> = Vec::new();
    let Ok(entries) = fs::read_dir(root) else {
        return Ok(moved);
    };
    for entry in entries.flatten() {
        let Ok(t) = entry.file_type() else { continue };
        if !t.is_dir() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().to_string();
        if !valid_index_name(&name) {
            continue;
        }
        // Legacy layout has a `YYYY-MM-DD` child; env containers (none) are skipped.
        let dir = entry.path();
        let mut has_day_child = false;
        if let Ok(children) = fs::read_dir(&dir) {
            for c in children.flatten() {
                let Ok(t) = c.file_type() else { continue };
                if !t.is_dir() {
                    continue;
                }
                let n = c.file_name().to_string_lossy().to_string();
                if NaiveDate::parse_from_str(&n, DAY_FMT).is_ok() {
                    has_day_child = true;
                    break;
                }
            }
        }
        if !has_day_child {
            continue;
        }
        let env = if name.starts_with('_') {
            SYSTEM_ENV
        } else {
            DEFAULT_ENV
        };
        let env_dir = root.join(env);
        fs::create_dir_all(&env_dir)
            .with_context(|| format!("creating env dir {}", env_dir.display()))?;
        let dest = env_dir.join(&name);
        if dest.exists() {
            // Defensive: a target collision (hand-crafted partial layout) is
            // skipped rather than clobbered.
            tracing::warn!(
                "migrate_to_env_layout: skipping {} — dest {} already exists",
                dir.display(),
                dest.display()
            );
            continue;
        }
        fs::rename(&dir, &dest)
            .with_context(|| format!("moving {} -> {}", dir.display(), dest.display()))?;
        moved.push((name, env.to_string()));
    }
    Ok(moved)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn index_templates_pass_validation() {
        // Plain names still validate; braces alone do not.
        assert!(valid_index_name("fluentbit-native"));
        assert!(!valid_index_name("fluentbit-{{service}}"));

        // Templates: safe literals around terminated {{...}} placeholders.
        assert!(valid_index_template("fluentbit-native-{{service}}"));
        assert!(valid_index_template("{{service}}"));
        assert!(valid_index_template("a-{{x}}-b-{{y}}"));
        // Rejected: unterminated, leading dot, unsafe literal chars.
        assert!(!valid_index_template("fluentbit-{{service"));
        assert!(!valid_index_template(".hidden-{{x}}"));
        assert!(!valid_index_template("bad/{{x}}"));

        // index_or_default accepts a template verbatim (resolved per event later).
        assert_eq!(
            index_or_default(Some("app-{{service}}"), "default").unwrap(),
            "app-{{service}}"
        );
        assert!(index_or_default(Some("bad/name"), "default").is_err());
    }

    fn d(y: i32, m: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, day).unwrap()
    }

    fn ts(y: i32, m: u32, day: u32) -> DateTime<Utc> {
        d(y, m, day).and_hms_opt(0, 0, 0).unwrap().and_utc()
    }

    fn sample() -> Vec<PartitionKey> {
        vec![
            PartitionKey::new("default", "web", d(2026, 1, 1)),
            PartitionKey::new("default", "web", d(2026, 1, 3)),
            PartitionKey::new("default", "api", d(2026, 1, 2)),
            PartitionKey::new("prod", "web", d(2026, 1, 2)),
        ]
    }

    #[test]
    fn index_matches_empty_is_match_all() {
        assert!(index_matches(&[], "anything"));
        assert!(index_matches(&["web*".to_string()], "WEB-1")); // case-insensitive
        assert!(!index_matches(&["web*".to_string()], "api"));
    }

    #[test]
    fn filter_partitions_env_is_hard_scope() {
        let out = filter_partitions(sample(), Some("prod"), &[], None, None);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].env, "prod");
    }

    #[test]
    fn filter_partitions_index_glob() {
        let out = filter_partitions(sample(), None, &["web".to_string()], None, None);
        assert!(out.iter().all(|k| k.index == "web"));
        assert_eq!(out.len(), 3);
    }

    #[test]
    fn filter_partitions_date_range_inclusive() {
        let out = filter_partitions(
            sample(),
            Some("default"),
            &[],
            Some(ts(2026, 1, 2)),
            Some(ts(2026, 1, 3)),
        );
        assert!(out
            .iter()
            .all(|k| k.day >= d(2026, 1, 2) && k.day <= d(2026, 1, 3)));
        assert_eq!(out.len(), 2); // web/01-03 and api/01-02
    }

    #[test]
    fn filter_partitions_sort_env_index_asc_day_desc() {
        let out = filter_partitions(sample(), None, &[], None, None);
        let labels: Vec<String> = out.iter().map(partition_label).collect();
        assert_eq!(
            labels,
            vec![
                "default/api/2026-01-02",
                "default/web/2026-01-03", // newer day first within (env,index)
                "default/web/2026-01-01",
                "prod/web/2026-01-02",
            ]
        );
    }

    #[test]
    fn valid_env_name_rules() {
        assert!(valid_env_name("default"));
        assert!(valid_env_name("_system")); // leading underscore allowed
        assert!(!valid_env_name(".hidden"));
        assert!(!valid_env_name("has space"));
        assert!(!valid_env_name(""));
    }

    #[test]
    fn env_or_default_falls_back_and_validates() {
        assert_eq!(env_or_default(None).unwrap(), DEFAULT_ENV);
        assert_eq!(env_or_default(Some("  ")).unwrap(), DEFAULT_ENV);
        assert_eq!(env_or_default(Some("prod")).unwrap(), "prod");
        assert!(env_or_default(Some("bad/env")).is_err());
    }

    #[test]
    fn day_for_and_partition_key_helpers() {
        assert_eq!(day_for(ts(2026, 6, 7)), d(2026, 6, 7));
        let k = PartitionKey::new("default", "web", d(2026, 6, 7));
        assert_eq!(k.day_string(), "2026-06-07");
        assert_eq!(partition_label(&k), "default/web/2026-06-07");
    }
}
