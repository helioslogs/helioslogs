// Copyright 2026 Appbird LLC. Licensed under "GNU Affero General Public License v3.0"

import { useCallback, useEffect, useState } from "react";
import { getUnackedAlertCount, listAlerts } from "../api/client";
import { onAlertsChanged } from "../api/events";
import type { Alert } from "../api/types";

// Fast poll so a newly-fired alert lands within a couple seconds ("fire and see").
const ALERT_POLL_MS = 5000;

// Cap on rows per view (filtering is server-side); exposed so the UI can hint truncation.
export const ALERT_LIMIT = 100;

export function useAlerts(unackedOnly: boolean, search = "", monitor: string | null = null) {
    const [items, setItems] = useState<Alert[]>([]);
    const [error, setError] = useState<string | null>(null);

    const refresh = useCallback(() => {
        listAlerts({ unackedOnly, search, monitor, limit: ALERT_LIMIT })
            .then((xs) => {
                setItems(xs);
                setError(null);
            })
            .catch((e: unknown) => setError(e instanceof Error ? e.message : String(e)));
    }, [unackedOnly, search, monitor]);

    useEffect(() => {
        refresh();
        const off = onAlertsChanged(refresh);
        const handle = setInterval(refresh, ALERT_POLL_MS);
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
        const refresh = () => {
            getUnackedAlertCount()
                .then((n) => {
                    if (!cancelled) setCount(n);
                })
                .catch(() => {
                    /* swallow — the badge just stays stale */
                });
        };
        refresh();
        const off = onAlertsChanged(refresh);
        const handle = setInterval(refresh, COUNT_POLL_MS);
        return () => {
            cancelled = true;
            off();
            clearInterval(handle);
        };
    }, []);

    return count;
}
