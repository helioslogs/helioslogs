// Copyright 2026 Appbird LLC. Licensed under "GNU Affero General Public License v3.0"

import { useEffect, useMemo, useState } from "react";
import { useNavigate } from "react-router-dom";
import { Braces, ChevronRight, ExternalLink, Table as TableIcon } from "lucide-react";
import { search } from "../../api/client";
import type { Hit, Widget } from "../../api/types";
import { formatTsForRow } from "../../lib/timezone";
import { detectRowSeverity } from "../../lib/severity";
import { useTimezone } from "../../state/timezone";
import { JsonTree } from "../JsonTree";
import { dashSearchHref, type DashRange } from "./util";

interface Props {
    widget: Widget;
    range: DashRange;
    refreshKey: number;
    // Display mode, owned by the parent so its toggle can live in the widget
    // title bar (out of the scrolling results area).
    view: ResultsView;
    onLoadingChange?: (loading: boolean) => void;
    onError?: (msg: string | null) => void;
}

// Per-widget, per-browser preference for the results display: the compact
// Time/Message table or a hierarchical JSON view of each event.
export type ResultsView = "table" | "json";
const viewKey = (id: string) => `helios-widget-results-view-${id}`;
export function readResultsView(id: string): ResultsView {
    try {
        return localStorage.getItem(viewKey(id)) === "json" ? "json" : "table";
    } catch {
        return "table";
    }
}
export function writeResultsView(id: string, view: ResultsView): void {
    try {
        localStorage.setItem(viewKey(id), view);
    } catch {
        // storage disabled — toggle still works for this session.
    }
}

// The open-in-search + Table/JSON toggle, rendered into the widget's title
// bar (via `WidgetFrame.headerRight`) so it never overlaps scrolling rows.
export function ResultsViewControls({
    view,
    onChange,
    onOpen,
}: {
    view: ResultsView;
    onChange: (view: ResultsView) => void;
    onOpen: () => void;
}) {
    return (
        <span
            className="widget-no-drag flex items-center gap-1.5 shrink-0"
            onMouseDown={(e) => e.stopPropagation()}
        >
            <button
                type="button"
                onClick={onOpen}
                title="Open in search"
                aria-label="Open in search"
                className="inline-flex items-center justify-center w-6 h-6 rounded text-stone-500 dark:text-stone-400 hover:text-orange-600 dark:hover:text-orange-400 hover:bg-stone-100 dark:hover:bg-stone-800"
            >
                <ExternalLink className="w-4 h-4" aria-hidden="true" />
            </button>
            <span
                className="inline-flex items-center rounded-md border border-stone-200 dark:border-stone-700 overflow-hidden"
                role="group"
                aria-label="Results view"
            >
                {[
                    { mode: "table" as const, Icon: TableIcon, label: "Table view" },
                    { mode: "json" as const, Icon: Braces, label: "JSON view" },
                ].map(({ mode, Icon, label }) => (
                    <button
                        key={mode}
                        type="button"
                        onClick={() => onChange(mode)}
                        className={`inline-flex items-center justify-center w-6 h-6 ${
                            view === mode
                                ? "bg-stone-100 dark:bg-stone-800 text-stone-900 dark:text-stone-100"
                                : "text-stone-400 dark:text-stone-500 hover:bg-stone-100 dark:hover:bg-stone-800"
                        }`}
                        title={label}
                        aria-label={label}
                        aria-pressed={view === mode}
                    >
                        <Icon className="w-4 h-4" aria-hidden="true" />
                    </button>
                ))}
            </span>
        </span>
    );
}

const SEV_BADGE: Record<string, string> = {
    DEBUG: "sev-badge-debug",
    INFO: "sev-badge-info",
    WARN: "sev-badge-warn",
    ERROR: "sev-badge-error",
    FATAL: "sev-badge-fatal",
};

// Parse a hit's verbatim event and detect its log level, mirroring the
// search-results rows. Null when the event isn't JSON or carries no level.
function hitSeverity(hit: Hit): string | null {
    if (!hit.raw) return null;
    try {
        const parsed = JSON.parse(hit.raw);
        return detectRowSeverity(typeof parsed === "object" && parsed !== null ? parsed : null);
    } catch {
        return null;
    }
}

// Compact level badge, sized for the dense widget rows.
function SevBadge({ severity }: { severity: string | null }) {
    if (!severity) return null;
    return (
        <span
            className={`shrink-0 inline-flex items-center px-1 rounded text-[10px] font-semibold ring-1 ring-inset uppercase ${
                SEV_BADGE[severity] ?? "sev-badge-debug"
            }`}
        >
            {severity}
        </span>
    );
}

