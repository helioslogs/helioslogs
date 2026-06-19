// Copyright 2026 Appbird LLC. Licensed under "GNU Affero General Public License v3.0"

import { useState } from "react";
import { AlertCircle, CheckCircle2, ChevronDown, ChevronRight, Loader2 } from "lucide-react";
import type { AgentToolCallUI } from "../state/useAgentChat";
import { formatDuration } from "../lib/formatDuration";
import { LiveDuration } from "./LiveDuration";

interface Props {
    call: AgentToolCallUI;
    // Label like Q1 / H1 / A1 / L1 based on tool type + position in turn.
    label: string;
}

const TOOL_PRETTY: Record<string, string> = {
    query_logs: "Query",
    histogram: "Histogram",
    aggregate: "Aggregate",
    discover_fields: "Discover fields",
    list_indexes: "List indexes",
    list_sources: "List sources",
    list_environments: "List envs",
};

export function AgentToolArtifact({ call, label }: Props) {
    const [open, setOpen] = useState(false);
    const pretty = TOOL_PRETTY[call.name] ?? call.name;
    const summary = summarize(call);

    return (
        <div className="rounded-lg border border-stone-200 dark:border-stone-700 bg-white dark:bg-stone-900 overflow-hidden">
            <button
                type="button"
                onClick={() => setOpen(!open)}
                className="w-full px-3 py-2 flex items-center gap-2 hover:bg-stone-50 dark:hover:bg-stone-800/40 text-left"
            >
                {open ? (
                    <ChevronDown className="w-3 h-3 text-stone-400 flex-shrink-0" />
                ) : (
                    <ChevronRight className="w-3 h-3 text-stone-400 flex-shrink-0" />
                )}
                <span className="font-mono font-semibold text-stone-700 dark:text-stone-200">
                    {label}
                </span>
                <span className="text-stone-500 dark:text-stone-400">{pretty}</span>
                <span className="flex-grow truncate text-stone-500 dark:text-stone-400">
                    {summary}
                </span>
                {call.status === "running" && call.startedAt !== undefined ? (
                    <span
                        className="text-stone-400 dark:text-stone-500 flex-shrink-0"
                        title="Running — wall-clock so far"
                    >
                        <LiveDuration startedAt={call.startedAt} />
                    </span>
                ) : call.durationMs !== undefined ? (
                    <span
                        className="text-stone-400 dark:text-stone-500 tabular-nums flex-shrink-0"
                        title="Wall-clock duration"
                    >
                        {formatDuration(call.durationMs)}
                    </span>
                ) : null}
                <StatusIcon status={call.status} />
            </button>
            {open && (
                <div className="border-t border-stone-100 dark:border-stone-800 bg-stone-50/50 dark:bg-stone-950/40">
                    <Section title="arguments">
                        <pre className="font-mono leading-snug whitespace-pre-wrap break-all text-stone-700 dark:text-stone-300">
                            {JSON.stringify(call.arguments, null, 2)}
                        </pre>
                    </Section>
                    {call.status === "ok" && call.result !== undefined && (
                        <Section title="result">
                            <pre className="font-mono leading-snug whitespace-pre-wrap break-all text-stone-700 dark:text-stone-300 max-h-80 overflow-auto">
                                {JSON.stringify(call.result, null, 2)}
                            </pre>
                        </Section>
                    )}
                    {call.status === "error" && (
                        <Section title="error">
                            <pre className="font-mono text-red-700 dark:text-red-300 whitespace-pre-wrap break-all">
                                {call.error}
                            </pre>
                        </Section>
                    )}
                </div>
            )}
        </div>
    );
}

function Section({ title, children }: { title: string; children: React.ReactNode }) {
    return (
        <div className="px-3 py-2 border-b border-stone-100 dark:border-stone-800 last:border-b-0">
            <div className="uppercase tracking-wider text-stone-400 dark:text-stone-500 mb-1">
                {title}
            </div>
            {children}
        </div>
    );
}

function StatusIcon({ status }: { status: AgentToolCallUI["status"] }) {
    if (status === "streaming" || status === "running") {
        return <Loader2 className="w-3.5 h-3.5 text-blue-500 animate-spin flex-shrink-0" />;
    }
    if (status === "ok") {
        return (
            <CheckCircle2 className="w-3.5 h-3.5 text-green-600 dark:text-green-400 flex-shrink-0" />
        );
    }
    return <AlertCircle className="w-3.5 h-3.5 text-red-600 dark:text-red-400 flex-shrink-0" />;
}

// A one-line "what did this call do" for the closed view. Pulls the most
// salient args for each tool type.
function summarize(call: AgentToolCallUI): string {
    const a = call.arguments;
    switch (call.name) {
        case "query_logs": {
            const r = call.result as { total?: number; took_us?: number } | undefined;
            const head = `q=${String(a.q ?? "*")}`;
            const range = a.start || a.end ? ` · ${a.start ?? "-6h"}…${a.end ?? "now"}` : "";
            const totals = r?.total !== undefined ? ` → ${r.total.toLocaleString()} hits` : "";
            return head + range + totals;
        }
        case "histogram": {
            const r = call.result as { buckets?: unknown[]; interval_ms?: number } | undefined;
            const head = `q=${String(a.q ?? "*")}`;
            const totals = r?.buckets ? ` → ${r.buckets.length} buckets` : "";
            return head + totals;
        }
        case "aggregate": {
            const fields = String(a.fields ?? "—");
            const head = `q=${String(a.q ?? "*")} · ${fields}`;
            return head;
        }
        case "discover_fields": {
            const r = call.result as { fields?: unknown[]; sample_size?: number } | undefined;
            const head = `q=${String(a.q ?? "*")}`;
            const totals = r?.fields
                ? ` → ${r.fields.length} fields (sample ${r.sample_size ?? "?"})`
                : "";
            return head + totals;
        }
        case "list_indexes": {
            const r = call.result as { indexes?: string[] } | undefined;
            return r?.indexes ? `→ ${r.indexes.length} indexes` : "listing indexes";
        }
        case "list_sources": {
            const r = call.result as { sources?: string[] } | undefined;
            return r?.sources ? `→ ${r.sources.length} sources` : "discovering sources";
        }
        case "list_environments": {
            const r = call.result as { environments?: unknown[] } | undefined;
            return r?.environments
                ? `→ ${r.environments.length} env${r.environments.length === 1 ? "" : "s"}`
                : "listing envs";
        }
        default:
            return JSON.stringify(a);
    }
}
