// Copyright 2026 Appbird LLC. Licensed under "GNU Affero General Public License v3.0"

// `/admin/mcp` — controls for the `helioslogs mcp` subcommand: enable toggle, API-key
// auth (managed on the API keys page), index allowlist, per-tool toggles, and
// recent MCP activity.

import { useCallback, useEffect, useState } from "react";
import { Link } from "react-router-dom";
import {
    AlertCircle,
    Bot,
    Check,
    CheckCircle2,
    Copy,
    ExternalLink,
    KeySquare,
    RefreshCw,
} from "lucide-react";
import {
    getCatalogIndexes,
    getSettings,
    listApiKeys,
    listEnvs,
    search,
    setEnv,
    updateSettings,
} from "../../api/client";
import type { EnvIndexAllow, Hit, Settings } from "../../api/types";
import { AllowlistEditor, allowlistUnrestricted } from "../../components/AllowlistEditor";
import { ActionButton, Card, ErrorBanner, Toast } from "../../components/admin";

// MCP tool catalog mirrored from `src/mcp/tools.rs`, hardcoded so the UI doesn't
// depend on the MCP process. New backend tools need an entry here (deliberate gate).
const TOOL_CATALOG: { name: string; description: string }[] = [
    { name: "list_indexes", description: "Enumerate indexes in the catalog." },
    {
        name: "list_environments",
        description: "List the envs (tenancy boundaries) registered in this HeliosLogs instance.",
    },
    {
        name: "discover_fields",
        description: "Sample events and rank JSON keys present (schema-on-read).",
    },
    { name: "query_logs", description: "Search events; supports pipe operators for analytics." },
    { name: "histogram", description: "Event counts over time. Spot incidents / ingest gaps." },
    { name: "aggregate", description: "Top values per field — terms aggregation." },
    { name: "get_stats", description: "Catalog rollup: docs / segments / partitions." },
    { name: "list_partitions", description: "Per-(index, day) doc counts + on-disk size." },
    {
        name: "get_index_info",
        description: "Schema + counts for one partition or the whole catalog.",
    },
    {
        name: "ingest_events",
        description: "Bulk-ingest JSON events. Writes a partition — needs the writer lock.",
    },
    {
        name: "list_dashboards",
        description: "List dashboards visible to MCP (public + MCP-created).",
    },
    {
        name: "get_dashboard",
        description: "Fetch one dashboard's full widget/layout spec.",
    },
    {
        name: "create_dashboard",
        description: "Create a public dashboard. Writes the control plane (MCP is public-only).",
    },
    {
        name: "update_dashboard",
        description: "Edit a public dashboard's name or widgets. Writes the control plane.",
    },
    {
        name: "list_alerts",
        description: "List alerts visible to MCP (public + MCP-created).",
    },
    {
        name: "acknowledge_alert",
        description: "Acknowledge an alert — shared: clears it from every user's inbox.",
    },
    {
        name: "list_monitors",
        description: "List scheduled monitors visible to MCP (public + MCP-created).",
    },
    {
        name: "create_monitor",
        description: "Create a public recurring AI or threshold monitor (MCP is public-only).",
    },
    {
        name: "update_monitor",
        description: "Edit a public monitor's schedule, prompt, threshold, or enabled state.",
    },
];

// `["*"]` (or empty) means "all". Used for the tool allowlist.
function isUnrestricted(list: string[]): boolean {
    return list.length === 0 || list.includes("*");
}

