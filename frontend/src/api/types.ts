// Copyright 2026 Appbird LLC. Licensed under "GNU Affero General Public License v3.0"

// Wire types — shapes returned by the HeliosLogs HTTP API.

// Schema-on-read: only universal-core fields are typed; everything else lives
// inside `raw` (verbatim event JSON) and is parsed by the frontend per row.
export interface Hit {
    timestamp?: string;
    message?: string;
    score: number;
    partition: string; // "<index>/<day>"
    // Per-event source tag, distinct from `index` (the on-disk partition key).
    source?: string;
    // Verbatim ingested JSON; pretty-printed, line-clamped, expandable.
    raw?: string;
}

// Footer-derived field catalog from `/api/discover_fields` — the true
// `(path, type)` schema over the window, query-independent. Drives the sidebar.
export interface DiscoveredField {
    name: string;
    // Fraction of in-window rows carrying this field (0.0–1.0).
    coverage: number;
    // Distinct-value lower bound (max in any one block); `0` for numeric columns.
    cardinality: number;
    value_kind: "string" | "int" | "float" | "bool" | "mixed";
    // Terms-agg yields useful buckets; sidebar lists these as Available.
    groupable: boolean;
    // Low-cardinality categorical worth auto-pinning; seeds the Pinned set.
    interesting: boolean;
}

export interface DiscoverFieldsResponse {
    took_us: number;
    // Total in-window rows scanned (the coverage denominator).
    total_rows: number;
    partitions_scanned: number;
    fields: DiscoveredField[];
}

export interface SearchResponse {
    total: number;
    took_us: number;
    hits: Hit[];
    highlight_terms: string[];
    partitions_scanned: number;
    // Echoed back from the request — 0-based offset of the first hit + page size.
    offset: number;
    limit: number;
    // Present for pipe queries; the UI renders a table instead of the hits list.
    table?: TableResult;
}

export interface TableResult {
    columns: string[];
    rows: (string | number | null)[][];
    took_us: number;
    scanned_docs: number;
    partitions_scanned: number;
    search: string;
    stages: string[];
}

export interface HistogramBucket {
    t: string;
    count: number;
}

export interface HistogramResponse {
    interval_ms: number;
    took_us: number;
    buckets: HistogramBucket[];
}

// Combined hits-page + histogram (one block pass per partition). Superset of
// `SearchResponse` (sans `table`) plus the histogram fields.
export interface SearchHistogramResponse {
    total: number;
    took_us: number;
    hits: Hit[];
    highlight_terms: string[];
    partitions_scanned: number;
    offset: number;
    limit: number;
    interval_ms: number;
    buckets: HistogramBucket[];
}

// One scannable partition — the unit the streaming search iterates over.
// `day` is `yyyy-mm-dd`. Wire format on the URL is `env:index:day`.
export interface SearchPartition {
    env: string;
    index: string;
    day: string;
}

export interface SearchPartitionsResponse {
    total: number;
    partitions: SearchPartition[];
}

export interface Stats {
    num_docs: number;
    num_segments: number;
    num_partitions: number;
}

export interface TopBucket {
    key: string | number;
    count: number;
}

export interface AggregateResponse {
    took_us: number;
    aggs: Record<string, TopBucket[]>;
    // Backend stride-sampled a too-wide window; counts are scaled estimates (UI shows "≈").
    sampled: boolean;
    sampled_partitions: number;
    total_partitions: number;
}

// --- admin ---

export interface SchemaField {
    name: string;
    type: string;
}

export interface SegmentMeta {
    id: string;
    num_docs: number;
    num_deleted_docs: number;
    max_doc: number;
    byte_size: number;
}

export interface PartitionSummary {
    env: string;
    index: string;
    day: string;
    num_docs: number;
    num_segments: number;
    byte_size: number;
}

export interface CatalogInfo {
    scope: "catalog";
    data_dir: string;
    num_partitions: number;
    num_docs: number;
    num_segments: number;
    total_bytes: number;
    schema: SchemaField[];
    partitions: PartitionSummary[];
}

