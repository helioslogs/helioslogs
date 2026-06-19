// Copyright 2026 Appbird LLC. Licensed under "GNU Affero General Public License v3.0"

import { createContext, useContext, useEffect, useRef, useState, type ReactNode } from "react";
import { getStoredTimezone, onTimezoneChange, setStoredTimezone } from "../lib/timezone";
import { useAuth } from "./useAuth";

const TimezoneContext = createContext<string>("UTC");

export function TimezoneProvider({ children }: { children: ReactNode }) {
    const { user } = useAuth();
    const [tz, setTz] = useState<string>(() => getStoredTimezone());
    const hydratedFor = useRef<string | null>(null);

    useEffect(() => onTimezoneChange((next) => setTz(next)), []);

    // Adopt the account's saved timezone on login (once per user). No write-back —
    // that only happens on an explicit user change (AccountPage).
    useEffect(() => {
        if (!user) {
            hydratedFor.current = null;
            return;
        }
        if (hydratedFor.current === user.user_id) return;
        hydratedFor.current = user.user_id;
        if (user.timezone && user.timezone !== getStoredTimezone()) {
            setStoredTimezone(user.timezone);
        }
    }, [user]);

    return <TimezoneContext.Provider value={tz}>{children}</TimezoneContext.Provider>;
}

// Subscribe to the display timezone; re-renders when the admin panel changes it.
export function useTimezone(): string {
    return useContext(TimezoneContext);
}
