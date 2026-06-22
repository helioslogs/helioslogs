// Copyright 2026 Appbird LLC. Licensed under "GNU Affero General Public License v3.0"

// Thin wrappers around the HeliosLogs HTTP API. Vite proxies /api → backend.

import type {
    AggregateResponse,
    Alert,
    ApiKeyScopes,
    ApiKeyView,
    CatalogInfo,
    CommitResult,
    CreatedApiKey,
    CreatedPushToken,
    Dashboard,
    DashboardInput,
    DashboardPatch,
    DiscoverFieldsResponse,
    IngestAuthConfig,
    GcResult,
    HistogramResponse,
    MergeResult,
    Monitor,
    MonitorInput,
    MonitorPatch,
    RuntimeConfigEntry,
    SamlStatus,
    SamlConfig,
    SamlConfigPatch,
    SyslogConfig,
    SyslogConfigPatch,
    SavedSearch,
    SavedSearchInput,
    Source,
    SourceDetail,
    SourceInput,
    SourcePatch,
    BrowseResult,
    SearchPartition,
    SearchPartitionsResponse,
    SearchResponse,
    SearchHistogramResponse,
    Settings,
    SettingsPatch,
    Tunable,
    Stats,
    TestWebhookResult,
} from "./types";
import { notifyDemoBlocked } from "./events";

// Auth is a signed JWT carried as `Authorization: Bearer`. The active env is
// a per-browser UI preference appended to every request as `?env=`. Both live
// in localStorage — there are no auth cookies.

const TOKEN_KEY = "helios.auth";
const ENV_KEY = "helios.env";

export function getToken(): string | null {
    return localStorage.getItem(TOKEN_KEY);
}
export function setToken(token: string): void {
    localStorage.setItem(TOKEN_KEY, token);
}
export function clearToken(): void {
    localStorage.removeItem(TOKEN_KEY);
}
export function getEnv(): string {
    return localStorage.getItem(ENV_KEY) || "default";
}
export function setEnv(env: string): void {
    localStorage.setItem(ENV_KEY, env);
}

export async function apiFetch(input: RequestInfo, init?: RequestInit): Promise<Response> {
    const headers = new Headers(init?.headers);
    const token = getToken();
    if (token) headers.set("Authorization", `Bearer ${token}`);

    const r = await fetch(withEnv(input), { ...init, headers });

    // Sliding renewal: the server hands back a fresh token once the current one
    // crosses its renewal threshold. Swap it in transparently.
    const refreshed = r.headers.get("X-Helios-Token-Refresh");
    if (refreshed) setToken(refreshed);

    if (r.status === 401) {
        // Missing / expired / revoked token — drop it and boot back to login.
        clearToken();
        window.dispatchEvent(new CustomEvent("helios-401"));
    } else if (r.status === 403) {
        // A write rejected by read-only demo mode carries `demo_mode: true`. Peek a
        // clone (the caller still reads the original) and surface a global toast.
        void r
            .clone()
            .json()
            .then((b) => {
                if (b && b.demo_mode) {
                    notifyDemoBlocked(
                        typeof b.error === "string"
                            ? b.error
                            : "This is a read-only demo — changes are disabled.",
                    );
                }
            })
            .catch(() => {
                /* non-JSON 403 (e.g. plain admin-only) — ignore */
            });
    }
    return r;
}

// Append the active env as `?env=` unless the caller already set one. Keeps
// the relative path+query form so the Vite proxy still routes /api → backend.
function withEnv(input: RequestInfo): RequestInfo {
    if (typeof input !== "string") return input;
    const u = new URL(input, window.location.origin);
    if (!u.searchParams.has("env")) u.searchParams.set("env", getEnv());
    return u.pathname + u.search;
}

function qs(params: Record<string, string | number | boolean | undefined> | object): string {
    const u = new URLSearchParams();
    for (const [k, v] of Object.entries(params)) {
        if (v === undefined || v === "" || v === false) continue;
        u.set(k, String(v));
    }
    const s = u.toString();
    return s ? `?${s}` : "";
}

export async function getStats(): Promise<Stats> {
    const r = await apiFetch("/api/stats");
    if (!r.ok) throw new Error(`stats: ${r.status}`);
    return r.json();
}

export interface SearchArgs {
    q: string;
    index?: string;
    // Override the active env (`?env=`) to scope a one-off search, e.g. `_system`.
    env?: string;
    start?: string;
    end?: string;
    // 0-based offset into the merged result set for page navigation.
    offset?: number;
    limit?: number;
    // Comma-separated `env:index:yyyy-mm-dd` triples; intersected with the regular plan (invalid ones dropped). Drives day-by-day streaming.
    partitions?: string;
}

export async function search(
    args: SearchArgs,
    init?: { signal?: AbortSignal },
): Promise<SearchResponse> {
    const r = await apiFetch(`/api/search${qs(args)}`, init);
    if (!r.ok) {
        const body = await r.text();
        throw new Error(`search ${r.status}: ${body}`);
    }
    return r.json();
}

export interface HistogramArgs {
    q: string;
    index?: string;
    start?: string;
    end?: string;
    interval?: string;
    // See `SearchArgs.partitions`.
    partitions?: string;
}