export function McpPanel() {
    const [settings, setSettings] = useState<Settings | null>(null);
    // Env → sorted index names, derived from the catalog's partition list.
    // `_system` is included so admins can opt MCP into self-log access.
    const [catalogByEnv, setCatalogByEnv] = useState<Record<string, string[]>>({});
    const [activity, setActivity] = useState<Hit[] | null>(null);
    const [activityError, setActivityError] = useState<string | null>(null);
    const [busy, setBusy] = useState<boolean>(false);
    const [error, setError] = useState<string | null>(null);
    const [toast, setToast] = useState<string | null>(null);
    // True once any usable MCP-scoped key exists — i.e. the endpoint now requires auth.
    const [authActive, setAuthActive] = useState(false);

    const refresh = useCallback(async () => {
        try {
            // Filter the disk catalog to registered envs so orphan on-disk
            // partitions don't show up as grantable (`_system` included).
            const [s, catalog, envs, keys] = await Promise.all([
                getSettings(),
                getCatalogIndexes(),
                listEnvs(true),
                listApiKeys(),
            ]);
            setSettings(s);
            const now = Date.now();
            setAuthActive(
                keys.some(
                    (k) =>
                        k.enabled && k.scopes.mcp && (k.expires_at == null || k.expires_at > now),
                ),
            );
            const registered = new Set(envs.map((e) => e.name));
            const map: Record<string, Set<string>> = {};
            for (const e of envs) map[e.name] = new Set();
            for (const p of catalog) {
                if (!registered.has(p.env)) continue;
                map[p.env].add(p.index);
            }
            const out: Record<string, string[]> = {};
            for (const [env, names] of Object.entries(map)) {
                out[env] = Array.from(names).sort();
            }
            setCatalogByEnv(out);
            setError(null);
        } catch (e) {
            setError(e instanceof Error ? e.message : String(e));
        }
    }, []);

    const refreshActivity = useCallback(async () => {
        try {
            setActivityError(null);
            const r = await search({
                q: "*",
                env: "_system",
                index: "_heliosmcp",
                start: "-24h",
                end: "now",
                limit: 20,
            });
            setActivity(r.hits);
        } catch (e) {
            setActivityError(e instanceof Error ? e.message : String(e));
        }
    }, []);

    useEffect(() => {
        void refresh();
        void refreshActivity();
    }, [refresh, refreshActivity]);

    const flashToast = (msg: string) => {
        setToast(msg);
        setTimeout(() => setToast(null), 3000);
    };

    const saveSettings = useCallback(async (patch: Partial<Settings>) => {
        setBusy(true);
        setError(null);
        try {
            const next = await updateSettings(patch);
            setSettings(next);
            flashToast("settings saved");
        } catch (e) {
            setError(e instanceof Error ? e.message : String(e));
        } finally {
            setBusy(false);
        }
    }, []);

    const handleToggleEnabled = useCallback(() => {
        if (!settings) return;
        void saveSettings({ mcp_enabled: !settings.mcp_enabled });
    }, [settings, saveSettings]);

    // Persist a wholesale allowlist edit from the shared editor.
    const handleAllowlistChange = useCallback(
        (next: EnvIndexAllow[]) => {
            void saveSettings({ mcp_allowed: next });
        },
        [saveSettings],
    );

    const handleResetAllowlist = useCallback(() => {
        void saveSettings({ mcp_allowed: [] });
    }, [saveSettings]);

    const handleToggleTool = useCallback(
        (name: string) => {
            if (!settings) return;
            const current = isUnrestricted(settings.mcp_enabled_tools)
                ? TOOL_CATALOG.map((t) => t.name)
                : settings.mcp_enabled_tools;
            const next = current.includes(name)
                ? current.filter((t) => t !== name)
                : [...current, name];
            // If the user just re-enabled everything, collapse back to ["*"]
            // so the DB row stays compact and future-tool-aware.
            const allOn =
                next.length === TOOL_CATALOG.length &&
                TOOL_CATALOG.every((t) => next.includes(t.name));
            void saveSettings({ mcp_enabled_tools: allOn ? ["*"] : next });
        },
        [settings, saveSettings],
    );

    const handleEnableAllTools = useCallback(() => {
        void saveSettings({ mcp_enabled_tools: ["*"] });
    }, [saveSettings]);

    if (!settings) {
        return (
            <div className="px-6 py-4">
                <ErrorBanner error={error} />
                {!error && <div className="text-stone-700 dark:text-stone-300">loading…</div>}
            </div>
        );
    }

    const toolsUnrestricted = isUnrestricted(settings.mcp_enabled_tools);
    const enabledToolSet = new Set(
        toolsUnrestricted ? TOOL_CATALOG.map((t) => t.name) : settings.mcp_enabled_tools,
    );
    const indexesUnrestricted = allowlistUnrestricted(settings.mcp_allowed);

    return (
        <div>
            {error && (
                <div className="px-6 py-4">
                    <ErrorBanner error={error} />
                </div>
            )}
            <Toast message={toast} />

            <Card title="MCP server">
                <div className="px-6 py-4 space-y-4 max-w-3xl">
                    <McpHelpFrame />

                    <div className="flex items-center gap-3">
                        <span className="font-semibold text-stone-800 dark:text-stone-100 min-w-[160px]">
                            Enabled
                        </span>
                        <Toggle
                            checked={settings.mcp_enabled}
                            busy={busy}
                            onChange={handleToggleEnabled}
                            labelOn="MCP is on"
                            labelOff="MCP is off — tools/list is empty and all calls error"
                        />
                    </div>

                    {/* Auth is gated by API keys (MCP scope), not a per-server token. */}
                    <div className="pt-3 border-t border-stone-200 dark:border-stone-800 space-y-2">
                        <div className="flex items-center gap-3 flex-wrap">
                            <span className="font-semibold text-stone-800 dark:text-stone-100 min-w-[160px]">
                                Authentication
                            </span>
                            {authActive ? (
                                <span className="inline-flex items-center gap-1.5 px-2 py-0.5 rounded-md bg-green-50 text-green-800 dark:bg-green-950/40 dark:text-green-300">
                                    <KeySquare className="w-3.5 h-3.5" />
                                    Enabled
                                </span>
                            ) : (
                                <>
                                    <span className="text-stone-700 dark:text-stone-300 italic">
                                        Disabled — the endpoint is open
                                    </span>
                                    <Link
                                        to="/admin/api-keys"
                                        className="text-orange-700 dark:text-orange-300 hover:underline inline-flex items-center gap-1"
                                    >
                                        enable via API keys
                                        <ExternalLink className="w-3.5 h-3.5" />
                                    </Link>
                                </>
                            )}
                        </div>
                        <div className="text-stone-700 dark:text-stone-300">
                            Create an API key with the <strong>MCP server</strong> scope on the{" "}
                            <Link
                                to="/admin/api-keys"
                                className="text-orange-700 dark:text-orange-300 hover:underline"
                            >
                                API keys
                            </Link>{" "}
                            page, then pass it as{" "}
                            <code className="font-mono">Authorization: Bearer hlk_…</code> (see the
                            snippets below).
                        </div>
                    </div>
                </div>
            </Card>

            <ClientConfigCard />

            <Card title="Index allowlist">
                <div className="px-6 py-4 space-y-4">
                    <div className="text-stone-700 dark:text-stone-300">
                        Pick which indexes MCP can see — per environment. Toggle{" "}
                        <strong>All indexes</strong> on an env to permit every index there
                        (including ones added later). Internal <code>_*</code> indexes and the{" "}
                        <code>_system</code> env are listed last so opt-in is deliberate.
                    </div>

                    <div className="flex items-center gap-3">
                        <span className="font-semibold text-stone-800 dark:text-stone-100">
                            Status:
                        </span>
                        {indexesUnrestricted ? (
                            <span className="inline-flex items-center gap-1.5 px-2 py-0.5 rounded-md bg-amber-50 text-amber-800 dark:bg-amber-950/40 dark:text-amber-200">
                                unrestricted — every env, every index
                            </span>
                        ) : (
                            <span className="inline-flex items-center gap-1.5 px-2 py-0.5 rounded-md bg-green-50 text-green-800 dark:bg-green-950/40 dark:text-green-300">
                                restricted — {settings.mcp_allowed.length} env rule
                                {settings.mcp_allowed.length === 1 ? "" : "s"}
                            </span>
                        )}
                        {!indexesUnrestricted && (
                            <button
                                type="button"
                                onClick={handleResetAllowlist}
                                disabled={busy}
                                className="px-3 py-1 font-medium text-stone-700 dark:text-stone-300 hover:bg-stone-100 dark:hover:bg-stone-800 rounded-md disabled:opacity-40 transition"
                                title="Drop all rules — MCP sees every env and every index"
                            >
                                Reset to unrestricted
                            </button>
                        )}
                    </div>

                    <AllowlistEditor
                        value={settings.mcp_allowed}
                        onChange={handleAllowlistChange}
                        catalogByEnv={catalogByEnv}
                        disabled={busy}
                    />
                </div>
            </Card>

            <Card title="Tool allowlist">
                <div className="px-6 py-4 space-y-3">
                    <div className="flex items-center justify-between flex-wrap gap-3">
                        <div className="text-stone-700 dark:text-stone-300">
                            Disabled tools are hidden from <code>tools/list</code> and rejected at
                            call time.
                        </div>
                        {!toolsUnrestricted && (
                            <ActionButton onClick={handleEnableAllTools} busy={busy}>
                                Enable all
                            </ActionButton>
                        )}
                    </div>
                    <ul className="divide-y divide-stone-100 dark:divide-stone-800 border border-stone-200 dark:border-stone-800 rounded-md overflow-hidden">
                        {TOOL_CATALOG.map((tool) => {
                            const enabled = enabledToolSet.has(tool.name);
                            return (
                                <li
                                    key={tool.name}
                                    className="flex items-start gap-3 px-3 py-2 bg-white dark:bg-stone-900"
                                >
                                    <input
                                        type="checkbox"
                                        id={`tool-${tool.name}`}
                                        checked={enabled}
                                        disabled={busy}
                                        onChange={() => handleToggleTool(tool.name)}
                                        className="mt-1 h-4 w-4 accent-orange-600"
                                    />
                                    <label
                                        htmlFor={`tool-${tool.name}`}
                                        className="flex-1 cursor-pointer"
                                    >
                                        <div className="font-mono text-stone-800 dark:text-stone-200">
                                            {tool.name}
                                        </div>
                                        <div className="text-stone-700 dark:text-stone-300">
                                            {tool.description}
                                        </div>
                                    </label>
                                </li>
                            );
                        })}
                    </ul>
                </div>
            </Card>

            <Card title="Recent activity">
                <div className="px-6 py-4 space-y-3">
                    <div className="flex items-center justify-between gap-3 flex-wrap">
                        <div className="text-stone-700 dark:text-stone-300">
                            Last 20 MCP tool calls from <code>_heliosmcp</code> (last 24h). Re-reads
                            on demand.
                        </div>
                        <div className="flex items-center gap-2">
                            <ActionButton onClick={() => void refreshActivity()} busy={busy}>
                                <RefreshCw className="w-3.5 h-3.5 inline mr-1.5" />
                                Refresh
                            </ActionButton>
                            <button
                                type="button"
                                onClick={() => {
                                    // `_heliosmcp` lives under the reserved `_system` env, so switch
                                    // to it (setEnv + full load) — mirrors EnvPicker/alertActions.
                                    setEnv("_system");
                                    window.location.assign(
                                        "/search?q=*&index=_heliosmcp&env=_system",
                                    );
                                }}
                                className="px-3 py-1.5 font-medium rounded-md text-stone-700 dark:text-stone-300 hover:bg-stone-100 dark:hover:bg-stone-800 transition inline-flex items-center gap-1.5"
                                title="Open in full search view (switches to the _system env)"
                            >
                                <ExternalLink className="w-3.5 h-3.5" />
                                Open in search
                            </button>
                        </div>
                    </div>

                    {activityError && <ErrorBanner error={activityError} />}

                    {!activityError && activity === null && (
                        <div className="text-stone-700 dark:text-stone-300">loading…</div>
                    )}

                    {!activityError && activity && activity.length === 0 && (
                        <div className="text-stone-700 dark:text-stone-300 italic">
                            No MCP tool calls in the last 24h. Configure a client with the snippet
                            above, run a query through it, and refresh.
                        </div>
                    )}

                    {!activityError && activity && activity.length > 0 && (
                        <ul className="divide-y divide-stone-100 dark:divide-stone-800 border border-stone-200 dark:border-stone-800 rounded-md overflow-hidden">
                            {activity.map((h, i) => (
                                <ActivityRow key={`${h.timestamp}-${i}`} hit={h} />
                            ))}
                        </ul>
                    )}
                </div>
            </Card>
        </div>
    );
}

