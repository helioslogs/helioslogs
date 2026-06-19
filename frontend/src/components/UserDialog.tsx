// Copyright 2026 Appbird LLC. Licensed under "GNU Affero General Public License v3.0"

// Create / edit user modal. Create returns a one-shot password; edit keeps
// `userid` immutable and offers inline password regeneration.

import { useEffect, useRef, useState, type FormEvent } from "react";
import { Check, Copy, KeyRound, ShieldCheck } from "lucide-react";
import {
    createUser,
    getCatalogIndexes,
    listEnvs,
    listUserAllowed,
    regenerateUserPassword,
    setUserAllowed,
    updateUser,
    type UserRecord,
} from "../api/client";
import type { EnvIndexAllow } from "../api/types";
import { AllowlistEditor } from "./AllowlistEditor";

export type CreateResult = {
    user: UserRecord;
    password: string;
};

type Mode =
    | { kind: "create"; onCreated: (r: CreateResult) => void }
    | { kind: "edit"; user: UserRecord; isSelf?: boolean };

interface Props {
    mode: Mode;
    onClose: () => void;
    // Notified after a successful save so the parent can re-fetch lists.
    onSaved?: () => void;
}

export function UserDialog({ mode, onClose, onSaved }: Props) {
    const isCreate = mode.kind === "create";
    const editingUser = !isCreate ? mode.user : null;
    const isSelf = !isCreate ? mode.isSelf === true : false;

    const [userid, setUserid] = useState("");
    const [email, setEmail] = useState(editingUser?.email ?? "");
    const [displayName, setDisplayName] = useState(editingUser?.display_name ?? "");
    const [isAdmin, setIsAdmin] = useState(editingUser?.is_admin ?? false);
    const [allowed, setAllowed] = useState<EnvIndexAllow[]>([]);
    // UI-only flag: `allowed: []` means unrestricted in storage, so we can't
    // otherwise represent "restricted but nothing picked yet".
    const [restricted, setRestricted] = useState(false);
    const [catalogByEnv, setCatalogByEnv] = useState<Record<string, string[]>>({});
    const [busy, setBusy] = useState(false);
    const [regenBusy, setRegenBusy] = useState(false);
    const [error, setError] = useState<string | null>(null);
    // Plaintext password from a fresh regenerate. Shown until the modal
    // closes — same one-shot semantics as the panel-level reveal banner.
    const [freshPassword, setFreshPassword] = useState<string | null>(null);
    const [pwCopied, setPwCopied] = useState(false);
    const firstFieldRef = useRef<HTMLInputElement>(null);

    // Load catalog + registered envs + existing allowlist; filtered to registered
    // envs so orphan on-disk partitions aren't grantable.
    useEffect(() => {
        let cancelled = false;
        void (async () => {
            try {
                const [catalog, envs] = await Promise.all([getCatalogIndexes(), listEnvs()]);
                if (cancelled) return;
                const registered = new Set(envs.map((e) => e.name));
                const map: Record<string, Set<string>> = {};
                // Seed every registered env so it appears even when no
                // partitions exist yet (admin can grant ahead of data landing).
                for (const e of envs) map[e.name] = new Set();
                for (const p of catalog) {
                    if (!registered.has(p.env)) continue;
                    map[p.env].add(p.index);
                }
                const out: Record<string, string[]> = {};
                for (const [env, names] of Object.entries(map)) {
                    out[env] = Array.from(names).sort();
                }
                setCatalogByEnv(out);
                if (editingUser) {
                    const current = await listUserAllowed(editingUser.id);
                    if (!cancelled) {
                        setAllowed(current);
                        setRestricted(current.length > 0);
                    }
                } else {
                    // New non-admins default to unrestricted (empty allowlist).
                    setAllowed([]);
                    setRestricted(false);
                }
            } catch (e) {
                if (!cancelled) setError(e instanceof Error ? e.message : String(e));
            }
        })();
        return () => {
            cancelled = true;
        };
    }, [editingUser]);

    useEffect(() => {
        firstFieldRef.current?.focus();
    }, []);

    const submit = async (e: FormEvent) => {
        e.preventDefault();
        setBusy(true);
        setError(null);
        try {
            // Unrestricted always persists `[]`, ignoring any half-edited rules in `allowed`.
            const allowedToSave = restricted ? allowed : [];

            if (isCreate) {
                const { user, password } = await createUser({
                    userid: userid.trim(),
                    email: email.trim(),
                    display_name: displayName.trim(),
                    is_admin: isAdmin,
                });
                // Admins bypass the allowlist; an empty `[]` PUT is a no-op anyway.
                if (!isAdmin && allowedToSave.length > 0) {
                    await setUserAllowed(user.id, allowedToSave);
                }
                (mode as Extract<Mode, { kind: "create" }>).onCreated({ user, password });
            } else if (editingUser) {
                // Only send patches for fields that actually changed.
                const patch: {
                    email?: string;
                    display_name?: string;
                    is_admin?: boolean;
                } = {};
                if (email.trim() !== editingUser.email) patch.email = email.trim();
                if (displayName.trim() !== editingUser.display_name) {
                    patch.display_name = displayName.trim();
                }
                if (isAdmin !== editingUser.is_admin) patch.is_admin = isAdmin;
                if (Object.keys(patch).length > 0) {
                    await updateUser(editingUser.id, patch);
                }
                // Allowlist only applies to non-admin users.
                if (!isAdmin) {
                    await setUserAllowed(editingUser.id, allowedToSave);
                }
                onSaved?.();
                onClose();
            }
        } catch (err) {
            setError(err instanceof Error ? err.message : String(err));
            setBusy(false);
        }
    };

    const handleRegenerate = async () => {
        if (!editingUser) return;
        if (
            !confirm(
                `Regenerate password for ${editingUser.userid}? Their existing sessions will be invalidated.`,
            )
        ) {
            return;
        }
        setRegenBusy(true);
        setError(null);
        try {
            const pw = await regenerateUserPassword(editingUser.id);
            setFreshPassword(pw);
            setPwCopied(false);
        } catch (err) {
            setError(err instanceof Error ? err.message : String(err));
        } finally {
            setRegenBusy(false);
        }
    };

    const copyPassword = async () => {
        if (!freshPassword) return;
        try {
            await navigator.clipboard.writeText(freshPassword);
            setPwCopied(true);
            setTimeout(() => setPwCopied(false), 2000);
        } catch {
            /* clipboard unavailable — user can still select-all */
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
                className="bg-white dark:bg-stone-900 rounded-xl border border-stone-200 dark:border-stone-700 shadow-2xl w-full max-w-3xl mx-4 max-h-[90vh] overflow-auto"
            >
                <div className="px-5 py-3 border-b border-stone-200 dark:border-stone-800 flex items-center justify-between">
                    <h2 className="font-semibold text-stone-900 dark:text-stone-100">
                        {isCreate ? "New user" : `Edit user — ${editingUser?.userid}`}
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

                    {isCreate ? (
                        <Field label="Userid" required>
                            <input
                                ref={firstFieldRef}
                                type="text"
                                required
                                value={userid}
                                onChange={(e) => setUserid(e.target.value)}
                                disabled={busy}
                                placeholder="alice"
                                className={FIELD}
                            />
                        </Field>
                    ) : (
                        <Field label="Userid">
                            <input
                                type="text"
                                value={editingUser?.userid ?? ""}
                                disabled
                                className={`${FIELD} opacity-60 cursor-not-allowed`}
                                title="Userid is the login key and can't be changed"
                            />
                        </Field>
                    )}
                    <Field label="Email" required>
                        <input
                            ref={isCreate ? undefined : firstFieldRef}
                            type="email"
                            required
                            value={email}
                            onChange={(e) => setEmail(e.target.value)}
                            disabled={busy}
                            placeholder="alice@example.com"
                            className={FIELD}
                        />
                    </Field>
                    <Field label="Display name" required>
                        <input
                            type="text"
                            required
                            value={displayName}
                            onChange={(e) => setDisplayName(e.target.value)}
                            disabled={busy}
                            placeholder="Alice"
                            className={FIELD}
                        />
                    </Field>
                    <label
                        className={`flex items-center gap-2 ${
                            !isCreate && isSelf ? "cursor-not-allowed opacity-60" : "cursor-pointer"
                        }`}
                        title={
                            !isCreate && isSelf
                                ? "You cannot demote your own admin role"
                                : undefined
                        }
                    >
                        <input
                            type="checkbox"
                            checked={isAdmin}
                            onChange={(e) => setIsAdmin(e.target.checked)}
                            disabled={busy || (!isCreate && isSelf)}
                            className="h-4 w-4 accent-orange-600"
                        />
                        <ShieldCheck className="w-4 h-4 text-orange-600" />
                        <span className="text-stone-700 dark:text-stone-300">
                            Administrator (bypasses the allowlist; can access every env and index)
                        </span>
                    </label>

                    {!isCreate && (
                        <div className="pt-3 border-t border-stone-200 dark:border-stone-800 space-y-2">
                            <div className="font-semibold text-stone-800 dark:text-stone-100">
                                Password
                            </div>
                            <div className="text-stone-700 dark:text-stone-300">
                                Generate a new random password and invalidate the user's existing
                                sessions. Shown once below — copy it before closing.
                            </div>
                            <button
                                type="button"
                                onClick={() => void handleRegenerate()}
                                disabled={busy || regenBusy}
                                className="px-3 py-1.5 font-medium rounded-md border border-stone-200 dark:border-stone-700 hover:bg-stone-100 dark:hover:bg-stone-800 disabled:opacity-50 inline-flex items-center gap-1.5 transition"
                            >
                                <KeyRound className="w-3.5 h-3.5" />
                                {regenBusy ? "regenerating…" : "Regenerate password"}
                            </button>
                            {freshPassword && (
                                <div className="rounded-md border border-amber-300 dark:border-amber-800 bg-amber-50 dark:bg-amber-950/30 px-3 py-3 space-y-2">
                                    <div className="text-amber-900 dark:text-amber-200 font-medium">
                                        New password — copy it now. It won't be shown again.
                                    </div>
                                    <div className="flex items-center gap-2">
                                        <code className="flex-1 font-mono break-all px-2 py-1.5 bg-white dark:bg-stone-900 border border-amber-200 dark:border-amber-900 rounded-md select-all">
                                            {freshPassword}
                                        </code>
                                        <button
                                            type="button"
                                            onClick={() => void copyPassword()}
                                            className="px-2.5 py-1.5 rounded-md bg-stone-900 hover:bg-stone-800 dark:bg-stone-800 dark:hover:bg-stone-700 text-white transition flex-shrink-0"
                                            title="Copy to clipboard"
                                        >
                                            {pwCopied ? (
                                                <Check className="w-3.5 h-3.5" />
                                            ) : (
                                                <Copy className="w-3.5 h-3.5" />
                                            )}
                                        </button>
                                    </div>
                                </div>
                            )}
                        </div>
                    )}

                    {!isCreate && isAdmin && (
                        <div className="px-3 py-2 rounded-md bg-orange-50 dark:bg-orange-950/30 text-orange-800 dark:text-orange-200 border border-orange-200 dark:border-orange-900/50">
                            Admin users bypass the allowlist — they implicitly have access to every
                            env and every index. The allowlist below is ignored.
                        </div>
                    )}

                    {!isAdmin && (
                        <div className="space-y-3 pt-3 border-t border-stone-200 dark:border-stone-800">
                            <div className="font-semibold text-stone-800 dark:text-stone-100">
                                Data access
                            </div>
                            <fieldset className="space-y-1.5" disabled={busy}>
                                <label className="flex items-start gap-2 cursor-pointer">
                                    <input
                                        type="radio"
                                        name="data-access"
                                        checked={!restricted}
                                        onChange={() => setRestricted(false)}
                                        disabled={busy}
                                        className="h-4 w-4 accent-orange-600 mt-1"
                                    />
                                    <span className="text-stone-700 dark:text-stone-300">
                                        <strong>Full access</strong> — every environment and index,
                                        including ones added later.
                                    </span>
                                </label>
                                <label className="flex items-start gap-2 cursor-pointer">
                                    <input
                                        type="radio"
                                        name="data-access"
                                        checked={restricted}
                                        onChange={() => setRestricted(true)}
                                        disabled={busy}
                                        className="h-4 w-4 accent-orange-600 mt-1"
                                    />
                                    <span className="text-stone-700 dark:text-stone-300">
                                        <strong>Scoped</strong> — pick which environments and
                                        indexes below.
                                    </span>
                                </label>
                            </fieldset>
                            {restricted && (
                                <>
                                    <div className="text-stone-700 dark:text-stone-300">
                                        Toggle <strong>All indexes</strong> on an env to permit
                                        every index there (including ones added later). Leaving an
                                        env unchecked denies that env entirely.
                                    </div>
                                    <div className="flex items-center gap-3">
                                        <span className="font-semibold text-stone-700 dark:text-stone-200">
                                            Status:
                                        </span>
                                        {allowed.length === 0 ? (
                                            <span className="inline-flex items-center gap-1.5 px-2 py-0.5 rounded-md bg-red-50 text-red-800 dark:bg-red-950/40 dark:text-red-300">
                                                no envs picked — user will see nothing
                                            </span>
                                        ) : (
                                            <span className="inline-flex items-center gap-1.5 px-2 py-0.5 rounded-md bg-green-50 text-green-800 dark:bg-green-950/40 dark:text-green-300">
                                                {allowed.length} env rule
                                                {allowed.length === 1 ? "" : "s"}
                                            </span>
                                        )}
                                    </div>
                                    <AllowlistEditor
                                        value={allowed}
                                        onChange={setAllowed}
                                        catalogByEnv={catalogByEnv}
                                        disabled={busy}
                                        // Parent owns the toggle, so don't derive "unrestricted"
                                        // from an empty value (else Scoped pre-checks every env).
                                        unrestricted={false}
                                    />
                                </>
                            )}
                        </div>
                    )}
                </div>

                <div className="px-5 py-3 border-t border-stone-200 dark:border-stone-800 flex items-center justify-end gap-2">
                    <button
                        type="button"
                        onClick={onClose}
                        disabled={busy}
                        className="px-3 py-1.5 rounded-md text-stone-700 dark:text-stone-300 hover:bg-stone-100 dark:hover:bg-stone-800 disabled:opacity-50 transition"
                    >
                        Cancel
                    </button>
                    <button
                        type="submit"
                        disabled={busy}
                        className="px-3 py-1.5 font-medium rounded-md bg-orange-600 hover:bg-orange-500 text-white disabled:opacity-50 disabled:cursor-not-allowed transition"
                    >
                        {busy ? "saving…" : isCreate ? "Create" : "Save"}
                    </button>
                </div>
            </form>
        </div>
    );
}

const FIELD =
    "w-full px-3 py-1.5 rounded-md border border-stone-200 dark:border-stone-700 bg-white dark:bg-stone-950 text-stone-900 dark:text-stone-100 focus:outline-none focus:border-orange-500";

function Field({
    label,
    required,
    children,
}: {
    label: string;
    required?: boolean;
    children: React.ReactNode;
}) {
    return (
        <label className="block space-y-1">
            <span className="text-stone-700 dark:text-stone-300 font-medium">
                {label}
                {required && <span className="text-red-500 ml-1">*</span>}
            </span>
            {children}
        </label>
    );
}
