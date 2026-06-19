// Copyright 2026 Appbird LLC. Licensed under "GNU Affero General Public License v3.0"

import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { Bell, Pause, Pencil, Play, Plus, Sparkles, Trash2, Zap } from "lucide-react";
import { Link, useSearchParams } from "react-router-dom";
import { createMonitor, deleteMonitor, runMonitorNow, updateMonitor } from "../api/client";
import { notifyMonitorsChanged, notifyOpenConversation, notifyRunMonitorLive } from "../api/events";
import type { Monitor, MonitorInput } from "../api/types";
import { timeAgo } from "../lib/format";
import { SortableTh, sortRows, useSort } from "../lib/sort";
import { EnvBadge } from "./EnvBadge";
import { VisibilityBadge } from "./VisibilityBadge";
import { AdminAllToggle } from "./AdminAllToggle";
import { useAlerts } from "../state/useAlerts";
import { useAgentEnabled } from "../state/useAgentEnabled";
import { useAuth } from "../state/useAuth";
import { useMonitors } from "../state/useMonitors";
import { MonitorDialog } from "./MonitorDialog";

// Monitors tab body in `/alerts`: list + filter + per-row actions. "Run now"
// just touches the API (the run is async); the Alerts column links to raised alerts.
export function MonitorsPanel() {
    const isAdmin = !!useAuth().user?.is_admin;
    // AI monitors can't run while the LLM provider is off; reflect that in the UI.
    const { enabled: agentEnabledRaw } = useAgentEnabled();
    const agentEnabled = agentEnabledRaw !== false;
    const [viewAll, setViewAll] = useState(false);
    // Owner column is always shown; View All (admin) only widens the data scope.
    const adminAll = viewAll && isAdmin;
    const { items, error } = useMonitors(adminAll);
    // All alerts (acked + unacked), grouped by monitor for the per-row count.
    const { items: alerts } = useAlerts(false);
    const alertCounts = useMemo(() => {
        const m = new Map<string, { total: number; unacked: number }>();
        for (const a of alerts) {
            const e = m.get(a.monitor_id) ?? { total: 0, unacked: 0 };
            e.total += 1;
            if (!a.acknowledged) e.unacked += 1;
            m.set(a.monitor_id, e);
        }
        return m;
    }, [alerts]);
    const [filter, setFilter] = useState("");
    const [editingId, setEditingId] = useState<string | null>(null);
    const [creating, setCreating] = useState(false);
    // Optional `?monitor=<id>` from an alert's "View monitor" link —
    // highlight that row and scroll it into view.
    const [params] = useSearchParams();
    const highlightId = params.get("monitor");
    const highlightRef = useRef<HTMLTableRowElement | null>(null);
    useEffect(() => {
        highlightRef.current?.scrollIntoView({ block: "center", behavior: "smooth" });
    }, [highlightId, items.length]);

    const { sort, toggle } = useSort();

    const filtered = useMemo(() => {
        const f = filter.trim().toLowerCase();
        const matched = items.filter((m) => {
            if (!f) return true;
            return (
                m.name.toLowerCase().includes(f) ||
                m.description.toLowerCase().includes(f) ||
                m.prompt.toLowerCase().includes(f)
            );
        });
        return sortRows(matched, sort, {
            status: (m) => (!m.enabled ? "paused" : m.running ? "running" : "active"),
            name: (m) => m.name,
            owner: (m) => m.owner ?? "",
            env: (m) => m.env,
            interval: (m) => m.interval_seconds,
            lastrun: (m) => m.last_run_at,
            alerts: (m) => alertCounts.get(m.id)?.total ?? 0,
        });
    }, [items, filter, sort, alertCounts]);

    const handleSaveNew = useCallback(async (input: MonitorInput) => {
        await createMonitor(input);
        notifyMonitorsChanged();
        setCreating(false);
    }, []);

    const handleSaveEdit = useCallback(async (id: string, input: MonitorInput) => {
        await updateMonitor(id, input);
        notifyMonitorsChanged();
        setEditingId(null);
    }, []);

    const handleDelete = async (m: Monitor) => {
        if (!window.confirm(`Delete monitor "${m.name}"?`)) return;
        try {
            await deleteMonitor(m.id);
            notifyMonitorsChanged();
        } catch (e: unknown) {
            window.alert(e instanceof Error ? e.message : String(e));
        }
    };

    const handleRunNow = async (m: Monitor) => {
        // AI monitors run immediately and stream the live trace into the panel;
        // threshold monitors have no agent trace, so they run on the next tick.
        if (m.kind === "ai") {
            notifyRunMonitorLive({ monitorId: m.id, name: m.name });
            return;
        }
        try {
            await runMonitorNow(m.id);
            notifyMonitorsChanged();
        } catch (e: unknown) {
            window.alert(e instanceof Error ? e.message : String(e));
        }
    };

    const handleToggleEnabled = async (m: Monitor) => {
        try {
            await updateMonitor(m.id, { enabled: !m.enabled });
            notifyMonitorsChanged();
        } catch (e: unknown) {
            window.alert(e instanceof Error ? e.message : String(e));
        }
    };

    const editing = editingId ? (items.find((m) => m.id === editingId) ?? null) : null;

    return (
        <div className="px-6 py-8">
            <header className="mb-6">
                <h1 className="font-semibold text-stone-900 dark:text-stone-100">Monitors</h1>
            </header>

            <MonitorsHelpFrame />

            <div className="flex items-center justify-between mb-4 gap-3">
                <input
                    type="text"
                    className="flex-grow max-w-md px-3 py-1.5 bg-white dark:bg-stone-900 border border-stone-200 dark:border-stone-700 rounded-md focus:outline-none focus:border-orange-500"
                    placeholder="filter by name, description, or prompt…"
                    value={filter}
                    onChange={(e) => setFilter(e.target.value)}
                />
                <div className="flex items-center gap-3">
                    {isAdmin && (
                        <AdminAllToggle checked={viewAll} onChange={setViewAll} noun="monitors" />
                    )}
                    <button
                        type="button"
                        onClick={() => setCreating(true)}
                        className="inline-flex items-center gap-1.5 px-3 py-1.5 font-medium rounded-md bg-orange-600 hover:bg-orange-500 text-white transition"
                    >
                        <Plus className="w-3.5 h-3.5" />
                        New monitor
                    </button>
                </div>
            </div>

            {error && (
                <div className="mb-4 px-3 py-2 rounded-md bg-red-50 text-red-800 border border-red-200 dark:bg-red-950 dark:text-red-200 dark:border-red-900">
                    {error}
                </div>
            )}

            <div className="rounded-xl border border-stone-200 dark:border-stone-800 bg-white dark:bg-stone-900 overflow-hidden">
                <table className="w-full">
                    <thead className="bg-stone-50 dark:bg-stone-900/50">
                        <tr>
                            <SortableTh sortKey="status" sort={sort} onSort={toggle}>
                                Status
                            </SortableTh>
                            <SortableTh sortKey="name" sort={sort} onSort={toggle}>
                                Name
                            </SortableTh>
                            <SortableTh sortKey="owner" sort={sort} onSort={toggle}>
                                Owner
                            </SortableTh>
                            <SortableTh sortKey="env" sort={sort} onSort={toggle}>
                                Env
                            </SortableTh>
                            <SortableTh sortKey="interval" sort={sort} onSort={toggle}>
                                Interval
                            </SortableTh>
                            <SortableTh sortKey="lastrun" sort={sort} onSort={toggle}>
                                Last run
                            </SortableTh>
                            <SortableTh sortKey="alerts" sort={sort} onSort={toggle}>
                                Alerts
                            </SortableTh>
                            <th className="border-b border-stone-200 dark:border-stone-800" />
                        </tr>
                    </thead>
                    <tbody className="divide-y divide-stone-100 dark:divide-stone-800">
                        {filtered.length === 0 && (
                            <tr>
                                <td
                                    colSpan={8}
                                    className="px-3 py-8 text-center text-stone-400 dark:text-stone-500"
                                >
                                    {filter
                                        ? "no matches"
                                        : 'no monitors yet — click "New monitor" or ask the Investigate panel to create one'}
                                </td>
                            </tr>
                        )}
                        {filtered.map((m) => (
                            <tr
                                key={m.id}
                                ref={m.id === highlightId ? highlightRef : undefined}
                                className={`group transition-colors ${
                                    m.id === highlightId
                                        ? "bg-orange-50/60 dark:bg-orange-950/30 ring-1 ring-inset ring-orange-300/60 dark:ring-orange-700/50"
                                        : ""
                                }`}
                            >
                                <td className="px-3 py-2 align-top">
                                    <StatusBadge monitor={m} agentEnabled={agentEnabled} />
                                </td>
                                <td className="px-3 py-2 align-top">
                                    <div className="flex flex-col gap-0.5">
                                        <div className="flex items-center gap-2">
                                            <strong className="text-stone-900 dark:text-stone-100">
                                                {m.name}
                                            </strong>
                                            <KindBadge kind={m.kind} />
                                            <VisibilityBadge isPublic={m.public} />
                                        </div>
                                        <span className="text-stone-500 dark:text-stone-400">
                                            {m.description || describeMonitor(m)}
                                        </span>
                                    </div>
                                </td>
                                <td className="px-3 py-2 align-top text-stone-600 dark:text-stone-300">
                                    {m.owner ?? "—"}
                                </td>
                                <td className="px-3 py-2 align-top">
                                    <EnvBadge env={m.env} />
                                </td>
                                <td className="px-3 py-2 align-top text-stone-700 dark:text-stone-300 tabular-nums">
                                    {formatInterval(m.interval_seconds)}
                                </td>
                                <td className="px-3 py-2 align-top text-stone-500 dark:text-stone-400">
                                    {m.last_run_at ? (
                                        <span
                                            title={
                                                m.last_status === "error"
                                                    ? `error: ${m.last_error ?? "unknown"}`
                                                    : new Date(m.last_run_at).toLocaleString()
                                            }
                                        >
                                            {timeAgo(new Date(m.last_run_at).toISOString())}
                                            {m.last_status === "error" && (
                                                <span className="ml-1.5 px-1.5 py-0.5 rounded bg-red-50 text-red-700 dark:bg-red-950/40 dark:text-red-300">
                                                    error
                                                </span>
                                            )}
                                        </span>
                                    ) : (
                                        <span className="text-stone-400">never</span>
                                    )}
                                </td>
                                <td className="px-3 py-2 align-top">
                                    <AlertsCell counts={alertCounts.get(m.id)} monitorId={m.id} />
                                </td>
                                <td className="px-3 py-2 align-top whitespace-nowrap text-right">
                                    <div className="inline-flex items-center gap-1">
                                        {m.last_conversation_id && (
                                            <IconButton
                                                onClick={() =>
                                                    notifyOpenConversation(m.last_conversation_id!)
                                                }
                                                title="View latest run — open the agent's investigation trace in the Investigate panel"
                                                aria-label={`view latest run of ${m.name}`}
                                            >
                                                <Sparkles className="w-3 h-3" />
                                            </IconButton>
                                        )}
                                        <IconButton
                                            onClick={() => handleRunNow(m)}
                                            disabled={m.kind === "ai" && !agentEnabled}
                                            title={
                                                m.kind === "ai"
                                                    ? agentEnabled
                                                        ? "Run & watch — runs now and streams the live trace in the Investigate panel"
                                                        : "Disabled — enable an LLM provider in admin settings"
                                                    : "Run now (next tick)"
                                            }
                                            aria-label={`run ${m.name}`}
                                        >
                                            <Zap className="w-3 h-3" />
                                        </IconButton>
                                        <IconButton
                                            onClick={() => handleToggleEnabled(m)}
                                            title={m.enabled ? "Pause" : "Resume"}
                                            aria-label={`${m.enabled ? "pause" : "resume"} ${m.name}`}
                                        >
                                            {m.enabled ? (
                                                <Pause className="w-3 h-3" />
                                            ) : (
                                                <Play className="w-3 h-3" fill="currentColor" />
                                            )}
                                        </IconButton>
                                        <IconButton
                                            onClick={() => setEditingId(m.id)}
                                            title="Edit"
                                            aria-label={`edit ${m.name}`}
                                        >
                                            <Pencil className="w-3 h-3" />
                                        </IconButton>
                                        <IconButton
                                            onClick={() => handleDelete(m)}
                                            title="Delete"
                                            aria-label={`delete ${m.name}`}
                                            danger
                                        >
                                            <Trash2 className="w-3.5 h-3.5" />
                                        </IconButton>
                                    </div>
                                </td>
                            </tr>
                        ))}
                    </tbody>
                </table>
            </div>

            {creating && (
                <MonitorDialog
                    monitor={null}
                    onSave={handleSaveNew}
                    onClose={() => setCreating(false)}
                />
            )}
            {editing && (
                <MonitorDialog
                    monitor={editing}
                    onSave={(input) => handleSaveEdit(editing.id, input)}
                    onClose={() => setEditingId(null)}
                />
            )}
        </div>
    );
}

