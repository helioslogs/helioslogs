// Copyright 2026 Appbird LLC. Licensed under "GNU Affero General Public License v3.0"

import { useCallback, useEffect, useState } from "react";
import { createDashboard, deleteDashboard, listDashboards } from "../api/client";
import type { Dashboard, DashboardInput } from "../api/types";

// Subscribes to the dashboard list. No polling — dashboards only change on local
// create/delete, so we just refetch after each mutation.
export function useDashboards(all = false) {
    const [items, setItems] = useState<Dashboard[]>([]);
    const [error, setError] = useState<string | null>(null);
    const [loading, setLoading] = useState(true);

    const refresh = useCallback(() => {
        listDashboards(all)
            .then((xs) => {
                setItems(xs);
                setError(null);
            })
            .catch((e: unknown) => setError(e instanceof Error ? e.message : String(e)))
            .finally(() => setLoading(false));
    }, [all]);

    useEffect(() => {
        refresh();
    }, [refresh]);

    const create = useCallback(
        async (input: DashboardInput) => {
            const d = await createDashboard(input);
            refresh();
            return d;
        },
        [refresh],
    );

    const remove = useCallback(
        async (id: string) => {
            await deleteDashboard(id);
            refresh();
        },
        [refresh],
    );

    return { items, error, loading, refresh, create, remove };
}
