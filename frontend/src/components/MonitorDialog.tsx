// Copyright 2026 Appbird LLC. Licensed under "GNU Affero General Public License v3.0"

import { useEffect, useRef, useState } from "react";
import { Globe, Info, Lock, Sparkles } from "lucide-react";
import type { Comparison, Monitor, MonitorInput, MonitorKind } from "../api/types";
import { QueryInput } from "./widgets/QueryInput";
import { useQuerySuggestData } from "../state/useQuerySuggestData";
import { useAgentEnabled } from "../state/useAgentEnabled";
import { useAuth } from "../state/useAuth";

interface Props {
    // `null` = create-new mode; non-null = edit mode (form prefilled).
    monitor: Monitor | null;
    // Optional initial values for create mode — used when the agent
    // drafts a monitor from chat and the user accepts the draft.
    initial?: Partial<MonitorInput>;
    onSave: (input: MonitorInput) => Promise<void>;
    onClose: () => void;
}

const PRESET_INTERVALS: { label: string; seconds: number }[] = [
    { label: "5 min", seconds: 300 },
    { label: "15 min", seconds: 900 },
    { label: "30 min", seconds: 1800 },
    { label: "1 hour", seconds: 3600 },
    { label: "6 hours", seconds: 6 * 3600 },
    { label: "24 hours", seconds: 24 * 3600 },
];

const WINDOWS: { value: number; label: string }[] = [
    { value: 60, label: "1 min" },
    { value: 300, label: "5 min" },
    { value: 900, label: "15 min" },
    { value: 1800, label: "30 min" },
    { value: 3600, label: "1 hour" },
    { value: 6 * 3600, label: "6 hours" },
    { value: 24 * 3600, label: "24 hours" },
];

const COMPARISONS: { value: Comparison; label: string }[] = [
    { value: "gt", label: "greater than (>)" },
    { value: "gte", label: "at least (≥)" },
    { value: "lt", label: "less than (<)" },
    { value: "lte", label: "at most (≤)" },
    { value: "eq", label: "equal to (=)" },
    { value: "neq", label: "not equal to (≠)" },
];

const SEVERITIES: { value: string; label: string }[] = [
    { value: "low", label: "low" },
    { value: "medium", label: "medium" },
    { value: "high", label: "high" },
];

const MIN_INTERVAL_SECONDS = 300;

