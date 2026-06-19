// Copyright 2026 Appbird LLC. Licensed under "GNU Affero General Public License v3.0"

import { useEffect, useMemo, useRef, useState, type ReactNode } from "react";
import {
    AlignLeft,
    Braces,
    Check,
    ChevronRight,
    Copy,
    ListChevronsDownUp,
    ListChevronsUpDown,
    Loader2,
    Sparkles,
} from "lucide-react";
import type { Hit } from "../api/types";
import { notifyInvestigateLog } from "../api/events";
import { formatTsForRow } from "../lib/timezone";
import { detectRowSeverity } from "../lib/severity";
import { useTimezone } from "../state/timezone";
import { Highlight } from "./Highlight";
import { ClickableText } from "./ClickableText";
import { JsonTree } from "./JsonTree";
import { Pagination } from "./Pagination";

// Per-browser preference for whether result rows start collapsed (one line
// each) or show the expanded preview. Persisted so it survives reloads.
const ROWS_COLLAPSED_KEY = "helios-rows-collapsed";
function readCollapsedPref(): boolean {
    try {
        return localStorage.getItem(ROWS_COLLAPSED_KEY) === "1";
    } catch {
        return false;
    }
}
function setCollapsedPref(collapsed: boolean): void {
    try {
        localStorage.setItem(ROWS_COLLAPSED_KEY, collapsed ? "1" : "0");
    } catch {
        // storage disabled / quota — toggling still works for this session.
    }
}

// Per-browser preference for how each event body renders: a hierarchical
// JSON tree or the flat pretty-printed raw text. Persisted across reloads.
type BodyView = "json" | "raw";
const BODY_VIEW_KEY = "helios-results-body-view";
function readBodyView(): BodyView {
    try {
        return localStorage.getItem(BODY_VIEW_KEY) === "raw" ? "raw" : "json";
    } catch {
        return "json";
    }
}
function setBodyViewPref(view: BodyView): void {
    try {
        localStorage.setItem(BODY_VIEW_KEY, view);
    } catch {
        // storage disabled / quota — toggling still works for this session.
    }
}

interface Props {
    hits: Hit[];
    total: number;
    tookUs: number;
    loading?: boolean;
    highlightTerms?: string[];
    // 1-based page number; page size is fixed per call (passed via pageSize).
    page: number;
    pageSize: number;
    onPageChange: (page: number) => void;
    // Appends `field:value` to the query (same contract as FieldPanel.onPick).
    onPick: (field: string, value: string | number) => void;
    // Sets an absolute window; reused by the per-row timestamp "zoom" popover.
    onPickTimeRange: (startMs: number, endMs: number) => void;
    // Toggle a bare word term — drives click-a-word-to-search in the event body.
    onPickTerm: (term: string) => void;
    // Current query, so clickable words can show which are already searched.
    query: string;
    // Optional content rendered under the "no matches" message (e.g. a first-run
    // "load sample data" action). Only shown when there are zero hits.
    emptyExtra?: ReactNode;
    // Overrides the spinner text shown while loading with no hits yet (default
    // "searching…"); e.g. "Loading sample data…" during first-run seeding.
    loadingLabel?: string;
}

// Symmetric windows offered on timestamp click; `ms` is the half-width each side.
const TIMESTAMP_WINDOWS: { label: string; ms: number }[] = [
    { label: "± 1s", ms: 1_000 },
    { label: "± 5s", ms: 5_000 },
    { label: "± 30s", ms: 30_000 },
    { label: "± 1m", ms: 60_000 },
    { label: "± 5m", ms: 5 * 60_000 },
    { label: "± 15m", ms: 15 * 60_000 },
    { label: "± 1h", ms: 60 * 60_000 },
];

const SEV_BADGE: Record<string, string> = {
    DEBUG: "sev-badge-debug",
    INFO: "sev-badge-info",
    WARN: "sev-badge-warn",
    ERROR: "sev-badge-error",
    FATAL: "sev-badge-fatal",
};