// One recent-activity row. Parses the event JSON for tool/status/duration/args;
// falls back to the raw message if parsing fails (defensive).
function ActivityRow({ hit }: { hit: Hit }) {
    let tool = "—";
    let status: "ok" | "error" | "unknown" = "unknown";
    let durationMs: number | null = null;
    let args: Record<string, unknown> | null = null;
    let errorText: string | null = null;
    let keyName: string | null = null;
    if (hit.raw) {
        try {
            const obj = JSON.parse(hit.raw) as Record<string, unknown>;
            if (typeof obj.tool === "string") tool = obj.tool;
            if (obj.status === "ok" || obj.status === "error") status = obj.status;
            if (typeof obj.duration_ms === "number") durationMs = obj.duration_ms;
            if (typeof obj.error === "string") errorText = obj.error;
            if (typeof obj.api_key_name === "string") keyName = obj.api_key_name;
            if (obj.arguments && typeof obj.arguments === "object") {
                args = obj.arguments as Record<string, unknown>;
            }
        } catch {
            /* ignore; show as raw */
        }
    }

    const ts = hit.timestamp
        ? new Date(hit.timestamp).toLocaleString(undefined, {
              month: "short",
              day: "numeric",
              hour: "2-digit",
              minute: "2-digit",
              second: "2-digit",
          })
        : "—";

    const argsSummary = args ? summarizeArgs(args) : null;

    return (
        <li className="px-3 py-2 flex items-start gap-3 bg-white dark:bg-stone-900">
            <span className="flex-shrink-0 mt-0.5">
                {status === "ok" ? (
                    <CheckCircle2 className="w-4 h-4 text-green-600 dark:text-green-400" />
                ) : status === "error" ? (
                    <AlertCircle className="w-4 h-4 text-red-600 dark:text-red-400" />
                ) : (
                    <AlertCircle className="w-4 h-4 text-stone-500 dark:text-stone-400" />
                )}
            </span>
            <span className="font-mono text-stone-700 dark:text-stone-300 flex-shrink-0 tabular-nums">
                {ts}
            </span>
            <span className="font-mono font-medium text-stone-800 dark:text-stone-200 flex-shrink-0">
                {tool}
            </span>
            {argsSummary && (
                <span className="font-mono text-stone-700 dark:text-stone-300 truncate min-w-0">
                    {argsSummary}
                </span>
            )}
            <span className="flex-1" />
            {keyName && (
                <span
                    className="inline-flex items-center gap-1 text-xs px-1.5 py-0.5 rounded bg-violet-100 dark:bg-violet-950/50 text-violet-800 dark:text-violet-300 flex-shrink-0"
                    title={`API key: ${keyName}`}
                >
                    <KeySquare className="w-3 h-3" />
                    {keyName}
                </span>
            )}
            {durationMs !== null && (
                <span className="text-stone-700 dark:text-stone-300 tabular-nums flex-shrink-0">
                    {durationMs < 1
                        ? `${(durationMs * 1000).toFixed(0)}µs`
                        : durationMs < 1000
                          ? `${durationMs.toFixed(1)}ms`
                          : `${(durationMs / 1000).toFixed(1)}s`}
                </span>
            )}
            {errorText && (
                <span
                    className="text-red-600 dark:text-red-400 truncate max-w-[40%] flex-shrink-0"
                    title={errorText}
                >
                    {errorText}
                </span>
            )}
        </li>
    );
}