// Inline help banner — same orange-tinted treatment as the Admin
// panels. One-sentence orienting blurb; the table covers the rest.
function MonitorsHelpFrame() {
    return (
        <div className="mb-6 flex gap-3 p-4 rounded-lg bg-orange-50/60 dark:bg-orange-950/20 border border-orange-200/70 dark:border-orange-900/40">
            <div className="flex-shrink-0 mt-0.5">
                <Bell className="w-4 h-4 text-orange-600 dark:text-orange-400" />
            </div>
            <p className="text-stone-700 dark:text-stone-200 leading-relaxed">
                Scheduled checks that raise alerts. <strong>Threshold</strong> monitors count search
                results over a window and fire when they cross a number; <strong>AI</strong>{" "}
                monitors run an agent prompt on a cadence and decide whether to alert. Findings land
                in the <strong>Inbox</strong>.
            </p>
        </div>
    );
}

// Per-monitor alert tally + link to its filtered alert list. Shows the
// total raised, with an orange "N new" pill when some are still unacked.
function AlertsCell({
    counts,
    monitorId,
}: {
    counts: { total: number; unacked: number } | undefined;
    monitorId: string;
}) {
    if (!counts || counts.total === 0) {
        return <span className="text-stone-400">—</span>;
    }
    return (
        <Link
            to={`/alerts/history?monitor=${encodeURIComponent(monitorId)}`}
            className="inline-flex items-center gap-1.5 text-stone-700 dark:text-stone-300 hover:text-orange-600 dark:hover:text-orange-400"
            title="View this monitor's alerts"
        >
            <span className="tabular-nums">{counts.total}</span>
            {counts.unacked > 0 && (
                <span className="inline-flex items-center justify-center px-1.5 h-5 rounded-full bg-orange-500 text-white text-xs font-semibold">
                    {counts.unacked} new
                </span>
            )}
        </Link>
    );
}

