// Copyright 2026 Appbird LLC. Licensed under "GNU Affero General Public License v3.0"

// Static catalogs of suggestable tokens (pipe commands, stats aggs, boolean operators)
// — the hardcoded parts of the query language; hand-synced with `src/search/`.

export interface StaticSuggestion {
    // Visible label.
    label: string;
    // Short hint shown next to the label.
    detail: string;
    // Text inserted on accept; may differ from `label` (e.g. `sum(`, `stats `).
    insert: string;
}

// Pipe commands recognized by `pipeline.rs::dispatch`.
export const COMMANDS: StaticSuggestion[] = [
    { label: "stats", detail: "aggregate (count/sum/avg/min/max/percentiles)", insert: "stats " },
    { label: "top", detail: "top N values of a field", insert: "top " },
    { label: "rare", detail: "rarest N values of a field", insert: "rare " },
    { label: "sort", detail: "sort by field (prefix - for desc)", insert: "sort " },
    { label: "head", detail: "first N rows", insert: "head " },
    { label: "tail", detail: "last N rows", insert: "tail " },
];

// Aggregation functions inside `stats`; all but `count` insert an open paren.
export const AGG_FUNCS: StaticSuggestion[] = [
    { label: "count", detail: "document count", insert: "count" },
    { label: "sum", detail: "field sum", insert: "sum(" },
    { label: "avg", detail: "arithmetic mean", insert: "avg(" },
    { label: "min", detail: "minimum", insert: "min(" },
    { label: "max", detail: "maximum", insert: "max(" },
    { label: "p50", detail: "50th percentile (median)", insert: "p50(" },
    { label: "p95", detail: "95th percentile", insert: "p95(" },
    { label: "p99", detail: "99th percentile", insert: "p99(" },
];

// Boolean operators; suggested only at term boundaries in the main segment.
export const BOOLEAN_OPS: StaticSuggestion[] = [
    { label: "AND", detail: "both terms required (default)", insert: "AND " },
    { label: "OR", detail: "either term matches", insert: "OR " },
    { label: "NOT", detail: "negate the next term", insert: "NOT " },
];

// `by` keyword for `stats` grouping; surfaced once an agg is present.
export const STATS_BY: StaticSuggestion = {
    label: "by",
    detail: "group by field(s)",
    insert: "by ",
};

// Universal-core fields, excluded from `/api/discover_fields` (known, not
// discovered), so the suggester adds them manually. Mirrors `discover.rs::UNIVERSAL_CORE`.
export const SYSTEM_FIELDS: { name: string; detail: string }[] = [
    { name: "index", detail: "partition key" },
    { name: "source", detail: "per-event origin tag" },
    { name: "message", detail: "event message text" },
    { name: "timestamp", detail: "event time (ISO 8601)" },
    { name: "raw", detail: "verbatim event JSON" },
];
