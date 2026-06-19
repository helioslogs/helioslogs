// Copyright 2026 Appbird LLC. Licensed under "GNU Affero General Public License v3.0"

import { useEffect, useState } from "react";
import { Link } from "react-router-dom";
import { ExternalLink, Search as SearchIcon } from "lucide-react";
import { listSearches } from "../../api/client";
import type { SavedSearch, Widget } from "../../api/types";
import { searchHref } from "../../state/url";

interface Props {
    widget: Widget;
    // Bumped on dashboard refresh / env change so the (env-scoped) list reloads.
    refreshKey: number;
}

// Saved-search shortcuts. Saved searches are env-scoped, so this reflects the
// active env and reloads when the dashboard refreshes.
export function SavedSearchesWidget({ widget, refreshKey }: Props) {
    const [items, setItems] = useState<SavedSearch[]>([]);
    const [error, setError] = useState<string | null>(null);
    const limit = widget.limit || 10;

    useEffect(() => {
        let cancelled = false;
        listSearches()
            .then((xs) => {
                if (!cancelled) {
                    setItems(xs);
                    setError(null);
                }
            })
            .catch((e: unknown) => {
                if (!cancelled) setError(e instanceof Error ? e.message : String(e));
            });
        return () => {
            cancelled = true;
        };
    }, [refreshKey]);

    if (error) return <p className="text-sm text-red-600 dark:text-red-300">{error}</p>;
    if (items.length === 0) {
        return <p className="text-sm text-stone-400 dark:text-stone-500">no saved searches</p>;
    }

    return (
        <ul className="divide-y divide-stone-100 dark:divide-stone-800 -my-1">
            {items.slice(0, limit).map((s) => (
                <li key={s.id}>
                    <Link
                        to={searchHref({
                            q: s.q,
                            range: s.range,
                            follow: s.follow,
                            start: s.start ?? undefined,
                            end: s.end ?? undefined,
                            index: s.index ?? undefined,
                        })}
                        className="group flex items-start gap-2 py-1.5 hover:bg-stone-50 dark:hover:bg-stone-800/60 rounded px-1 -mx-1"
                    >
                        <SearchIcon className="w-3.5 h-3.5 shrink-0 mt-0.5 text-stone-500 dark:text-stone-400" />
                        <span className="min-w-0 flex-1">
                            <span className="block text-sm font-medium text-stone-900 dark:text-stone-100 truncate group-hover:underline">
                                {s.name}
                            </span>
                            <span className="block text-xs text-stone-600 dark:text-stone-300 font-mono truncate">
                                {s.q || "*"}
                            </span>
                        </span>
                        <ExternalLink
                            className="w-4 h-4 shrink-0 mt-0.5 text-stone-400 dark:text-stone-500 group-hover:text-orange-600 dark:group-hover:text-orange-400"
                            aria-hidden="true"
                        />
                    </Link>
                </li>
            ))}
        </ul>
    );
}
