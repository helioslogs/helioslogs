// Copyright 2026 Appbird LLC. Licensed under "GNU Affero General Public License v3.0"

import { useEffect, useRef, useState } from "react";
import { Check, ChevronDown, ChevronRight, Loader2, Pin, PinOff } from "lucide-react";
import type { AggregateResponse, TopBucket } from "../api/types";
import { isFilterActive } from "../lib/query";
import { isSeverityShapedField, normalizeLevel } from "../lib/severity";

// Glanceable, footer-derived metadata for the field (coverage + kind), shown
// in the header before — and instead of needing — a value breakdown.
export interface FieldMeta {
    coverage: number;
    valueKind: string;
    cardinality: number;
}

interface Props {
    field: string;
    // Lazy fetcher called on first open; full response lets us flag scaled (sampled) counts.
    fetch: (field: string) => Promise<AggregateResponse>;
    // Toggle a `field:value` filter — adds on first click, removes when the
    // value is already an active filter.
    onPick: (field: string, value: string | number) => void;
    // Current query string, so each value row can show whether its filter is
    // already active (→ a checkmark + "click to remove").
    query: string;
    // Query-context key; a change drives an in-place refetch (old buckets dimmed, no flash).
    cacheKey: string;
    // Expand + fetch on mount, for pinned facets; available fields stay collapsed until clicked.
    defaultOpen?: boolean;
    // Defer the aggregate while the main search/histogram is in flight, so the sidebar
    // doesn't steal backend cores; fires once the gate opens.
    mainLoading?: boolean;
    // Footer-derived coverage/kind for the header. Absent for `index`/`source`
    // (universal-core, not in the catalog).
    meta?: FieldMeta;
    // Whether this field is currently pinned (renders a filled pin).
    pinned?: boolean;
    // Pin/unpin handler. Omitted ⇒ no pin affordance (e.g. the always-pinned
    // `index`).
    onTogglePin?: (field: string) => void;
}

const SEV_CLASS: Record<string, string> = {
    DEBUG: "sev-debug",
    INFO: "sev-info",
    WARN: "sev-warn",
    ERROR: "sev-error",
    FATAL: "sev-fatal",
    TRACE: "sev-debug",
};

function formatKey(_field: string, key: string | number): string {
    // returns numeric terms-agg keys as floats (e.g. 200.0); show ints.
    if (typeof key === "number") {
        return Number.isInteger(key) ? String(key) : key.toFixed(1);
    }
    return String(key);
}

// Rows before "show more"; matches discover-fields sample_values keep-count.
const COLLAPSED_LIMIT = 5;

