// Copyright 2026 Appbird LLC. Licensed under "GNU Affero General Public License v3.0"

// First-run setup. Shown by App instead of LoginPage when the instance has no
// users yet (`/api/auth/setup_status`). The first visitor claims it by creating
// the admin account; there's no URL route for this — it's gated on server state.

import { useState, type FormEvent } from "react";
import { Layers, Loader2 } from "lucide-react";
import { useAuth } from "../state/useAuth";
import { setupAdmin } from "../api/client";

export function SetupPage() {
    const { refresh } = useAuth();
    const [userid, setUserid] = useState("admin");
    const [password, setPassword] = useState("");
    const [confirm, setConfirm] = useState("");
    const [busy, setBusy] = useState(false);
    const [error, setError] = useState<string | null>(null);

    async function onSubmit(e: FormEvent) {
        e.preventDefault();
        if (busy) return;
        if (password.length < 8) {
            setError("Password must be at least 8 characters.");
            return;
        }
        if (password !== confirm) {
            setError("Passwords don't match.");
            return;
        }
        setBusy(true);
        setError(null);
        try {
            await setupAdmin({ userid: userid.trim(), password });
            // refresh() flips the app to the main view, unmounting this screen. The
            // empty search state guides the user to load sample data or send their own.
            await refresh();
        } catch (err) {
            setError(err instanceof Error ? err.message : String(err));
            setBusy(false);
        }
    }

    return (
        <div className="min-h-screen flex items-center justify-center bg-stone-50 dark:bg-stone-950 px-4">
            <form
                onSubmit={onSubmit}
                className="w-full max-w-sm bg-white dark:bg-stone-900 border border-stone-200 dark:border-stone-800 rounded-lg shadow-sm p-6"
            >
                <div className="flex items-center gap-2 mb-1">
                    <div className="w-9 h-9 rounded-lg bg-gradient-to-br from-orange-500 to-orange-700 flex items-center justify-center text-white">
                        <Layers className="w-5 h-5" />
                    </div>
                    <div>
                        <div className="font-semibold text-stone-900 dark:text-stone-100">
                            Welcome to HeliosLogs
                        </div>
                        <div className="text-stone-500 dark:text-stone-400">
                            Create your admin account
                        </div>
                    </div>
                </div>
                <p className="text-xs text-stone-500 dark:text-stone-400 mb-5 mt-3">
                    This is the first run, so you get to claim the instance. This account is a full
                    administrator — you can add more users later.
                </p>

                <label className="block font-medium text-stone-700 dark:text-stone-300 mb-1">
                    Username
                </label>
                <input
                    type="text"
                    autoFocus
                    autoComplete="username"
                    value={userid}
                    onChange={(e) => setUserid(e.target.value)}
                    disabled={busy}
                    className="w-full mb-3 px-2.5 py-1.5 bg-stone-50 dark:bg-stone-950 border border-stone-200 dark:border-stone-700 rounded-md focus:outline-none focus:border-orange-500"
                />

                <label className="block font-medium text-stone-700 dark:text-stone-300 mb-1">
                    Password
                </label>
                <input
                    type="password"
                    autoComplete="new-password"
                    value={password}
                    onChange={(e) => setPassword(e.target.value)}
                    placeholder="at least 8 characters"
                    disabled={busy}
                    className="w-full mb-3 px-2.5 py-1.5 bg-stone-50 dark:bg-stone-950 border border-stone-200 dark:border-stone-700 rounded-md focus:outline-none focus:border-orange-500"
                />

                <label className="block font-medium text-stone-700 dark:text-stone-300 mb-1">
                    Confirm password
                </label>
                <input
                    type="password"
                    autoComplete="new-password"
                    value={confirm}
                    onChange={(e) => setConfirm(e.target.value)}
                    disabled={busy}
                    className="w-full mb-4 px-2.5 py-1.5 bg-stone-50 dark:bg-stone-950 border border-stone-200 dark:border-stone-700 rounded-md focus:outline-none focus:border-orange-500"
                />

                {error && (
                    <div className="mb-3 px-3 py-2 rounded-md bg-red-50 text-red-800 border border-red-200 dark:bg-red-950 dark:text-red-200 dark:border-red-900">
                        {error}
                    </div>
                )}

                <button
                    type="submit"
                    disabled={busy || !userid.trim() || !password || !confirm}
                    className="w-full px-3 py-2 font-medium text-white bg-orange-600 hover:bg-orange-500 rounded-md transition disabled:opacity-50 disabled:cursor-not-allowed inline-flex items-center justify-center gap-2"
                >
                    {busy && <Loader2 className="w-4 h-4 animate-spin" aria-hidden="true" />}
                    {busy ? "Setting up…" : "Create account & continue"}
                </button>
            </form>
        </div>
    );
}
