// Copyright 2026 Appbird LLC. Licensed under "GNU Affero General Public License v3.0"

import { useEffect, useState } from "react";
import { Check, Pencil, X } from "lucide-react";
import { createMonitor, listMonitors } from "../api/client";
import { notifyMonitorsChanged, onMonitorsChanged } from "../api/events";
import type { Monitor, MonitorInput } from "../api/types";
import type { AgentToolCallUI } from "../state/useAgentChat";
import { MonitorDialog } from "./MonitorDialog";

interface Props {
    call: AgentToolCallUI;
}

// Confirmation card for a create_monitor draft (the tool returns a draft, not
// a DB write); the user must confirm here before the monitor is scheduled.
export function MonitorDraftCard({ call }: Props) {
    const [state, setState] = useState<"draft" | "creating" | "created" | "dismissed">("draft");
    const [editing, setEditing] = useState(false);
    const [err, setErr] = useState<string | null>(null);
    // An existing monitor matching this draft's name, so reloads show "scheduled" not a re-offer.
    const [existing, setExisting] = useState<Monitor | null>(null);

    const draft = extractDraft(call);
    const draftName = draft?.name.trim().toLowerCase() ?? null;

    // One-shot lookup on mount (no polling), refreshed on monitors-changed events.
    useEffect(() => {
        if (!draftName) return;
        let cancelled = false;
        const check = () => {
            listMonitors()
                .then((xs) => {
                    if (cancelled) return;
                    setExisting(xs.find((m) => m.name.trim().toLowerCase() === draftName) ?? null);
                })
                .catch(() => {});
        };
        check();
        const off = onMonitorsChanged(check);
        return () => {
            cancelled = true;
            off();
        };
    }, [draftName]);

    if (!draft) {
        return (
            <div className="rounded-lg border border-stone-200 dark:border-stone-700 bg-white dark:bg-stone-900 px-3 py-2 text-stone-500 dark:text-stone-400">
                Monitor draft was malformed; nothing to confirm.
            </div>
        );
    }

    const handleCreate = async (input: MonitorInput) => {
        setState("creating");
        setErr(null);
        try {
            await createMonitor(input);
            notifyMonitorsChanged();
            setState("created");
            setEditing(false);
        } catch (e: unknown) {
            setErr(e instanceof Error ? e.message : String(e));
            setState("draft");
        }
    };

    if (state === "created") {
        return (
            <div className="rounded-lg border border-emerald-200 dark:border-emerald-900 bg-emerald-50 dark:bg-emerald-950/30 px-3 py-2 text-emerald-800 dark:text-emerald-200 flex items-center gap-2">
                <Check className="w-4 h-4 flex-shrink-0" />
                <span>
                    Created monitor <strong>{draft.name}</strong> — runs every{" "}
                    {formatInterval(draft.interval_seconds ?? 1800)}.
                </span>
            </div>
        );
    }

    if (state === "dismissed") {
        return (
            <div className="rounded-lg border border-stone-200 dark:border-stone-700 bg-stone-50 dark:bg-stone-800/40 px-3 py-2 text-stone-500 dark:text-stone-400 flex items-center gap-2">
                <X className="w-4 h-4 flex-shrink-0" />
                <span>Draft dismissed.</span>
            </div>
        );
    }

    // Reloaded conversation: this draft's monitor already exists, so show it as
    // scheduled rather than re-offering the Create/Edit/Dismiss actions.
    if (existing) {
        return (
            <div className="rounded-lg border border-emerald-200 dark:border-emerald-900 bg-emerald-50 dark:bg-emerald-950/30 px-3 py-2 text-emerald-800 dark:text-emerald-200 flex items-center gap-2">
                <Check className="w-4 h-4 flex-shrink-0" />
                <span>
                    Monitor <strong>{draft.name}</strong> is scheduled — runs every{" "}
                    {formatInterval(existing.interval_seconds)}.
                </span>
            </div>
        );
    }

    return (
        <div className="rounded-lg border border-orange-200 dark:border-orange-900 bg-orange-50/60 dark:bg-orange-950/20 overflow-hidden">
            <div className="px-3 py-2 border-b border-orange-200/70 dark:border-orange-900/60 bg-orange-100/60 dark:bg-orange-950/40 text-orange-900 dark:text-orange-200">
                <strong>Monitor draft</strong> — review and click Create to schedule, or Edit to
                adjust.
            </div>
            <div className="px-3 py-3 space-y-2">
                <Row label="Name" value={draft.name} />
                {draft.description && <Row label="Description" value={draft.description} />}
                <Row label="Interval" value={formatInterval(draft.interval_seconds ?? 1800)} />
                <div>
                    <div className="text-stone-500 dark:text-stone-400 uppercase tracking-wider mb-1">
                        Prompt
                    </div>
                    <pre className="font-mono whitespace-pre-wrap break-words text-stone-700 dark:text-stone-300 bg-white dark:bg-stone-900 border border-stone-200 dark:border-stone-700 rounded-md px-2 py-1.5 max-h-40 overflow-auto">
                        {draft.prompt}
                    </pre>
                </div>
                {err && (
                    <div className="px-2 py-1.5 rounded-md bg-red-50 text-red-800 border border-red-200 dark:bg-red-950 dark:text-red-200 dark:border-red-900">
                        {err}
                    </div>
                )}
                <div className="flex items-center gap-2 pt-1">
                    <button
                        type="button"
                        onClick={() =>
                            handleCreate({
                                name: draft.name,
                                description: draft.description ?? "",
                                prompt: draft.prompt,
                                interval_seconds: draft.interval_seconds ?? 1800,
                                enabled: true,
                            })
                        }
                        disabled={state === "creating"}
                        className="inline-flex items-center gap-1 px-2.5 py-1 rounded-md bg-orange-600 hover:bg-orange-500 text-white disabled:opacity-60"
                    >
                        <Check className="w-3.5 h-3.5" />
                        {state === "creating" ? "Creating…" : "Create monitor"}
                    </button>
                    <button
                        type="button"
                        onClick={() => setEditing(true)}
                        className="inline-flex items-center gap-1 px-2.5 py-1 rounded-md border border-stone-200 dark:border-stone-700 text-stone-700 dark:text-stone-300 hover:bg-stone-50 dark:hover:bg-stone-800"
                    >
                        <Pencil className="w-3.5 h-3.5" />
                        Edit first
                    </button>
                    <button
                        type="button"
                        onClick={() => setState("dismissed")}
                        className="inline-flex items-center gap-1 px-2.5 py-1 rounded-md text-stone-500 dark:text-stone-400 hover:text-stone-800 dark:hover:text-stone-200"
                    >
                        <X className="w-3.5 h-3.5" />
                        Dismiss
                    </button>
                </div>
            </div>

            {editing && (
                <MonitorDialog
                    monitor={null}
                    initial={{
                        name: draft.name,
                        description: draft.description ?? "",
                        prompt: draft.prompt,
                        interval_seconds: draft.interval_seconds ?? 1800,
                        enabled: true,
                    }}
                    onSave={handleCreate}
                    onClose={() => setEditing(false)}
                />
            )}
        </div>
    );
}

