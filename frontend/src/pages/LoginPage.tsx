// Copyright 2026 Appbird LLC. Licensed under "GNU Affero General Public License v3.0"

// Centered login form. Rendered by AuthGate whenever there is no active
// session — there is no route-level URL for it.

import { useEffect, useState, type FormEvent } from "react";
import { Layers } from "lucide-react";
import { useAuth } from "../state/useAuth";
import { getSamlStatus } from "../api/client";

// `?local=1` opens the password form directly, bypassing the SSO auto-redirect —
// the admin break-glass path when SSO is the default.
const wantsLocal = () => new URLSearchParams(window.location.search).get("local") === "1";

export function LoginPage({ demo }: { demo?: { login: string; password: string } }) {
    const { login } = useAuth();
    const [loginField, setLoginField] = useState(demo?.login ?? "");
    const [password, setPassword] = useState(demo?.password ?? "");
    const [busy, setBusy] = useState(false);
    const [error, setError] = useState<string | null>(null);
    const [redirecting, setRedirecting] = useState(false);
    const [sso, setSso] = useState<{
        enabled: boolean;
        label: string;
        local_login_disabled: boolean;
    }>({ enabled: false, label: "Sign in with SSO", local_login_disabled: false });

    // SSO-only mode auto-redirects to the IdP; the form only shows via `?local=1`,
    // a stashed SSO error, or right after logout. Time-guard breaks redirect loops.
    useEffect(() => {
        const stashed = sessionStorage.getItem("helios.sso_error");
        if (stashed) {
            setError(stashed);
            sessionStorage.removeItem("helios.sso_error");
        }
        const afterLogout = sessionStorage.getItem("helios.sso_logout") === "1";
        if (afterLogout) sessionStorage.removeItem("helios.sso_logout");

        getSamlStatus()
            .then((st) => {
                setSso(st);
                if (!st.enabled || !st.local_login_disabled) return; // form + SSO button
                if (wantsLocal() || stashed || afterLogout) return; // break-glass / error
                const last = Number(sessionStorage.getItem("helios.sso_redirect_at") || "0");
                if (Date.now() - last < 3000) return; // tight-loop guard
                sessionStorage.setItem("helios.sso_redirect_at", String(Date.now()));
                setRedirecting(true);
                const path = window.location.pathname;
                const target =
                    path && path !== "/" && path !== "/login" ? path + window.location.search : "";
                const q = target ? `?next=${encodeURIComponent(target)}` : "";
                window.location.assign(`/api/auth/saml/login${q}`);
            })
            .catch(() => {
                /* no SSO button if status can't be read */
            });
    }, []);

    async function onSubmit(e: FormEvent) {
        e.preventDefault();
        if (busy) return;
        setBusy(true);
        setError(null);
        try {
            await login(loginField.trim(), password);
        } catch (err) {
            setError(err instanceof Error ? err.message : String(err));
        } finally {
            setBusy(false);
        }
    }

    if (redirecting) {
        return (
            <div className="min-h-screen flex items-center justify-center bg-stone-50 dark:bg-stone-950 px-4 text-stone-500 dark:text-stone-400">
                Redirecting to sign-in…
            </div>
        );
    }

    return (
        <div className="min-h-screen flex items-center justify-center bg-stone-50 dark:bg-stone-950 px-4">
            <form
                onSubmit={onSubmit}
                className="w-full max-w-sm bg-white dark:bg-stone-900 border border-stone-200 dark:border-stone-800 rounded-lg shadow-sm p-6"
            >
                <div className="flex items-center gap-2 mb-6">
                    <div className="w-9 h-9 rounded-lg bg-gradient-to-br from-orange-500 to-orange-700 flex items-center justify-center text-white">
                        <Layers className="w-5 h-5" />
                    </div>
                    <div>
                        <div className="font-semibold text-stone-900 dark:text-stone-100">
                            HeliosLogs
                        </div>
                        <div className="text-stone-500 dark:text-stone-400">
                            Sign in to continue
                        </div>
                    </div>
                </div>

                {demo && (
                    <div className="mb-4 px-3 py-2 rounded-md bg-amber-50 text-amber-800 border border-amber-200 dark:bg-amber-950/40 dark:text-amber-200 dark:border-amber-900">
                        Live Demo mode — just click <span className="font-medium">Sign in</span> to
                        explore.
                    </div>
                )}

                {sso.enabled && sso.local_login_disabled && (
                    <div className="mb-4 px-3 py-2 rounded-md bg-amber-50 text-amber-800 border border-amber-200 dark:bg-amber-950/40 dark:text-amber-200 dark:border-amber-900">
                        Password sign-in is restricted to administrators. Everyone else should use{" "}
                        {sso.label}.
                    </div>
                )}

                <label className="block font-medium text-stone-700 dark:text-stone-300 mb-1">
                    Username or email
                </label>
                <input
                    type="text"
                    autoFocus
                    autoComplete="username"
                    value={loginField}
                    onChange={(e) => setLoginField(e.target.value)}
                    className="w-full mb-3 px-2.5 py-1.5 bg-stone-50 dark:bg-stone-950 border border-stone-200 dark:border-stone-700 rounded-md focus:outline-none focus:border-orange-500"
                />

                <label className="block font-medium text-stone-700 dark:text-stone-300 mb-1">
                    Password
                </label>
                <input
                    type="password"
                    autoComplete="current-password"
                    value={password}
                    onChange={(e) => setPassword(e.target.value)}
                    className="w-full mb-4 px-2.5 py-1.5 bg-stone-50 dark:bg-stone-950 border border-stone-200 dark:border-stone-700 rounded-md focus:outline-none focus:border-orange-500"
                />

                {error && (
                    <div className="mb-3 px-3 py-2 rounded-md bg-red-50 text-red-800 border border-red-200 dark:bg-red-950 dark:text-red-200 dark:border-red-900">
                        {error}
                    </div>
                )}

                <button
                    type="submit"
                    disabled={busy || !loginField || !password}
                    className="w-full px-3 py-2 font-medium text-white bg-orange-600 hover:bg-orange-500 rounded-md transition disabled:opacity-50 disabled:cursor-not-allowed"
                >
                    {busy ? "Signing in…" : "Sign in"}
                </button>

                {sso.enabled && (
                    <>
                        <div className="my-4 flex items-center gap-3 text-stone-400 dark:text-stone-600">
                            <span className="h-px flex-1 bg-stone-200 dark:bg-stone-800" />
                            <span className="text-xs uppercase tracking-wider">or</span>
                            <span className="h-px flex-1 bg-stone-200 dark:bg-stone-800" />
                        </div>
                        <button
                            type="button"
                            onClick={() => window.location.assign("/api/auth/saml/login")}
                            className="w-full px-3 py-2 font-medium text-stone-800 dark:text-stone-100 bg-stone-100 dark:bg-stone-800 hover:bg-stone-200 dark:hover:bg-stone-700 border border-stone-200 dark:border-stone-700 rounded-md transition"
                        >
                            {sso.label}
                        </button>
                    </>
                )}
            </form>
        </div>
    );
}