export function MonitorDialog({ monitor, initial, onSave, onClose }: Props) {
    const isEdit = monitor !== null;
    const th = monitor?.threshold ?? initial?.threshold ?? undefined;

    // System-wide agent switch. `undefined` while loading → treat as enabled.
    const { enabled: agentEnabledRaw } = useAgentEnabled();
    const agentEnabled = agentEnabledRaw !== false;
    const isAdmin = !!useAuth().user?.is_admin;

    const [kind, setKind] = useState<MonitorKind>(monitor?.kind ?? initial?.kind ?? "threshold");
    const [name, setName] = useState(monitor?.name ?? initial?.name ?? "");
    const [description, setDescription] = useState(
        monitor?.description ?? initial?.description ?? "",
    );
    const [prompt, setPrompt] = useState(monitor?.prompt ?? initial?.prompt ?? "");
    // Threshold-monitor fields.
    const [query, setQuery] = useState(th?.query ?? "");
    const [index, setIndex] = useState(th?.index ?? "");
    const [windowSeconds, setWindowSeconds] = useState(th?.window_seconds ?? 900);
    const [comparison, setComparison] = useState<Comparison>(th?.comparison ?? "gt");
    const [threshold, setThreshold] = useState<number>(th?.threshold ?? 50);
    const [severity, setSeverity] = useState(th?.severity ?? "medium");

    // Per-monitor webhook override; empty URL = use the global alerting target.
    const [notifyUrl, setNotifyUrl] = useState(monitor?.notify?.webhook_url ?? "");
    const [notifyFormat, setNotifyFormat] = useState(monitor?.notify?.format ?? "generic");

    const [intervalSeconds, setIntervalSeconds] = useState(
        monitor?.interval_seconds ?? initial?.interval_seconds ?? 1800,
    );
    const [enabled, setEnabled] = useState(monitor?.enabled ?? initial?.enabled ?? true);
    // Default public — alerts are shared with the team unless made private.
    const [isPublic, setIsPublic] = useState(monitor?.public ?? initial?.public ?? true);
    const [saving, setSaving] = useState(false);
    const [err, setErr] = useState<string | null>(null);
    const firstFieldRef = useRef<HTMLInputElement>(null);
    const suggest = useQuerySuggestData();

    useEffect(() => {
        firstFieldRef.current?.focus();
    }, []);

    // Close on Escape — convention matches the existing edit-search dialog.
    useEffect(() => {
        const onKey = (e: KeyboardEvent) => {
            if (e.key === "Escape") onClose();
        };
        window.addEventListener("keydown", onKey);
        return () => window.removeEventListener("keydown", onKey);
    }, [onClose]);

    const submit = async (e: React.FormEvent) => {
        e.preventDefault();
        if (!name.trim()) {
            setErr("name is required");
            return;
        }
        const clampedInterval = Math.max(MIN_INTERVAL_SECONDS, intervalSeconds);
        let input: MonitorInput;
        if (kind === "ai") {
            if (!prompt.trim()) {
                setErr("agent prompt is required");
                return;
            }
            input = {
                name: name.trim(),
                description: description.trim(),
                kind: "ai",
                prompt: prompt.trim(),
                interval_seconds: clampedInterval,
                enabled,
                public: isPublic,
                notify: { webhook_url: notifyUrl.trim(), format: notifyFormat },
            };
        } else {
            if (!Number.isFinite(threshold) || threshold < 0) {
                setErr("threshold must be a number ≥ 0");
                return;
            }
            input = {
                name: name.trim(),
                description: description.trim(),
                kind: "threshold",
                interval_seconds: clampedInterval,
                enabled,
                public: isPublic,
                notify: { webhook_url: notifyUrl.trim(), format: notifyFormat },
                threshold: {
                    query: query.trim() || "*",
                    index: index.trim() || null,
                    window_seconds: Math.max(60, windowSeconds),
                    comparison,
                    threshold,
                    severity,
                },
            };
        }
        setSaving(true);
        setErr(null);
        try {
            await onSave(input);
        } catch (e: unknown) {
            setErr(e instanceof Error ? e.message : String(e));
            setSaving(false);
        }
    };

    return (
        <div
            className="fixed inset-0 z-50 flex items-center justify-center bg-stone-900/50 dark:bg-black/60"
            onClick={onClose}
        >
            <form
                onSubmit={submit}
                onClick={(e) => e.stopPropagation()}
                className="bg-white dark:bg-stone-900 rounded-xl border border-stone-200 dark:border-stone-700 shadow-2xl w-full max-w-2xl mx-4 max-h-[90vh] overflow-auto"
            >
                <div className="px-5 py-3 border-b border-stone-200 dark:border-stone-800 flex items-center justify-between sticky top-0 bg-white dark:bg-stone-900 z-10">
                    <h2 className="font-semibold text-stone-900 dark:text-stone-100">
                        {isEdit ? "Edit monitor" : "New monitor"}
                    </h2>
                    <button
                        type="button"
                        onClick={onClose}
                        className="text-stone-400 hover:text-stone-700 dark:hover:text-stone-200"
                        aria-label="Close"
                    >
                        ✕
                    </button>
                </div>

                <div className="p-5 space-y-4">
                    <Field label="Type">
                        <div className="grid grid-cols-2 gap-2">
                            <KindOption
                                active={kind === "threshold"}
                                onClick={() => setKind("threshold")}
                                title="Threshold"
                                blurb="Count search results over a window and alert when they cross a number. Deterministic, no LLM."
                            />
                            <KindOption
                                active={kind === "ai"}
                                onClick={() => setKind("ai")}
                                title="AI Monitor"
                                blurb={
                                    "An agent runs your prompt each interval, investigates with tools, and decides whether to alert."
                                }
                            />
                        </div>
                    </Field>

                    {kind === "ai" && !agentEnabled ? (
                        <AiDisabledNotice isAdmin={isAdmin} />
                    ) : (
                        <>
                            <Field label="Name" required>
                                <input
                                    ref={firstFieldRef}
                                    type="text"
                                    className={FIELD}
                                    value={name}
                                    onChange={(e) => setName(e.target.value)}
                                    placeholder={
                                        kind === "threshold"
                                            ? "orders-api 5xx threshold"
                                            : "orders-api error spike watcher"
                                    }
                                />
                            </Field>

                            <Field
                                label="Description"
                                hint="optional — surfaced on hover in the monitors list"
                            >
                                <input
                                    type="text"
                                    className={FIELD}
                                    value={description}
                                    onChange={(e) => setDescription(e.target.value)}
                                    placeholder="what this monitor watches and why"
                                />
                            </Field>

                            {kind === "ai" ? (
                                <Field
                                    label="Agent prompt"
                                    required
                                    hint="Instruction handed to the agent every interval. Be specific about what to check, what the baseline is, and the alert criteria."
                                >
                                    <textarea
                                        className={`${FIELD} font-mono min-h-[160px] resize-y`}
                                        value={prompt}
                                        onChange={(e) => setPrompt(e.target.value)}
                                        placeholder={
                                            "e.g. Every 30 minutes, check the error rate in index:orders-api against the prior hour baseline. Use discover_fields if needed to find the level field. Raise an alert (severity high) if the current 5-min window shows >2x the baseline error rate, including which services are most affected."
                                        }
                                    />
                                </Field>
                            ) : (
                                <>
                                    <Field
                                        label="Query"
                                        required
                                        hint="What to count. Pipelined — use index:foo, level:error, status_code:>=500, etc."
                                    >
                                        <QueryInput
                                            value={query}
                                            onChange={setQuery}
                                            fields={suggest.fields}
                                            indexes={suggest.indexes}
                                            start={suggest.start}
                                            end={suggest.end}
                                            placeholder="status_code:>=500"
                                            className={`${FIELD} font-mono`}
                                        />
                                    </Field>

                                    <div className="grid grid-cols-2 gap-4">
                                        <Field label="Index" hint="optional — narrows to one index">
                                            <input
                                                type="text"
                                                className={`${FIELD} font-mono`}
                                                value={index}
                                                onChange={(e) => setIndex(e.target.value)}
                                                placeholder="all indexes"
                                            />
                                        </Field>
                                        <Field
                                            label="Severity"
                                            hint="stamped on the alert it raises"
                                        >
                                            <select
                                                className={FIELD}
                                                value={severity}
                                                onChange={(e) => setSeverity(e.target.value)}
                                            >
                                                {SEVERITIES.map((s) => (
                                                    <option key={s.value} value={s.value}>
                                                        {s.label}
                                                    </option>
                                                ))}
                                            </select>
                                        </Field>
                                    </div>

                                    <Field
                                        label="Condition"
                                        hint="Evaluated every interval over the trailing window. Alerts fire once when the condition starts holding, and re-arm when it clears."
                                    >
                                        <div className="flex items-center gap-2 flex-wrap text-stone-700 dark:text-stone-300">
                                            <span>Alert when the count is</span>
                                            <select
                                                className={`${FIELD} w-auto`}
                                                value={comparison}
                                                onChange={(e) =>
                                                    setComparison(e.target.value as Comparison)
                                                }
                                            >
                                                {COMPARISONS.map((c) => (
                                                    <option key={c.value} value={c.value}>
                                                        {c.label}
                                                    </option>
                                                ))}
                                            </select>
                                            <input
                                                type="number"
                                                min={0}
                                                className={`${FIELD} w-28`}
                                                value={Number.isFinite(threshold) ? threshold : ""}
                                                onChange={(e) =>
                                                    setThreshold(parseInt(e.target.value, 10))
                                                }
                                            />
                                            <span>over the last</span>
                                            <select
                                                className={`${FIELD} w-auto`}
                                                value={windowSeconds}
                                                onChange={(e) =>
                                                    setWindowSeconds(parseInt(e.target.value, 10))
                                                }
                                            >
                                                {WINDOWS.map((w) => (
                                                    <option key={w.value} value={w.value}>
                                                        {w.label}
                                                    </option>
                                                ))}
                                            </select>
                                        </div>
                                    </Field>
                                </>
                            )}

                            <Field
                                label="Webhook override"
                                hint="optional — replaces the global alert webhook for this monitor"
                            >
                                <div className="grid grid-cols-[1fr_auto] gap-2">
                                    <input
                                        type="url"
                                        className={FIELD}
                                        value={notifyUrl}
                                        onChange={(e) => setNotifyUrl(e.target.value)}
                                        placeholder="https://hooks.slack.com/services/… (empty = global target)"
                                    />
                                    <select
                                        className={FIELD}
                                        value={notifyFormat ?? "generic"}
                                        onChange={(e) =>
                                            setNotifyFormat(e.target.value as "generic" | "slack")
                                        }
                                    >
                                        <option value="generic">Generic JSON</option>
                                        <option value="slack">Slack</option>
                                    </select>
                                </div>
                            </Field>

                            <div className="grid grid-cols-2 gap-4">
                                <Field label="Interval" hint="how often the monitor checks">
                                    <div className="space-y-2">
                                        <select
                                            className={FIELD}
                                            value={
                                                PRESET_INTERVALS.some(
                                                    (p) => p.seconds === intervalSeconds,
                                                )
                                                    ? intervalSeconds
                                                    : "custom"
                                            }
                                            onChange={(e) => {
                                                const v = e.target.value;
                                                if (v !== "custom")
                                                    setIntervalSeconds(parseInt(v, 10));
                                            }}
                                        >
                                            {PRESET_INTERVALS.map((p) => (
                                                <option key={p.seconds} value={p.seconds}>
                                                    {p.label}
                                                </option>
                                            ))}
                                            <option value="custom">custom…</option>
                                        </select>
                                        {!PRESET_INTERVALS.some(
                                            (p) => p.seconds === intervalSeconds,
                                        ) && (
                                            <div className="flex items-center gap-2">
                                                <input
                                                    type="number"
                                                    min={MIN_INTERVAL_SECONDS}
                                                    className={`${FIELD} w-32`}
                                                    value={intervalSeconds}
                                                    onChange={(e) =>
                                                        setIntervalSeconds(
                                                            parseInt(e.target.value, 10) || 0,
                                                        )
                                                    }
                                                />
                                                <span className="text-stone-500 dark:text-stone-400">
                                                    seconds (min 300)
                                                </span>
                                            </div>
                                        )}
                                    </div>
                                </Field>

                                <Field label="Visibility">
                                    <div className="flex gap-2">
                                        <VisibilityOption
                                            active={isPublic}
                                            onClick={() => setIsPublic(true)}
                                            icon={<Globe className="w-3.5 h-3.5" />}
                                            title="Public"
                                            detail="All users see its alerts; anyone can acknowledge."
                                        />
                                        <VisibilityOption
                                            active={!isPublic}
                                            onClick={() => setIsPublic(false)}
                                            icon={<Lock className="w-3.5 h-3.5" />}
                                            title="Private"
                                            detail="Only you see and acknowledge its alerts."
                                        />
                                    </div>
                                </Field>

                                <Field label="Status">
                                    <label className="flex items-center gap-2 cursor-pointer select-none">
                                        <input
                                            type="checkbox"
                                            checked={enabled}
                                            onChange={(e) => setEnabled(e.target.checked)}
                                            className="rounded border-stone-300 text-orange-500 focus:ring-orange-500"
                                        />
                                        <span className="text-stone-700 dark:text-stone-300">
                                            {enabled
                                                ? "Enabled — will run on schedule"
                                                : "Paused — won't run"}
                                        </span>
                                    </label>
                                </Field>
                            </div>
                        </>
                    )}

                    {err && (
                        <div className="px-3 py-2 rounded-md bg-red-50 text-red-800 border border-red-200 dark:bg-red-950 dark:text-red-200 dark:border-red-900">
                            {err}
                        </div>
                    )}
                </div>

                <div className="px-5 py-3 border-t border-stone-200 dark:border-stone-800 flex items-center justify-end gap-2 sticky bottom-0 bg-white dark:bg-stone-900">
                    <button
                        type="button"
                        onClick={onClose}
                        className="px-3 py-1.5 rounded-md border border-stone-200 dark:border-stone-700 text-stone-700 dark:text-stone-300 hover:bg-stone-50 dark:hover:bg-stone-800"
                    >
                        Cancel
                    </button>
                    <button
                        type="submit"
                        disabled={saving || (kind === "ai" && !agentEnabled)}
                        className="px-3 py-1.5 rounded-md bg-orange-600 hover:bg-orange-500 text-white font-medium disabled:opacity-60 disabled:cursor-not-allowed"
                    >
                        {saving ? "Saving…" : isEdit ? "Save changes" : "Create monitor"}
                    </button>
                </div>
            </form>
        </div>
    );
}

