// Copyright 2026 Appbird LLC. Licensed under "GNU Affero General Public License v3.0"

//! Cross-partition query planning shared by search + pipeline: parse → extract/strip
//! `index:` patterns → ask the catalog for matching partitions → return AST + list.

use anyhow::Result;
use chrono::{DateTime, Utc};
use std::collections::HashSet;

use crate::catalog::{Catalog, PartitionKey};
use crate::control::settings::EnvIndexAllow;
use crate::search::query::{extract_partition_patterns, parse, strip_partition_filters, Node};

/// What [`plan`] returns: partitions to scan plus the `index:`-stripped AST.
/// `node` is `None` for an empty or `*` (match-all) query.
pub struct Plan {
    pub keys: Vec<PartitionKey>,
    pub node: Option<Node>,
}

/// Combines query-level (`index:foo`) and URL-level (`?index=foo`) filters (query
/// wins). `ui_env` is a hard scope (`None` = user envs only); `extra_allow` AND-ed in.
pub fn plan(
    catalog: &Catalog,
    query_str: &str,
    ui_env: Option<&str>,
    ui_index: Option<&str>,
    start: Option<DateTime<Utc>>,
    end: Option<DateTime<Utc>>,
    extra_allow: &[EnvIndexAllow],
) -> Result<Plan> {
    plan_with_explicit_keys(
        catalog,
        query_str,
        ui_env,
        ui_index,
        start,
        end,
        extra_allow,
        None,
    )
}

/// Same as [`plan`] but narrows to `explicit_keys` (streaming search). Keys are
/// **intersected** with the full planned set, so RBAC/time/env gates still apply.
#[allow(clippy::too_many_arguments)]
pub fn plan_with_explicit_keys(
    catalog: &Catalog,
    query_str: &str,
    ui_env: Option<&str>,
    ui_index: Option<&str>,
    start: Option<DateTime<Utc>>,
    end: Option<DateTime<Utc>>,
    extra_allow: &[EnvIndexAllow],
    explicit_keys: Option<Vec<PartitionKey>>,
) -> Result<Plan> {
    let parsed = parse(query_str)?;
    let patterns: Vec<String> = match parsed.as_ref() {
        Some(n) => extract_partition_patterns(n),
        None => Vec::new(),
    };
    let effective_patterns = if !patterns.is_empty() {
        patterns
    } else if let Some(idx) = ui_index.filter(|s| !s.is_empty()) {
        vec![idx.to_string()]
    } else {
        Vec::new()
    };
    let node = parsed.map(strip_partition_filters);
    let env_scope = ui_env.filter(|s| !s.is_empty());
    // Discover partitions from the active engine (block store in block mode —
    // which may be a shared/S3 store — else the local catalog), then filter.
    let mut keys = crate::catalog::filter_partitions(
        crate::engine::discover_partitions(catalog),
        env_scope,
        &effective_patterns,
        start,
        end,
    );

    // System envs (`_*`) are reachable only by scoping to them explicitly; a
    // no-env scan covers user envs only so self-logs never leak into it.
    if env_scope.is_none() {
        keys.retain(|k| !k.env.starts_with('_'));
    }

    // AND the caller-supplied allowlist in: keep only (env, index) pairs
    // matching a rule. Empty slice = no cap.
    if !extra_allow.is_empty() {
        keys.retain(|k| allow_includes(extra_allow, &k.env, &k.index));
    }

    // Intersect with the caller-supplied subset; keys not already planned are
    // dropped. This is the safety boundary for the streaming search path.
    if let Some(explicit) = explicit_keys {
        let allowed: HashSet<&PartitionKey> = keys.iter().collect();
        let explicit_set: HashSet<PartitionKey> = explicit
            .into_iter()
            .filter(|k| allowed.contains(k))
            .collect();
        keys.retain(|k| explicit_set.contains(k));
    }

    Ok(Plan { keys, node })
}

/// Same matcher as [`crate::control::settings::McpSettings::allows`] on a
/// borrowed slice. Both call sites must stay in sync.
fn allow_includes(rules: &[EnvIndexAllow], env: &str, index: &str) -> bool {
    for r in rules {
        let env_match = r.env == "*" || r.env.eq_ignore_ascii_case(env);
        if !env_match {
            continue;
        }
        if r.indexes.iter().any(|p| p == "*") {
            return true;
        }
        if crate::catalog::index_matches(&r.indexes, index) {
            return true;
        }
    }
    false
}
