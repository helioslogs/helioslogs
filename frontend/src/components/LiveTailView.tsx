// Copyright 2026 Appbird LLC. Licensed under "GNU Affero General Public License v3.0"

import { useEffect, useMemo, useRef, useState } from "react";
import { ArrowDown, Check, Copy, Pause, Radio, Sparkles } from "lucide-react";
import type { Hit } from "../api/types";
import type { LiveTail } from "../state/useLiveTail";
import { notifyInvestigateLog } from "../api/events";
import { detectRowSeverity } from "../lib/severity";
import { formatTsForRow } from "../lib/timezone";
import { useTimezone } from "../state/timezone";
import { JsonTree } from "./JsonTree";
import { buildInvestigatePrompt, buildRowContext, prettyRaw } from "./ResultsList";

const SEV_BADGE: Record<string, string> = {
    DEBUG: "sev-badge-debug",
    INFO: "sev-badge-info",
    WARN: "sev-badge-warn",
    ERROR: "sev-badge-error",
    FATAL: "sev-badge-fatal",
};

// Scroll distance from the bottom past which we treat the user as "reading
// history" and pause the tail.
const PAUSE_THRESHOLD_PX = 48;

interface Props {
    tail: LiveTail;
    // Current query + term toggle — makes expanded JSON leaves click-to-search,
    // same contract as ResultsList. A pick restarts the tail with the new filter.
    query: string;
    onPickTerm: (term: string) => void;
}

export function LiveTailView({ tail, query, onPickTerm }: Props) {
    const { rows, paused, bufferedCount, error, pause, resume } = tail;
    const scrollRef = useRef<HTMLDivElement>(null);
    const pausedRef = useRef(paused);
    pausedRef.current = paused;

    // Pin to the bottom while live; a resume also snaps back down.
    useEffect(() => {
        if (paused) return;
        const el = scrollRef.current;
        if (el) el.scrollTop = el.scrollHeight;
    }, [rows, paused]);

    const handleScroll = () => {
        const el = scrollRef.current;
        if (!el) return;
        const fromBottom = el.scrollHeight - el.scrollTop - el.clientHeight;
        if (!pausedRef.current && fromBottom > PAUSE_THRESHOLD_PX) pause();
    };

    return (
        <div className="rounded-xl border border-stone-200 dark:border-stone-800 bg-white dark:bg-stone-900 flex flex-col relative">
            <div className="flex items-center gap-2 px-4 py-2 border-b border-stone-200 dark:border-stone-800 text-stone-500 dark:text-stone-400">
                <Radio
                    className={`w-3.5 h-3.5 ${paused ? "" : "text-orange-500 animate-pulse"}`}
                    aria-hidden="true"
                />
                <span>
                    {paused ? "paused" : "live"} — {rows.length.toLocaleString()} event
                    {rows.length === 1 ? "" : "s"}
                </span>
                {error && <span className="text-red-600 dark:text-red-400">· {error}</span>}
                <div className="flex-grow" />
                {!paused && (
                    <button
                        type="button"
                        onClick={pause}
                        className="inline-flex items-center gap-1 px-2 py-0.5 rounded-md border border-stone-200 dark:border-stone-700 hover:bg-stone-50 dark:hover:bg-stone-800"
                    >
                        <Pause className="w-3 h-3" aria-hidden="true" /> pause
                    </button>
                )}
            </div>
            <div
                ref={scrollRef}
                onScroll={handleScroll}
                className="overflow-auto font-mono text-[13px] leading-relaxed"
                style={{ height: "calc(100vh - 230px)" }}
            >
                {rows.length === 0 && (
                    <div className="px-4 py-10 text-center text-stone-400 dark:text-stone-500 font-sans">
                        waiting for events…
                    </div>
                )}
                {rows.map((h, i) => (
                    <TailRow
                        key={`${h.timestamp ?? ""}-${i}`}
                        hit={h}
                        query={query}
                        onPickTerm={onPickTerm}
                    />
                ))}
            </div>
            {paused && (
                <button
                    type="button"
                    onClick={resume}
                    className="absolute bottom-4 left-1/2 -translate-x-1/2 inline-flex items-center gap-1.5 px-3 py-1.5 rounded-full shadow-lg bg-orange-600 hover:bg-orange-500 text-white font-medium"
                >
                    <ArrowDown className="w-3.5 h-3.5" aria-hidden="true" />
                    {bufferedCount > 0
                        ? `${bufferedCount.toLocaleString()} new event${bufferedCount === 1 ? "" : "s"} — resume`
                        : "resume"}
                </button>
            )}
        </div>
    );
}