export async function histogram(
    args: HistogramArgs,
    init?: { signal?: AbortSignal },
): Promise<HistogramResponse> {
    const r = await apiFetch(`/api/histogram${qs(args)}`, init);
    if (!r.ok) {
        const body = await r.text();
        throw new Error(`histogram ${r.status}: ${body}`);
    }
    return r.json();
}

// Hits page + histogram in one request — one filter pass per partition for
// both, halving the work versus separate `search` + `histogram` calls.
export async function searchHistogram(
    args: SearchArgs & { interval?: string },
    init?: { signal?: AbortSignal },
): Promise<SearchHistogramResponse> {
    const r = await apiFetch(`/api/search_histogram${qs(args)}`, init);
    if (!r.ok) {
        const body = await r.text();
        throw new Error(`search_histogram ${r.status}: ${body}`);
    }
    return r.json();
}

// Partitions the query+range would scan, most-recent day first. Plans the
// day-by-day streaming UX (one search/histogram pair fired per day).
export async function listSearchPartitions(
    args: {
        q: string;
        index?: string;
        start?: string;
        end?: string;
    },
    init?: { signal?: AbortSignal },
): Promise<SearchPartitionsResponse> {
    const r = await apiFetch(`/api/search_partitions${qs(args)}`, init);
    if (!r.ok) {
        const body = await r.text();
        throw new Error(`search_partitions ${r.status}: ${body}`);
    }
    return r.json();
}

// Wire format for a single partition triple in the `partitions=` param.
export function partitionToParam(p: SearchPartition): string {
    return `${p.env}:${p.index}:${p.day}`;
}

export async function aggregate(args: {
    q: string;
    index?: string;
    start?: string;
    end?: string;
    fields?: string;
    size?: number;
    // Opt into stride sampling for wide queries. Backend trades exact
    // counts for latency when the query touches > 16 partitions.
    approximate?: boolean;
}): Promise<AggregateResponse> {
    const r = await apiFetch(`/api/aggregate${qs(args)}`);
    if (!r.ok) {
        const body = await r.text();
        throw new Error(`aggregate ${r.status}: ${body}`);
    }
    return r.json();
}

// Fetch the footer-derived field catalog for the current env/index/window.
// Drives the sidebar — the true (path, type) schema, stable across searches.
export async function discoverFields(
    args: {
        q: string;
        index?: string;
        start?: string;
        end?: string;
        top?: number;
    },
    init?: { signal?: AbortSignal },
): Promise<DiscoverFieldsResponse> {
    const r = await apiFetch(`/api/discover_fields${qs(args)}`, init);
    if (!r.ok) {
        const body = await r.text();
        throw new Error(`discover_fields ${r.status}: ${body}`);
    }
    return r.json();
}

// --- admin ---

export async function getIndexes(): Promise<string[]> {
    const r = await apiFetch(`/api/indexes`);
    if (!r.ok) throw new Error(`indexes: ${r.status}`);
    const j = await r.json();
    return j.indexes ?? [];
}

export async function getCatalogInfo(): Promise<CatalogInfo> {
    const r = await apiFetch("/api/admin/index-info");
    if (!r.ok) throw new Error(`index-info: ${r.status}`);
    return r.json();
}

// Distinct `(env, index)` names only — far cheaper than `index-info`. For the
// allowlist UIs (user grants, MCP) that just need the names.
export async function getCatalogIndexes(): Promise<{ env: string; index: string }[]> {
    const r = await apiFetch("/api/admin/index-catalog");
    if (!r.ok) throw new Error(`index-catalog: ${r.status}`);
    const j = await r.json();
    return j.indexes ?? [];
}

export async function getSettings(): Promise<Settings> {
    const r = await apiFetch("/api/admin/settings");
    if (!r.ok) throw new Error(`settings: ${r.status}`);
    return r.json();
}

// Public — drives the "Sign in with SSO" button on the login page. Works
// without a token.
export async function getSamlStatus(): Promise<SamlStatus> {
    const r = await apiFetch("/api/auth/saml/status");
    if (!r.ok) throw new Error(`saml status: ${r.status}`);
    return r.json();
}

export async function getSamlConfig(): Promise<SamlConfig> {
    const r = await apiFetch("/api/admin/saml");
    if (!r.ok) throw new Error(`saml config: ${r.status}`);
    return r.json();
}

export async function updateSamlConfig(patch: SamlConfigPatch): Promise<SamlConfig> {
    const r = await apiFetch("/api/admin/saml", {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: JSON.stringify(patch),
    });
    if (!r.ok) {
        const body = await r.text();
        throw new Error(`updateSamlConfig ${r.status}: ${body}`);
    }
    return r.json();
}

export async function getSyslogConfig(): Promise<SyslogConfig> {
    const r = await apiFetch("/api/admin/syslog");
    if (!r.ok) throw new Error(`syslog config: ${r.status}`);
    return r.json();
}

export async function updateSyslogConfig(patch: SyslogConfigPatch): Promise<SyslogConfig> {
    const r = await apiFetch("/api/admin/syslog", {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: JSON.stringify(patch),
    });
    if (!r.ok) {
        const body = await r.text();
        throw new Error(`updateSyslogConfig ${r.status}: ${body}`);
    }
    return r.json();
}

