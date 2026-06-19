// Copyright 2026 Appbird LLC. Licensed under "GNU Affero General Public License v3.0"

import { useEffect, useState } from "react";
import { Bell, ChevronDown, History as HistoryIcon, Inbox, Search, X } from "lucide-react";
import { Link, NavLink, Outlet, useSearchParams } from "react-router-dom";
import type { Alert, MonitorKind } from "../api/types";
import { EnvBadge } from "../components/EnvBadge";
import {
    AlertActions,
    AlertDetailBody,
    SeverityBadge,
    SeverityIcon,
    stripMarkdown,
} from "../components/AlertDetail";
import { timeAgo } from "../lib/format";
import { formatTsForRow } from "../lib/timezone";
import { useAlerts } from "../state/useAlerts";
import { useMonitors } from "../state/useMonitors";
import { useTimezone } from "../state/timezone";

// `/alerts` shell — left rail + outlet. Nested routes own content:
// `/inbox` = unacked findings (default), `/history` = everything (API-capped at 500).
export function AlertsPage() {
    return (
        <div className="flex">
            <aside className="w-60 flex-shrink-0 border-r border-stone-200 dark:border-stone-800 bg-white dark:bg-stone-900 sticky top-0 self-start">
                <div className="px-4 pt-4 pb-2 font-semibold uppercase tracking-wider text-stone-700 dark:text-stone-300">
                    Alerts
                </div>
                <nav className="px-2 pb-3 space-y-0.5">
                    <SideNavLink to="inbox" icon={<Inbox className="w-4 h-4" />}>
                        Inbox
                    </SideNavLink>
                    <SideNavLink to="history" icon={<HistoryIcon className="w-4 h-4" />}>
                        History
                    </SideNavLink>
                    <SideNavLink to="monitors" icon={<Bell className="w-4 h-4" />}>
                        Monitors
                    </SideNavLink>
                </nav>
            </aside>
            <main className="flex-1 min-w-0 bg-white dark:bg-stone-900">
                <Outlet />
            </main>
        </div>
    );
}

function SideNavLink({
    to,
    icon,
    children,
}: {
    to: string;
    icon: React.ReactNode;
    children: React.ReactNode;
}) {
    const base = "flex items-center gap-2 px-3 py-2 rounded-md transition";
    return (
        <NavLink
            to={to}
            end={false}
            className={({ isActive }) =>
                isActive
                    ? `${base} bg-orange-50 text-orange-900 dark:bg-orange-950/40 dark:text-orange-100 font-medium`
                    : `${base} text-stone-800 dark:text-stone-200 hover:bg-stone-100 dark:hover:bg-stone-800`
            }
        >
            {icon}
            {children}
        </NavLink>
    );
}

// `/alerts/inbox` — unacknowledged findings. The default landing for
// the nav badge click.
export function AlertsInboxPanel() {
    return <AlertsList unackedOnly title="Inbox" />;
}

// `/alerts/history` — every alert, acked or not.
export function AlertsHistoryPanel() {
    return <AlertsList unackedOnly={false} title="History" />;
}

