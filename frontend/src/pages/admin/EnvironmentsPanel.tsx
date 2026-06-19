// Copyright 2026 Appbird LLC. Licensed under "GNU Affero General Public License v3.0"

// Admin → Environments: the admin-only env list + create/delete affordance.
// Delete is refused (backend-enforced) for reserved or non-empty envs.

import { useCallback, useEffect, useState } from "react";
import { Boxes } from "lucide-react";
import { createEnv, deleteEnv, listEnvs, setEnvRetention, type EnvRow } from "../../api/client";
import { Card, ErrorBanner, HelpFrame } from "../../components/admin";

export function EnvironmentsPanel() {
    const [envs, setEnvs] = useState<EnvRow[]>([]);
    const [name, setName] = useState("");
    const [busy, setBusy] = useState(false);
    const [error, setError] = useState<string | null>(null);

    const refresh = useCallback(async () => {
        try {
            setEnvs(await listEnvs());
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
                    {envs.map((e) => (
                        <li
                            key={e.name}
                            className="px-4 py-2 flex items-center gap-3 text-stone-700 dark:text-stone-300"
                        >
                            <code className="font-mono font-semibold text-stone-900 dark:text-stone-100">
                                {e.name}
                            </code>
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