export async function getRuntimeConfig(): Promise<RuntimeConfigEntry[]> {
    const r = await apiFetch("/api/admin/runtime-config");
    if (!r.ok) throw new Error(`runtime-config: ${r.status}`);
    const body = await r.json();
    return body.entries as RuntimeConfigEntry[];
}

// Editable server tunables (env > control setting > default).
export async function getTunables(): Promise<Tunable[]> {
    const r = await apiFetch("/api/admin/tunables");
    if (!r.ok) throw new Error(`tunables: ${r.status}`);
    const body = await r.json();
    return body.entries as Tunable[];
}

// Set a tunable's stored value, or clear it (back to env/default) with null.
// Returns the refreshed list.
export async function updateTunable(id: string, value: number | null): Promise<Tunable[]> {
    const r = await apiFetch("/api/admin/tunables", {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: JSON.stringify({ id, value }),
    });
    if (!r.ok) {
        const body = await r.text();
        throw new Error(`updateTunable ${r.status}: ${body}`);
    }
    const body = await r.json();
    return body.entries as Tunable[];
}

export async function updateSettings(patch: SettingsPatch): Promise<Settings> {
    const r = await apiFetch("/api/admin/settings", {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: JSON.stringify(patch),
    });
    if (!r.ok) {
        const body = await r.text();
        throw new Error(`updateSettings ${r.status}: ${body}`);
    }
    return r.json();
}

// Send a synthetic alert at the given (or saved) webhook target.
export async function testAlertWebhook(args: {
    url?: string;
    format?: string;
}): Promise<TestWebhookResult> {
    const r = await apiFetch("/api/admin/alerts/test-webhook", {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: JSON.stringify(args),
    });
    if (!r.ok) {
        const body = await r.text();
        throw new Error(`test-webhook ${r.status}: ${body}`);
    }
    return r.json();
}

export async function mergeSegments(args?: {
    env?: string;
    index?: string;
    day?: string;
}): Promise<MergeResult> {
    const u = new URLSearchParams();
    if (args?.env) u.set("env", args.env);
    if (args?.index) u.set("index", args.index);
    if (args?.day) u.set("day", args.day);
    const r = await apiFetch(`/api/admin/merge${u.toString() ? `?${u}` : ""}`, { method: "POST" });
    if (!r.ok) {
        const body = await r.text();
        throw new Error(`merge ${r.status}: ${body}`);
    }
    return r.json();
}

export async function forceCommit(): Promise<CommitResult> {
    const r = await apiFetch("/api/admin/commit", { method: "POST" });
    if (!r.ok) {
        const body = await r.text();
        throw new Error(`commit ${r.status}: ${body}`);
    }
    return r.json();
}

export async function gcFiles(): Promise<GcResult> {
    const r = await apiFetch("/api/admin/gc", { method: "POST" });
    if (!r.ok) {
        const body = await r.text();
        throw new Error(`gc ${r.status}: ${body}`);
    }
    return r.json();
}

// --- saved searches ---

export async function listSearches(all = false): Promise<SavedSearch[]> {
    const r = await apiFetch(`/api/searches${all ? "?all=true" : ""}`);
    if (!r.ok) throw new Error(`searches: ${r.status}`);
    const j = await r.json();
    return j.searches ?? [];
}

export async function createSearch(input: SavedSearchInput): Promise<SavedSearch> {
    const r = await apiFetch("/api/searches", {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: JSON.stringify(input),
    });
    if (!r.ok) {
        const body = await r.text();
        throw new Error(`create ${r.status}: ${body}`);
    }
    return r.json();
}

// Like `SavedSearchInput`, but nullable fields accept `null` to clear them
// server-side (backend `Option<Option<_>>` for index/start/end).
export type SavedSearchPatch = Partial<Omit<SavedSearchInput, "index" | "start" | "end">> & {
    index?: string | null;
    start?: string | null;
    end?: string | null;
};

export async function updateSearch(id: string, patch: SavedSearchPatch): Promise<SavedSearch> {
    const r = await apiFetch(`/api/searches/${encodeURIComponent(id)}`, {
        method: "PATCH",
        headers: { "content-type": "application/json" },
        body: JSON.stringify(patch),
    });
    if (!r.ok) {
        const body = await r.text();
        throw new Error(`update ${r.status}: ${body}`);
    }
    return r.json();
}

export async function deleteSearch(id: string): Promise<void> {
    const r = await apiFetch(`/api/searches/${encodeURIComponent(id)}`, { method: "DELETE" });
    if (!r.ok) {
        const body = await r.text();
        throw new Error(`delete ${r.status}: ${body}`);
    }
}

// --- dashboards ---
// Not env-scoped: widgets follow the active env at view time; the `?env=`
// apiFetch appends is ignored by the backend.

export async function listDashboards(all = false): Promise<Dashboard[]> {
    const r = await apiFetch(`/api/dashboards${all ? "?all=true" : ""}`);
    if (!r.ok) throw new Error(`dashboards: ${r.status}`);
    const j = await r.json();
    return j.dashboards ?? [];
}

