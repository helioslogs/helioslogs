// Copyright 2026 Appbird LLC. Licensed under "GNU Affero General Public License v3.0"

import { useState } from "react";
import { useNavigate } from "react-router-dom";
import { AlertTriangle, Bell, Search as SearchIcon, Sparkles } from "lucide-react";
import { acknowledgeAlert, getEnv, setEnv } from "../api/client";
import { notifyAlertsChanged } from "../api/events";
import { investigateInEnv, openConversationInEnv } from "../lib/alertActions";
import type { Alert } from "../api/types";
import { Markdown } from "./Markdown";
import { timeAgo } from "../lib/format";
import { DEFAULTS, searchHref } from "../state/url";

export interface ThresholdEvidence {
    query: string;
    windowSeconds: number;
    index: string | null;
    count?: number;
    threshold?: number;
}

// Pull a threshold monitor's reproducible search context out of an alert's
// evidence. Returns null for AI alerts (no `query`/`window_seconds`).
export function thresholdEvidence(alert: Alert): ThresholdEvidence | null {
    const e = alert.evidence;
    if (!e || typeof e !== "object") return null;
    const ev = e as Record<string, unknown>;
    const query = typeof ev.query === "string" ? ev.query : null;
    const windowSeconds = typeof ev.window_seconds === "number" ? ev.window_seconds : null;
    if (query === null || windowSeconds === null) return null;
    return {
        query,
        windowSeconds,
        index: typeof ev.index === "string" && ev.index ? ev.index : null,
        count: typeof ev.count === "number" ? ev.count : undefined,
        threshold: typeof ev.threshold === "number" ? ev.threshold : undefined,
    };
}

// Seed text for investigating a threshold alert via the AI panel — hands
// the agent the query, window, and observed numbers, then lets it dig in.
export function buildThresholdInvestigatePrompt(alert: Alert, th: ThresholdEvidence): string {
    const mins = Math.max(1, Math.round(th.windowSeconds / 60));
    const lines: string[] = [
        "Investigate this monitor alert and explain what's going on.",
        "",
        `- Alert: ${alert.title}`,
        `- Query: ${th.query}`,
    ];
    if (th.index) lines.push(`- Index: ${th.index}`);
    lines.push(`- Window: last ${mins}m ending ${new Date(alert.created_at).toISOString()}`);
    if (th.count !== undefined && th.threshold !== undefined) {
        lines.push(`- Observed count: ${th.count} (threshold ${th.threshold})`);
    }
    lines.push(
        "",
        "Look at the matching events and the period around this time, find related or " +
            "correlated events across services and indexes, and make judgement calls about the " +
            "likely cause and severity. Use the available tools.",
    );
    return lines.join("\n");
}