function AlertsList({ unackedOnly, title }: { unackedOnly: boolean; title: string }) {
    const [params, setParams] = useSearchParams();
    // Optional `?monitor=<id>` filter — set when arriving from a monitor's
    // "Alerts" link or an alert's monitor name. Applied server-side.
    const monitorId = params.get("monitor");

    // The search box is immediate; a debounced copy drives the request. Search,
    // monitor, and the recent-row cap are all enforced server-side.
    const [query, setQuery] = useState("");
    const [debounced, setDebounced] = useState("");
    useEffect(() => {
        const h = setTimeout(() => setDebounced(query.trim()), 250);
        return () => clearTimeout(h);
    }, [query]);

    const { items, error, limit } = useAlerts(unackedOnly, debounced, monitorId);
    const { items: monitors } = useMonitors();
    const activeMonitors = monitors.filter((m) => m.enabled).length;
    const monitorName = monitorId ? (monitors.find((m) => m.id === monitorId)?.name ?? null) : null;

    // Kind per alert via its monitor; fall back to the conversation trace for
    // alerts whose monitor was since deleted (threshold runs never create one).
    const kindById = new Map(monitors.map((m) => [m.id, m.kind]));
    const kindOf = (a: Alert): MonitorKind =>
        kindById.get(a.monitor_id) ?? (a.conversation_id ? "ai" : "threshold");

    const searching = query.trim().length > 0;
    const atLimit = items.length >= limit;

    return (
        <div className="px-6 py-8">
            <header className="mb-6">
                <h1 className="font-semibold text-stone-900 dark:text-stone-100">{title}</h1>
            </header>

            {monitorId ? (
                <div className="mb-4 flex items-center justify-between gap-3 px-4 py-3 rounded-lg bg-orange-50/60 dark:bg-orange-950/20 border border-orange-200/70 dark:border-orange-900/40">
                    <span className="text-stone-700 dark:text-stone-200">
                        Showing {unackedOnly ? "inbox" : "history"} alerts from{" "}
                        <strong>{monitorName ?? "this monitor"}</strong>
                    </span>
                    <button
                        type="button"
                        onClick={() => setParams({}, { replace: true })}
                        className="inline-flex items-center gap-1 px-2 py-1 rounded-md text-stone-700 dark:text-stone-200 hover:bg-orange-100/60 dark:hover:bg-orange-900/30"
                    >
                        <X className="w-3 h-3" />
                        Clear filter
                    </button>
                </div>
            ) : (
                <AlertsHelpFrame unackedOnly={unackedOnly} />
            )}

            {/* Monitor stat strip + manage entrypoint, right above the list. Labelled
          as monitors so the counts aren't mistaken for alert counts. */}
            <div className="mb-4 flex items-center justify-between gap-3 flex-wrap">
                {monitors.length === 0 ? (
                    <span className="text-stone-600 dark:text-stone-300">
                        No monitors yet — create one to start watching your logs.
                    </span>
                ) : (
                    <span className="inline-flex items-center gap-1.5 text-stone-600 dark:text-stone-300">
                        <span
                            className="w-1.5 h-1.5 rounded-full bg-emerald-500"
                            aria-hidden="true"
                        />
                        <strong className="font-semibold tabular-nums text-stone-900 dark:text-stone-100">
                            {activeMonitors}
                        </strong>
                        active {activeMonitors === 1 ? "monitor" : "monitors"}
                    </span>
                )}
                <Link
                    to="/alerts/monitors"
                    className="inline-flex items-center gap-1.5 px-3 py-1.5 font-medium rounded-md bg-orange-600 hover:bg-orange-500 text-white transition"
                    title="Create and manage the monitors that raise these alerts"
                >
                    <Bell className="w-4 h-4" />
                    Manage monitors
                </Link>
            </div>

            {error && (
                <div className="mb-4 px-3 py-2 rounded-md bg-red-50 text-red-800 border border-red-200 dark:bg-red-950 dark:text-red-200 dark:border-red-900">
                    {error}
                </div>
            )}

            {(items.length > 0 || searching) && (
                <div className="relative mb-3">
                    <Search className="pointer-events-none absolute left-2.5 top-1/2 -translate-y-1/2 w-4 h-4 text-stone-400 dark:text-stone-500" />
                    <input
                        type="text"
                        value={query}
                        onChange={(e) => setQuery(e.target.value)}
                        placeholder="Search alerts by title, summary, or monitor…"
                        className="w-full pl-9 pr-8 py-2 rounded-md border border-stone-200 dark:border-stone-700 bg-white dark:bg-stone-900 text-stone-900 dark:text-stone-100 placeholder-stone-400 dark:placeholder-stone-500 focus:outline-none focus:border-orange-500"
                    />
                    {query && (
                        <button
                            type="button"
                            onClick={() => setQuery("")}
                            className="absolute right-2 top-1/2 -translate-y-1/2 p-0.5 rounded text-stone-400 hover:text-stone-600 dark:hover:text-stone-200 hover:bg-stone-200 dark:hover:bg-stone-700"
                            aria-label="Clear search"
                        >
                            <X className="w-3.5 h-3.5" />
                        </button>
                    )}
                </div>
            )}

            {items.length === 0 ? (
                <div className="rounded-xl border border-stone-200 dark:border-stone-800 bg-white dark:bg-stone-900 px-4 py-12 text-center text-stone-600 dark:text-stone-300">
                    {searching
                        ? `no alerts match “${query.trim()}”`
                        : monitorId
                          ? "no alerts from this monitor"
                          : unackedOnly
                            ? "no alerts in your inbox — your monitors haven't found anything to flag"
                            : "no alert history yet"}
                </div>
            ) : (
                <>
                    <div className="space-y-3">
                        {items.map((a) => (
                            <AlertCard key={a.id} alert={a} kind={kindOf(a)} />
                        ))}
                    </div>
                    {atLimit && (
                        <p className="mt-4 text-center text-stone-500 dark:text-stone-400">
                            Showing the {limit} most recent
                            {searching ? " matches" : ""} — refine your search to find older alerts.
                        </p>
                    )}
                </>
            )}
        </div>
    );
}