export async function getDashboard(id: string): Promise<Dashboard> {
    const r = await apiFetch(`/api/dashboards/${encodeURIComponent(id)}`);
    if (!r.ok) {
        const body = await r.text();
        throw new Error(`dashboard ${r.status}: ${body}`);
    }
    return r.json();
}

export async function createDashboard(input: DashboardInput): Promise<Dashboard> {
    const r = await apiFetch("/api/dashboards", {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: JSON.stringify(input),
    });
    if (!r.ok) {
        const body = await r.text();
        throw new Error(`create ${r.status}: ${body}`);
    }
    return r.json();
}

export async function updateDashboard(id: string, patch: DashboardPatch): Promise<Dashboard> {
    const r = await apiFetch(`/api/dashboards/${encodeURIComponent(id)}`, {
        method: "PATCH",
        headers: { "content-type": "application/json" },
        body: JSON.stringify(patch),
    });
    if (!r.ok) {
        const body = await r.text();
        throw new Error(`update ${r.status}: ${body}`);
    }
    return r.json();
}

export async function deleteDashboard(id: string): Promise<void> {
    const r = await apiFetch(`/api/dashboards/${encodeURIComponent(id)}`, { method: "DELETE" });
    if (!r.ok) {
        const body = await r.text();
        throw new Error(`delete ${r.status}: ${body}`);
    }
}

// --- monitors ---

export async function listMonitors(all = false): Promise<Monitor[]> {
    const r = await apiFetch(`/api/monitors${all ? "?all=true" : ""}`);
    if (!r.ok) throw new Error(`monitors: ${r.status}`);
    const j = await r.json();
    return j.monitors ?? [];
}

export async function getMonitor(id: string): Promise<Monitor> {
    const r = await apiFetch(`/api/monitors/${encodeURIComponent(id)}`);
    if (!r.ok) throw new Error(`monitor ${r.status}`);
    return r.json();
}

export async function createMonitor(input: MonitorInput): Promise<Monitor> {
    const r = await apiFetch("/api/monitors", {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: JSON.stringify(input),
    });
    if (!r.ok) {
        const body = await r.text();
        throw new Error(`create monitor ${r.status}: ${body}`);
    }
    return r.json();
}

export async function updateMonitor(id: string, patch: MonitorPatch): Promise<Monitor> {
    const r = await apiFetch(`/api/monitors/${encodeURIComponent(id)}`, {
        method: "PATCH",
        headers: { "content-type": "application/json" },
        body: JSON.stringify(patch),
    });
    if (!r.ok) {
        const body = await r.text();
        throw new Error(`update monitor ${r.status}: ${body}`);
    }
    return r.json();
}

export async function deleteMonitor(id: string): Promise<void> {
    const r = await apiFetch(`/api/monitors/${encodeURIComponent(id)}`, { method: "DELETE" });
    if (!r.ok) {
        const body = await r.text();
        throw new Error(`delete monitor ${r.status}: ${body}`);
    }
}

// Run a monitor on the next scheduler tick (within 10s). Server clears
// `last_run_at` so the next walk picks it up; the run itself is async.
export async function runMonitorNow(id: string): Promise<void> {
    const r = await apiFetch(`/api/monitors/${encodeURIComponent(id)}/run`, {
        method: "POST",
    });
    if (!r.ok) {
        const body = await r.text();
        throw new Error(`run monitor ${r.status}: ${body}`);
    }
}

// --- ingestion sources ---

// Lists every source the caller owns across all environments (admin view).
export async function listSources(): Promise<Source[]> {
    const r = await apiFetch("/api/sources?all=true");
    if (!r.ok) throw new Error(`sources: ${r.status}`);
    const j = await r.json();
    return j.sources ?? [];
}

// One source plus its per-file ingest checkpoint (progress / state view).
export async function getSource(id: string): Promise<SourceDetail> {
    const r = await apiFetch(`/api/sources/${encodeURIComponent(id)}`);
    if (!r.ok) throw new Error(`source ${r.status}`);
    return r.json();
}

// Server-side directory listing for the folder picker (admin only). `path`
// defaults to the filesystem root when omitted.
export async function browseDir(path?: string): Promise<BrowseResult> {
    const u = new URL("/api/sources/browse", window.location.origin);
    if (path) u.searchParams.set("path", path);
    const r = await apiFetch(u.pathname + u.search);
    if (!r.ok) {
        const body = await r.text();
        let msg = `browse ${r.status}`;
        try {
            msg = JSON.parse(body).error ?? msg;
        } catch {
            /* ignore */
        }
        throw new Error(msg);
    }
    return r.json();
}

// Creates a source. `env` overrides the active env (the dialog lets the
// operator pick the target workspace).
export async function createSource(input: SourceInput, env?: string): Promise<Source> {
    const u = new URL("/api/sources", window.location.origin);
    if (env) u.searchParams.set("env", env);
    const r = await apiFetch(u.pathname + u.search, {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: JSON.stringify(input),
    });
    if (!r.ok) {
        const body = await r.text();
        throw new Error(`create source ${r.status}: ${body}`);
    }
    return r.json();
}

