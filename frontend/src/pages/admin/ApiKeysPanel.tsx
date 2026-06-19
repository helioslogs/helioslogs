// Copyright 2026 Appbird LLC. Licensed under "GNU Affero General Public License v3.0"

// Admin → API keys. Long-lived bearer secrets for remote access. Each key carries
// a multi-select scope (standard API, admin API, MCP server). Secrets are shown
// once at creation.

import { useCallback, useEffect, useState, type FormEvent } from "react";
import { Copy, KeySquare, Plus, ShieldAlert, Trash2 } from "lucide-react";
import { createApiKey, deleteApiKey, listApiKeys, setApiKeyEnabled } from "../../api/client";
import type { ApiKeyScopes, ApiKeyView, CreatedApiKey } from "../../api/types";
import { Card, Empty, ErrorBanner, HelpFrame, Th, Toast } from "../../components/admin";

const FIELD =
    "w-full px-2.5 py-1.5 bg-stone-50 dark:bg-stone-950 border border-stone-200 dark:border-stone-700 rounded-md focus:outline-none focus:border-orange-500 focus:bg-white dark:focus:bg-stone-900 text-stone-900 dark:text-stone-100";

const EXPIRY_OPTIONS: { label: string; days: number | null }[] = [
    { label: "Never", days: null },
    { label: "30 days", days: 30 },
    { label: "90 days", days: 90 },
    { label: "1 year", days: 365 },
];

const SCOPE_OPTIONS: { key: keyof ApiKeyScopes; label: string; hint: string }[] = [
    {
        key: "api",
        label: "Standard API",
        hint: "Non-admin /api/* — same access as a regular user.",
    },
    {
        key: "admin",
        label: "Admin API",
        hint: "Full admin /api/admin/* access. Implies Standard API.",
    },
    { key: "mcp", label: "MCP server", hint: "Run tool calls against the /mcp endpoint." },
];

const NO_SCOPES: ApiKeyScopes = { api: false, admin: false, mcp: false };