export function ResultsList({
    hits,
    total,
    tookUs,
    loading,
    highlightTerms = [],
    page,
    pageSize,
    onPageChange,
    onPick,
    onPickTimeRange,
    onPickTerm,
    query,
    emptyExtra,
    loadingLabel,
}: Props) {
    // Collapsed = one-line headers until per-row expand; seeded from the persisted pref.
    const [collapsed, setCollapsed] = useState(readCollapsedPref);
    const toggleCollapsed = () =>
        setCollapsed((v) => {
            const next = !v;
            setCollapsedPref(next);
            return next;
        });
    const [bodyView, setBodyView] = useState<BodyView>(readBodyView);
    const pickBodyView = (view: BodyView) => {
        setBodyView(view);
        setBodyViewPref(view);
    };
    const totalPages = Math.max(1, Math.ceil(total / pageSize));
    const showingFrom = total === 0 ? 0 : (page - 1) * pageSize + 1;
    const showingTo = (page - 1) * pageSize + hits.length;
    const pager =
        totalPages > 1 ? (
            <Pagination
                page={page}
                totalPages={totalPages}
                onChange={onPageChange}
                disabled={loading}
            />
        ) : null;
    const viewToggle = (
        <div
            className="inline-flex items-center rounded-md border border-stone-200 dark:border-stone-700 overflow-hidden"
            role="group"
            aria-label="Event body view"
        >
            {[
                { mode: "json" as const, Icon: Braces, label: "JSON tree view" },
                { mode: "raw" as const, Icon: AlignLeft, label: "Raw text view" },
            ].map(({ mode, Icon, label }) => (
                <button
                    key={mode}
                    type="button"
                    onClick={() => pickBodyView(mode)}
                    className={`inline-flex items-center justify-center w-6 h-6 ${
                        bodyView === mode
                            ? "bg-stone-100 dark:bg-stone-800 text-stone-900 dark:text-stone-100"
                            : "text-stone-400 dark:text-stone-500 hover:bg-stone-100 dark:hover:bg-stone-800"
                    }`}
                    title={label}
                    aria-label={label}
                    aria-pressed={bodyView === mode}
                >
                    <Icon className="w-4 h-4" aria-hidden="true" />
                </button>
            ))}
        </div>
    );
    const collapseToggle = (
        <button
            type="button"
            onClick={toggleCollapsed}
            className="inline-flex items-center justify-center w-6 h-6 rounded text-stone-900 dark:text-stone-100 hover:bg-stone-100 dark:hover:bg-stone-800"
            title={collapsed ? "Expand rows" : "Collapse rows"}
            aria-label={collapsed ? "Expand rows" : "Collapse rows"}
        >
            {collapsed ? (
                <ListChevronsUpDown className="w-5 h-5" aria-hidden="true" />
            ) : (
                <ListChevronsDownUp className="w-5 h-5" aria-hidden="true" />
            )}
        </button>
    );

    return (
        <div className="rounded-xl border border-stone-200 dark:border-stone-800 bg-white dark:bg-stone-900">
            <div className="flex items-center justify-between px-4 py-2 border-b border-stone-200 dark:border-stone-800 text-stone-700 dark:text-stone-400">
                <span className="inline-flex items-center gap-1.5">
                    {loading && <Loader2 className="w-3.5 h-3.5 animate-spin" aria-hidden="true" />}
                    {loading ? (
                        <span>{hits.length > 0 ? "refreshing…" : "searching…"}</span>
                    ) : (
                        <>
                            <span className="font-semibold text-stone-900 dark:text-stone-200">
                                {total.toLocaleString()}
                            </span>{" "}
                            hits in{" "}
                            {tookUs < 1000 ? `${tookUs}µs` : `${(tookUs / 1000).toFixed(2)}ms`}
                            {total > 0 && (
                                <span className="text-stone-700 dark:text-stone-500">
                                    {" "}
                                    · showing {showingFrom.toLocaleString()}–
                                    {showingTo.toLocaleString()} (page {page} of {totalPages})
                                </span>
                            )}
                        </>
                    )}
                </span>
                <div className="flex items-center gap-2">
                    {viewToggle}
                    {collapseToggle}
                    {pager}
                </div>
            </div>
            <div className="divide-y divide-stone-100 dark:divide-stone-800">
                {hits.map((h, i) => (
                    <Row
                        key={`${collapsed}-${i}`}
                        hit={h}
                        terms={highlightTerms}
                        collapsed={collapsed}
                        bodyView={bodyView}
                        onPick={onPick}
                        onPickTimeRange={onPickTimeRange}
                        onPickTerm={onPickTerm}
                        query={query}
                    />
                ))}
                {hits.length === 0 && loading && (
                    <div className="px-4 py-10 flex items-center justify-center gap-2 text-stone-500 dark:text-stone-400">
                        <Loader2 className="w-4 h-4 animate-spin" aria-hidden="true" />
                        <span>{loadingLabel ?? "searching…"}</span>
                    </div>
                )}
                {hits.length === 0 && !loading && (
                    <div className="px-4 py-8 flex flex-col items-center gap-4 text-center text-stone-400 dark:text-stone-500">
                        <span>no matches</span>
                        {emptyExtra}
                    </div>
                )}
            </div>
            {pager && (
                <div className="px-4 py-2 border-t border-stone-200 dark:border-stone-800 flex justify-end">
                    {pager}
                </div>
            )}
        </div>
    );
}