export async function updateSource(id: string, patch: SourcePatch): Promise<Source> {
    const r = await apiFetch(`/api/sources/${encodeURIComponent(id)}`, {
        method: "PATCH",
        headers: { "content-type": "application/json" },
        body: JSON.stringify(patch),
    });
    if (!r.ok) {
        const body = await r.text();
        throw new Error(`update source ${r.status}: ${body}`);
    }
    return r.json();
}

export async function deleteSource(id: string): Promise<void> {
    const r = await apiFetch(`/api/sources/${encodeURIComponent(id)}`, { method: "DELETE" });
    if (!r.ok) {
        const body = await r.text();
        throw new Error(`delete source ${r.status}: ${body}`);
    }
}

// Trigger an immediate poll on the next supervisor tick (within ~5s). Server
// clears `last_run_at`; the run itself is async.
export async function runSourceNow(id: string): Promise<void> {
    const r = await apiFetch(`/api/sources/${encodeURIComponent(id)}/run`, {
        method: "POST",
    });
    if (!r.ok) {
        const body = await r.text();
        throw new Error(`run source ${r.status}: ${body}`);
    }
}

// Wipe ingestion state (checkpoint + counters) so the next run re-ingests
// every matching file. Refuses (409) while a run is in flight.
export async function resetSource(id: string): Promise<void> {
    const r = await apiFetch(`/api/sources/${encodeURIComponent(id)}/reset`, {
        method: "POST",
    });
    if (!r.ok) {
        const body = await r.text();
        let msg = `reset source ${r.status}`;
        try {
            msg = JSON.parse(body).error ?? msg;
        } catch {
            /* ignore */
        }
        throw new Error(msg);
    }
}

// --- scoped push tokens (admin) ---

export async function getIngestTokens(): Promise<IngestAuthConfig> {
    const r = await apiFetch("/api/admin/ingest-tokens");
    if (!r.ok) throw new Error(`ingest-tokens: ${r.status}`);
    return r.json();
}

export async function createIngestToken(input: {
    name: string;
    env: string;
    indexes: string[];
}): Promise<CreatedPushToken> {
    const r = await apiFetch("/api/admin/ingest-tokens", {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: JSON.stringify(input),
    });
    if (!r.ok) {
        const body = await r.text();
        throw new Error(`create token ${r.status}: ${body}`);
    }
    return r.json();
}

export async function setIngestTokenEnabled(id: string, enabled: boolean): Promise<void> {
    const r = await apiFetch(`/api/admin/ingest-tokens/${encodeURIComponent(id)}`, {
        method: "PATCH",
        headers: { "content-type": "application/json" },
        body: JSON.stringify({ enabled }),
    });
    if (!r.ok) throw new Error(`update token ${r.status}`);
}

export async function deleteIngestToken(id: string): Promise<void> {
    const r = await apiFetch(`/api/admin/ingest-tokens/${encodeURIComponent(id)}`, {
        method: "DELETE",
    });
    if (!r.ok) throw new Error(`delete token ${r.status}`);
}

// --- REST API keys (admin) ---

export async function listApiKeys(): Promise<ApiKeyView[]> {
    const r = await apiFetch("/api/admin/api-keys");
    if (!r.ok) throw new Error(`api-keys: ${r.status}`);
    const j = await r.json();
    return j.keys ?? [];
}

export async function createApiKey(input: {
    name: string;
    description?: string;
    scopes: ApiKeyScopes;
    expires_in_days?: number | null;
}): Promise<CreatedApiKey> {
    const r = await apiFetch("/api/admin/api-keys", {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: JSON.stringify(input),
    });
    if (!r.ok) {
        const body = await r.text();
        let msg = `create key ${r.status}`;
        try {
            msg = JSON.parse(body).error ?? msg;
        } catch {
            /* ignore */
        }
        throw new Error(msg);
    }
    return r.json();
}

export async function setApiKeyEnabled(id: string, enabled: boolean): Promise<void> {
    const r = await apiFetch(`/api/admin/api-keys/${encodeURIComponent(id)}`, {
        method: "PATCH",
        headers: { "content-type": "application/json" },
        body: JSON.stringify({ enabled }),
    });
    if (!r.ok) throw new Error(`update key ${r.status}`);
}

export async function deleteApiKey(id: string): Promise<void> {
    const r = await apiFetch(`/api/admin/api-keys/${encodeURIComponent(id)}`, {
        method: "DELETE",
    });
    if (!r.ok) throw new Error(`delete key ${r.status}`);
}

export async function setIngestRequire(require: boolean): Promise<void> {
    const r = await apiFetch("/api/admin/ingest-auth", {
        method: "PUT",
        headers: { "content-type": "application/json" },
        body: JSON.stringify({ require }),
    });
    if (!r.ok) throw new Error(`set ingest require ${r.status}`);
}

// Enable/disable whole HTTP ingestion classes (native API and/or shims).
export async function setIngestEndpoints(patch: {
    api_enabled?: boolean;
    shims_enabled?: boolean;
}): Promise<void> {
    const r = await apiFetch("/api/admin/ingest-auth", {
        method: "PUT",
        headers: { "content-type": "application/json" },
        body: JSON.stringify(patch),
    });
    if (!r.ok) throw new Error(`set ingest endpoints ${r.status}`);
}

