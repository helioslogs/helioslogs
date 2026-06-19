// Copyright 2026 Appbird LLC. Licensed under "GNU Affero General Public License v3.0"

// `/admin/users` — admin user-management panel. Server-generated passwords
// (on create / regenerate) are shown inline once — there's no email flow.

import { useCallback, useEffect, useState } from "react";
import {
    Copy,
    KeyRound,
    Pencil,
    ShieldCheck,
    Trash2,
    UserPlus,
    Users as UsersIcon,
    X,
} from "lucide-react";
import {
    deleteUser,
    getSamlStatus,
    listUserAllowed,
    listUsers,
    type UserRecord,
} from "../../api/client";
import type { EnvIndexAllow, SamlStatus } from "../../api/types";
import { Link } from "react-router-dom";
import { UserDialog } from "../../components/UserDialog";
import { allowlistUnrestricted } from "../../components/AllowlistEditor";
import { useAuth } from "../../state/useAuth";
import { Card, ErrorBanner } from "../../components/admin";

type Reveal = {
    userid: string;
    password: string;
    // "created" vs "regenerated" only changes the banner copy.
    kind: "created" | "regenerated";
};

export function UsersPanel() {
    const { user: me } = useAuth();
    const [users, setUsers] = useState<UserRecord[] | null>(null);
    // Per-user allowlist rules keyed by user id (admins omitted — they bypass it).
    const [allowedByUser, setAllowedByUser] = useState<Record<string, EnvIndexAllow[]>>({});
    const [error, setError] = useState<string | null>(null);
    const [busyId, setBusyId] = useState<string | null>(null);
    const [reveal, setReveal] = useState<Reveal | null>(null);
    const [saml, setSaml] = useState<SamlStatus | null>(null);
    // `null` = no dialog open. `{kind:"create"}` for the New-user modal.
    // `{kind:"edit", user}` for editing an existing user's allowlist.
    const [dialog, setDialog] = useState<
        { kind: "create" } | { kind: "edit"; user: UserRecord } | null
    >(null);

    const refresh = useCallback(async () => {
        try {
            const list = await listUsers();
            setUsers(list);
            // Fetch every non-admin's allowlist in parallel. Cheap (one
            // SELECT per user) and avoids a per-row loading spinner.
            const nonAdmins = list.filter((u) => !u.is_admin);
            const entries = await Promise.all(
                nonAdmins.map(async (u) => {
                    try {
                        return [u.id, await listUserAllowed(u.id)] as const;
                    } catch {
                        return [u.id, []] as const;
                    }
                }),
            );
            setAllowedByUser(Object.fromEntries(entries));
        } catch (e) {
            setError(e instanceof Error ? e.message : String(e));
        }
    }, []);

    useEffect(() => {
        void refresh();
    }, [refresh]);

    useEffect(() => {
        getSamlStatus()
            .then(setSaml)
            .catch(() => {});
    }, []);

    async function handleDelete(u: UserRecord) {
        if (!confirm(`Delete user ${u.userid}? This cannot be undone.`)) return;
        setBusyId(u.id);
        setError(null);
        try {
            await deleteUser(u.id);
            await refresh();
        } catch (e) {
            setError(e instanceof Error ? e.message : String(e));
        } finally {
            setBusyId(null);
        }
    }

    return (
        <div>
            <Card title="User management">
                <div className="p-6 space-y-4">
                    <UsersHelpFrame />

                    <ErrorBanner error={error} />

                    {reveal && <PasswordReveal reveal={reveal} onDismiss={() => setReveal(null)} />}

                    <div className="flex items-center gap-3">
                        <button
                            type="button"
                            onClick={() => setDialog({ kind: "create" })}
                            className="px-3 py-1.5 font-medium text-white bg-orange-600 hover:bg-orange-500 rounded-md transition flex items-center gap-1.5"
                        >
                            <UserPlus className="w-4 h-4" />
                            New user
                        </button>
                        {users && (
                            <span className="text-stone-700 dark:text-stone-300">
                                {users.length} {users.length === 1 ? "user" : "users"} total
                            </span>
                        )}
                        <div className="ml-auto flex items-center gap-3">
                            {saml && (
                                <span className="flex items-center gap-1.5 text-stone-600 dark:text-stone-300">
                                    <span
                                        className={`w-2 h-2 rounded-full ${
                                            saml.enabled
                                                ? "bg-emerald-500"
                                                : "bg-stone-400 dark:bg-stone-600"
                                        }`}
                                    />
                                    {saml.enabled
                                        ? `SSO on${saml.local_login_disabled ? " · local logins off" : ""}`
                                        : "SSO off"}
                                </span>
                            )}
                            <Link
                                to="/admin/users/saml"
                                className="px-3 py-1.5 font-medium rounded-md border border-stone-200 dark:border-stone-700 text-stone-700 dark:text-stone-200 hover:bg-stone-100 dark:hover:bg-stone-800 transition flex items-center gap-1.5"
                            >
                                <ShieldCheck className="w-4 h-4" />
                                SAML / SSO
                            </Link>
                        </div>
                    </div>
                </div>

                <UserTable
                    users={users}
                    allowedByUser={allowedByUser}
                    meId={me?.user_id}
                    busyId={busyId}
                    onEdit={(u) => setDialog({ kind: "edit", user: u })}
                    onDelete={handleDelete}
                />
            </Card>

            {dialog?.kind === "create" && (
                <UserDialog
                    mode={{
                        kind: "create",
                        onCreated: ({ user, password }) => {
                            setReveal({ userid: user.userid, password, kind: "created" });
                            setDialog(null);
                            void refresh();
                        },
                    }}
                    onClose={() => setDialog(null)}
                />
            )}
            {dialog?.kind === "edit" && (
                <UserDialog
                    mode={{
                        kind: "edit",
                        user: dialog.user,
                        isSelf: dialog.user.id === me?.user_id,
                    }}
                    onClose={() => {
                        // Refresh on close too — a regenerate-password action has no
                        // explicit "save" step but still mutates server state.
                        setDialog(null);
                        void refresh();
                    }}
                    onSaved={() => void refresh()}
                />
            )}
        </div>
    );
}