// One MCP allow-list row: an env scope plus index glob patterns.
// `env === "*"` matches any env (legacy); `indexes: ["*"]` matches all.
export interface EnvIndexAllow {
    env: string;
    indexes: string[];
}

// One read-only `HELIOS_*` server config knob shown on the admin General page.
// All are read once at startup, so changes need a restart.
export interface RuntimeConfigEntry {
    name: string;
    category: string;
    value: string;
    // True when explicitly set via env (vs. running on the built-in default).
    overridden: boolean;
    description: string;
}

// One editable server tunable (Admin → General). Resolved as env > setting >
// default; an env override wins and locks the field.
export interface Tunable {
    id: string;
    env: string;
    category: string;
    label: string;
    unit: string;
    default: number;
    // Value the server is using right now.
    effective: number;
    // Value stored in the control plane, or null when on the default.
    configured: number | null;
    // Env value when the env var pins this knob (field then read-only), else null.
    env_override: number | null;
    // False = a change applies only after a restart (e.g. query threads).
    live: boolean;
    description: string;
}

export interface Settings {
    // Master switch; when false `helioslogs mcp` returns no tools and rejects calls.
    mcp_enabled: boolean;
    // Env-aware index allowlist; empty or `{env:"*",indexes:["*"]}` = no restriction.
    mcp_allowed: EnvIndexAllow[];
    // Tool allowlist. `["*"]` means all known tools enabled.
    mcp_enabled_tools: string[];
    // Alert webhook delivery. The URL is never echoed (may embed a secret).
    alert_webhook_enabled: boolean;
    alert_webhook_url_set: boolean;
    alert_webhook_format: "generic" | "slack";
    // Configured default partition retention in days; 0 = keep forever.
    retention_default_days: number;
    // Effective retention (folds in the env override below); 0 = keep forever.
    retention_default_days_effective: number;
    // Raw HELIOS_RETENTION_DEFAULT_DAYS value when set, else null.
    retention_default_days_env: string | null;
    // True when the env var pins retention (the input is then read-only).
    retention_default_days_env_overridden: boolean;
    // Instance theme defaults; users override on their account page.
    theme_default_appearance: "light" | "dark";
    theme_default_palette: string;
}

// Patch shape for POST /api/admin/settings — write-only fields included.
export interface SettingsPatch {
    mcp_enabled?: boolean;
    mcp_allowed?: EnvIndexAllow[];
    mcp_enabled_tools?: string[];
    alert_webhook_enabled?: boolean;
    // Write-only; empty string clears the saved URL.
    alert_webhook_url?: string;
    alert_webhook_format?: "generic" | "slack";
    // 0 clears (keep forever).
    retention_default_days?: number;
    theme_default_appearance?: "light" | "dark";
    theme_default_palette?: string;
}

export interface TestWebhookResult {
    ok: boolean;
    status?: number;
    error?: string;
}

// Public SAML status for the login page.
export interface SamlStatus {
    // SP-initiated SSO is fully configured and can be started.
    enabled: boolean;
    // Login-button label.
    label: string;
    // Password login is restricted to admins (break-glass) — non-admins must SSO.
    local_login_disabled: boolean;
}

// Admin view of the SAML config. The pinned cert is never echoed — only a
// fingerprint + "set" flag.
export interface SamlConfig {
    enabled: boolean;
    idp_entity_id: string;
    idp_sso_url: string;
    sp_entity_id: string;
    acs_url: string;
    email_attr: string | null;
    button_label: string;
    local_login_disabled: boolean;
    cert_set: boolean;
    cert_fingerprint: string | null;
}

// Patch for the admin SAML config. Omitted fields are unchanged; an empty
// string clears that field (and an empty `idp_cert` clears the pinned cert).
export interface SamlConfigPatch {
    enabled?: boolean;
    idp_entity_id?: string;
    idp_sso_url?: string;
    idp_cert?: string;
    sp_entity_id?: string;
    acs_url?: string;
    email_attr?: string;
    button_label?: string;
    local_login_disabled?: boolean;
}

// One syslog routing rule: when a parsed field matches, send to env/index
// (each falling back to the configured default when blank/omitted).
export interface SyslogRoute {
    field: string;
    op: string;
    value: string;
    env?: string | null;
    index?: string | null;
}

