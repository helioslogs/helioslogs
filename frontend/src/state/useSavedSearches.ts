// Copyright 2026 Appbird LLC. Licensed under "GNU Affero General Public License v3.0"

import { useCallback, useEffect, useState } from "react";
import { listSearches } from "../api/client";
import { onSavedChanged } from "../api/events";
import type { SavedSearch } from "../api/types";

// Subscribes to the saved-search list — fetches on mount, re-fetches
// after any local mutation via the `helios-saved-changed` event.
export function useSavedSearches(all = false) {
    const [items, setItems] = useState<SavedSearch[]>([]);
    const [error, setError] = useState<string | null>(null);
    const refresh = useCallback(() => {
        listSearches(all)
            .then((xs) => {
                setItems(xs);
                setError(null);
            })
            .catch((e: unknown) => setError(e instanceof Error ? e.message : String(e)));
    }, [all]);
    useEffect(() => {
        refresh();
        return onSavedChanged(refresh);
    }, [refresh]);
    return { items, error, refresh };
}
