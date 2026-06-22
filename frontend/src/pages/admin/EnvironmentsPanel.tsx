// Copyright 2026 Appbird LLC. Licensed under "GNU Affero General Public License v3.0"

// Admin → Environments: the admin-only env list + create/delete affordance.
// Delete is refused (backend-enforced) for reserved or non-empty envs.

import { useCallback, useEffect, useState } from "react";
import { Boxes, ChevronDown, ChevronUp, Star } from "lucide-react";
import {
    createEnv,
    deleteEnv,
    listEnvsWithDefault,
    reorderEnvs,
    setDefaultEnv,
    setEnvRetention,
    type EnvRow,
} from "../../api/client";
import { Card, ErrorBanner, HelpFrame } from "../../components/admin";

export function EnvironmentsPanel() {
    const [envs, setEnvs] = useState<EnvRow[]>([]);
    const [defaultEnv, setDefaultEnvState] = useState<string | null>(null);
    const [name, setName] = useState("");
    const [busy, setBusy] = useState(false);
    const [error, setError] = useState<string | null>(null);

    const refresh = useCallback(async () => {
        try {
            const { envs, defaultEnv } = await listEnvsWithDefault();
            setEnvs(envs);
            setDefaultEnvState(defaultEnv);
            setError(null);
        } catch (e) {
            setError(e instanceof Error ? e.message : String(e));
        }
    }, []);

    useEffect(() => {
        void refresh();
    }, [refresh]);

    const handleCreate = async () => {
        const trimmed = name.trim();
        if (!trimmed) return;
        if (!/^[A-Za-z0-9_-]+$/.test(trimmed)) {
            setError(
                "Env name becomes a folder on disk — use only letters, digits, '-' and '_' (no spaces, slashes, or other punctuation).",
            );
            return;
        }
        if (trimmed.startsWith("_")) {
            setError("Env names starting with '_' are reserved for the system.");
            return;
        }
        setBusy(true);
        setError(null);
        try {
            await createEnv(trimmed);
            setName("");
            await refresh();
        } catch (e) {
            setError(e instanceof Error ? e.message : String(e));
        } finally {
            setBusy(false);
        }
    };

    const handleDelete = async (n: string) => {
        if (!confirm(`Delete env "${n}"? Refuses unless empty.`)) return;
        setBusy(true);
        setError(null);
        try {
            await deleteEnv(n);
            await refresh();
        } catch (e) {
            setError(e instanceof Error ? e.message : String(e));
        } finally {
            setBusy(false);
        }
    };

    // Swap an env with its neighbour and persist the whole order.
    const handleMove = async (index: number, dir: -1 | 1) => {
        const target = index + dir;
        if (target < 0 || target >= envs.length) return;
        const next = envs.slice();
        [next[index], next[target]] = [next[target], next[index]];
        setEnvs(next); // optimistic
        setBusy(true);
        setError(null);
        try {
            await reorderEnvs(next.map((e) => e.name));
            await refresh();
        } catch (e) {
            setError(e instanceof Error ? e.message : String(e));
            await refresh(); // revert to server truth
        } finally {
            setBusy(false);
        }
    };

    // Toggle which env new users land on (clicking the current default clears it).
    const handleToggleDefault = async (n: string) => {
        setBusy(true);
        setError(null);
        try {
            await setDefaultEnv(defaultEnv === n ? null : n);
            await refresh();
        } catch (e) {
            setError(e instanceof Error ? e.message : String(e));
        } finally {
            setBusy(false);
        }
    };

    return (
        <Card title="Environments">
            <div className="p-6 space-y-4 max-w-3xl">
                <HelpFrame icon={<Boxes className="w-4 h-4" />}>
                    <p>
                        Environments are the top-level tenancy boundary — every index, saved search,
                        monitor, and agent conversation belongs to exactly one env. Switch envs from
                        the top-nav picker; deletion is allowed only when the env has no on-disk
                        partitions and no control-plane references.
                    </p>
                    <p className="mt-2">
                        Use the arrows to set the order envs appear in the picker, and the{" "}
                        <Star className="inline w-3.5 h-3.5 -mt-0.5" /> to mark the env new users
                        land on at first login (returning users keep their last-used env).
                    </p>
                </HelpFrame>
                <ErrorBanner error={error} />
                <div className="flex items-center gap-2">
                    <input
                        type="text"
                        value={name}
                        onChange={(e) => setName(e.target.value)}
                        onKeyDown={(e) => {
                            if (e.key === "Enter") void handleCreate();
                        }}
                        placeholder="new env name (e.g. dev, test, prod)"
                        className="flex-1 px-3 py-1.5 rounded-md border border-stone-200 dark:border-stone-700 bg-white dark:bg-stone-950 text-stone-900 dark:text-stone-100 font-mono"
                        disabled={busy}
                    />
                    <button
                        type="button"
                        onClick={() => void handleCreate()}
                        disabled={busy || !name.trim()}
                        className="px-3 py-1.5 font-medium rounded-md bg-orange-600 hover:bg-orange-500 text-white disabled:opacity-50 disabled:cursor-not-allowed transition"
                    >
                        Create
                    </button>
                </div>
                <p className="text-xs text-stone-700 dark:text-stone-300">
                    Becomes a folder on disk — letters, digits, <code className="font-mono">-</code>{" "}
                    and <code className="font-mono">_</code> only; no spaces or punctuation. Names
                    starting with <code className="font-mono">_</code> are reserved for the system.
                </p>
                <ul className="divide-y divide-stone-100 dark:divide-stone-800 border border-stone-200 dark:border-stone-800 rounded-md overflow-hidden">
                    {envs.length === 0 && (
                        <li className="px-4 py-3 text-stone-700 dark:text-stone-300 italic">
                            No user envs — everything lives under{" "}
                            <code className="font-mono">default</code>.
                        </li>
                    )}
                    {envs.map((e, i) => (
                        <li
                            key={e.name}
                            className="px-4 py-2 flex items-center gap-3 text-stone-700 dark:text-stone-300"
                        >
                            <div className="flex flex-col -my-1">
                                <button
                                    type="button"
                                    aria-label={`Move ${e.name} up`}
                                    title="Move up"
                                    onClick={() => void handleMove(i, -1)}
                                    disabled={busy || i === 0}
                                    className="text-stone-400 hover:text-stone-900 dark:hover:text-stone-100 disabled:opacity-30 disabled:cursor-not-allowed"
                                >
                                    <ChevronUp className="w-4 h-4" />
                                </button>
                                <button
                                    type="button"
                                    aria-label={`Move ${e.name} down`}
                                    title="Move down"
                                    onClick={() => void handleMove(i, 1)}
                                    disabled={busy || i === envs.length - 1}
                                    className="text-stone-400 hover:text-stone-900 dark:hover:text-stone-100 disabled:opacity-30 disabled:cursor-not-allowed"
                                >
                                    <ChevronDown className="w-4 h-4" />
                                </button>
                            </div>
                            <code className="font-mono font-semibold text-stone-900 dark:text-stone-100">
                                {e.name}
                            </code>
                            <button
                                type="button"
                                onClick={() => void handleToggleDefault(e.name)}
                                disabled={busy}
                                title={
                                    defaultEnv === e.name
                                        ? "Default for new users — click to clear"
                                        : "Set as default for new users"
                                }
                                className={`inline-flex items-center gap-1 px-1.5 py-0.5 rounded text-xs transition disabled:opacity-40 ${
                                    defaultEnv === e.name
                                        ? "text-orange-500 hover:text-orange-400"
                                        : "text-stone-400 hover:text-stone-700 dark:hover:text-stone-200"
                                }`}
                            >
                                <Star
                                    className="w-3.5 h-3.5"
                                    fill={defaultEnv === e.name ? "currentColor" : "none"}
                                />
                                {defaultEnv === e.name && (
                                    <span className="font-medium">default</span>
                                )}
                            </button>
                            <span className="text-stone-700 dark:text-stone-300">
                                created {e.created_at.slice(0, 10)}
                            </span>
                            <div className="flex-grow" />
                            <RetentionEditor
                                env={e}
                                busy={busy}
                                onSaved={() => void refresh()}
                                onError={setError}
                            />
                            {e.name !== "default" && (
                                <button
                                    type="button"
                                    onClick={() => void handleDelete(e.name)}
                                    disabled={busy}
                                    className="px-2 py-1 rounded-md border border-stone-200 dark:border-stone-700 text-stone-600 dark:text-stone-300 hover:border-red-300 hover:bg-red-50/40 dark:hover:bg-red-950/30 disabled:opacity-40 transition"
                                >
                                    delete
                                </button>
                            )}
                        </li>
                    ))}
                </ul>
                <p className="text-xs text-stone-700 dark:text-stone-300">
                    Retention drops day-partitions older than N days (hourly sweep; also via Admin →
                    Indexes → "Run cleanup now"). Empty = use the global default from Admin →
                    Indexes → Retention; a global default of 0 keeps data forever.
                </p>
            </div>
        </Card>
    );
}