function TailRow({
    hit,
    query,
    onPickTerm,
}: {
    hit: Hit;
    query: string;
    onPickTerm: (term: string) => void;
}) {
    const [expanded, setExpanded] = useState(false);
    const [copied, setCopied] = useState(false);
    const tz = useTimezone();
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
    const line = hit.message || hit.raw || "";
    const indexName = hit.partition.split("/", 1)[0];

    return (
        <div
            className="group px-4 py-0.5 hover:bg-stone-50 dark:hover:bg-stone-800/40 cursor-pointer border-l-2 border-transparent hover:border-orange-300"
            onClick={() => setExpanded((v) => !v)}
        >
            <div className="flex items-baseline gap-2 whitespace-nowrap overflow-hidden">
                <span className="text-stone-400 dark:text-stone-500 flex-shrink-0 tabular-nums">
                    {formatTsForRow(hit.timestamp, tz)}
                </span>
                {severity && (
                    <span
                        className={`flex-shrink-0 px-1 rounded text-[11px] font-semibold ${SEV_BADGE[severity] ?? "sev-badge-debug"}`}
                    >
                        {severity}
                    </span>
                )}
                <span className="flex-1 min-w-0 text-stone-800 dark:text-stone-200 overflow-hidden text-ellipsis">
                    {line}
                </span>
                {/* Same per-row actions as the normal results view. */}
                <button
                    type="button"
                    onClick={(e) => {
                        e.stopPropagation();
                        void navigator.clipboard
                            .writeText(
                                buildRowContext(hit, indexName, prettyRaw(hit.raw, hit.message)),
                            )
                            .then(() => {
                                setCopied(true);
                                setTimeout(() => setCopied(false), 1500);
                            })
                            .catch(() => {});
                    }}
                    className="shrink-0 self-center p-1 rounded opacity-0 group-hover:opacity-100 focus-visible:opacity-100 text-stone-400 dark:text-stone-500 group-hover:text-orange-600 dark:group-hover:text-orange-400 hover:bg-stone-100 dark:hover:bg-stone-800 transition"
                    title={copied ? "Copied!" : "Copy this event's context to the clipboard"}
                    aria-label="copy this event's context"
                >
                    {copied ? (
                        <Check className="w-3.5 h-3.5 text-emerald-600 dark:text-emerald-400" />
                    ) : (
                        <Copy className="w-3.5 h-3.5" />
                    )}
                </button>
                <button
                    type="button"
                    onClick={(e) => {
                        e.stopPropagation();
                        notifyInvestigateLog(
                            buildInvestigatePrompt(hit, indexName, prettyRaw(hit.raw, hit.message)),
                        );
                    }}
                    className="shrink-0 self-center p-1 rounded opacity-0 group-hover:opacity-100 focus-visible:opacity-100 text-stone-400 dark:text-stone-500 group-hover:text-orange-600 dark:group-hover:text-orange-400 hover:bg-stone-100 dark:hover:bg-stone-800 transition"
                    title="Investigate this event with the AI agent"
                    aria-label="investigate this event"
                >
                    <Sparkles className="w-3.5 h-3.5" />
                </button>
            </div>
            {expanded && event && (
                // Same interactive JSON tree as the results view: clickable
                // leaves toggle search terms. Clicks inside don't collapse the row.
                <div
                    className="my-1 px-3 py-1.5 rounded-md bg-stone-100 dark:bg-stone-800/60 border-l-2 border-stone-200 dark:border-stone-700 text-stone-900 dark:text-stone-100 cursor-default"
                    onClick={(e) => e.stopPropagation()}
                >
                    <JsonTree
                        data={event}
                        terms={[]}
                        query={query}
                        onPickTerm={onPickTerm}
                        defaultOpenDepth={99}
                    />
                </div>
            )}
            {expanded && !event && (
                <pre className="my-1 px-3 py-2 rounded-md bg-stone-100 dark:bg-stone-800/60 border-l-2 border-stone-200 dark:border-stone-700 text-stone-700 dark:text-stone-300 overflow-auto whitespace-pre-wrap break-all">
                    {hit.raw ?? hit.message ?? ""}
                </pre>
            )}
        </div>
    );
}