// Render tool args as a compact "k=v, k=v" string; long values truncated,
// nested objects summarized by shape.
function summarizeArgs(args: Record<string, unknown>): string {
    const parts: string[] = [];
    for (const [k, v] of Object.entries(args)) {
        let s: string;
        if (typeof v === "string") {
            s = v.length > 40 ? `${v.slice(0, 37)}…` : v;
        } else if (typeof v === "number" || typeof v === "boolean") {
            s = String(v);
        } else if (Array.isArray(v)) {
            s = `[${v.length}]`;
        } else if (v === null) {
            s = "null";
        } else {
            s = "{…}";
        }
        parts.push(`${k}=${s}`);
    }
    return parts.join(", ");
}

// ============================================================================
// Client configuration card
// ============================================================================

// Renders ready-to-paste config snippets for popular MCP clients. The Bearer
// header carries an API key with the MCP scope; we splice a placeholder the user
// replaces with their own `hlk_…` key from the API keys page.
function ClientConfigCard() {
    // `window.location` is safe at render time — this component is browser-only.
    const defaultUrl = `${window.location.origin}/mcp`;
    const [url, setUrl] = useState<string>(defaultUrl);
    const tokenForSnippet = "hlk_<your-api-key>";

    const snippets = {
        mcpJson: mcpJsonSnippet(url, tokenForSnippet),
        curl: curlSnippet(url, tokenForSnippet),
    };

    return (
        <Card title="Client configuration">
            <div className="px-6 py-4 space-y-5 max-w-3xl">
                <div className="text-stone-700 dark:text-stone-300">
                    Drop the snippet into your client's{" "}
                    <code className="font-mono">mcpServers</code> config. The URL is auto-detected
                    from this page; edit it if HeliosLogs is reachable at a different address from
                    the client (e.g. in <code className="font-mono">npm run dev</code> the SPA sits
                    behind Vite while HeliosLogs listens on <code className="font-mono">:7300</code>
                    ).
                </div>

                <div className="grid grid-cols-[10rem_1fr] gap-x-4 gap-y-1 items-start">
                    <label
                        htmlFor="mcp-endpoint-url"
                        className="pt-1.5 font-semibold text-stone-800 dark:text-stone-100"
                    >
                        Endpoint URL
                    </label>
                    <input
                        id="mcp-endpoint-url"
                        type="text"
                        value={url}
                        onChange={(e) => setUrl(e.target.value)}
                        className="w-full px-2.5 py-1.5 font-mono bg-white dark:bg-stone-950 border border-stone-200 dark:border-stone-700 rounded-md focus:outline-none focus:border-orange-500 focus:ring-1 focus:ring-orange-500 text-stone-900 dark:text-stone-100"
                    />
                </div>

                <div className="px-3 py-2 rounded-md bg-amber-50 dark:bg-amber-950/30 border border-amber-200 dark:border-amber-900 text-amber-900 dark:text-amber-200">
                    Replace <code className="font-mono">hlk_&lt;your-api-key&gt;</code> with a key
                    that has the <strong>MCP server</strong> scope from the{" "}
                    <Link to="/admin/api-keys" className="underline">
                        API keys
                    </Link>{" "}
                    page. The secret is shown only once, at creation.
                </div>

                <Snippet
                    title="mcpServers"
                    paths={[
                        "Drop into your MCP client's config file (e.g. .mcp.json, ~/.cursor/mcp.json).",
                    ]}
                    language="json"
                    body={snippets.mcpJson}
                />

                <Snippet
                    title="curl (smoke test)"
                    paths={["Verify the endpoint responds before configuring a client."]}
                    language="sh"
                    body={snippets.curl}
                />

                <div className="text-stone-700 dark:text-stone-300">
                    The server name (<code className="font-mono">helioslogs</code>) is arbitrary —
                    pick anything; it shows up in the client as the MCP server identifier. Already
                    have an <code className="font-mono">mcpServers</code> object in your file? Merge
                    this entry rather than replacing the whole file.
                </div>
            </div>
        </Card>
    );
}