// Inline per-env retention override editor: empty = inherit global default.
function RetentionEditor({
    env,
    busy,
    onSaved,
    onError,
}: {
    env: EnvRow;
    busy: boolean;
    onSaved: () => void;
    onError: (msg: string | null) => void;
}) {
    const [value, setValue] = useState(env.retention_days?.toString() ?? "");
    const [saving, setSaving] = useState(false);
    const saved = env.retention_days?.toString() ?? "";
    const dirty = value.trim() !== saved;

    const save = async () => {
        const trimmed = value.trim();
        const days = trimmed === "" ? null : parseInt(trimmed, 10);
        if (days !== null && (!Number.isFinite(days) || days < 1)) {
            onError("retention must be a whole number of days ≥ 1 (or empty to inherit)");
            return;
        }
        setSaving(true);
        onError(null);
        try {
            await setEnvRetention(env.name, days);
            onSaved();
        } catch (e) {
            onError(e instanceof Error ? e.message : String(e));
        } finally {
            setSaving(false);
        }
    };

    return (
        <span className="inline-flex items-center gap-1.5 text-xs">
            <span className="text-stone-500 dark:text-stone-400">retain</span>
            <input
                type="number"
                min={1}
                value={value}
                onChange={(e) => setValue(e.target.value)}
                onKeyDown={(e) => {
                    if (e.key === "Enter") void save();
                }}
                placeholder="∞"
                disabled={busy || saving}
                className="w-16 px-2 py-1 rounded-md border border-stone-200 dark:border-stone-700 bg-white dark:bg-stone-950 text-stone-900 dark:text-stone-100 font-mono tabular-nums"
            />
            <span className="text-stone-500 dark:text-stone-400">days</span>
            {dirty && (
                <button
                    type="button"
                    onClick={() => void save()}
                    disabled={busy || saving}
                    className="px-2 py-1 rounded-md bg-stone-900 dark:bg-stone-800 text-white disabled:opacity-50"
                >
                    {saving ? "…" : "set"}
                </button>
            )}
        </span>
    );
}