export function ApiKeysPanel() {
    const [keys, setKeys] = useState<ApiKeyView[] | null>(null);
    const [error, setError] = useState<string | null>(null);
    const [toast, setToast] = useState<string | null>(null);
    const [reveal, setReveal] = useState<CreatedApiKey | null>(null);
    const [busy, setBusy] = useState(false);
    const [showDialog, setShowDialog] = useState(false);

    const refresh = useCallback(async () => {
        try {
            setKeys(await listApiKeys());
            setError(null);
        } catch (e) {
            setError(e instanceof Error ? e.message : String(e));
        }
    }, []);

    useEffect(() => {
        void refresh();
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

    return (
        <div>
            <Card title="API keys">
                <div className="p-6 space-y-4 max-w-4xl">
                    <HelpFrame icon={<KeySquare className="w-4 h-4" />}>
                        <p>
                            Long-lived bearer tokens for reaching HeliosLogs from external
                            integrations. Callers send{" "}
                            <code className="text-orange-700 dark:text-orange-300">
                                Authorization: Bearer &lt;token&gt;
                            </code>
                            . Each key's <strong>scope</strong> controls what it can reach: the
                            standard (non-admin) API, the admin API, and/or the{" "}
                            <strong>MCP server</strong>. Different users get their own MCP key, so
                            every MCP action is attributable to a key.
                        </p>
                    </HelpFrame>

                    <ErrorBanner error={error} />

                    {/* reveal-once banner */}
                    {reveal && (
                        <div className="px-4 py-3 rounded-lg border border-emerald-300 bg-emerald-50 dark:border-emerald-800 dark:bg-emerald-950/40">
                            <div className="font-medium text-emerald-900 dark:text-emerald-100 mb-1">
                                Key "{reveal.name}" created — copy it now, it won't be shown again.
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

                    <div>
                        <button
                            type="button"
                            onClick={() => setShowDialog(true)}
                            disabled={busy}
                            className="px-3 py-1.5 font-medium text-white bg-orange-600 hover:bg-orange-500 rounded-md transition flex items-center gap-1.5 disabled:opacity-50"
                        >
                            <Plus className="w-4 h-4" />
                            New API Key
                        </button>
                    </div>
                </div>

                {keys && keys.length === 0 ? (
                    <Empty>
                        <KeySquare className="w-5 h-5 inline mr-1.5 -mt-0.5" />
                        No API keys yet.
                    </Empty>
                ) : (
                    <table className="w-full">
                        <thead className="bg-stone-50 dark:bg-stone-950/40 border-y border-stone-200 dark:border-stone-800">
                            <tr>
                                <Th>Name</Th>
                                <Th>Key</Th>
                                <Th>Scope</Th>
                                <Th>Last used</Th>
                                <Th>Expires</Th>
                                <Th align="right">Actions</Th>
                            </tr>
                        </thead>
                        <tbody className="divide-y divide-stone-100 dark:divide-stone-800">
                            {keys?.map((k) => (
                                <tr
                                    key={k.id}
                                    className="text-stone-800 dark:text-stone-200 hover:bg-stone-50/60 dark:hover:bg-stone-800/30"
                                >
                                    <td className="px-3 py-2 font-medium">
                                        {k.name}
                                        {!k.enabled && (
                                            <span className="ml-2 text-xs px-1.5 py-0.5 rounded bg-stone-200 dark:bg-stone-700 text-stone-600 dark:text-stone-300">
                                                disabled
                                            </span>
                                        )}
                                        {k.description && (
                                            <div className="text-xs font-normal text-stone-500 dark:text-stone-400">
                                                {k.description}
                                            </div>
                                        )}
                                    </td>
                                    <td className="px-3 py-2 font-mono text-stone-700 dark:text-stone-300">
                                        {k.token_hint}
                                    </td>
                                    <td className="px-3 py-2">
                                        <ScopeBadges scopes={k.scopes} />
                                    </td>
                                    <td className="px-3 py-2 text-stone-700 dark:text-stone-300">
                                        {k.last_used_at
                                            ? new Date(k.last_used_at).toLocaleString()
                                            : "never"}
                                    </td>
                                    <td className="px-3 py-2 text-stone-700 dark:text-stone-300">
                                        {k.expires_at
                                            ? new Date(k.expires_at).toLocaleDateString()
                                            : "never"}
                                    </td>
                                    <td className="px-3 py-2">
                                        <div className="flex items-center justify-end gap-1">
                                            <button
                                                type="button"
                                                disabled={busy}
                                                onClick={() =>
                                                    void guard(() =>
                                                        setApiKeyEnabled(k.id, !k.enabled),
                                                    )
                                                }
                                                className="px-2 py-1 text-xs rounded-md hover:bg-stone-200 dark:hover:bg-stone-700 disabled:opacity-40"
                                            >
                                                {k.enabled ? "Disable" : "Enable"}
                                            </button>
                                            <button
                                                type="button"
                                                disabled={busy}
                                                title="Delete"
                                                onClick={() => {
                                                    if (
                                                        !confirm(
                                                            `Delete key "${k.name}"? Integrations using it will break.`,
                                                        )
                                                    )
                                                        return;
                                                    void guard(() => deleteApiKey(k.id));
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
            {showDialog && (
                <NewApiKeyDialog
                    onClose={() => setShowDialog(false)}
                    onCreated={(created) => {
                        setReveal(created);
                        setShowDialog(false);
                        void refresh();
                    }}
                />
            )}
            <Toast message={toast} />
        </div>
    );
}

const SCOPE_BADGE: Record<keyof ApiKeyScopes, { label: string; cls: string }> = {
    admin: {
        label: "admin",
        cls: "bg-amber-100 dark:bg-amber-950/50 text-amber-800 dark:text-amber-300",
    },
    api: {
        label: "api",
        cls: "bg-sky-100 dark:bg-sky-950/50 text-sky-800 dark:text-sky-300",
    },
    mcp: {
        label: "mcp",
        cls: "bg-violet-100 dark:bg-violet-950/50 text-violet-800 dark:text-violet-300",
    },
};

function ScopeBadges({ scopes }: { scopes: ApiKeyScopes }) {
    const active = (Object.keys(SCOPE_BADGE) as (keyof ApiKeyScopes)[]).filter((k) => scopes[k]);
    if (active.length === 0) {
        return <span className="text-xs text-stone-500 dark:text-stone-400">none</span>;
    }
    return (
        <span className="flex flex-wrap gap-1">
            {active.map((k) => (
                <span key={k} className={`text-xs px-1.5 py-0.5 rounded ${SCOPE_BADGE[k].cls}`}>
                    {SCOPE_BADGE[k].label}
                </span>
            ))}
        </span>
    );
}

// Create modal: name + purpose, multi-select scope, and expiry.
function NewApiKeyDialog({
    onClose,
    onCreated,
}: {
    onClose: () => void;
    onCreated: (created: CreatedApiKey) => void;
}) {
    const [name, setName] = useState("");
    const [description, setDescription] = useState("");
    const [scopes, setScopes] = useState<ApiKeyScopes>(NO_SCOPES);
    const [expiryDays, setExpiryDays] = useState<number | null>(null);
    const [busy, setBusy] = useState(false);
    const [error, setError] = useState<string | null>(null);

    const anyScope = scopes.api || scopes.admin || scopes.mcp;

    function toggle(key: keyof ApiKeyScopes) {
        setScopes((s) => ({ ...s, [key]: !s[key] }));
    }

    async function submit(e: FormEvent) {
        e.preventDefault();
        if (!name.trim() || !anyScope) return;
        setBusy(true);
        setError(null);
        try {
            const created = await createApiKey({
                name: name.trim(),
                description: description.trim(),
                scopes,
                expires_in_days: expiryDays,
            });
            onCreated(created);
        } catch (err) {
            setError(err instanceof Error ? err.message : String(err));
        } finally {
            setBusy(false);
        }
    }

    return (
        <div
            className="fixed inset-0 z-50 flex items-center justify-center bg-stone-900/50 dark:bg-black/60"
            onClick={onClose}
        >
            <form
                onSubmit={submit}
                onClick={(e) => e.stopPropagation()}
                className="bg-white dark:bg-stone-900 rounded-xl border border-stone-200 dark:border-stone-700 shadow-2xl w-full max-w-lg mx-4 max-h-[90vh] overflow-auto"
            >
                <div className="px-5 py-3 border-b border-stone-200 dark:border-stone-800 flex items-center justify-between">
                    <h2 className="font-semibold text-stone-900 dark:text-stone-100">
                        New API Key
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
                    {error && (
                        <div className="px-3 py-2 rounded-md bg-red-50 dark:bg-red-950/30 text-red-700 dark:text-red-300 border border-red-200 dark:border-red-900/50">
                            {error}
                        </div>
                    )}

                    <div>
                        <label className="block text-sm font-medium text-stone-600 dark:text-stone-300 mb-1">
                            Name
                        </label>
                        <input
                            autoFocus
                            className={FIELD}
                            value={name}
                            onChange={(e) => setName(e.target.value)}
                            placeholder="e.g. ci-pipeline or jane's laptop"
                        />
                    </div>

                    <div>
                        <label className="block text-sm font-medium text-stone-600 dark:text-stone-300 mb-1">
                            Purpose <span className="text-stone-400">(optional)</span>
                        </label>
                        <input
                            className={FIELD}
                            value={description}
                            onChange={(e) => setDescription(e.target.value)}
                            placeholder="what this key is for"
                        />
                    </div>

                    <div>
                        <span className="block text-sm font-medium text-stone-600 dark:text-stone-300 mb-1.5">
                            Scope
                        </span>
                        <div className="space-y-2">
                            {SCOPE_OPTIONS.map((opt) => (
                                <label
                                    key={opt.key}
                                    className="flex items-start gap-2.5 px-3 py-2 rounded-md border border-stone-200 dark:border-stone-700 cursor-pointer hover:bg-stone-50 dark:hover:bg-stone-800/40"
                                >
                                    <input
                                        type="checkbox"
                                        className="mt-0.5 accent-orange-600"
                                        checked={scopes[opt.key]}
                                        onChange={() => toggle(opt.key)}
                                    />
                                    <span>
                                        <span className="font-medium text-stone-800 dark:text-stone-100">
                                            {opt.label}
                                        </span>
                                        <span className="block text-xs text-stone-500 dark:text-stone-400">
                                            {opt.hint}
                                        </span>
                                    </span>
                                </label>
                            ))}
                        </div>
                        {scopes.admin && (
                            <p className="flex items-start gap-1.5 text-xs text-amber-700 dark:text-amber-400 mt-2">
                                <ShieldAlert className="w-4 h-4 flex-shrink-0 mt-px" />
                                Admin keys grant full control of this instance (users, settings,
                                every environment) and bypass SSO. Treat them like a root password.
                            </p>
                        )}
                    </div>

                    <div>
                        <label className="block text-sm font-medium text-stone-600 dark:text-stone-300 mb-1">
                            Expires
                        </label>
                        <select
                            className={FIELD}
                            value={expiryDays ?? ""}
                            onChange={(e) =>
                                setExpiryDays(e.target.value ? Number(e.target.value) : null)
                            }
                        >
                            {EXPIRY_OPTIONS.map((o) => (
                                <option key={o.label} value={o.days ?? ""}>
                                    {o.label}
                                </option>
                            ))}
                        </select>
                    </div>
                </div>

                <div className="px-5 py-3 border-t border-stone-200 dark:border-stone-800 flex items-center justify-end gap-2">
                    <button
                        type="button"
                        onClick={onClose}
                        className="px-3 py-1.5 font-medium rounded-md text-stone-700 dark:text-stone-300 hover:bg-stone-100 dark:hover:bg-stone-800 transition"
                    >
                        Cancel
                    </button>
                    <button
                        type="submit"
                        disabled={busy || !name.trim() || !anyScope}
                        className="px-3 py-1.5 font-medium text-white bg-orange-600 hover:bg-orange-500 rounded-md transition flex items-center gap-1.5 disabled:opacity-50"
                    >
                        <Plus className="w-4 h-4" />
                        Create key
                    </button>
                </div>
            </form>
        </div>
    );
}