export function FieldPanel({
    field,
    fetch,
    onPick,
    query,
    cacheKey,
    defaultOpen = false,
    mainLoading = false,
    meta,
    pinned = false,
    onTogglePin,
}: Props) {
    const [open, setOpen] = useState(defaultOpen);
    const [expanded, setExpanded] = useState(false);
    const [resp, setResp] = useState<AggregateResponse | null>(null);
    const [loading, setLoading] = useState(false); // first load — nothing on screen yet
    const [refetching, setRefetching] = useState(false); // reloading with stale buckets shown
    const [error, setError] = useState<string | null>(null);

    // `resp` belongs to the query context in `loadedKeyRef`. Tracking it lets us
    // refetch on a context change without first clearing what's rendered.
    const respRef = useRef<AggregateResponse | null>(null);
    respRef.current = resp;
    const loadedKeyRef = useRef<string | null>(null);

    // On context change: open panels refetch in place; collapsed ones drop their cache.
    useEffect(() => {
        if (!open) {
            setResp(null);
            setExpanded(false);
            loadedKeyRef.current = null;
        }
        // eslint-disable-next-line react-hooks/exhaustive-deps
    }, [cacheKey]);

    // Load buckets when open and stale; deferred while mainLoading so the sidebar doesn't steal cores.
    useEffect(() => {
        if (!open || mainLoading) return;
        if (loadedKeyRef.current === cacheKey) return;
        loadedKeyRef.current = cacheKey;
        if (respRef.current) setRefetching(true);
        else setLoading(true);
        setError(null);
        fetch(field)
            .then((r) => setResp(r))
            .catch((e: unknown) => {
                if (e instanceof DOMException && e.name === "AbortError") return;
                setError(e instanceof Error ? e.message : String(e));
            })
            .finally(() => {
                setLoading(false);
                setRefetching(false);
            });
        // eslint-disable-next-line react-hooks/exhaustive-deps
    }, [open, mainLoading, cacheKey, field]);

    // "Updating" = reloading with stale buckets still shown, or waiting on the
    // deferred refetch behind the main search. Dims the list + shows a spinner.
    const busy = refetching || (mainLoading && open && !!resp);

    const buckets: TopBucket[] = resp?.aggs[field] ?? [];
    const total = buckets.reduce((acc, b) => acc + b.count, 0);
    const isSeverity = isSeverityShapedField(field);
    const visible = expanded ? buckets : buckets.slice(0, COLLAPSED_LIMIT);
    const hiddenCount = Math.max(0, buckets.length - COLLAPSED_LIMIT);
    const sampled = resp?.sampled ?? false;
    const samplingTitle = sampled
        ? `Counts scaled from ${resp?.sampled_partitions} of ${resp?.total_partitions} days — the window is wider than the exact-count budget. Narrow the time range for exact counts.`
        : undefined;
    const coveragePct = meta ? Math.round(meta.coverage * 100) : null;
    const coverageTitle =
        coveragePct !== null
            ? `present in ~${coveragePct}% of rows${
                  meta && meta.cardinality > 0 ? ` · ~${meta.cardinality} distinct` : ""
              }`
            : undefined;
    // Unpinned fields show coverage %; pinned ones show their loaded value count.
    const showPercent = !pinned && coveragePct !== null;

    return (
        <div className="border-b border-stone-100 dark:border-stone-800/60 last:border-b-0">
            <div className="group w-full flex items-center gap-1.5 px-3 py-2 hover:bg-stone-50 dark:hover:bg-stone-800/50 text-stone-700 dark:text-stone-300">
                <button
                    type="button"
                    className="flex items-center gap-1.5 text-left min-w-0 flex-grow"
                    onClick={() => setOpen(!open)}
                    title={meta ? `${field} · ${meta.valueKind}` : field}
                >
                    {open ? (
                        <ChevronDown className="w-3 h-3 text-stone-400 flex-shrink-0" />
                    ) : (
                        <ChevronRight className="w-3 h-3 text-stone-400 flex-shrink-0" />
                    )}
                    <span className="font-medium uppercase tracking-wide truncate">{field}</span>
                </button>
                <span className="flex items-center gap-1.5 text-stone-600 dark:text-stone-300 tabular-nums flex-shrink-0">
                    {sampled && (
                        <span
                            className="text-amber-600 dark:text-amber-400 text-[10px] uppercase tracking-wide font-medium"
                            title={samplingTitle}
                        >
                            scaled
                        </span>
                    )}
                    {showPercent ? (
                        <span
                            className="text-xs text-stone-500 dark:text-stone-400"
                            title={coverageTitle}
                        >
                            {coveragePct}%
                        </span>
                    ) : resp ? (
                        <span
                            title={`${buckets.length} value${buckets.length === 1 ? "" : "s"} shown`}
                        >
                            {buckets.length}
                        </span>
                    ) : coveragePct !== null ? (
                        <span
                            className="text-xs text-stone-500 dark:text-stone-400"
                            title={coverageTitle}
                        >
                            {coveragePct}%
                        </span>
                    ) : null}
                    {((loading && !resp) || busy) && (
                        <Loader2 className="w-3 h-3 animate-spin text-stone-400" />
                    )}
                    {onTogglePin && (
                        <button
                            type="button"
                            onClick={(e) => {
                                e.stopPropagation();
                                onTogglePin(field);
                            }}
                            title={pinned ? "Unpin field" : "Pin field"}
                            aria-label={pinned ? "Unpin field" : "Pin field"}
                            className={`p-0.5 rounded hover:bg-stone-200 dark:hover:bg-stone-700 ${
                                pinned
                                    ? "text-orange-600 dark:text-orange-400"
                                    : "text-stone-400 dark:text-stone-500 group-hover:text-orange-600 dark:group-hover:text-orange-400"
                            }`}
                        >
                            {pinned ? <PinOff className="w-3 h-3" /> : <Pin className="w-3 h-3" />}
                        </button>
                    )}
                </span>
            </div>
            {open && (
                <ul
                    className={`px-2 pb-2 space-y-0.5 transition-opacity ${busy ? "opacity-40" : ""}`}
                >
                    {loading && !resp && (
                        <li className="px-2 py-1 text-stone-400 italic">loading…</li>
                    )}
                    {error && (
                        <li className="px-2 py-1 text-red-600 dark:text-red-400">error: {error}</li>
                    )}
                    {resp && buckets.length === 0 && !error && (
                        <li className="px-2 py-1 text-stone-400 italic">no values</li>
                    )}
                    {visible.map((b, i) => {
                        const k = formatKey(field, b.key);
                        const pct = total > 0 ? (b.count / total) * 100 : 0;
                        const sevClass = isSeverity
                            ? (SEV_CLASS[normalizeLevel(k) ?? ""] ?? "")
                            : "";
                        const active = isFilterActive(query, field, b.key);
                        return (
                            <li
                                key={i}
                                onClick={() => onPick(field, b.key)}
                                className={`relative flex items-center gap-2 px-2 py-1 rounded cursor-pointer group ${
                                    active
                                        ? "bg-blue-100/70 dark:bg-blue-900/40 text-blue-800 dark:text-blue-200"
                                        : "text-stone-700 dark:text-stone-300 hover:bg-blue-50/60 dark:hover:bg-blue-950/30"
                                }`}
                                title={
                                    active
                                        ? `remove filter: ${field}:${k}`
                                        : `add to query: ${field}:${k}${sampled ? " (count scaled — wide window)" : ""}`
                                }
                            >
                                <span
                                    className="absolute inset-y-0.5 left-0 rounded bg-blue-100/60 dark:bg-blue-900/30 group-hover:bg-blue-200/70 dark:group-hover:bg-blue-800/40 pointer-events-none"
                                    style={{ width: `${pct.toFixed(1)}%` }}
                                />
                                {active && (
                                    <Check className="relative w-3 h-3 flex-shrink-0 text-blue-600 dark:text-blue-300" />
                                )}
                                <span
                                    className={`relative flex-grow truncate font-mono ${active ? "font-semibold" : ""} ${sevClass}`}
                                >
                                    {k}
                                </span>
                                <span className="relative tabular-nums">
                                    {b.count.toLocaleString()}
                                </span>
                            </li>
                        );
                    })}
                    {hiddenCount > 0 && (
                        <li>
                            <button
                                type="button"
                                onClick={() => setExpanded(!expanded)}
                                className="w-full px-2 py-1 text-left text-stone-500 dark:text-stone-400 hover:text-blue-600 dark:hover:text-blue-400 hover:bg-stone-50 dark:hover:bg-stone-800/50 rounded"
                            >
                                {expanded ? "Show less" : `Show ${hiddenCount} more…`}
                            </button>
                        </li>
                    )}
                </ul>
            )}
        </div>
    );
}