// Admin view of the raw syslog listener config. `route_fields`/`route_ops` are
// the server-supplied vocabularies the editor uses to populate dropdowns.
export interface SyslogConfig {
    enabled: boolean;
    bind: string;
    udp_port: number;
    tcp_port: number;
    default_env: string;
    default_index: string;
    routes: SyslogRoute[];
    route_fields: string[];
    route_ops: string[];
    /// Set when this instance was launched with `--syslog-port`; the listener binds
    /// this port (UDP + TCP) and ignores the configured ports above.
    port_override: number | null;
}

// Patch for the syslog config. Omitted fields are unchanged; `routes`, when
// present, fully replaces the rule list.
export interface SyslogConfigPatch {
    enabled?: boolean;
    bind?: string;
    udp_port?: number;
    tcp_port?: number;
    default_env?: string;
    default_index?: string;
    routes?: SyslogRoute[];
}

export interface MergeResult {
    merged_segments_total?: number;
    deleted_files_total?: number;
    partitions_touched?: number;
    partitions_skipped?: string[];
    took_ms?: number;
    message?: string;
}

export interface CommitResult {
    committed: string[];
    took_ms: number;
}

export interface GcResult {
    deleted_files: number;
    took_ms: number;
    // Retention sweep outcome (gc runs the sweep).
    partitions_dropped?: number;
    blocks_deleted?: number;
    message?: string;
}

// --- saved searches ---

export interface SavedSearch {
    id: string;
    name: string;
    q: string;
    index?: string | null;
    range: string;
    start?: string | null;
    end?: string | null;
    follow: boolean;
    // `true` = visible to and editable by all users; `false` = owner-only.
    public: boolean;
    created_at: string;
    updated_at: string;
    // Owner display label — only present in the admin "view all" listing.
    owner?: string;
}

export interface SavedSearchInput {
    name: string;
    q: string;
    index?: string;
    range: string;
    start?: string;
    end?: string;
    follow: boolean;
    public: boolean;
}

// --- monitors + alerts ---

// "ai" = LLM-driven investigation; "threshold" = deterministic count check.
export type MonitorKind = "ai" | "threshold";

export type Comparison = "gt" | "gte" | "lt" | "lte" | "eq" | "neq";

export interface ThresholdConfig {
    // query the count is taken over (may include `index:foo`).
    query: string;
    index?: string | null;
    // Lookback window the count spans, ending at evaluation time.
    window_seconds: number;
    comparison: Comparison;
    threshold: number;
    // Severity stamped on the alert when breached.
    severity: string;
}

// Per-monitor alert webhook override; replaces the global target when set.
export interface NotifyOverride {
    webhook_url: string;
    format?: "generic" | "slack" | null;
}

export interface Monitor {
    id: string;
    name: string;
    description: string;
    kind: MonitorKind;
    // AI monitors only — instruction handed to the agent each tick.
    prompt: string;
    // Threshold monitors only — the count/comparison config.
    threshold?: ThresholdConfig | null;
    // Webhook override; null/absent = global settings target.
    notify?: NotifyOverride | null;
    // Cadence in seconds. Floor: 300 (5 min). Default: 1800 (30 min).
    interval_seconds: number;
    enabled: boolean;
    // ms epoch of the last completed run, or null if never run.
    last_run_at: number | null;
    // "ok" | "error" — outcome of the last run.
    last_status: string | null;
    last_error: string | null;
    // Trace of the last run (alert-inbox click-through); null for threshold monitors.
    last_conversation_id: string | null;
    // True while the scheduler holds a lease on the monitor.
    running: boolean;
    running_since: number | null;
    // Alert visibility: true (default) = public, false = owner-only.
    public: boolean;
    // Env this monitor runs against (stamped from the active env at create).
    env: string;
    created_at: string;
    updated_at: string;
    // Owner display label — only present in the admin "view all" listing.
    owner?: string;
}