// Small tag distinguishing the two monitor kinds in the list.
function KindBadge({ kind }: { kind: Monitor["kind"] }) {
    if (kind === "threshold") {
        return (
            <span className="inline-flex items-center px-1.5 py-0.5 rounded border uppercase tracking-wider border-sky-200 bg-sky-50 text-sky-700 dark:border-sky-900 dark:bg-sky-950/40 dark:text-sky-300">
                threshold
            </span>
        );
    }
    return (
        <span className="inline-flex items-center px-1.5 py-0.5 rounded border uppercase tracking-wider border-violet-200 bg-violet-50 text-violet-700 dark:border-violet-900 dark:bg-violet-950/40 dark:text-violet-300">
            AI
        </span>
    );
}

// Fallback name-cell line when a monitor has no description: render the threshold condition.
function describeMonitor(m: Monitor): string {
    if (m.kind !== "threshold" || !m.threshold) return "";
    const t = m.threshold;
    const op =
        { gt: ">", gte: "≥", lt: "<", lte: "≤", eq: "=", neq: "≠" }[t.comparison] ?? t.comparison;
    const where = t.index ? ` in ${t.index}` : "";
    return `count(${t.query || "*"})${where} ${op} ${t.threshold} over ${formatInterval(t.window_seconds)}`;
}