// The latest events matching a query over the widget's window. Shows either a
// compact Time/Message table or a per-event JSON tree, toggled in the widget.
export function SearchResultsWidget({
    widget,
    range,
    refreshKey,
    view,
    onLoadingChange,
    onError,
}: Props) {
    const query = widget.series?.[0]?.query || "*";
    const limit = widget.limit || 20;
    const [hits, setHits] = useState<Hit[]>([]);
    const tz = useTimezone();
    const navigate = useNavigate();

    useEffect(() => {
        let cancelled = false;
        onLoadingChange?.(true);
        search({ q: query, start: range.start, end: range.end, limit })
            .then((r) => {
                if (cancelled) return;
                setHits(r.hits);
                onError?.(null);
            })
            .catch((e: unknown) => onError?.(e instanceof Error ? e.message : String(e)))
            .finally(() => {
                if (!cancelled) onLoadingChange?.(false);
            });
        return () => {
            cancelled = true;
        };
        // eslint-disable-next-line react-hooks/exhaustive-deps
    }, [query, limit, range.start, range.end, refreshKey]);

    const open = () => navigate(dashSearchHref(query, range));

    if (hits.length === 0) {
        return <p className="text-sm text-stone-500 dark:text-stone-400">no matching events</p>;
    }

    return (
        <>
            {view === "json" ? (
                <ul className="divide-y divide-stone-100 dark:divide-stone-800 -my-1">
                    {hits.map((h, i) => (
                        <JsonEventRow key={i} hit={h} tz={tz} />
                    ))}
                </ul>
            ) : (
                <table className="w-full text-sm">
                    <thead>
                        <tr className="text-left text-[11px] uppercase tracking-wider text-stone-500 dark:text-stone-400">
                            <th className="font-semibold py-1 pr-3 w-px whitespace-nowrap">Time</th>
                            <th className="font-semibold py-1">Message</th>
                        </tr>
                    </thead>
                    <tbody className="divide-y divide-stone-100 dark:divide-stone-800">
                        {hits.map((h, i) => (
                            <tr
                                key={i}
                                onClick={open}
                                className="cursor-pointer hover:bg-stone-50 dark:hover:bg-stone-800/60"
                                title="open in search"
                            >
                                <td className="py-1 pr-3 align-top whitespace-nowrap text-stone-600 dark:text-stone-300 tabular-nums">
                                    {formatTsForRow(h.timestamp, tz)}
                                </td>
                                <td className="py-1 align-top text-stone-800 dark:text-stone-100">
                                    <span className="flex items-start gap-1.5">
                                        <SevBadge severity={hitSeverity(h)} />
                                        <span className="line-clamp-2 break-all font-mono text-xs">
                                            {h.message || h.raw || ""}
                                        </span>
                                    </span>
                                </td>
                            </tr>
                        ))}
                    </tbody>
                </table>
            )}
        </>
    );
}

// One JSON-mode event: collapsible header expanding to a read-only JSON tree.
function JsonEventRow({ hit, tz }: { hit: Hit; tz: string }) {
    const [open, setOpen] = useState(false);
    const event = useMemo<Record<string, unknown> | null>(() => {
        if (!hit.raw) return null;
        try {
            const parsed = JSON.parse(hit.raw);
            return typeof parsed === "object" && parsed !== null ? parsed : null;
        } catch {
            return null;
        }
    }, [hit.raw]);
    const severity = detectRowSeverity(event);

    return (
        <li className="py-1">
            <button
                type="button"
                onClick={() => setOpen((v) => !v)}
                className="w-full flex items-start gap-2 text-left rounded px-1 -mx-1 hover:bg-stone-50 dark:hover:bg-stone-800/60"
            >
                <ChevronRight
                    className={`w-3.5 h-3.5 shrink-0 mt-0.5 text-stone-400 dark:text-stone-500 transition-transform ${
                        open ? "rotate-90" : ""
                    }`}
                    aria-hidden="true"
                />
                <span className="shrink-0 mt-0.5 text-xs text-stone-600 dark:text-stone-300 tabular-nums">
                    {formatTsForRow(hit.timestamp, tz)}
                </span>
                <SevBadge severity={severity} />
                <span className="min-w-0 flex-1 text-xs font-mono text-stone-800 dark:text-stone-100 truncate">
                    {hit.message || hit.raw || ""}
                </span>
            </button>
            {open && (
                <div className="mt-1 ml-5 font-mono text-xs rounded-md px-3 py-1.5 bg-stone-100 dark:bg-stone-800/60 border-l-2 border-stone-200 dark:border-stone-700">
                    {event ? (
                        <JsonTree data={event} terms={[]} defaultOpenDepth={99} />
                    ) : (
                        <pre className="whitespace-pre-wrap break-all text-stone-900 dark:text-stone-100">
                            {hit.raw || hit.message || ""}
                        </pre>
                    )}
                </div>
            )}
        </li>
    );
}