export async function listMonitorAlerts(id: string): Promise<Alert[]> {
    const r = await apiFetch(`/api/monitors/${encodeURIComponent(id)}/alerts`);
    if (!r.ok) throw new Error(`monitor alerts ${r.status}`);
    const j = await r.json();
    return j.alerts ?? [];
}

// --- alerts ---

export async function listAlerts(
    opts: {
        unackedOnly?: boolean;
        search?: string;
        monitor?: string | null;
        limit?: number;
    } = {},
): Promise<Alert[]> {
    const p = new URLSearchParams();
    if (opts.unackedOnly) p.set("unacked", "true");
    const q = opts.search?.trim();
    if (q) p.set("q", q);
    if (opts.monitor) p.set("monitor", opts.monitor);
    if (opts.limit) p.set("limit", String(opts.limit));
    const qs = p.toString();
    const r = await apiFetch(`/api/alerts${qs ? `?${qs}` : ""}`);
    if (!r.ok) throw new Error(`alerts: ${r.status}`);
    const j = await r.json();
    return j.alerts ?? [];
}

// Unacknowledged-alert count — drives the nav badge. Cheap; polled at
// the top level on a slow timer.
export async function getUnackedAlertCount(): Promise<number> {
    const r = await apiFetch("/api/alerts/unacked-count");
    if (!r.ok) throw new Error(`unacked count: ${r.status}`);
    const j = await r.json();
    return j.unacked ?? 0;
}

export async function acknowledgeAlert(id: string): Promise<void> {
    const r = await apiFetch(`/api/alerts/${encodeURIComponent(id)}`, {
        method: "PATCH",
        headers: { "content-type": "application/json" },
        body: JSON.stringify({ acknowledged: true }),
    });
    if (!r.ok) {
        const body = await r.text();
        throw new Error(`ack alert ${r.status}: ${body}`);
    }
}

// Toast dismissal — server-side so it persists across reloads/tabs/devices.
// Does not acknowledge: the alert stays in the inbox.
export async function dismissAlert(id: string): Promise<void> {
    const r = await apiFetch(`/api/alerts/${encodeURIComponent(id)}`, {
        method: "PATCH",
        headers: { "content-type": "application/json" },
        body: JSON.stringify({ dismissed: true }),
    });
    if (!r.ok) {
        const body = await r.text();
        throw new Error(`dismiss alert ${r.status}: ${body}`);
    }
}

export async function dismissAllAlerts(): Promise<void> {
    const r = await apiFetch("/api/alerts/dismiss-all", { method: "POST" });
    if (!r.ok) {
        const body = await r.text();
        throw new Error(`dismiss all ${r.status}: ${body}`);
    }
}

// --- auth ---

export type AuthUser = {
    user_id: string;
    userid: string;
    email: string;
    display_name: string;
    is_admin: boolean;
    // Env this session is pinned to; default read scope when `&env=` is absent.
    active_env: string;
    // `null` = unset (fall back to localStorage / instance default).
    timezone?: string | null;
    theme?: string | null;
    palette?: string | null;
};

// Persist the caller's display preferences (omitted fields unchanged; empty
// string clears a pref so the account follows the instance default).
// Best-effort write-through behind the localStorage cache; fire-and-forget.
export async function updateAccountPreferences(patch: {
    timezone?: string;
    theme?: string;
    palette?: string;
}): Promise<void> {
    const r = await apiFetch("/api/account/preferences", {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: JSON.stringify(patch),
    });
    if (!r.ok) throw new Error(`preferences: ${r.status}`);
}

export type EnvRow = {
    name: string;
    is_system: boolean;
    created_at: string;
    // Days to keep this env's day-partitions; absent = global default applies.
    retention_days?: number | null;
    // Picker display order (ascending); server-assigned, rewritten by reorder.
    order_index?: number;
};

// Set or clear (null) an env's retention override.
export async function setEnvRetention(name: string, days: number | null): Promise<EnvRow> {
    const r = await apiFetch(`/api/admin/envs/${encodeURIComponent(name)}`, {
        method: "PATCH",
        headers: { "content-type": "application/json" },
        body: JSON.stringify({ retention_days: days }),
    });
    if (!r.ok) {
        const body = await r.text();
        throw new Error(`set retention ${r.status}: ${body}`);
    }
    const j = await r.json();
    return j.env;
}

// Registered envs visible to the caller (top-nav picker). `includeSystem`
// adds `_system` — admin-only, silently ignored for non-admins.
export async function listEnvs(includeSystem = false): Promise<EnvRow[]> {
    const qs = includeSystem ? "?include_system=true" : "";
    const r = await apiFetch(`/api/envs${qs}`);
    if (!r.ok) throw new Error(`envs: ${r.status}`);
    const j = await r.json();
    return j.envs ?? [];
}

// Like `listEnvs` but also returns the admin-set login default (null if unset).
export async function listEnvsWithDefault(
    includeSystem = false,
): Promise<{ envs: EnvRow[]; defaultEnv: string | null }> {
    const qs = includeSystem ? "?include_system=true" : "";
    const r = await apiFetch(`/api/envs${qs}`);
    if (!r.ok) throw new Error(`envs: ${r.status}`);
    const j = await r.json();
    return { envs: j.envs ?? [], defaultEnv: j.default_env ?? null };
}