// Flatten agent Markdown to plain text for collapsed one-line previews.
export function stripMarkdown(md: string): string {
    return md
        .replace(/`([^`]+)`/g, "$1")
        .replace(/\*\*([^*]+)\*\*/g, "$1")
        .replace(/__([^_]+)__/g, "$1")
        .replace(/\*([^*]+)\*/g, "$1")
        .replace(/\[([^\]]+)\]\([^)]*\)/g, "$1")
        .replace(/^#+\s*/gm, "")
        .replace(/\s+/g, " ")
        .trim();
}

export function SeverityIcon({ severity }: { severity: Alert["severity"] }) {
    const cls =
        severity === "high"
            ? "text-red-600 dark:text-red-400"
            : severity === "medium"
              ? "text-orange-500"
              : "text-stone-500 dark:text-stone-400";
    return <AlertTriangle className={`w-4 h-4 flex-shrink-0 mt-0.5 ${cls}`} />;
}

export function SeverityBadge({ severity }: { severity: Alert["severity"] }) {
    const cls =
        severity === "high"
            ? "border-red-200 bg-red-50 text-red-700 dark:border-red-900 dark:bg-red-950/40 dark:text-red-300"
            : severity === "medium"
              ? "border-orange-200 bg-orange-50 text-orange-800 dark:border-orange-900 dark:bg-orange-950/40 dark:text-orange-200"
              : "border-stone-200 bg-stone-50 text-stone-600 dark:border-stone-700 dark:bg-stone-800/60 dark:text-stone-400";
    return (
        <span
            className={`inline-flex items-center px-1.5 py-0.5 rounded border uppercase tracking-wider ${cls}`}
        >
            {severity}
        </span>
    );
}

export function EvidenceBlock({ evidence }: { evidence: Record<string, unknown> }) {
    const entries = Object.entries(evidence);
    if (entries.length === 0) return null;
    return (
        <div className="rounded-md bg-stone-50 dark:bg-stone-800/60 border border-stone-200 dark:border-stone-700 px-3 py-2">
            <div className="text-stone-500 dark:text-stone-400 uppercase tracking-wider mb-1">
                Evidence
            </div>
            <dl className="grid grid-cols-[max-content_1fr] gap-x-3 gap-y-1 font-mono">
                {entries.map(([k, v]) => (
                    <FragmentRow key={k} k={k} v={v} />
                ))}
            </dl>
        </div>
    );
}

function FragmentRow({ k, v }: { k: string; v: unknown }) {
    const display =
        typeof v === "string" || typeof v === "number" || typeof v === "boolean"
            ? String(v)
            : JSON.stringify(v);
    return (
        <>
            <dt className="text-stone-500 dark:text-stone-400">{k}</dt>
            <dd className="text-stone-800 dark:text-stone-200 break-all">{display}</dd>
        </>
    );
}

// The expanded detail body shared by the inbox card and the dashboard
// modal: the agent's Markdown summary plus the structured evidence block.
export function AlertDetailBody({ alert }: { alert: Alert }) {
    return (
        <div className="space-y-3">
            {alert.summary && (
                <div className="text-stone-700 dark:text-stone-300 leading-relaxed">
                    <Markdown>{alert.summary}</Markdown>
                </div>
            )}
            {alert.evidence && <EvidenceBlock evidence={alert.evidence} />}
        </div>
    );
}

// Action row shared by inbox card and dashboard modal; `onActed` lets a host
// modal close itself after a navigating action (inbox card omits it).
export function AlertActions({ alert, onActed }: { alert: Alert; onActed?: () => void }) {
    const [acking, setAcking] = useState(false);
    const navigate = useNavigate();
    const th = thresholdEvidence(alert);
    const monitorHref = `/alerts/monitors?monitor=${encodeURIComponent(alert.monitor_id)}`;

    const handleAck = async () => {
        setAcking(true);
        try {
            await acknowledgeAlert(alert.id);
            notifyAlertsChanged();
            onActed?.();
        } catch (e: unknown) {
            window.alert(e instanceof Error ? e.message : String(e));
            setAcking(false);
        }
    };

    const viewResults = () => {
        if (!th) return;
        const end = new Date(alert.created_at).toISOString();
        const start = new Date(alert.created_at - th.windowSeconds * 1000).toISOString();
        const href = searchHref({
            q: th.query,
            range: DEFAULTS.range,
            follow: false,
            start,
            end,
            index: th.index ?? undefined,
            page: 1,
            env: alert.env || undefined,
        });
        if (alert.env && alert.env !== getEnv()) {
            setEnv(alert.env);
            window.location.assign(href);
        } else {
            navigate(href);
        }
        onActed?.();
    };

    return (
        <div className="flex items-center gap-2 flex-wrap">
            {alert.conversation_id ? (
                <button
                    type="button"
                    onClick={() => {
                        openConversationInEnv(alert.env, alert.conversation_id!);
                        onActed?.();
                    }}
                    className="inline-flex items-center gap-1 px-2.5 py-1 rounded-md border border-stone-200 dark:border-stone-700 text-stone-700 dark:text-stone-300 hover:border-orange-300 hover:bg-orange-50/40 dark:hover:bg-orange-950/30"
                    title="Open this monitor's investigation in the agent panel — review the trace and ask follow-up questions."
                >
                    <Sparkles className="w-3 h-3" />
                    Investigate
                </button>
            ) : (
                th && (
                    <>
                        <button
                            type="button"
                            onClick={viewResults}
                            className="inline-flex items-center gap-1 px-2.5 py-1 rounded-md border border-stone-200 dark:border-stone-700 text-stone-700 dark:text-stone-300 hover:border-orange-300 hover:bg-orange-50/40 dark:hover:bg-orange-950/30"
                            title="Open the search page with this monitor's query over the window it counted."
                        >
                            <SearchIcon className="w-3 h-3" />
                            View results
                        </button>
                        <button
                            type="button"
                            onClick={() => {
                                investigateInEnv(
                                    alert.env,
                                    buildThresholdInvestigatePrompt(alert, th),
                                );
                                onActed?.();
                            }}
                            className="inline-flex items-center gap-1 px-2.5 py-1 rounded-md border border-stone-200 dark:border-stone-700 text-stone-700 dark:text-stone-300 hover:border-orange-300 hover:bg-orange-50/40 dark:hover:bg-orange-950/30"
                            title="Open the AI panel in a fresh thread to investigate these results."
                        >
                            <Sparkles className="w-3 h-3" />
                            Investigate
                        </button>
                    </>
                )
            )}
            <button
                type="button"
                onClick={() => {
                    navigate(monitorHref);
                    onActed?.();
                }}
                className="inline-flex items-center gap-1 px-2.5 py-1 rounded-md border border-stone-200 dark:border-stone-700 text-stone-700 dark:text-stone-300 hover:border-orange-300 hover:bg-orange-50/40 dark:hover:bg-orange-950/30"
                title={`View the “${alert.monitor_name}” monitor`}
            >
                <Bell className="w-3 h-3" />
                View monitor
            </button>
            <div className="flex-grow" />
            {!alert.acknowledged ? (
                <button
                    type="button"
                    disabled={acking}
                    onClick={handleAck}
                    className="px-2.5 py-1 rounded-md bg-orange-600 hover:bg-orange-500 text-white disabled:opacity-60"
                >
                    {acking ? "Acknowledging…" : "Acknowledge"}
                </button>
            ) : (
                <span className="text-stone-400 dark:text-stone-500">
                    {alert.acknowledged_at
                        ? `Acknowledged ${timeAgo(new Date(alert.acknowledged_at).toISOString())}`
                        : "Acknowledged"}
                </span>
            )}
        </div>
    );
}