function Snippet({
    title,
    paths,
    language,
    body,
    note,
}: {
    title: string;
    paths: string[];
    language: "json" | "sh";
    body: string;
    note?: React.ReactNode;
}) {
    const [copied, setCopied] = useState(false);
    const copy = useCallback(async () => {
        try {
            await navigator.clipboard.writeText(body);
            setCopied(true);
            setTimeout(() => setCopied(false), 2000);
        } catch {
            /* clipboard unavailable — fall back to manual select */
        }
    }, [body]);

    return (
        <div className="space-y-1.5">
            <div className="flex items-start justify-between gap-3 flex-wrap">
                <div className="min-w-0">
                    <div className="font-medium text-stone-800 dark:text-stone-200">{title}</div>
                    <ul className="text-stone-700 dark:text-stone-300 font-mono break-all">
                        {paths.map((p, i) => (
                            <li key={i}>{p}</li>
                        ))}
                    </ul>
                </div>
                <button
                    type="button"
                    onClick={copy}
                    className="px-2.5 py-1.5 rounded-md bg-stone-900 hover:bg-stone-800 dark:bg-stone-800 dark:hover:bg-stone-700 text-white transition inline-flex items-center gap-1.5 flex-shrink-0"
                    title="Copy to clipboard"
                >
                    {copied ? (
                        <>
                            <Check className="w-3.5 h-3.5" />
                            copied
                        </>
                    ) : (
                        <>
                            <Copy className="w-3.5 h-3.5" />
                            copy
                        </>
                    )}
                </button>
            </div>
            <pre className="font-mono whitespace-pre-wrap break-all px-3 py-2 bg-stone-50 dark:bg-stone-950 border border-stone-200 dark:border-stone-800 rounded-md overflow-auto max-h-72">
                <code className={`language-${language}`}>{body}</code>
            </pre>
            {note && <div className="text-stone-700 dark:text-stone-300">{note}</div>}
        </div>
    );
}