const FIELD =
    "w-full px-2.5 py-1.5 bg-stone-50 dark:bg-stone-950 border border-stone-200 dark:border-stone-700 rounded-md focus:outline-none focus:border-orange-500 focus:bg-white dark:focus:bg-stone-900 text-stone-900 dark:text-stone-100";

// Shown in place of the agent-prompt field when AI features are turned off.
function AiDisabledNotice({ isAdmin }: { isAdmin: boolean }) {
    return (
        <div className="flex flex-col items-center text-center px-4 py-8 rounded-lg border border-dashed border-stone-300 dark:border-stone-700 bg-stone-50/60 dark:bg-stone-950/40">
            <div className="w-9 h-9 rounded-lg bg-stone-100 dark:bg-stone-800 flex items-center justify-center text-stone-400 dark:text-stone-500 mb-2.5">
                <Sparkles className="w-4.5 h-4.5" />
            </div>
            <div className="font-medium text-stone-800 dark:text-stone-100">
                AI monitors are disabled
            </div>
            <p className="mt-1.5 max-w-sm text-stone-500 dark:text-stone-400 leading-relaxed">
                AI agent functionality is turned off until an administrator enables an LLM provider.
                Use a <strong>Threshold</strong> monitor instead, or enable the agent first.
            </p>
            {isAdmin && (
                <a
                    href="/admin/agent"
                    className="mt-3 text-orange-600 hover:text-orange-500 dark:text-orange-400 underline"
                >
                    Configure the LLM provider
                </a>
            )}
        </div>
    );
}