// Rewrite the env picker order (admin). `names` is the desired order (ascending).
export async function reorderEnvs(names: string[]): Promise<EnvRow[]> {
    const r = await apiFetch("/api/admin/env-order", {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: JSON.stringify({ names }),
    });
    if (!r.ok) {
        const body = await r.text();
        let msg = `reorder envs ${r.status}`;
        try {
            msg = JSON.parse(body).error ?? msg;
        } catch {
            /* ignore */
        }
        throw new Error(msg);
    }
    const j = await r.json();
    return j.envs ?? [];
}

// Set or clear (null) the env auto-selected for newly logged-in users (admin).
export async function setDefaultEnv(name: string | null): Promise<string | null> {
    const r = await apiFetch("/api/admin/env-default", {
        method: name === null ? "DELETE" : "PUT",
        headers: name === null ? undefined : { "content-type": "application/json" },
        body: name === null ? undefined : JSON.stringify({ name }),
    });
    if (!r.ok) {
        const body = await r.text();
        let msg = `set default env ${r.status}`;
        try {
            msg = JSON.parse(body).error ?? msg;
        } catch {
            /* ignore */
        }
        throw new Error(msg);
    }
    const j = await r.json();
    return j.default_env ?? null;
}

// Active env is a client-side preference (see `getEnv`/`setEnv`); the old
// `setSessionEnv` PATCH endpoint is gone.

export async function createEnv(name: string): Promise<EnvRow> {
    const r = await apiFetch("/api/admin/envs", {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: JSON.stringify({ name }),
    });
    if (!r.ok) {
        const body = await r.text();
        let msg = `create env ${r.status}`;
        try {
            msg = JSON.parse(body).error ?? msg;
        } catch {
            /* ignore */
        }
        throw new Error(msg);
    }
    const j = await r.json();
    return j.env;
}

// Per-user env+index allow list. Admins bypass it, so no rows are stored
// for them; this returns the explicit rules only.
export async function listUserAllowed(userId: string): Promise<import("./types").EnvIndexAllow[]> {
    const r = await apiFetch(`/api/admin/users/${encodeURIComponent(userId)}/allowed`);
    if (!r.ok) throw new Error(`user allowed ${r.status}`);
    const j = await r.json();
    return j.allowed ?? [];
}

// Replaces the user's allow list. Server validates each env exists
// and rejects unknown names with a 400.
export async function setUserAllowed(
    userId: string,
    allowed: import("./types").EnvIndexAllow[],
): Promise<void> {
    const r = await apiFetch(`/api/admin/users/${encodeURIComponent(userId)}/allowed`, {
        method: "PUT",
        headers: { "content-type": "application/json" },
        body: JSON.stringify({ allowed }),
    });
    if (!r.ok) {
        const body = await r.text();
        let msg = `set user allowed ${r.status}`;
        try {
            msg = JSON.parse(body).error ?? msg;
        } catch {
            /* ignore */
        }
        throw new Error(msg);
    }
}

export async function deleteEnv(name: string): Promise<void> {
    const r = await apiFetch(`/api/admin/envs/${encodeURIComponent(name)}`, {
        method: "DELETE",
    });
    if (!r.ok) {
        const body = await r.text();
        let msg = `delete env ${r.status}`;
        try {
            msg = JSON.parse(body).error ?? msg;
        } catch {
            /* ignore */
        }
        throw new Error(msg);
    }
}

// Current user, or `null` if not logged in. Short-circuits with no token, and
// uses raw `fetch` so a stale-token 401 doesn't fire the global logout event.
export async function getMe(): Promise<AuthUser | null> {
    const token = getToken();
    if (!token) return null;
    const r = await fetch("/api/auth/me", {
        headers: { Authorization: `Bearer ${token}` },
    });
    if (r.status === 401) {
        clearToken();
        return null;
    }
    if (!r.ok) throw new Error(`me: ${r.status}`);
    const j = await r.json();
    return j.user;
}

// Login with userid-or-email + password. Raw `fetch` so a 401 surfaces as a
// bad-credentials error to the form, not the global logout event.
export async function login(input: { login: string; password: string }): Promise<AuthUser> {
    const r = await fetch("/api/auth/login", {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: JSON.stringify(input),
    });
    if (r.status === 401) throw new Error("invalid credentials");
    if (!r.ok) {
        const body = await r.text();
        throw new Error(`login ${r.status}: ${body}`);
    }
    const j = await r.json();
    setToken(j.token);
    // First login on a fresh browser adopts the admin-configured default env
    // (echoed as `active_env`); returning users keep their stored choice.
    if (localStorage.getItem(ENV_KEY) == null && j.user?.active_env) {
        setEnv(j.user.active_env);
    }
    return j.user;
}

export async function logout(): Promise<void> {
    const token = getToken();
    try {
        // Send the token so the server can revoke it (bump credentials_version);
        // raw `fetch` so the inevitable post-logout 401s elsewhere don't race.
        await fetch("/api/auth/logout", {
            method: "POST",
            headers: token ? { Authorization: `Bearer ${token}` } : undefined,
        });
    } finally {
        clearToken();
    }
}