// --- snippet builders ---
// Each helper takes the URL + optional token; a `null` token omits the headers
// block so a copy-paste onto an open server has no dangling fake Authorization.

// Generic HTTP MCP transport — works with every modern MCP client that
// supports HTTP (`"type": "http"` + URL + optional headers).
function mcpJsonSnippet(url: string, token: string | null): string {
    const cfg: Record<string, unknown> = {
        type: "http",
        url,
    };
    if (token !== null) {
        cfg.headers = { Authorization: `Bearer ${token}` };
    }
    return JSON.stringify({ mcpServers: { helioslogs: cfg } }, null, 2);
}

// curl one-liner. Calls `tools/list` because it works in every state and needs
// no arguments, making it a meaningful liveness check.
function curlSnippet(url: string, token: string | null): string {
    const lines: string[] = [`curl -X POST ${url} \\`, `  -H 'content-type: application/json' \\`];
    if (token !== null) {
        lines.push(`  -H 'Authorization: Bearer ${token}' \\`);
    }
    lines.push(`  -d '{"jsonrpc":"2.0","id":1,"method":"tools/list"}'`);
    return lines.join("\n");
}

// Inline help banner; same visual treatment as the LLM panel's help frame.
function McpHelpFrame() {
    return (
        <div className="flex gap-3 p-4 rounded-lg bg-orange-50/60 dark:bg-orange-950/20 border border-orange-200/70 dark:border-orange-900/40">
            <div className="flex-shrink-0 mt-0.5">
                <Bot className="w-4 h-4 text-orange-600 dark:text-orange-400" />
            </div>
            <div className="space-y-1.5 text-stone-700 dark:text-stone-200 leading-relaxed">
                <p>
                    HeliosLogs exposes its search surface as an <strong>MCP server</strong> at{" "}
                    <code className="font-mono">/mcp</code> so external agents can run{" "}
                    <code className="font-mono">query_logs</code>,{" "}
                    <code className="font-mono">aggregate</code>,{" "}
                    <code className="font-mono">histogram</code>,{" "}
                    <code className="font-mono">discover_fields</code>, and the rest of the tool
                    catalog over HTTP.
                </p>
                <p className="text-stone-700 dark:text-stone-300">
                    Off by default — opt in below. Settings here take effect on the next tool call,
                    no restart needed. Pair the master switch with an MCP-scoped API key and an
                    index allowlist before exposing this beyond localhost.
                </p>
            </div>
        </div>
    );
}

function Toggle({
    checked,
    busy,
    onChange,
    labelOn,
    labelOff,
}: {
    checked: boolean;
    busy: boolean;
    onChange: () => void;
    labelOn: string;
    labelOff: string;
}) {
    return (
        <div className="flex items-center gap-3">
            <button
                type="button"
                role="switch"
                aria-checked={checked}
                onClick={onChange}
                disabled={busy}
                className={`relative inline-flex h-6 w-11 items-center rounded-full transition disabled:opacity-50 ${
                    checked ? "bg-orange-600" : "bg-stone-300 dark:bg-stone-700"
                }`}
            >
                <span
                    className={`inline-block h-5 w-5 transform rounded-full bg-white transition ${
                        checked ? "translate-x-5" : "translate-x-0.5"
                    }`}
                />
            </button>
            <span className="text-stone-700 dark:text-stone-300">
                {checked ? labelOn : labelOff}
            </span>
        </div>
    );
}