export interface MonitorInput {
    name: string;
    description?: string;
    kind?: MonitorKind;
    // Required for AI monitors.
    prompt?: string;
    // Required for threshold monitors.
    threshold?: ThresholdConfig | null;
    notify?: NotifyOverride | null;
    interval_seconds?: number;
    enabled?: boolean;
    // Alert visibility. Defaults to public (true) when omitted.
    public?: boolean;
}

export interface MonitorPatch {
    name?: string;
    description?: string;
    kind?: MonitorKind;
    prompt?: string;
    threshold?: ThresholdConfig | null;
    // Empty webhook_url clears the override.
    notify?: NotifyOverride | null;
    interval_seconds?: number;
    enabled?: boolean;
    // Alert visibility. Defaults to public (true) when omitted.
    public?: boolean;
}

export interface Alert {
    id: string;
    monitor_id: string;
    monitor_name: string;
    // Env the raising monitor targets (denormalized at create time).
    env: string;
    // Trace conversation — null if the agent crashed before writing it.
    conversation_id: string | null;
    severity: "low" | "medium" | "high";
    title: string;
    summary: string;
    // Free-form JSON evidence; the inbox renders it as a key/value block.
    evidence: Record<string, unknown> | null;
    // True = public monitor (all users see/ack it); false = owner-only.
    public: boolean;
    acknowledged: boolean;
    acknowledged_at: number | null;
    // Toast-dismissed per requesting user; independent of `acknowledged`, still shown in inbox.
    dismissed: boolean;
    created_at: number;
}

// --- dashboards ---

// The widget kinds a dashboard can hold. Query-driven charts +
// list/status widgets over existing control-plane entities.
export type WidgetKind =
    | "timeseries"
    | "stat"
    | "topn"
    | "search_results"
    | "alerts"
    | "saved_searches"
    // Deprecated kinds kept for existing dashboards; not in the picker.
    | "alerts_history"
    | "monitors";

// One plotted query in a chart widget. `query` is the full pipelined
// search (index lives inside it, e.g. `index:api level:error`).
export interface Series {
    id: string;
    label: string;
    query: string;
    color: string;
}

// react-grid-layout placement for a widget (grid units; cols=12).
export interface GridPos {
    x: number;
    y: number;
    w: number;
    h: number;
}

export interface Widget {
    id: string;
    kind: WidgetKind;
    title: string;
    layout: GridPos;
    // timeseries multi-series; stat/search_results/topn use series[0].
    series?: Series[];
    chart?: "line" | "bar" | "area";
    // topn: the field to break down by + how many rows.
    field?: string;
    size?: number;
    // list / search-results widgets: max rows to show.
    limit?: number;
    // Per-widget time-range override; falls back to the dashboard's range.
    time?: { range?: string; start?: string; end?: string };
}

// The opaque `spec` blob persisted on the backend. Frontend owns the schema.
export interface DashboardSpec {
    // Default relative range (e.g. `-24h`) unless the picker sets absolute bounds.
    time_range: string;
    start?: string;
    end?: string;
    // Auto-refresh cadence in seconds; 0/undefined = manual only.
    refresh_secs?: number;
    widgets: Widget[];
}

export interface Dashboard {
    id: string;
    name: string;
    description: string;
    spec: DashboardSpec;
    public: boolean;
    created_at: string;
    updated_at: string;
    // Owner display label — only present in the admin "view all" listing.
    owner?: string;
}

export interface DashboardInput {
    name: string;
    description?: string;
    spec?: DashboardSpec;
    public?: boolean;
}

export interface DashboardPatch {
    name?: string;
    description?: string;
    spec?: DashboardSpec;
    public?: boolean;
}

// --- ingestion sources ---