// Inline help banner — matches the LLM / MCP / General panels.
function UsersHelpFrame() {
    return (
        <div className="flex gap-3 p-4 rounded-lg bg-orange-50/60 dark:bg-orange-950/20 border border-orange-200/70 dark:border-orange-900/40">
            <div className="flex-shrink-0 mt-0.5">
                <UsersIcon className="w-4 h-4 text-orange-600 dark:text-orange-400" />
            </div>
            <div className="space-y-1.5 text-stone-700 dark:text-stone-200 leading-relaxed">
                <p>
                    Create user accounts and manage their passwords. There is no signup flow and no
                    email delivery — when you create a user or regenerate a password, the plaintext
                    is shown <strong>once</strong> in a banner; hand it to the user yourself.
                </p>
                <p className="text-stone-700 dark:text-stone-300">
                    Admins can manage users, indexes, MCP, and the LLM provider. Standard users get
                    search and the Investigate panel only. You can't delete your own account.
                </p>
            </div>
        </div>
    );
}

function UserTable({
    users,
    allowedByUser,
    meId,
    busyId,
    onEdit,
    onDelete,
}: {
    users: UserRecord[] | null;
    allowedByUser: Record<string, EnvIndexAllow[]>;
    meId: string | undefined;
    busyId: string | null;
    onEdit: (u: UserRecord) => void;
    onDelete: (u: UserRecord) => void;
}) {
    return (
        <div className="border-t border-stone-200 dark:border-stone-800">
            <table className="w-full">
                <thead className="bg-stone-50 dark:bg-stone-950/40 text-stone-700 dark:text-stone-300 border-b border-stone-200 dark:border-stone-800">
                    <tr>
                        <th className="text-left font-semibold uppercase tracking-wider px-4 py-2">
                            Userid
                        </th>
                        <th className="text-left font-semibold uppercase tracking-wider px-4 py-2">
                            Email
                        </th>
                        <th className="text-left font-semibold uppercase tracking-wider px-4 py-2">
                            Name
                        </th>
                        <th className="text-left font-semibold uppercase tracking-wider px-4 py-2">
                            Role
                        </th>
                        <th className="text-left font-semibold uppercase tracking-wider px-4 py-2">
                            Data access
                        </th>
                        <th className="text-left font-semibold uppercase tracking-wider px-4 py-2">
                            Created
                        </th>
                        <th className="text-right font-semibold uppercase tracking-wider px-4 py-2">
                            Actions
                        </th>
                    </tr>
                </thead>
                <tbody className="divide-y divide-stone-100 dark:divide-stone-800">
                    {users === null && (
                        <tr>
                            <td
                                colSpan={7}
                                className="px-4 py-6 text-stone-700 dark:text-stone-300"
                            >
                                Loading…
                            </td>
                        </tr>
                    )}
                    {users?.length === 0 && (
                        <tr>
                            <td
                                colSpan={7}
                                className="px-4 py-6 text-stone-700 dark:text-stone-300 italic"
                            >
                                No users yet.
                            </td>
                        </tr>
                    )}
                    {users?.map((u) => {
                        const isSelf = u.id === meId;
                        return (
                            <tr
                                key={u.id}
                                className="text-stone-800 dark:text-stone-200 hover:bg-stone-50/60 dark:hover:bg-stone-800/30 transition-colors"
                            >
                                <td className="px-4 py-2 font-mono">
                                    {u.userid}
                                    {isSelf && (
                                        <span className="ml-2 px-1.5 py-0.5 rounded bg-stone-100 dark:bg-stone-800 text-stone-700 dark:text-stone-300 font-sans">
                                            you
                                        </span>
                                    )}
                                </td>
                                <td className="px-4 py-2">{u.email}</td>
                                <td className="px-4 py-2">{u.display_name}</td>
                                <td className="px-4 py-2">
                                    {u.is_admin ? (
                                        <span className="inline-flex items-center gap-1 px-1.5 py-0.5 rounded bg-orange-50 text-orange-800 border border-orange-200 dark:bg-orange-950/40 dark:text-orange-200 dark:border-orange-900/50">
                                            <ShieldCheck className="w-3 h-3" />
                                            admin
                                        </span>
                                    ) : (
                                        <span className="text-stone-700 dark:text-stone-300">
                                            user
                                        </span>
                                    )}
                                </td>
                                <td className="px-4 py-2">
                                    <PermissionsBadge user={u} allowed={allowedByUser[u.id]} />
                                </td>
                                <td className="px-4 py-2 text-stone-700 dark:text-stone-300 tabular-nums">
                                    {formatDate(u.created_at)}
                                </td>
                                <td className="px-4 py-2 text-right space-x-1 whitespace-nowrap">
                                    <button
                                        type="button"
                                        onClick={() => onEdit(u)}
                                        className="px-2 py-1 rounded-md border border-stone-200 dark:border-stone-700 hover:bg-stone-100 dark:hover:bg-stone-800 inline-flex items-center gap-1"
                                        title="Edit permissions"
                                    >
                                        <Pencil className="w-3 h-3" />
                                        Edit
                                    </button>
                                    <button
                                        type="button"
                                        disabled={busyId === u.id || isSelf}
                                        onClick={() => onDelete(u)}
                                        className="px-2 py-1 rounded-md border border-red-200 dark:border-red-900/50 text-red-700 dark:text-red-300 hover:bg-red-50 dark:hover:bg-red-950/30 disabled:opacity-30 inline-flex items-center gap-1"
                                        title={
                                            isSelf
                                                ? "You cannot delete your own account"
                                                : "Delete user"
                                        }
                                    >
                                        <Trash2 className="w-3 h-3" />
                                        Delete
                                    </button>
                                </td>
                            </tr>
                        );
                    })}
                </tbody>
            </table>
        </div>
    );
}