// First-run probe: true while the instance has no users yet, so the SPA shows the
// setup screen instead of the login form. Also carries the instance theme defaults
// for the pre-login UI. Raw `fetch` (no token exists at this point).
export type SetupStatus = {
    needs_setup: boolean;
    default_appearance?: string;
    default_palette?: string;
    // Read-only demo instance; when set, the UI locks down + pre-fills the login.
    demo_mode?: boolean;
    demo_login?: string | null;
    demo_password?: string | null;
};

export async function getSetupStatus(): Promise<SetupStatus> {
    try {
        const r = await fetch("/api/auth/setup_status");
        if (!r.ok) return { needs_setup: false };
        return await r.json();
    } catch {
        return { needs_setup: false };
    }
}

// Claim a fresh instance: create the first (admin) user and log in. Mirrors `login`'s
// token handling. 409 once anyone exists (someone else claimed it first).
export async function setupAdmin(input: {
    userid: string;
    password: string;
    email?: string;
    display_name?: string;
}): Promise<AuthUser> {
    const r = await fetch("/api/auth/setup", {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: JSON.stringify(input),
    });
    if (!r.ok) {
        const body = await r.text();
        let msg = `setup ${r.status}`;
        try {
            msg = JSON.parse(body).error ?? msg;
        } catch {
            /* ignore */
        }
        throw new Error(msg);
    }
    const j = await r.json();
    setToken(j.token);
    return j.user;
}

// Onboarding: ingest a batch of synthetic logs into the active env. Admin-only.
// Returns how many events landed so the caller can confirm + refresh.
export async function loadSampleData(): Promise<{ ingested: number }> {
    const r = await apiFetch("/api/admin/load_sample", { method: "POST" });
    if (!r.ok) {
        const body = await r.text();
        let msg = `load sample ${r.status}`;
        try {
            msg = JSON.parse(body).error ?? msg;
        } catch {
            /* ignore */
        }
        throw new Error(msg);
    }
    return await r.json();
}

export async function changePassword(input: {
    current_password: string;
    new_password: string;
}): Promise<void> {
    const r = await apiFetch("/api/auth/password", {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: JSON.stringify(input),
    });
    if (!r.ok) {
        const body = await r.text();
        let msg = `change-password ${r.status}`;
        try {
            msg = JSON.parse(body).error ?? msg;
        } catch {
            /* ignore */
        }
        throw new Error(msg);
    }
    // The server revoked all old tokens and minted a fresh one for us so the
    // change doesn't log us out of our own session — store it.
    const j = await r.json();
    if (j?.token) setToken(j.token);
}

// --- admin: users ---

export type UserRecord = {
    id: string;
    userid: string;
    email: string;
    display_name: string;
    is_admin: boolean;
    created_at: string;
};

export async function listUsers(): Promise<UserRecord[]> {
    const r = await apiFetch("/api/admin/users");
    if (!r.ok) throw new Error(`users: ${r.status}`);
    const j = await r.json();
    return j.users ?? [];
}

export async function createUser(input: {
    userid: string;
    email: string;
    display_name: string;
    is_admin?: boolean;
}): Promise<{ user: UserRecord; password: string }> {
    const r = await apiFetch("/api/admin/users", {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: JSON.stringify(input),
    });
    if (!r.ok) {
        const body = await r.text();
        let msg = `create user ${r.status}`;
        try {
            msg = JSON.parse(body).error ?? msg;
        } catch {
            /* ignore */
        }
        throw new Error(msg);
    }
    return r.json();
}

export async function updateUser(
    id: string,
    patch: { email?: string; display_name?: string; is_admin?: boolean },
): Promise<UserRecord> {
    const r = await apiFetch(`/api/admin/users/${encodeURIComponent(id)}`, {
        method: "PATCH",
        headers: { "content-type": "application/json" },
        body: JSON.stringify(patch),
    });
    if (!r.ok) {
        const body = await r.text();
        let msg = `update user ${r.status}`;
        try {
            msg = JSON.parse(body).error ?? msg;
        } catch {
            /* ignore */
        }
        throw new Error(msg);
    }
    const j = await r.json();
    return j.user;
}

export async function deleteUser(id: string): Promise<void> {
    const r = await apiFetch(`/api/admin/users/${encodeURIComponent(id)}`, { method: "DELETE" });
    if (!r.ok) {
        const body = await r.text();
        let msg = `delete user ${r.status}`;
        try {
            msg = JSON.parse(body).error ?? msg;
        } catch {
            /* ignore */
        }
        throw new Error(msg);
    }
}

export async function regenerateUserPassword(id: string): Promise<string> {
    const r = await apiFetch(`/api/admin/users/${encodeURIComponent(id)}/password`, {
        method: "POST",
    });
    if (!r.ok) {
        const body = await r.text();
        let msg = `regenerate ${r.status}`;
        try {
            msg = JSON.parse(body).error ?? msg;
        } catch {
            /* ignore */
        }
        throw new Error(msg);
    }
    const j = await r.json();
    return j.password;
}