export interface Source {
    id: string;
    name: string;
    env: string;
    index: string;
    // "fs" (local/mounted files) | "s3" (object store). Only these two today.
    kind: string;
    // "pull" (poll) — "watch"/"event" reserved for later.
    mode: string;
    // Glob the source matches, e.g. `/var/log/**/*.log` or `s3://bucket/prefix/**/*.gz`.
    path: string;
    exclude: string[];
    // auto | ndjson | json | text | syslog.
    format: string;
    // auto | none | gzip | zstd.
    compression: string;
    multiline_pattern: string | null;
    multiline_max_lines: number | null;
    // Grok / named-capture-regex pattern (or preset name) for format=grok.
    grok_pattern: string | null;
    // Poll interval for pull mode (floor 5s).
    interval_seconds: number;
    source_tag: string | null;
    enabled: boolean;
    // ms epoch of the last completed poll, or null if never run.
    last_run_at: number | null;
    // "ok" | "error" — outcome of the last poll.
    last_status: string | null;
    last_error: string | null;
    // Lifetime rows ingested across all polls.
    total_ingested: number;
    running: boolean;
    running_since: number | null;
    // Rows ingested so far in the current run (live; 0 when not running).
    progress_ingested: number;
    // File/key currently being read this run, or null.
    progress_file: string | null;
    created_at: string;
    updated_at: string;
}

// How far one file has been consumed. `offset` is bytes read (uncompressed
// tail) or last-seen size (whole-file mode); `mtime_ms` detects change.
export interface FileMark {
    offset: number;
    mtime_ms: number;
}

export interface SourceCheckpoint {
    files: Record<string, FileMark>;
}

// `GET /api/sources/:id` — a source plus its per-file ingest checkpoint.
export interface SourceDetail {
    source: Source;
    checkpoint: SourceCheckpoint;
}

// One directory entry from the server-side folder picker.
export interface BrowseEntry {
    name: string;
    path: string;
}

export interface BrowseResult {
    // Absolute, canonicalized path that was listed.
    path: string;
    // Parent directory, or null at the filesystem root.
    parent: string | null;
    dirs: BrowseEntry[];
}

export interface SourceInput {
    name: string;
    // Target workspace. Defaults server-side to the request's active env.
    env?: string;
    index: string;
    kind?: string;
    mode?: string;
    path: string;
    exclude?: string[];
    format?: string;
    compression?: string;
    multiline_pattern?: string | null;
    multiline_max_lines?: number | null;
    grok_pattern?: string | null;
    interval_seconds?: number;
    source_tag?: string | null;
    enabled?: boolean;
}

export interface SourcePatch {
    name?: string;
    // Move the source to a different workspace.
    env?: string;
    index?: string;
    mode?: string;
    path?: string;
    exclude?: string[];
    format?: string;
    compression?: string;
    multiline_pattern?: string | null;
    multiline_max_lines?: number | null;
    grok_pattern?: string | null;
    interval_seconds?: number;
    source_tag?: string | null;
    enabled?: boolean;
}

// --- scoped push tokens ---

export interface PushTokenView {
    id: string;
    name: string;
    // Masked secret, e.g. `…a1b2`. The full value is shown only once at creation.
    token_hint: string;
    env: string;
    indexes: string[];
    enabled: boolean;
    last_used_at: number | null;
    created_at: string;
    updated_at: string;
}

export interface IngestAuthConfig {
    // When true, ingest/shim requests without a valid token are rejected.
    require: boolean;
    // HTTP ingestion classes, on by default. `api` = /api/ingest(+/raw);
    // `shims` = the Elasticsearch/OTLP/Loki/HEC compatibility endpoints.
    api_enabled: boolean;
    shims_enabled: boolean;
    tokens: PushTokenView[];
}

// Create response — carries the full secret, returned exactly once.
export interface CreatedPushToken {
    id: string;
    name: string;
    env: string;
    indexes: string[];
    token: string;
}

// --- API keys ---

// Multi-select grant. `admin` is a superset of `api`. `mcp` gates the MCP server.
export interface ApiKeyScopes {
    api: boolean;
    admin: boolean;
    mcp: boolean;
}

export interface ApiKeyView {
    id: string;
    name: string;
    description: string;
    // Masked secret, e.g. `…a1b2`. The full value is shown only once at creation.
    token_hint: string;
    scopes: ApiKeyScopes;
    enabled: boolean;
    created_at: string;
    last_used_at: number | null;
    expires_at: number | null;
}

// Create response — carries the full secret, returned exactly once.
export interface CreatedApiKey {
    id: string;
    name: string;
    description: string;
    scopes: ApiKeyScopes;
    expires_at: number | null;
    token: string;
}