// Per-user permissions cell. Admins show "all (admin)"; non-admins show
// "unrestricted" (empty allowlist) or a summary like "dev, prod (2 envs)".
function PermissionsBadge({
    user,
    allowed,
}: {
    user: UserRecord;
    allowed: EnvIndexAllow[] | undefined;
}) {
    if (user.is_admin) {
        return <span className="text-stone-700 dark:text-stone-300 italic">all (admin)</span>;
    }
    if (allowed === undefined) {
        return <span className="text-stone-700 dark:text-stone-300 italic">…</span>;
    }
    if (allowlistUnrestricted(allowed)) {
        return (
            <span className="inline-flex items-center gap-1 px-1.5 py-0.5 rounded bg-amber-50 text-amber-800 border border-amber-200 dark:bg-amber-950/40 dark:text-amber-200 dark:border-amber-900/50">
                full access
            </span>
        );
    }
    const envs = allowed.map((r) => r.env);
    const head = envs.slice(0, 2).join(", ");
    const tail = envs.length > 2 ? ` +${envs.length - 2}` : "";
    return (
        <span className="text-stone-700 dark:text-stone-300" title={envs.join(", ")}>
            {head}
            {tail}
            <span className="text-stone-700 dark:text-stone-300 ml-1">
                ({envs.length} env{envs.length === 1 ? "" : "s"})
            </span>
        </span>
    );
}