// Pretty-prints the raw JSON for display. Falls back to the original
// string if it isn't parseable as JSON (e.g. plain-text logs).
export function prettyRaw(raw: string | undefined, fallbackMessage: string | undefined): string {
    if (raw) {
        try {
            return JSON.stringify(JSON.parse(raw), null, 2);
        } catch {
            return raw;
        }
    }
    return fallbackMessage ?? "";
}

// Row-header label field, first match wins; schema-on-read has no canonical service field.
const LABEL_FIELD_PRIORITY = ["service", "app", "logger", "application", "service.name"];

function pickRowLabel(event: Record<string, unknown> | null, hit: Hit): string {
    if (hit.message) return hit.message;
    if (event) {
        const msg = event["message"];
        if (typeof msg === "string" && msg) return msg;
        for (const key of LABEL_FIELD_PRIORITY) {
            const v = event[key];
            if (typeof v === "string" && v) return v;
            if (typeof v === "number") return String(v);
        }
    }
    return hit.source ?? "—";
}

// Plain-text context for a row, for copying to the clipboard: metadata header
// plus the pretty-printed event. No analysis brief (cf. buildInvestigatePrompt).
export function buildRowContext(hit: Hit, indexName: string, rawPretty: string): string {
    const lines: string[] = [`Timestamp: ${hit.timestamp ?? "(unknown)"}`, `Index: ${indexName}`];
    if (hit.source) lines.push(`Source: ${hit.source}`);
    lines.push("", rawPretty);
    return lines.join("\n");
}

// Seed text for a per-row investigation: verbatim event plus an analysis brief.
export function buildInvestigatePrompt(hit: Hit, indexName: string, rawPretty: string): string {
    const lines: string[] = [
        "Investigate this single log entry and explain what's going on.",
        "",
        `- Timestamp: ${hit.timestamp ?? "(unknown)"}`,
        `- Index: ${indexName}`,
    ];
    if (hit.source) lines.push(`- Source: ${hit.source}`);
    lines.push(
        "",
        "Log entry:",
        "```json",
        rawPretty,
        "```",
        "",
        "Look at what happened in the minutes around this timestamp, find related " +
            "or correlated events across services and indexes, and make judgement " +
            "calls about the likely cause and severity. Use the available tools.",
    );
    return lines.join("\n");
}

