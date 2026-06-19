// Copyright 2026 Appbird LLC. Licensed under "GNU Affero General Public License v3.0"

// AuthProvider — owns the cached current-user state, exposes login/logout,
// and clears the cache when any `apiFetch` call returns 401 (via the
// `helios-401` window event dispatched in api/client.ts).

import { createContext, useCallback, useContext, useEffect, useState, type ReactNode } from "react";
import { getMe, login as apiLogin, logout as apiLogout, type AuthUser } from "../api/client";

type AuthState = {
    // `null` means "not authenticated"; `undefined` means "still checking".
    user: AuthUser | null | undefined;
    login: (login: string, password: string) => Promise<void>;
    logout: () => Promise<void>;
    // Re-fetches `/api/auth/me` and refreshes the cached user.
    refresh: () => Promise<void>;
};

const Ctx = createContext<AuthState | null>(null);

export function AuthProvider({ children }: { children: ReactNode }) {
    const [user, setUser] = useState<AuthUser | null | undefined>(undefined);

    const refresh = useCallback(async () => {
        try {
            setUser(await getMe());
        } catch {
            setUser(null);
        }
    }, []);

    // Initial boot check.
    useEffect(() => {
        void refresh();
    }, [refresh]);

    // A 401 anywhere in the app boots us back to the login screen.
    useEffect(() => {
        const handler = () => setUser(null);
        window.addEventListener("helios-401", handler);
        return () => window.removeEventListener("helios-401", handler);
    }, []);

    const login = useCallback(async (loginField: string, password: string) => {
        const u = await apiLogin({ login: loginField, password });
        setUser(u);
    }, []);

    const logout = useCallback(async () => {
        try {
            await apiLogout();
        } finally {
            // One-shot so explicit logout lands on the login form instead of being
            // re-authed by a live IdP session in SSO-only mode. Consumed by the login page.
            sessionStorage.setItem("helios.sso_logout", "1");
            setUser(null);
        }
    }, []);

    return <Ctx.Provider value={{ user, login, logout, refresh }}>{children}</Ctx.Provider>;
}

export function useAuth(): AuthState {
    const v = useContext(Ctx);
    if (!v) throw new Error("useAuth must be used inside <AuthProvider>");
    return v;
}