function PasswordReveal({ reveal, onDismiss }: { reveal: Reveal; onDismiss: () => void }) {
    const [copied, setCopied] = useState(false);
    async function copy() {
        try {
            await navigator.clipboard.writeText(reveal.password);
            setCopied(true);
            setTimeout(() => setCopied(false), 2000);
        } catch {
            /* clipboard blocked — leave it visible */
        }
    }
    const verb = reveal.kind === "created" ? "Created" : "Regenerated password for";
    return (
        <div className="bg-amber-50 dark:bg-amber-950/30 border border-amber-200 dark:border-amber-900/50 rounded-lg p-4 flex items-start gap-3">
            <KeyRound className="w-5 h-5 text-amber-700 dark:text-amber-300 flex-shrink-0 mt-0.5" />
            <div className="flex-grow min-w-0">
                <div className="font-semibold text-amber-900 dark:text-amber-100">
                    {verb} <span className="font-mono">{reveal.userid}</span>
                </div>
                <div className="text-amber-800 dark:text-amber-300 mb-2">
                    Hand this password to the user now — it is shown only once.
                </div>
                <div className="flex items-center gap-2">
                    <code className="flex-1 min-w-0 px-2 py-1 bg-white dark:bg-stone-900 border border-amber-200 dark:border-amber-900/50 rounded font-mono break-all select-all">
                        {reveal.password}
                    </code>
                    <button
                        type="button"
                        onClick={copy}
                        className="px-2.5 py-1.5 rounded-md border border-amber-300 dark:border-amber-800 hover:bg-amber-100 dark:hover:bg-amber-900/40 text-amber-900 dark:text-amber-100 inline-flex items-center gap-1 flex-shrink-0"
                    >
                        <Copy className="w-3 h-3" />
                        {copied ? "Copied" : "Copy"}
                    </button>
                </div>
            </div>
            <button
                type="button"
                onClick={onDismiss}
                className="text-amber-800 dark:text-amber-300 hover:text-amber-900 dark:hover:text-amber-100 flex-shrink-0"
                title="Dismiss"
            >
                <X className="w-4 h-4" />
            </button>
        </div>
    );
}

function formatDate(iso: string): string {
    try {
        return new Date(iso).toLocaleString();
    } catch {
        return iso;
    }
}