function AlertsHelpFrame({ unackedOnly }: { unackedOnly: boolean }) {
    return (
        <div className="mb-6 flex gap-3 p-4 rounded-lg bg-orange-50/60 dark:bg-orange-950/20 border border-orange-200/70 dark:border-orange-900/40">
            <div className="flex-shrink-0 mt-0.5">
                <Bell className="w-4 h-4 text-orange-600 dark:text-orange-400" />
            </div>
            <p className="text-stone-700 dark:text-stone-200 leading-relaxed">
                {unackedOnly
                    ? "Findings your scheduled monitors raised. Click an alert to see the agent's summary and evidence, then Acknowledge to clear it from the inbox."
                    : "Every alert your monitors have raised, including ones you've already acknowledged."}
            </p>
        </div>
    );
}

// AI vs Threshold origin tag — colours mirror the monitor stat strip
// (AI = violet, threshold = sky).
function KindBadge({ kind }: { kind: MonitorKind }) {
    const ai = kind === "ai";
    return (
        <span
            className={`uppercase tracking-wider px-1.5 py-0.5 rounded border ${
                ai
                    ? "border-violet-200 bg-violet-50 text-violet-700 dark:border-violet-900/50 dark:bg-violet-950/40 dark:text-violet-300"
                    : "border-sky-200 bg-sky-50 text-sky-700 dark:border-sky-900/50 dark:bg-sky-950/40 dark:text-sky-300"
            }`}
            title={ai ? "Raised by an AI monitor" : "Raised by a threshold monitor"}
        >
            {ai ? "AI" : "Threshold"}
        </span>
    );
}

function AlertCard({ alert, kind }: { alert: Alert; kind: MonitorKind }) {
    const [expanded, setExpanded] = useState(false);
    const tz = useTimezone();
    const createdIso = new Date(alert.created_at).toISOString();

    const monitorHref = `/alerts/monitors?monitor=${encodeURIComponent(alert.monitor_id)}`;
    const toggle = () => setExpanded((v) => !v);

    return (
        <div
            className={`rounded-xl border bg-white dark:bg-stone-900 overflow-hidden ${
                alert.acknowledged
                    ? "border-stone-200 dark:border-stone-800 opacity-70"
                    : "border-stone-200 dark:border-stone-800"
            }`}
        >
            {/* Header — click to expand. A plain div (not a <button>) so the
          monitor name can be a real nested link. */}
            <div
                role="button"
                tabIndex={0}
                onClick={toggle}
                onKeyDown={(e) => {
                    if (e.key === "Enter" || e.key === " ") {
                        e.preventDefault();
                        toggle();
                    }
                }}
                className="px-4 py-3 flex items-start gap-3 cursor-pointer hover:bg-stone-50 dark:hover:bg-stone-800/40 transition"
            >
                <SeverityIcon severity={alert.severity} />
                <div className="flex-grow min-w-0">
                    <div className="flex items-center gap-2 flex-wrap">
                        <strong className="text-stone-900 dark:text-stone-100">
                            {alert.title}
                        </strong>
                        <KindBadge kind={kind} />
                        <SeverityBadge severity={alert.severity} />
                        <EnvBadge env={alert.env} />
                        {alert.acknowledged && (
                            <span className="uppercase tracking-wider px-1.5 py-0.5 rounded border border-stone-200 bg-stone-50 text-stone-500 dark:border-stone-700 dark:bg-stone-800/60 dark:text-stone-400">
                                acked
                            </span>
                        )}
                    </div>
                    <div className="text-stone-500 dark:text-stone-400 mt-0.5 flex items-center gap-2">
                        <Bell className="w-3 h-3 flex-shrink-0" />
                        <Link
                            to={monitorHref}
                            onClick={(e) => e.stopPropagation()}
                            className="hover:text-orange-600 dark:hover:text-orange-400 hover:underline truncate"
                            title={`View the “${alert.monitor_name}” monitor`}
                        >
                            {alert.monitor_name}
                        </Link>
                        <span>·</span>
                        <span className="flex-shrink-0" title={timeAgo(createdIso)}>
                            {formatTsForRow(createdIso, tz)}
                        </span>
                    </div>
                    {!expanded && alert.summary && (
                        <div className="text-stone-700 dark:text-stone-300 mt-1 line-clamp-2">
                            {stripMarkdown(alert.summary)}
                        </div>
                    )}
                </div>
                <ChevronDown
                    className={`w-4 h-4 flex-shrink-0 mt-0.5 text-stone-400 dark:text-stone-500 transition-transform ${
                        expanded ? "rotate-180" : ""
                    }`}
                    aria-hidden="true"
                />
            </div>

            {expanded && (
                <div className="px-4 pb-4 pt-1 border-t border-stone-100 dark:border-stone-800">
                    <AlertDetailBody alert={alert} />
                </div>
            )}

            {/* Actions — always visible (collapsed and expanded). */}
            <div className="px-4 py-2.5 border-t border-stone-100 dark:border-stone-800">
                <AlertActions alert={alert} />
            </div>
        </div>
    );
}
