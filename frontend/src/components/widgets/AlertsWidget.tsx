// Copyright 2026 Appbird LLC. Licensed under "GNU Affero General Public License v3.0"

import { useState } from "react";
import { Check, Sparkles } from "lucide-react";
import { acknowledgeAlert } from "../../api/client";
import { notifyAlertsChanged } from "../../api/events";
import { investigateInEnv, openConversationInEnv } from "../../lib/alertActions";
import { buildThresholdInvestigatePrompt, thresholdEvidence } from "../AlertDetail";
import { AlertDetailModal } from "../AlertDetailModal";
import type { Alert, Widget } from "../../api/types";
import { useAlerts } from "../../state/useAlerts";
import { formatTsForRow } from "../../lib/timezone";
import { useTimezone } from "../../state/timezone";

interface Props {
    widget: Widget;
    // false = inbox (unacked only); true = recent history (incl. acknowledged).
    history: boolean;
}

const SEV_BADGE: Record<string, string> = {
    high: "sev-badge-error",
    medium: "sev-badge-warn",
    low: "sev-badge-info",
};

// Alert inbox / history list with inline Acknowledge + Investigate actions.
// Reuses the shared `useAlerts` hook (self-polling) so it stays live.
export function AlertsWidget({ widget, history }: Props) {
    const { items, error } = useAlerts(!history);
    const tz = useTimezone();
    const limit = widget.limit || 10;
    // The alert whose detail modal is open (null = closed).
    const [detail, setDetail] = useState<Alert | null>(null);

    if (error) return <p className="text-sm text-red-600 dark:text-red-300">{error}</p>;
    if (items.length === 0) {
        return (
            <p className="text-sm text-stone-500 dark:text-stone-400">
                {history ? "no alerts yet" : "inbox clear — no unacknowledged alerts"}
            </p>
        );
    }

    return (
        <>
            <ul className="divide-y divide-stone-100 dark:divide-stone-800 -my-1">
                {items.slice(0, limit).map((a) => (
                    <AlertRow key={a.id} alert={a} tz={tz} onOpen={() => setDetail(a)} />
                ))}
            </ul>
            {detail && <AlertDetailModal alert={detail} onClose={() => setDetail(null)} />}
        </>
    );
}

function AlertRow({ alert: a, tz, onOpen }: { alert: Alert; tz: string; onOpen: () => void }) {
    const [acking, setAcking] = useState(false);

    const ack = async () => {
        setAcking(true);
        try {
            await acknowledgeAlert(a.id);
            notifyAlertsChanged();
        } catch (e: unknown) {
            window.alert(e instanceof Error ? e.message : String(e));
            setAcking(false);
        }
    };

    // AI alerts open their trace conversation; threshold alerts seed a fresh
    // investigation thread (reusing the same prompt the inbox builds).
    const investigate = () => {
        if (a.conversation_id) {
            openConversationInEnv(a.env, a.conversation_id);
            return;
        }
        const th = thresholdEvidence(a);
        investigateInEnv(
            a.env,
            th
                ? buildThresholdInvestigatePrompt(a, th)
                : `Investigate this alert and explain what's going on.\n\n- Alert: ${a.title}\n- Monitor: ${a.monitor_name}\n- Summary: ${a.summary}`,
        );
    };

    return (
        <li
            role="button"
            tabIndex={0}
            onClick={onOpen}
            onKeyDown={(e) => {
                if (e.key === "Enter" || e.key === " ") {
                    e.preventDefault();
                    onOpen();
                }
            }}
            className="group flex items-start gap-2 py-1.5 px-1 -mx-1 rounded cursor-pointer hover:bg-stone-50 dark:hover:bg-stone-800/60"
        >
            <span
                className={`shrink-0 mt-0.5 px-1.5 py-0.5 rounded text-[10px] font-medium uppercase ring-1 ring-inset ${
                    SEV_BADGE[a.severity] ?? "sev-badge-info"
                }`}
            >
                {a.severity}
            </span>
            <span className="min-w-0 flex-1">
                <span className="block text-sm text-stone-900 dark:text-stone-100 truncate group-hover:underline">
                    {a.title}
                </span>
                <span className="block text-xs text-stone-600 dark:text-stone-300 truncate">
                    {a.monitor_name} · {formatTsForRow(new Date(a.created_at).toISOString(), tz)}
                    {a.acknowledged && " · acked"}
                </span>
            </span>
            <span className="shrink-0 flex items-center gap-0.5">
                <button
                    type="button"
                    onClick={(e) => {
                        e.stopPropagation();
                        investigate();
                    }}
                    title="Investigate in the AI panel"
                    className="p-1 rounded text-stone-800 dark:text-stone-200 hover:text-orange-600 dark:hover:text-orange-400 hover:bg-orange-50 dark:hover:bg-orange-950/30"
                >
                    <Sparkles className="w-3.5 h-3.5" />
                </button>
                {!a.acknowledged && (
                    <button
                        type="button"
                        onClick={(e) => {
                            e.stopPropagation();
                            ack();
                        }}
                        disabled={acking}
                        title="Acknowledge"
                        className="p-1 rounded text-stone-800 dark:text-stone-200 hover:text-orange-600 dark:hover:text-orange-400 hover:bg-orange-50 dark:hover:bg-orange-950/30 disabled:opacity-50"
                    >
                        <Check className="w-3.5 h-3.5" />
                    </button>
                )}
            </span>
        </li>
    );
}