interface DraftShape {
    name: string;
    description?: string;
    prompt: string;
    interval_seconds?: number;
}

// Pulls the draft from the tool result, falling back to `arguments` mid-stream.
function extractDraft(call: AgentToolCallUI): DraftShape | null {
    const r = call.result as { status?: string; monitor?: DraftShape } | undefined;
    if (r && r.status === "draft" && r.monitor && r.monitor.name && r.monitor.prompt) {
        return r.monitor;
    }
    const a = call.arguments as unknown as DraftShape | undefined;
    if (a && a.name && a.prompt) return a;
    return null;
}

function formatInterval(seconds: number): string {
    if (seconds < 3600) return `${Math.round(seconds / 60)} min`;
    if (seconds < 86400) {
        const h = seconds / 3600;
        return Number.isInteger(h) ? `${h} hours` : `${h.toFixed(1)} hours`;
    }
    const d = seconds / 86400;
    return Number.isInteger(d) ? `${d} days` : `${d.toFixed(1)} days`;
}

function Row({ label, value }: { label: string; value: string }) {
    return (
        <div className="flex gap-3">
            <div className="text-stone-500 dark:text-stone-400 uppercase tracking-wider min-w-[5rem] flex-shrink-0 mt-0.5">
                {label}
            </div>
            <div className="text-stone-800 dark:text-stone-200 break-words">{value}</div>
        </div>
    );
}
