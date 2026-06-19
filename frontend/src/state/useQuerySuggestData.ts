// Copyright 2026 Appbird LLC. Licensed under "GNU Affero General Public License v3.0"

import { useEffect, useState } from "react";
import { discoverFields, getIndexes } from "../api/client";
import type { DiscoveredField } from "../api/types";

// Loads the field catalog + index list over a default window so forms without their
// own search context can drive <QueryInput> autocomplete. Failures degrade to empty.
export function useQuerySuggestData(): {
    fields: DiscoveredField[];
    indexes: string[];
    start: string;
    end: string;
} {
    const start = "-24h";
    const end = "now";
    const [fields, setFields] = useState<DiscoveredField[]>([]);
    const [indexes, setIndexes] = useState<string[]>([]);

    useEffect(() => {
        let cancelled = false;
        const ctrl = new AbortController();
        discoverFields({ q: "*", start, end, top: 200 }, { signal: ctrl.signal })
            .then((r) => !cancelled && setFields(r.fields))
            .catch(() => {});
        getIndexes()
            .then((i) => !cancelled && setIndexes(i))
            .catch(() => {});
        return () => {
            cancelled = true;
            ctrl.abort();
        };
    }, []);

    return { fields, indexes, start, end };
}