function Row({
    hit,
    terms,
    collapsed,
    bodyView,
    onPick,
    onPickTimeRange,
    onPickTerm,
    query,
}: {
    hit: Hit;
    terms: string[];
    collapsed: boolean;
    bodyView: BodyView;
    onPick: (field: string, value: string | number) => void;
    onPickTimeRange: (startMs: number, endMs: number) => void;
    onPickTerm: (term: string) => void;
    query: string;
}) {
    const [open, setOpen] = useState(false);
    const [tsMenuOpen, setTsMenuOpen] = useState(false);
    const [copied, setCopied] = useState(false);
    const tsMenuRef = useRef<HTMLDivElement>(null);
    const tz = useTimezone();

    // Close the timestamp popover on outside click; mousedown so it dismisses before onClick.
    useEffect(() => {
        if (!tsMenuOpen) return;
        const handler = (e: MouseEvent) => {
            if (tsMenuRef.current && !tsMenuRef.current.contains(e.target as Node)) {
                setTsMenuOpen(false);
            }
        };
        window.addEventListener("mousedown", handler);
        return () => window.removeEventListener("mousedown", handler);
    }, [tsMenuOpen]);

    const tsMs = useMemo(() => {
        if (!hit.timestamp) return null;
        const n = Date.parse(hit.timestamp);
        return Number.isFinite(n) ? n : null;
    }, [hit.timestamp]);
    // Parse `raw` once per row so the severity-shaped check and label pick
    // share the work. Falls back to null if the event isn't JSON.
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
    const sevBadge = severity ? (SEV_BADGE[severity] ?? "sev-badge-debug") : null;
    const label = pickRowLabel(event, hit);
    const rawPretty = prettyRaw(hit.raw, hit.message);
    // `hit.partition` is `<index>/<day>`; surface just the index, as the user would query it.
    const indexName = hit.partition.split("/", 1)[0];
    // Closed collapsed rows are single-line; body + footer appear only once expanded.
    const showBody = open || !collapsed;

    return (
        <div
            className={`group px-4 py-2 cursor-pointer transition-colors ${
                open
                    ? "bg-stone-50 dark:bg-stone-800/40"
                    : "hover:bg-stone-50 dark:hover:bg-stone-800/40"
            }`}
            onClick={() => setOpen(!open)}
        >
            <div className="flex items-center gap-3">
                {/* Chevron rotates 90° on expand; flush-left keeps timestamps aligned. */}
                <ChevronRight
                    className={`w-3.5 h-3.5 shrink-0 text-stone-400 dark:text-stone-500 transition-transform ${
                        open ? "rotate-90" : ""
                    }`}
                    aria-hidden="true"
                />
                <div className="relative shrink-0" ref={tsMenuRef}>
                    <button
                        type="button"
                        onClick={(e) => {
                            e.stopPropagation();
                            if (tsMs !== null) setTsMenuOpen((v) => !v);
                        }}
                        disabled={tsMs === null}
                        className="font-mono text-stone-700 dark:text-stone-400 tabular-nums whitespace-nowrap hover:text-blue-600 dark:hover:text-blue-400 disabled:hover:text-stone-700 disabled:dark:hover:text-stone-400 disabled:cursor-default"
                        title={
                            tsMs === null
                                ? hit.timestamp
                                : `${hit.timestamp} — click to set a time window around this event`
                        }
                    >
                        {formatTsForRow(hit.timestamp, tz)}
                    </button>
                    {tsMenuOpen && tsMs !== null && (
                        <div
                            className="absolute left-0 top-full mt-1 z-20 min-w-[10rem] rounded-md border border-stone-200 dark:border-stone-700 bg-white dark:bg-stone-900 shadow-lg py-1"
                            onClick={(e) => e.stopPropagation()}
                        >
                            <div className="px-3 py-1 text-stone-500 dark:text-stone-400 border-b border-stone-100 dark:border-stone-800">
                                Window around event
                            </div>
                            {TIMESTAMP_WINDOWS.map((w) => (
                                <button
                                    key={w.ms}
                                    type="button"
                                    onClick={(e) => {
                                        e.stopPropagation();
                                        setTsMenuOpen(false);
                                        onPickTimeRange(tsMs - w.ms, tsMs + w.ms);
                                    }}
                                    className="block w-full text-left px-3 py-1 hover:bg-stone-100 dark:hover:bg-stone-800 text-stone-700 dark:text-stone-300"
                                >
                                    {w.label}
                                </button>
                            ))}
                        </div>
                    )}
                </div>
                {severity && sevBadge && (
                    <span
                        className={`inline-flex items-center px-1.5 py-0.5 rounded font-semibold ring-1 ring-inset uppercase shrink-0 ${sevBadge}`}
                    >
                        {severity}
                    </span>
                )}
                <span
                    className="flex-1 font-mono text-stone-900 dark:text-stone-100 truncate min-w-0"
                    title={label}
                >
                    <Highlight text={label} terms={terms} />
                </span>
                {/* Copy this event's context (metadata + JSON) to the clipboard. */}
                <button
                    type="button"
                    onClick={(e) => {
                        e.stopPropagation();
                        void navigator.clipboard
                            .writeText(buildRowContext(hit, indexName, rawPretty))
                            .then(() => {
                                setCopied(true);
                                setTimeout(() => setCopied(false), 1500);
                            })
                            .catch(() => {});
                    }}
                    className="shrink-0 p-1 rounded text-stone-400 dark:text-stone-500 group-hover:text-orange-600 dark:group-hover:text-orange-400 hover:bg-stone-100 dark:hover:bg-stone-800 transition"
                    title={copied ? "Copied!" : "Copy this event's context to the clipboard"}
                    aria-label="copy this event's context"
                >
                    {copied ? (
                        <Check className="w-3.5 h-3.5 text-emerald-600 dark:text-emerald-400" />
                    ) : (
                        <Copy className="w-3.5 h-3.5" />
                    )}
                </button>
                {/* Investigate this event with the agent — revealed on row hover.
            Opens a fresh thread seeded with the entry + an analysis brief. */}
                <button
                    type="button"
                    onClick={(e) => {
                        e.stopPropagation();
                        notifyInvestigateLog(buildInvestigatePrompt(hit, indexName, rawPretty));
                    }}
                    className="shrink-0 p-1 rounded text-stone-400 dark:text-stone-500 group-hover:text-orange-600 dark:group-hover:text-orange-400 hover:bg-stone-100 dark:hover:bg-stone-800 transition"
                    title="Investigate this event with the AI agent"
                    aria-label="investigate this event"
                >
                    <Sparkles className="w-3.5 h-3.5" />
                </button>
            </div>
            {showBody && (
                <>
                    {/* Event payload as JSON tree or raw text per the header toggle;
              clamped to ~5 lines when closed so big events don't push rows off-screen. */}
                    {bodyView === "json" && event ? (
                        <div
                            className={`mt-1.5 font-mono text-stone-900 dark:text-stone-100 rounded-md px-3 py-1.5 bg-stone-100 dark:bg-stone-800/60 border-l-2 border-stone-200 dark:border-stone-700 ${
                                open ? "" : "max-h-32 overflow-hidden"
                            }`}
                        >
                            <JsonTree
                                key={open ? "full" : "preview"}
                                data={event}
                                terms={terms}
                                query={query}
                                onPickTerm={onPickTerm}
                                defaultOpenDepth={open ? 99 : 1}
                            />
                        </div>
                    ) : (
                        <pre
                            className={`mt-1.5 font-mono leading-snug text-stone-900 dark:text-stone-100 whitespace-pre-wrap break-all rounded-md px-3 py-1.5 bg-stone-100 dark:bg-stone-800/60 border-l-2 border-stone-200 dark:border-stone-700 ${
                                open ? "" : "line-clamp-5"
                            }`}
                        >
                            <ClickableText
                                text={rawPretty}
                                terms={terms}
                                query={query}
                                onPickTerm={onPickTerm}
                            />
                        </pre>
                    )}
                    {/* Index left, source right — both one-click filters that don't toggle the row. */}
                    <div className="mt-1 flex items-center justify-between gap-4 font-mono text-stone-500 dark:text-stone-500">
                        <button
                            type="button"
                            onClick={(e) => {
                                e.stopPropagation();
                                onPick("index", indexName);
                            }}
                            className="shrink-0 hover:text-blue-600 dark:hover:text-blue-400"
                            title={`filter: index:${indexName}`}
                        >
                            index=
                            <span className="text-stone-700 dark:text-stone-300 group-hover:underline">
                                "{indexName}"
                            </span>
                        </button>
                        {hit.source && (
                            <button
                                type="button"
                                onClick={(e) => {
                                    e.stopPropagation();
                                    onPick("source", hit.source!);
                                }}
                                className="min-w-0 flex hover:text-blue-600 dark:hover:text-blue-400"
                                title={`filter: source:${hit.source}`}
                            >
                                <span className="shrink-0">source=&quot;</span>
                                <span className="truncate text-stone-700 dark:text-stone-300 max-w-[24rem]">
                                    {hit.source}
                                </span>
                                <span className="shrink-0">&quot;</span>
                            </button>
                        )}
                    </div>
                </>
            )}
        </div>
    );
}
