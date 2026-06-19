// Copyright 2026 Appbird LLC. Licensed under "GNU Affero General Public License v3.0"

import { useCallback, useEffect, useRef, useState } from "react";
import type { Alert } from "../api/types";
import { dismissAlert, dismissAllAlerts } from "../api/client";
import { notifyAlertsChanged } from "../api/events";
import { playAlertSound } from "../lib/alertSound";
import { useAlerts } from "./useAlerts";

// Foreground toasts for newly-fired alerts, on the inbox's 5s unacked poll.
// Dismissal is server-side (survives reloads, syncs across tabs); mute is per-browser.

const MUTED_KEY = "helios-toast-muted";

function readMuted(): boolean {
    try {
        return localStorage.getItem(MUTED_KEY) === "1";
    } catch {
        return false;
    }
}

export interface AlertToastsState {
    toasts: Alert[];
    dismiss: (id: string) => void;
    dismissAll: () => void;
    muted: boolean;
    toggleMuted: () => void;
}

export function useAlertToasts(): AlertToastsState {
    const { items } = useAlerts(true);
    const [muted, setMuted] = useState<boolean>(readMuted);
    // Optimistic hide so a dismissed toast vanishes before the server flag catches up.
    const [pending, setPending] = useState<Set<string>>(new Set());

    // `null` until first poll, so alerts already unacked at open don't chime.
    const seenIds = useRef<Set<string> | null>(null);
    const mutedRef = useRef(muted);
    mutedRef.current = muted;

    useEffect(() => {
        const ids = items.map((a) => a.id);
        if (seenIds.current === null) {
            seenIds.current = new Set(ids);
        } else {
            const hasNew = items.some(
                (a) => !seenIds.current!.has(a.id) && !a.dismissed && !a.acknowledged,
            );
            if (hasNew && !mutedRef.current && document.visibilityState === "visible") {
                playAlertSound();
            }
            seenIds.current = new Set(ids);
        }

        // Drop optimistic ids the server has caught up on (dismissed) or that aged
        // out of the unacked list, so the set can't grow unbounded.
        setPending((prev) => {
            if (prev.size === 0) return prev;
            const live = new Map(items.map((a) => [a.id, a]));
            const next = new Set(
                [...prev].filter((id) => live.get(id) && !live.get(id)!.dismissed),
            );
            return next.size === prev.size ? prev : next;
        });
    }, [items]);

    const dismiss = useCallback((id: string) => {
        setPending((prev) => new Set(prev).add(id));
        dismissAlert(id)
            .then(notifyAlertsChanged)
            .catch(() => {
                // Roll back the optimistic hide so the toast doesn't silently vanish.
                setPending((prev) => {
                    const next = new Set(prev);
                    next.delete(id);
                    return next;
                });
            });
    }, []);

    const dismissAll = useCallback(() => {
        setPending((prev) => {
            const next = new Set(prev);
            for (const a of items) next.add(a.id);
            return next;
        });
        dismissAllAlerts()
            .then(notifyAlertsChanged)
            .catch(() => setPending(new Set()));
    }, [items]);

    const toggleMuted = useCallback(() => {
        setMuted((prev) => {
            const next = !prev;
            try {
                if (next) localStorage.setItem(MUTED_KEY, "1");
                else localStorage.removeItem(MUTED_KEY);
            } catch {
                /* private mode — mute just won't persist */
            }
            return next;
        });
    }, []);

    const toasts = items.filter((a) => !a.dismissed && !pending.has(a.id));
    return { toasts, dismiss, dismissAll, muted, toggleMuted };
}
