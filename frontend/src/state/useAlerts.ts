// Copyright 2026 Appbird LLC. Licensed under "GNU Affero General Public License v3.0"

import { useCallback, useEffect, useRef, useState } from "react";
import { getUnackedAlertCount, listAlerts } from "../api/client";
import { onAlertsChanged } from "../api/events";
import type { Alert } from "../api/types";

// Background poll for scheduler-fired alerts; local actions refresh via `helios-alerts-changed`.
const ALERT_POLL_MS = 15000;

// Cap on rows per view (filtering is server-side); exposed so the UI can hint truncation.
export const ALERT_LIMIT = 100;

export function useAlerts(unackedOnly: boolean, search = "", monitor: string | null = null) {
    const [items, setItems] = useState<Alert[]>([]);
    const [error, setError] = useState<string | null>(null);
    const seq = useRef(0);
    const inFlight = useRef(false);

    const refresh = useCallback(() => {
        const mine = ++seq.current;
        inFlight.current = true;
        listAlerts({ unackedOnly, search, monitor, limit: ALERT_LIMIT })
            .then((xs) => {
                if (mine !== seq.current) return;
                setItems(xs);
                setError(null);
            })
            .catch((e: unknown) => {
                if (mine === seq.current) setError(e instanceof Error ? e.message : String(e));
            })
            .finally(() => {
                if (mine === seq.current) inFlight.current = false;
            });
    }, [unackedOnly, search, monitor]);

    useEffect(() => {
        refresh();
        const off = onAlertsChanged(refresh);
        const handle = setInterval(() => {
            if (!inFlight.current) refresh();
        }, ALERT_POLL_MS);
        return () => {
            off();
            clearInterval(handle);
        };
    }, [refresh]);

    return { items, error, refresh, limit: ALERT_LIMIT };
}

// Nav-badge count. Polls slowly (badge is a hint); `helios-alerts-changed`
// covers local actions, so the poll only needs to catch scheduler-fired alerts.
const COUNT_POLL_MS = 30000;

export function useUnackedAlertCount(): number {
    const [count, setCount] = useState(0);

    useEffect(() => {
        let cancelled = false;
        let inFlight = false;
        const refresh = () => {
            inFlight = true;
            getUnackedAlertCount()
                .then((n) => {
                    if (!cancelled) setCount(n);
                })
                .catch(() => {
                    /* swallow — the badge just stays stale */
                })
                .finally(() => {
                    inFlight = false;
                });
        };
        refresh();
        const off = onAlertsChanged(refresh);
        const handle = setInterval(() => {
            if (!inFlight) refresh();
        }, COUNT_POLL_MS);
        return () => {
            cancelled = true;
            off();
            clearInterval(handle);
        };
    }, []);

    return count;
}
