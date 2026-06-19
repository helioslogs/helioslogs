// Copyright 2026 Appbird LLC. Licensed under "GNU Affero General Public License v3.0"

import { useCallback, useEffect, useState } from "react";
import { listMonitors } from "../api/client";
import { onMonitorsChanged } from "../api/events";
import type { Monitor } from "../api/types";

// Subscribes to the monitor list (mount + `helios-monitors-changed` + fast poll).
// 4s poll surfaces scheduler-driven run/finish transitions within a few seconds.
const MONITOR_POLL_MS = 4000;

export function useMonitors(all = false) {
    const [items, setItems] = useState<Monitor[]>([]);
    const [error, setError] = useState<string | null>(null);

    const refresh = useCallback(() => {
        listMonitors(all)
            .then((xs) => {
                setItems(xs);
                setError(null);
            })
            .catch((e: unknown) => setError(e instanceof Error ? e.message : String(e)));
    }, [all]);

    useEffect(() => {
        refresh();
        const off = onMonitorsChanged(refresh);
        const handle = setInterval(refresh, MONITOR_POLL_MS);
        return () => {
            off();
            clearInterval(handle);
        };
    }, [refresh]);

    return { items, error, refresh };
}
