// Copyright 2026 Appbird LLC. Licensed under "GNU Affero General Public License v3.0"

//! Control plane (identity, RBAC, saved searches, monitors, alerts, settings,
//! conversations): small encrypted JSON files on a [`store::ControlStore`] mutated
//! via CAS. The backend + [`Control`] facade live in [`backend`].

use anyhow::Result;

pub mod alerts;
pub mod api_keys;
pub mod backend;
pub mod crypto;
pub mod dashboards;
pub mod envs;
pub mod ingest_tokens;
pub mod monitors;
pub mod saved;
pub mod settings;
pub mod sources;
pub mod store;
pub mod users;

pub use backend::Control;

use crate::catalog::{DEFAULT_ENV, SYSTEM_ENV};

/// First-run bootstrap: if `HELIOS_ADMIN_*` is configured and no users exist, create
/// the admin from those vars. With NO admin env set, we intentionally leave the
/// instance uninitialized so the browser shows the first-run setup screen (the first
/// visitor claims it via `POST /api/auth/setup`) — far less friction than hunting a
/// generated password out of the logs. Break-glass `HELIOS_ADMIN_RESET=1` still forces
/// the password every boot (revoking tokens).
pub async fn ensure_admin(control: &Control) -> Result<()> {
    let userid = env_nonempty("HELIOS_ADMIN_USER").unwrap_or_else(|| "admin".to_string());

    if env_truthy("HELIOS_ADMIN_RESET") {
        match env_nonempty("HELIOS_ADMIN_PASSWORD") {
            Some(pw) => return reset_admin(control, &userid, &pw).await,
            None => {
                tracing::warn!(
                    "control: HELIOS_ADMIN_RESET is set but HELIOS_ADMIN_PASSWORD is empty — \
                     nothing to reset"
                );
                return Ok(());
            }
        }
    }

    if control.user_count().await? > 0 {
        return Ok(());
    }

    // No users yet. Only auto-create when an admin is explicitly configured; otherwise
    // hand off to the setup screen (see `setup_handler`). A bare `HELIOS_ADMIN_USER`
    // with no password still falls through to setup — there's nothing to log in with.
    let Some(password) = env_nonempty("HELIOS_ADMIN_PASSWORD") else {
        tracing::info!(
            "control: no users and no HELIOS_ADMIN_PASSWORD — first-run setup screen will claim \
             the instance"
        );
        return Ok(());
    };
    let email = env_nonempty("HELIOS_ADMIN_EMAIL").unwrap_or_else(|| format!("{userid}@localhost"));

    control
        .create_user(&userid, &email, "Administrator", &password, true)
        .await?;
    tracing::info!("control: created admin {userid:?} from HELIOS_ADMIN_PASSWORD");
    Ok(())
}

/// First-run bootstrap: register the two reserved envs so the control plane has
/// a row for every disk env that exists.
pub async fn ensure_reserved_envs(control: &Control) -> Result<()> {
    control.upsert_env(DEFAULT_ENV, false).await?;
    control.upsert_env(SYSTEM_ENV, true).await?;
    Ok(())
}

/// Break-glass admin reset: set the named admin's password (create the account
/// if missing) and revoke any outstanding tokens.
async fn reset_admin(control: &Control, userid: &str, password: &str) -> Result<()> {
    match control.find_user_by_login(userid).await? {
        Some(u) => {
            control.set_password(&u.id, password).await?;
            control.bump_credentials_version(&u.id).await?;
            tracing::warn!(
                "control: reset password for admin {userid:?} via HELIOS_ADMIN_RESET \
                 (outstanding tokens revoked)"
            );
        }
        None => {
            let email =
                env_nonempty("HELIOS_ADMIN_EMAIL").unwrap_or_else(|| format!("{userid}@localhost"));
            control
                .create_user(userid, &email, "Administrator", password, true)
                .await?;
            tracing::warn!("control: created admin {userid:?} via HELIOS_ADMIN_RESET");
        }
    }
    Ok(())
}

/// Reads an env var, returning `Some` only for a present, non-blank value.
fn env_nonempty(key: &str) -> Option<String> {
    std::env::var(key)
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
}

/// True when an env var is set to a truthy value (`1`/`true`/`yes`/`on`).
fn env_truthy(key: &str) -> bool {
    env_nonempty(key)
        .is_some_and(|v| matches!(v.to_ascii_lowercase().as_str(), "1" | "true" | "yes" | "on"))
}
