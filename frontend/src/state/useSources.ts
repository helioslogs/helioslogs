// Copyright 2026 Appbird LLC. Licensed under "GNU Affero General Public License v3.0"

import { useCallback, useEffect, useState } from "react";
import { listSources } from "../api/client";
import { onSourcesChanged } from "../api/events";
import type { Source } from "../api/types";

// Subscribes to the ingestion-source list (mount + `helios-sources-changed` + fast poll).
// 4s poll surfaces the supervisor's run status (it ticks every 5s) within seconds.
const SOURCE_POLL_MS = 4000;

export function useSources() {
    const [items, setItems] = useState<Source[]>([]);
    const [error, setError] = useState<string | null>(null);

    const refresh = useCallback(() => {
        listSources()
            .then((xs) => {
                setItems(xs);
                setError(null);
            })
            .catch((e: unknown) => setError(e instanceof Error ? e.message : String(e)));
    }, []);

    useEffect(() => {
        refresh();
        const off = onSourcesChanged(refresh);
        const handle = setInterval(refresh, SOURCE_POLL_MS);
        return () => {
            off();
            clearInterval(handle);
        };
    }, [refresh]);

    return { items, error, refresh };
}