function StatusBadge({ monitor, agentEnabled }: { monitor: Monitor; agentEnabled: boolean }) {
    if (monitor.kind === "ai" && !agentEnabled) {
        return (
            <span
                title="AI agent functionality is disabled — enable an LLM provider in admin settings"
                className="inline-flex items-center px-1.5 py-0.5 rounded border uppercase tracking-wider border-amber-200 bg-amber-50 text-amber-700 dark:border-amber-900 dark:bg-amber-950/40 dark:text-amber-300"
            >
                disabled
            </span>
        );
    }
    if (!monitor.enabled) {
        return (
            <span className="inline-flex items-center px-1.5 py-0.5 rounded border uppercase tracking-wider border-stone-200 bg-stone-50 text-stone-600 dark:border-stone-700 dark:bg-stone-800/60 dark:text-stone-400">
                paused
            </span>
        );
    }
    if (monitor.running) {
        return (
            <span className="inline-flex items-center px-1.5 py-0.5 rounded border uppercase tracking-wider border-blue-200 bg-blue-50 text-blue-700 dark:border-blue-900 dark:bg-blue-950/40 dark:text-blue-300">
                running
            </span>
        );
    }
    return (
        <span className="inline-flex items-center px-1.5 py-0.5 rounded border uppercase tracking-wider border-emerald-200 bg-emerald-50 text-emerald-700 dark:border-emerald-900 dark:bg-emerald-950/40 dark:text-emerald-300">
            active
        </span>
    );
}

function IconButton({
    children,
    onClick,
    title,
    danger,
    disabled,
    ...rest
}: {
    children: React.ReactNode;
    onClick: () => void;
    title: string;
    danger?: boolean;
    disabled?: boolean;
    "aria-label"?: string;
}) {
    return (
        <button
            type="button"
            title={title}
            onClick={onClick}
            disabled={disabled}
            className={`p-1 rounded disabled:opacity-40 disabled:cursor-not-allowed ${
                danger
                    ? "text-stone-900 dark:text-stone-100 hover:text-red-700 dark:hover:text-red-300 hover:bg-red-50 dark:hover:bg-red-950/30"
                    : "text-stone-900 dark:text-stone-100 hover:bg-stone-100 dark:hover:bg-stone-800"
            }`}
            {...rest}
        >
            {children}
        </button>
    );
}

function formatInterval(seconds: number): string {
    if (seconds < 3600) return `${Math.round(seconds / 60)}m`;
    if (seconds < 86400) {
        const h = seconds / 3600;
        return Number.isInteger(h) ? `${h}h` : `${h.toFixed(1)}h`;
    }
    const d = seconds / 86400;
    return Number.isInteger(d) ? `${d}d` : `${d.toFixed(1)}d`;
}
