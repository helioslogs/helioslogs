// Copyright 2026 Appbird LLC. Licensed under "GNU Affero General Public License v3.0"

// Admin → Ingest tokens. Scoped push tokens authenticating external log shippers,
// plus the "require a token" master switch. Secrets are shown once at creation.

import { useCallback, useEffect, useState } from "react";
import { Copy, KeyRound, Plus, Trash2 } from "lucide-react";
import {
    createIngestToken,
    deleteIngestToken,
    getIngestTokens,
    listEnvs,
    setIngestRequire,
    setIngestTokenEnabled,
    type EnvRow,
} from "../../api/client";
import type { CreatedPushToken, IngestAuthConfig } from "../../api/types";
import { Card, Empty, ErrorBanner, HelpFrame, Th, Toast } from "../../components/admin";

const FIELD =
    "w-full px-2.5 py-1.5 bg-stone-50 dark:bg-stone-950 border border-stone-200 dark:border-stone-700 rounded-md focus:outline-none focus:border-orange-500 focus:bg-white dark:focus:bg-stone-900 text-stone-900 dark:text-stone-100";

export function PushTokensPanel() {
    const [cfg, setCfg] = useState<IngestAuthConfig | null>(null);
    const [error, setError] = useState<string | null>(null);
    const [toast, setToast] = useState<string | null>(null);
    const [reveal, setReveal] = useState<CreatedPushToken | null>(null);
    const [busy, setBusy] = useState(false);
    // create form
    const [name, setName] = useState("");
    const [env, setEnv] = useState("default");
    const [indexes, setIndexes] = useState("");
    const [envs, setEnvs] = useState<EnvRow[]>([]);

    const refresh = useCallback(async () => {
        try {
            setCfg(await getIngestTokens());
            setError(null);
        } catch (e) {
            setError(e instanceof Error ? e.message : String(e));
        }
    }, []);

    useEffect(() => {
        void refresh();
        listEnvs()
            .then(setEnvs)
            .catch(() => {});
    }, [refresh]);

    function flash(msg: string) {
        setToast(msg);
        setTimeout(() => setToast(null), 2500);
    }

    async function guard(fn: () => Promise<void>) {
        setBusy(true);
        setError(null);
        try {
            await fn();
            await refresh();
        } catch (e) {
            setError(e instanceof Error ? e.message : String(e));
        } finally {
            setBusy(false);
        }
    }

    async function handleCreate(e: React.FormEvent) {
        e.preventDefault();
        const idxs = indexes
            .split(/[\s,]+/)
            .map((x) => x.trim())
            .filter(Boolean);
        setBusy(true);
        setError(null);
        try {
            const created = await createIngestToken({
                name: name.trim(),
                env: env.trim(),
                indexes: idxs,
            });
            setReveal(created);
            setName("");
            setIndexes("");
            await refresh();
        } catch (e) {
            setError(e instanceof Error ? e.message : String(e));
        } finally {
            setBusy(false);
        }
    }

    return (
        <div>
            <Card title="Ingest tokens">
                <div className="p-6 space-y-4 max-w-4xl">
                    <HelpFrame icon={<KeyRound className="w-4 h-4" />}>
                        <p>
                            Scoped bearer tokens for external log shippers posting to{" "}
                            <code className="text-orange-700 dark:text-orange-300">
                                /api/ingest
                            </code>
                            , <code className="text-orange-700 dark:text-orange-300">/_bulk</code>,{" "}
                            <code className="text-orange-700 dark:text-orange-300">
                                /api/es/_bulk
                            </code>
                            , <code className="text-orange-700 dark:text-orange-300">/v1/logs</code>
                            ,{" "}
                            <code className="text-orange-700 dark:text-orange-300">
                                /api/otlp/*
                            </code>
                            ,{" "}
                            <code className="text-orange-700 dark:text-orange-300">
                                /services/collector/*
                            </code>
                            . A token pins one environment and, optionally, an index allowlist.
                            Shippers send{" "}
                            <code className="text-orange-700 dark:text-orange-300">
                                Authorization: Bearer &lt;token&gt;
                            </code>{" "}
                            .
                        </p>
                    </HelpFrame>

                    <ErrorBanner error={error} />

                    {/* require switch */}
                    <label className="flex items-center gap-2 text-stone-700 dark:text-stone-200">
                        <input
                            type="checkbox"
                            className="accent-orange-600"
                            checked={cfg?.require ?? false}
                            disabled={busy || !cfg}
                            onChange={(e) => void guard(() => setIngestRequire(e.target.checked))}
                        />
                        Require a valid token for all ingest (reject untokened requests)
                    </label>
                    {!cfg?.require && (
                        <p className="text-xs text-amber-700 dark:text-amber-400">
                            Ingest is currently <strong>open</strong> — anyone who can reach the
                            server can post logs. Enable the switch above (or front with a proxy)
                            before exposing to an untrusted network.
                        </p>
                    )}

                    {/* reveal-once banner */}
                    {reveal && (
                        <div className="px-4 py-3 rounded-lg border border-emerald-300 bg-emerald-50 dark:border-emerald-800 dark:bg-emerald-950/40">
                            <div className="font-medium text-emerald-900 dark:text-emerald-100 mb-1">
                                Token created — copy it now, it won't be shown again.
                            </div>
                            <div className="flex items-center gap-2">
                                <code className="flex-1 px-2 py-1 rounded bg-white dark:bg-stone-900 border border-emerald-200 dark:border-emerald-800 font-mono text-sm break-all">
                                    {reveal.token}
                                </code>
                                <button
                                    type="button"
                                    className="p-1.5 rounded-md hover:bg-emerald-100 dark:hover:bg-emerald-900"
                                    title="Copy"
                                    onClick={() => {
                                        void navigator.clipboard?.writeText(reveal.token);
                                        flash("Copied");
                                    }}
                                >
                                    <Copy className="w-4 h-4" />
                                </button>
                                <button
                                    type="button"
                                    className="px-2 py-1 text-sm rounded-md hover:bg-emerald-100 dark:hover:bg-emerald-900"
                                    onClick={() => setReveal(null)}
                                >
                                    Dismiss
                                </button>
                            </div>
                        </div>
                    )}

                    {/* create form */}
                    <form onSubmit={handleCreate} className="flex flex-wrap items-end gap-3">
                        <div className="flex-1 min-w-[12rem]">
                            <label className="block text-sm font-medium text-stone-600 dark:text-stone-300 mb-1">
                                Name
                            </label>
                            <input
                                className={FIELD}
                                value={name}
                                onChange={(e) => setName(e.target.value)}
                                placeholder="vector-prod"
                            />
                        </div>
                        <div className="w-40">
                            <label className="block text-sm font-medium text-stone-600 dark:text-stone-300 mb-1">
                                Env
                            </label>
                            <select
                                className={FIELD}
                                value={env}
                                onChange={(e) => setEnv(e.target.value)}
                            >
                                {/* Keep the current value selectable even before envs load. */}
                                {!envs.some((x) => x.name === env) && (
                                    <option value={env}>{env}</option>
                                )}
                                {envs.map((x) => (
                                    <option key={x.name} value={x.name}>
                                        {x.name}
                                    </option>
                                ))}
                            </select>
                        </div>
                        <div className="flex-1 min-w-[12rem]">
                            <label className="block text-sm font-medium text-stone-600 dark:text-stone-300 mb-1">
                                Index allowlist (optional, comma-separated)
                            </label>
                            <input
                                className={FIELD}
                                value={indexes}
                                onChange={(e) => setIndexes(e.target.value)}
                                placeholder="any index if blank"
                            />
                        </div>
                        <button
                            type="submit"
                            disabled={busy || !name.trim()}
                            className="px-3 py-1.5 font-medium text-white bg-orange-600 hover:bg-orange-500 rounded-md transition flex items-center gap-1.5 disabled:opacity-50"
                        >
                            <Plus className="w-4 h-4" />
                            Create token
                        </button>
                    </form>
                </div>

                {cfg && cfg.tokens.length === 0 ? (
                    <Empty>
                        <KeyRound className="w-5 h-5 inline mr-1.5 -mt-0.5" />
                        No tokens yet.
                    </Empty>
                ) : (
                    <table className="w-full">
                        <thead className="bg-stone-50 dark:bg-stone-950/40 border-y border-stone-200 dark:border-stone-800">
                            <tr>
                                <Th>Name</Th>
                                <Th>Token</Th>
                                <Th>Env</Th>
                                <Th>Indexes</Th>
                                <Th>Last used</Th>
                                <Th align="right">Actions</Th>
                            </tr>
                        </thead>
                        <tbody className="divide-y divide-stone-100 dark:divide-stone-800">
                            {cfg?.tokens.map((t) => (
                                <tr
                                    key={t.id}
                                    className="text-stone-800 dark:text-stone-200 hover:bg-stone-50/60 dark:hover:bg-stone-800/30"
                                >
                                    <td className="px-3 py-2 font-medium">
                                        {t.name}
                                        {!t.enabled && (
                                            <span className="ml-2 text-xs px-1.5 py-0.5 rounded bg-stone-200 dark:bg-stone-700 text-stone-600 dark:text-stone-300">
                                                disabled
                                            </span>
                                        )}
                                    </td>
                                    <td className="px-3 py-2 font-mono text-stone-700 dark:text-stone-300">
                                        {t.token_hint}
                                    </td>
                                    <td className="px-3 py-2">{t.env}</td>
                                    <td className="px-3 py-2 text-stone-700 dark:text-stone-300">
                                        {t.indexes.length ? t.indexes.join(", ") : "any"}
                                    </td>
                                    <td className="px-3 py-2 text-stone-700 dark:text-stone-300">
                                        {t.last_used_at
                                            ? new Date(t.last_used_at).toLocaleString()
                                            : "never"}
                                    </td>
                                    <td className="px-3 py-2">
                                        <div className="flex items-center justify-end gap-1">
                                            <button
                                                type="button"
                                                disabled={busy}
                                                onClick={() =>
                                                    void guard(() =>
                                                        setIngestTokenEnabled(t.id, !t.enabled),
                                                    )
                                                }
                                                className="px-2 py-1 text-xs rounded-md hover:bg-stone-200 dark:hover:bg-stone-700 disabled:opacity-40"
                                            >
                                                {t.enabled ? "Disable" : "Enable"}
                                            </button>
                                            <button
                                                type="button"
                                                disabled={busy}
                                                title="Delete"
                                                onClick={() => {
                                                    if (
                                                        !confirm(
                                                            `Delete token "${t.name}"? Shippers using it will break.`,
                                                        )
                                                    )
                                                        return;
                                                    void guard(() => deleteIngestToken(t.id));
                                                }}
                                                className="p-1.5 rounded-md hover:bg-stone-200 dark:hover:bg-stone-700 disabled:opacity-40"
                                            >
                                                <Trash2 className="w-4 h-4 text-red-600 dark:text-red-400" />
                                            </button>
                                        </div>
                                    </td>
                                </tr>
                            ))}
                        </tbody>
                    </table>
                )}
            </Card>
            <Toast message={toast} />
        </div>
    );
}