// One of the two monitor-type cards at the top of the dialog.
function KindOption({
    active,
    onClick,
    title,
    blurb,
}: {
    active: boolean;
    onClick: () => void;
    title: string;
    blurb: string;
}) {
    return (
        <button
            type="button"
            onClick={onClick}
            className={`text-left p-3 rounded-lg border transition ${
                active
                    ? "border-orange-400 bg-orange-50/60 dark:border-orange-600 dark:bg-orange-950/30"
                    : "border-stone-200 dark:border-stone-700 hover:border-stone-300 dark:hover:border-stone-600"
            }`}
        >
            <div className="font-medium text-stone-900 dark:text-stone-100">{title}</div>
            <div className="text-stone-600 dark:text-stone-400 mt-0.5 leading-snug">{blurb}</div>
        </button>
    );
}

// One side of the public/private segmented control.
function VisibilityOption({
    active,
    onClick,
    icon,
    title,
    detail,
}: {
    active: boolean;
    onClick: () => void;
    icon: React.ReactNode;
    title: string;
    detail: string;
}) {
    return (
        <button
            type="button"
            onClick={onClick}
            className={`flex-1 text-left px-3 py-2 rounded-md border transition ${
                active
                    ? "border-orange-500 bg-orange-50 dark:bg-orange-950/30"
                    : "border-stone-200 dark:border-stone-700 hover:bg-stone-50 dark:hover:bg-stone-800"
            }`}
        >
            <div className="flex items-center gap-1.5 font-medium text-stone-800 dark:text-stone-200">
                {icon}
                {title}
            </div>
            <div className="text-xs text-stone-500 dark:text-stone-400 mt-0.5">{detail}</div>
        </button>
    );
}

function Field({
    label,
    hint,
    required,
    children,
}: {
    label: string;
    hint?: string;
    required?: boolean;
    children: React.ReactNode;
}) {
    return (
        <label className="block">
            <div className="flex items-center gap-1.5 mb-1">
                <span className="font-semibold text-stone-800 dark:text-stone-200">
                    {label}
                    {required && (
                        <span className="text-orange-600 dark:text-orange-400 ml-1">*</span>
                    )}
                </span>
                {hint && <InfoTip text={hint} />}
            </div>
            {children}
        </label>
    );
}

// Small info affordance: the per-field guidance now lives in a hover/focus
// tooltip instead of an always-on line, so the form stays compact.
function InfoTip({ text }: { text: string }) {
    return (
        <span
            tabIndex={0}
            title={text}
            aria-label={text}
            className="text-stone-400 hover:text-stone-600 dark:hover:text-stone-300 cursor-help"
            // Don't let clicking the icon toggle the wrapping <label>'s control.
            onClick={(e) => e.preventDefault()}
        >
            <Info className="w-3.5 h-3.5" />
        </span>
    );
}
