// Copyright 2026 Appbird LLC. Licensed under "GNU Affero General Public License v3.0"

import { useCallback, useEffect, useMemo, useState } from "react";
import { Pencil, Play, Plus, RefreshCw, Star, Trash2 } from "lucide-react";
import {
    createSearch,
    deleteSearch,
    search,
    updateSearch,
    type SavedSearchPatch,
} from "../api/client";
import { notifySavedChanged } from "../api/events";
import type { SavedSearch } from "../api/types";
import { AdminAllToggle } from "../components/AdminAllToggle";
import { EditSearchDialog, type SavedSearchFormValues } from "../components/EditSearchDialog";
import { VisibilityBadge } from "../components/VisibilityBadge";
import { useAuth } from "../state/useAuth";
import { compactNumber, timeAgo } from "../lib/format";
import { sameAsCurrent } from "../lib/query";
import { SortableTh, sortRows, useSort } from "../lib/sort";
import { useSavedSearches } from "../state/useSavedSearches";
import type { SearchInput } from "../state/url";

interface CountEntry {
    status: "loading" | "ok" | "error";
    total?: number;
    tookUs?: number;
    partitions?: number;
    error?: string;
}

// `/saved` — the saved-searches page. Takes `current`/`onLoad` as props (not
// outlet context); `current` highlights the active row, `onLoad` loads /search.
export function SavedSearchesPanel({
    current,
    onLoad,
}: {
    current: SearchInput;
    onLoad: (s: SearchInput) => void;
}) {
    const isAdmin = !!useAuth().user?.is_admin;
    const [viewAll, setViewAll] = useState(false);
    // Owner column is always shown; View All (admin) only widens the data scope.
    const adminAll = viewAll && isAdmin;
    const { items, error: listError } = useSavedSearches(adminAll);
    const [filter, setFilter] = useState("");
    const [counts, setCounts] = useState<Record<string, CountEntry>>({});
    // id of the row whose edit modal is open; null when no modal is shown.
    const [editingId, setEditingId] = useState<string | null>(null);
    // true while the "new saved search" modal is open.
    const [creating, setCreating] = useState(false);
    // Bumped each time the user hits "refresh counts" so counts re-fetch.
    const [refreshTick, setRefreshTick] = useState(0);

    const fetchOne = useCallback(async (s: SavedSearch) => {
        setCounts((c) => ({ ...c, [s.id]: { status: "loading" } }));
        try {
            const r = await search({
                q: s.q,
                index: s.index ?? undefined,
                start: s.start ?? (s.follow ? "-5m" : s.range),
                end: s.end ?? "now",
                offset: 0,
                limit: 1,
            });
            setCounts((c) => ({
                ...c,
                [s.id]: {
                    status: "ok",
                    total: r.total,
                    tookUs: r.took_us,
                    partitions: r.partitions_scanned,
                },
            }));
        } catch (e: unknown) {
            setCounts((c) => ({
                ...c,
                [s.id]: { status: "error", error: e instanceof Error ? e.message : String(e) },
            }));
        }
    }, []);

    useEffect(() => {
        items.forEach(fetchOne);
        // eslint-disable-next-line react-hooks/exhaustive-deps
    }, [items, refreshTick]);

    const { sort, toggle } = useSort({ key: "updated", dir: "desc" });

    const filtered = useMemo(() => {
        const f = filter.trim().toLowerCase();
        const matched = !f
            ? items
            : items.filter(
                  (s) =>
                      s.name.toLowerCase().includes(f) ||
                      s.q.toLowerCase().includes(f) ||
                      (s.index ?? "").toLowerCase().includes(f),
              );
        return sortRows(matched, sort, {
            name: (s) => s.name,
            owner: (s) => s.owner ?? "",
            query: (s) => s.q,
            visibility: (s) => s.public,
            range: (s) => (s.start && s.end ? "custom" : s.range),
            hits: (s) => counts[s.id]?.total,
            updated: (s) => s.updated_at,
        });
    }, [items, filter, sort, counts]);

    // onLoad navigates to /search with these params baked into the URL.
    const handleLoad = (s: SavedSearch) => {
        onLoad({
            q: s.q,
            index: s.index ?? undefined,
            range: s.range,
            start: s.start ?? undefined,
            end: s.end ?? undefined,
            follow: s.follow,
            page: 1,
        });
    };

    const handleDelete = async (s: SavedSearch) => {
        if (!window.confirm(`Delete "${s.name}"?`)) return;
        try {
            await deleteSearch(s.id);
            notifySavedChanged();
        } catch (e: unknown) {
            window.alert(e instanceof Error ? e.message : String(e));
        }
    };

    const handleEditSave = async (s: SavedSearch, v: SavedSearchFormValues) => {
        // Diff to a patch so only changed fields go up. start/end send `null` to
        // clear, matching the backend's `Option<Option<_>>`. Index is never patched.
        const patch: SavedSearchPatch = {};
        if (v.name !== s.name) patch.name = v.name;
        if (v.q !== s.q) patch.q = v.q;
        if (v.range !== s.range) patch.range = v.range;
        if ((v.start ?? null) !== (s.start ?? null)) patch.start = v.start ?? null;
        if ((v.end ?? null) !== (s.end ?? null)) patch.end = v.end ?? null;
        if (v.follow !== s.follow) patch.follow = v.follow;
        if (v.public !== s.public) patch.public = v.public;
        // Empty patch (no fields changed) — just close. Avoids a no-op round trip.
        if (Object.keys(patch).length === 0) {
            setEditingId(null);
            return;
        }
        try {
            await updateSearch(s.id, patch);
            notifySavedChanged();
            setEditingId(null);
        } catch (e: unknown) {
            window.alert(e instanceof Error ? e.message : String(e));
        }
    };

    const handleCreateSave = async (v: SavedSearchFormValues) => {
        try {
            await createSearch({
                name: v.name,
                q: v.q,
                range: v.range,
                start: v.start,
                end: v.end,
                follow: v.follow,
                public: v.public,
            });
            notifySavedChanged();
            setCreating(false);
        } catch (e: unknown) {
            window.alert(e instanceof Error ? e.message : String(e));
        }
    };

    const totalLoading = Object.values(counts).filter((c) => c.status === "loading").length;
    const editing = editingId ? (items.find((s) => s.id === editingId) ?? null) : null;

    return (
        <div className="px-6 py-8">
            <header className="mb-6">
                <h1 className="font-semibold text-stone-900 dark:text-stone-100">Saved searches</h1>
            </header>

            <SearchesHelpFrame />

            <div className="flex items-center justify-between mb-4 gap-3">
                <input
                    type="text"
                    className="flex-grow max-w-md px-3 py-1.5 bg-white dark:bg-stone-900 border border-stone-200 dark:border-stone-700 rounded-md focus:outline-none focus:border-orange-500"
                    placeholder="filter by name, query, or index…"
                    value={filter}
                    onChange={(e) => setFilter(e.target.value)}
                />
                <div className="flex items-center gap-3">
                    {isAdmin && (
                        <AdminAllToggle checked={viewAll} onChange={setViewAll} noun="searches" />
                    )}
                    <button
                        type="button"
                        className="inline-flex items-center gap-1.5 px-3 py-1.5 font-medium rounded-md border border-stone-200 dark:border-stone-700 text-stone-700 dark:text-stone-300 hover:border-orange-300 hover:bg-orange-50/40 dark:hover:bg-orange-950/30 disabled:opacity-50 transition"
                        onClick={() => setRefreshTick((n) => n + 1)}
                        disabled={totalLoading > 0}
                        title="Re-run all queries to refresh hit counts"
                    >
                        <RefreshCw
                            className={`w-3.5 h-3.5 ${totalLoading > 0 ? "animate-spin" : ""}`}
                        />
                        {totalLoading > 0 ? `refreshing (${totalLoading})…` : "refresh counts"}
                    </button>
                    <button
                        type="button"
                        className="inline-flex items-center gap-1.5 px-3 py-1.5 font-medium rounded-md bg-orange-600 hover:bg-orange-500 text-white transition"
                        onClick={() => setCreating(true)}
                    >
                        <Plus className="w-4 h-4" />
                        New saved search
                    </button>
                </div>
            </div>

            {listError && (
                <div className="mb-4 px-3 py-2 rounded-md bg-red-50 text-red-800 border border-red-200 dark:bg-red-950 dark:text-red-200 dark:border-red-900">
                    {listError}
                </div>
            )}

            <div className="rounded-xl border border-stone-200 dark:border-stone-800 bg-white dark:bg-stone-900 overflow-hidden">
                <table className="w-full">
                    <thead className="bg-stone-50 dark:bg-stone-900/50">
                        <tr>
                            <SortableTh sortKey="name" sort={sort} onSort={toggle}>
                                Name
                            </SortableTh>
                            <SortableTh sortKey="owner" sort={sort} onSort={toggle}>
                                Owner
                            </SortableTh>
                            <SortableTh sortKey="query" sort={sort} onSort={toggle}>
                                Query
                            </SortableTh>
                            <SortableTh sortKey="visibility" sort={sort} onSort={toggle}>
                                Visibility
                            </SortableTh>
                            <SortableTh sortKey="range" sort={sort} onSort={toggle}>
                                Range
                            </SortableTh>
                            <SortableTh sortKey="hits" sort={sort} onSort={toggle} align="right">
                                Hits
                            </SortableTh>
                            <SortableTh sortKey="updated" sort={sort} onSort={toggle}>
                                Updated
                            </SortableTh>
                            <th className="border-b border-stone-200 dark:border-stone-800" />
                        </tr>
                    </thead>
                    <tbody className="divide-y divide-stone-100 dark:divide-stone-800">
                        {filtered.length === 0 && (
                            <tr>
                                <td
                                    colSpan={8}
                                    className="px-3 py-8 text-center text-stone-400 dark:text-stone-500"
                                >
                                    {filter
                                        ? "no matches"
                                        : "no saved searches yet — go to the search page, build a query, and click ☆"}
                                </td>
                            </tr>
                        )}
                        {filtered.map((s) => {
                            const isActive = sameAsCurrent(s, current);
                            const c = counts[s.id];
                            return (
                                <tr
                                    key={s.id}
                                    className={`group cursor-pointer transition-colors ${
                                        isActive
                                            ? "bg-orange-50/50 dark:bg-orange-950/30 hover:bg-orange-100/60 dark:hover:bg-orange-900/40"
                                            : "hover:bg-orange-50/40 dark:hover:bg-orange-950/20"
                                    }`}
                                    onClick={(e) => {
                                        if ((e.target as HTMLElement).closest("button, input"))
                                            return;
                                        handleLoad(s);
                                    }}
                                    title="click to load this search"
                                >
                                    <td className="px-3 py-2 align-top">
                                        <div className="flex items-center gap-2">
                                            <strong className="text-stone-900 dark:text-stone-100">
                                                {s.name}
                                            </strong>
                                            {isActive && (
                                                <span className="uppercase tracking-wider px-1.5 py-0.5 rounded bg-orange-100 text-orange-800 dark:bg-orange-900/50 dark:text-orange-200">
                                                    active
                                                </span>
                                            )}
                                        </div>
                                    </td>
                                    <td className="px-3 py-2 align-top text-stone-600 dark:text-stone-300">
                                        {s.owner ?? "—"}
                                    </td>
                                    <td className="px-3 py-2 align-top">
                                        <code className="font-mono text-stone-700 dark:text-stone-300 line-clamp-2 break-all">
                                            {s.q || "*"}
                                        </code>
                                    </td>
                                    <td className="px-3 py-2 align-top">
                                        <VisibilityBadge isPublic={s.public} />
                                    </td>
                                    <td className="px-3 py-2 align-top text-stone-700 dark:text-stone-300">
                                        {s.start && s.end ? (
                                            <span
                                                className="text-stone-400 dark:text-stone-500"
                                                title={`${s.start} → ${s.end}`}
                                            >
                                                custom
                                            </span>
                                        ) : (
                                            s.range
                                        )}
                                        {s.follow && (
                                            <span className="ml-1 px-1.5 py-0.5 rounded bg-green-50 text-green-700 dark:bg-green-950/40 dark:text-green-300">
                                                live
                                            </span>
                                        )}
                                    </td>
                                    <td className="px-3 py-2 align-top text-right tabular-nums">
                                        {!c || c.status === "loading" ? (
                                            <span className="text-stone-400">…</span>
                                        ) : c.status === "error" ? (
                                            <span
                                                className="text-red-700 dark:text-red-300"
                                                title={c.error}
                                            >
                                                err
                                            </span>
                                        ) : (
                                            <span
                                                title={`${c.total?.toLocaleString()} hits across ${c.partitions ?? 0} partitions in ${c.tookUs ?? 0}µs`}
                                            >
                                                <strong>{compactNumber(c.total ?? 0)}</strong>
                                            </span>
                                        )}
                                    </td>
                                    <td
                                        className="px-3 py-2 align-top text-stone-500 dark:text-stone-400"
                                        title={s.updated_at}
                                    >
                                        {timeAgo(s.updated_at)}
                                    </td>
                                    <td className="px-3 py-2 align-top whitespace-nowrap text-right">
                                        <div className="inline-flex items-center gap-2">
                                            <button
                                                type="button"
                                                className="p-1 rounded text-orange-600 dark:text-orange-400 opacity-0 group-hover:opacity-100 hover:bg-orange-100 dark:hover:bg-orange-900/40 transition-opacity"
                                                onClick={(e) => {
                                                    e.stopPropagation();
                                                    handleLoad(s);
                                                }}
                                                title="Load this search"
                                                aria-label={`load ${s.name}`}
                                            >
                                                <Play className="w-3 h-3" fill="currentColor" />
                                            </button>
                                            <button
                                                type="button"
                                                className="p-1 rounded text-stone-800 dark:text-stone-200 hover:text-stone-900 hover:bg-stone-100 dark:hover:bg-stone-700"
                                                onClick={(e) => {
                                                    e.stopPropagation();
                                                    setEditingId(s.id);
                                                }}
                                                title="Edit"
                                                aria-label={`edit ${s.name}`}
                                            >
                                                <Pencil className="w-3.5 h-3.5" />
                                            </button>
                                            <button
                                                type="button"
                                                className="p-1 rounded text-stone-800 dark:text-stone-200 hover:text-red-700 dark:hover:text-red-300 hover:bg-red-50 dark:hover:bg-red-950/30"
                                                onClick={(e) => {
                                                    e.stopPropagation();
                                                    handleDelete(s);
                                                }}
                                                title="Delete"
                                            >
                                                <Trash2 className="w-3.5 h-3.5" />
                                            </button>
                                        </div>
                                    </td>
                                </tr>
                            );
                        })}
                    </tbody>
                </table>
            </div>

            {editing && (
                <EditSearchDialog
                    search={editing}
                    onSave={(v) => handleEditSave(editing, v)}
                    onClose={() => setEditingId(null)}
                />
            )}

            {creating && (
                <EditSearchDialog onSave={handleCreateSave} onClose={() => setCreating(false)} />
            )}
        </div>
    );
}

// Inline help banner — mirrors the orange-tinted treatment of the Admin panels.
function SearchesHelpFrame() {
    return (
        <div className="mb-6 flex gap-3 p-4 rounded-lg bg-orange-50/60 dark:bg-orange-950/20 border border-orange-200/70 dark:border-orange-900/40">
            <div className="flex-shrink-0 mt-0.5">
                <Star className="w-4 h-4 text-orange-600 dark:text-orange-400" />
            </div>
            <p className="text-stone-700 dark:text-stone-200 leading-relaxed">
                Saved queries you can re-run with one click. Click the{" "}
                <Star className="inline w-3 h-3 align-text-bottom" /> in the search bar to save the
                current view, or hit <strong>New saved search</strong> to build one here; click a
                row to load it back.
            </p>
        </div>
    );
}
