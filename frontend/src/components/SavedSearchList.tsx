// Copyright 2026 Appbird LLC. Licensed under "GNU Affero General Public License v3.0"

import { useCallback, useMemo, useState } from "react";
import { Pencil, X } from "lucide-react";
import { deleteSearch, updateSearch } from "../api/client";
import { notifySavedChanged } from "../api/events";
import type { SavedSearch } from "../api/types";
import { sameAsCurrent } from "../lib/query";
import { useSavedSearches } from "../state/useSavedSearches";
import type { SearchInput } from "../state/url";

interface Props {
    current: SearchInput;
    onLoad: (s: SearchInput) => void;
    // Optional client-side text filter applied to name + query.
    filter?: string;
    // Show the inline rename action on each row (page mode). Defaults to false.
    allowRename?: boolean;
    // Max items to show — popover uses ~20, page uses Infinity.
    limit?: number;
    // "no saved searches yet" copy override.
    emptyText?: string;
}

export function SavedSearchList({
    current,
    onLoad,
    filter,
    allowRename,
    limit = Infinity,
    emptyText = "no saved searches yet",
}: Props) {
    const { items, error } = useSavedSearches();
    const [renamingId, setRenamingId] = useState<string | null>(null);
    const [draft, setDraft] = useState("");

    const filtered = useMemo(() => {
        const f = (filter ?? "").trim().toLowerCase();
        const base = f
            ? items.filter(
                  (s) =>
                      s.name.toLowerCase().includes(f) ||
                      s.q.toLowerCase().includes(f) ||
                      (s.index ?? "").toLowerCase().includes(f),
              )
            : items;
        return Number.isFinite(limit) ? base.slice(0, limit as number) : base;
    }, [items, filter, limit]);

    const handleClick = useCallback(
        (s: SavedSearch) => {
            if (renamingId === s.id) return;
            onLoad({
                q: s.q,
                index: s.index ?? undefined,
                range: s.range,
                start: s.start ?? undefined,
                end: s.end ?? undefined,
                follow: s.follow,
                page: 1,
            });
        },
        [onLoad, renamingId],
    );

    const handleDelete = useCallback(async (ev: React.MouseEvent, s: SavedSearch) => {
        ev.stopPropagation();
        if (!window.confirm(`Delete "${s.name}"?`)) return;
        try {
            await deleteSearch(s.id);
            notifySavedChanged();
        } catch (e: unknown) {
            window.alert(e instanceof Error ? e.message : String(e));
        }
    }, []);

    const startRename = useCallback((ev: React.MouseEvent, s: SavedSearch) => {
        ev.stopPropagation();
        setRenamingId(s.id);
        setDraft(s.name);
    }, []);

    const commitRename = useCallback(
        async (s: SavedSearch) => {
            const next = draft.trim();
            setRenamingId(null);
            if (!next || next === s.name) return;
            try {
                await updateSearch(s.id, { name: next });
                notifySavedChanged();
            } catch (e: unknown) {
                window.alert(e instanceof Error ? e.message : String(e));
            }
        },
        [draft],
    );

    if (error) {
        return <div className="px-3 py-2 text-red-700 dark:text-red-300">{error}</div>;
    }
    if (filtered.length === 0) {
        return (
            <div className="px-3 py-4 text-stone-400 dark:text-stone-500 text-center italic">
                {filter ? "no matches" : emptyText}
            </div>
        );
    }

    return (
        <ul>
            {filtered.map((s) => {
                const isActive = sameAsCurrent(s, current);
                const isRenaming = renamingId === s.id;
                return (
                    <li
                        key={s.id}
                        className={`group px-3 py-2 cursor-pointer border-l-2 ${
                            isActive
                                ? "border-orange-500 bg-orange-50/50 dark:bg-orange-950/30"
                                : "border-transparent hover:bg-stone-50 dark:hover:bg-stone-800/40"
                        }`}
                        onClick={() => handleClick(s)}
                        title={`${s.q || "*"}  ·  ${s.index ? `index=${s.index}  ·  ` : ""}${s.range}`}
                    >
                        <div className="flex items-center gap-2">
                            {isRenaming ? (
                                <input
                                    className="flex-grow px-1.5 py-0.5 bg-white dark:bg-stone-950 border border-stone-300 dark:border-stone-600 rounded focus:outline-none focus:border-orange-500"
                                    value={draft}
                                    onChange={(e) => setDraft(e.target.value)}
                                    onClick={(e) => e.stopPropagation()}
                                    onKeyDown={(e) => {
                                        if (e.key === "Enter") commitRename(s);
                                        if (e.key === "Escape") setRenamingId(null);
                                    }}
                                    onBlur={() => commitRename(s)}
                                    autoFocus
                                />
                            ) : (
                                <span className="flex-grow font-medium text-stone-900 dark:text-stone-100 truncate">
                                    {s.name}
                                </span>
                            )}
                            <div
                                className="flex items-center gap-0.5 opacity-0 group-hover:opacity-100"
                                onClick={(e) => e.stopPropagation()}
                            >
                                {allowRename && !isRenaming && (
                                    <button
                                        type="button"
                                        className="p-1 rounded text-stone-400 hover:text-stone-700 dark:hover:text-stone-200 hover:bg-stone-100 dark:hover:bg-stone-700"
                                        onClick={(e) => startRename(e, s)}
                                        title="Rename"
                                    >
                                        <Pencil className="w-3 h-3" />
                                    </button>
                                )}
                                <button
                                    type="button"
                                    className="p-1 rounded text-stone-400 hover:text-red-700 dark:hover:text-red-300 hover:bg-red-50 dark:hover:bg-red-950/30"
                                    onClick={(e) => handleDelete(e, s)}
                                    aria-label={`delete ${s.name}`}
                                    title="Delete"
                                >
                                    <X className="w-3 h-3" />
                                </button>
                            </div>
                        </div>
                        <div className="mt-0.5 flex items-center gap-2 text-stone-500 dark:text-stone-400">
                            <code className="font-mono truncate flex-grow">{s.q || "*"}</code>
                            <span className="flex-shrink-0">
                                {s.index ? `${s.index} · ` : ""}
                                {s.range}
                            </span>
                        </div>
                    </li>
                );
            })}
        </ul>
    );
}
