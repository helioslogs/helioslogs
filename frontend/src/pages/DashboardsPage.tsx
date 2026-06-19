// Copyright 2026 Appbird LLC. Licensed under "GNU Affero General Public License v3.0"

import { useMemo, useState } from "react";
import { useNavigate } from "react-router-dom";
import { ExternalLink, LayoutDashboard, Plus, Trash2 } from "lucide-react";
import { useDashboards } from "../state/useDashboards";
import { timeAgo } from "../lib/format";
import { SortableTh, sortRows, useSort } from "../lib/sort";
import { VisibilityBadge } from "../components/VisibilityBadge";
import { AdminAllToggle } from "../components/AdminAllToggle";
import { useAuth } from "../state/useAuth";
import type { Dashboard } from "../api/types";

export function DashboardsPage() {
    const isAdmin = !!useAuth().user?.is_admin;
    const [viewAll, setViewAll] = useState(false);
    // Owner column is always shown; View All (admin) only widens the data scope.
    const adminAll = viewAll && isAdmin;
    const { items, error, loading, remove } = useDashboards(adminAll);
    const [filter, setFilter] = useState("");
    const navigate = useNavigate();

    const { sort, toggle } = useSort({ key: "updated", dir: "desc" });

    const rows = useMemo(() => {
        const f = filter.trim().toLowerCase();
        const matched = !f ? items : items.filter((d) => d.name.toLowerCase().includes(f));
        return sortRows<Dashboard>(matched, sort, {
            name: (d) => d.name,
            owner: (d) => d.owner ?? "",
            visibility: (d) => d.public,
            widgets: (d) => d.spec?.widgets?.length ?? 0,
            updated: (d) => d.updated_at,
        });
    }, [items, filter, sort]);

    return (
        <div className="px-6 py-8">
            <header className="mb-6">
                <h1 className="font-semibold text-stone-900 dark:text-stone-100">Dashboards</h1>
            </header>

            <div className="mb-6 flex gap-3 p-4 rounded-lg bg-orange-50/60 dark:bg-orange-950/20 border border-orange-200/70 dark:border-orange-900/40">
                <div className="flex-shrink-0 mt-0.5">
                    <LayoutDashboard className="w-4 h-4 text-orange-600 dark:text-orange-400" />
                </div>
                <p className="text-stone-700 dark:text-stone-200 leading-relaxed">
                    Charts and widgets over your searches and alerts. Plot match counts over time,
                    overlay multiple queries, break a field down by value, and drop in live lists.
                    Query widgets run against the active environment; hit{" "}
                    <strong>New dashboard</strong> to start one.
                </p>
            </div>

            <div className="flex items-center justify-between mb-4 gap-3">
                <input
                    type="text"
                    className="flex-grow max-w-md px-3 py-1.5 bg-white dark:bg-stone-900 border border-stone-200 dark:border-stone-700 rounded-md focus:outline-none focus:border-orange-500"
                    placeholder="filter by name…"
                    value={filter}
                    onChange={(e) => setFilter(e.target.value)}
                />
                <div className="flex items-center gap-3">
                    {isAdmin && (
                        <AdminAllToggle checked={viewAll} onChange={setViewAll} noun="dashboards" />
                    )}
                    <button
                        type="button"
                        onClick={() => navigate("/dashboards/new")}
                        className="inline-flex items-center gap-1.5 px-3 py-1.5 font-medium rounded-md bg-orange-600 hover:bg-orange-500 text-white transition"
                    >
                        <Plus className="w-4 h-4" />
                        New dashboard
                    </button>
                </div>
            </div>

            {error && (
                <div className="mb-4 px-3 py-2 rounded-md bg-red-50 text-red-800 border border-red-200 dark:bg-red-950 dark:text-red-200 dark:border-red-900">
                    {error}
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
                            <SortableTh sortKey="visibility" sort={sort} onSort={toggle}>
                                Visibility
                            </SortableTh>
                            <SortableTh sortKey="widgets" sort={sort} onSort={toggle} align="right">
                                Widgets
                            </SortableTh>
                            <SortableTh sortKey="updated" sort={sort} onSort={toggle}>
                                Updated
                            </SortableTh>
                            <th className="border-b border-stone-200 dark:border-stone-800" />
                        </tr>
                    </thead>
                    <tbody className="divide-y divide-stone-100 dark:divide-stone-800">
                        {!loading && rows.length === 0 && (
                            <tr>
                                <td
                                    colSpan={6}
                                    className="px-3 py-8 text-center text-stone-400 dark:text-stone-500"
                                >
                                    {filter
                                        ? "no matches"
                                        : "no dashboards yet — hit New dashboard to create one"}
                                </td>
                            </tr>
                        )}
                        {loading && (
                            <tr>
                                <td
                                    colSpan={6}
                                    className="px-3 py-8 text-center text-stone-400 dark:text-stone-500"
                                >
                                    loading…
                                </td>
                            </tr>
                        )}
                        {rows.map((d) => (
                            <tr
                                key={d.id}
                                className="group cursor-pointer transition-colors hover:bg-orange-50/40 dark:hover:bg-orange-950/20"
                                onClick={(e) => {
                                    if ((e.target as HTMLElement).closest("button, input")) return;
                                    navigate(`/dashboards/${encodeURIComponent(d.id)}`);
                                }}
                                title="click to open this dashboard"
                            >
                                <td className="px-3 py-2 align-top">
                                    <div className="flex items-center gap-2">
                                        <LayoutDashboard className="w-4 h-4 text-orange-500 shrink-0" />
                                        <strong className="text-stone-900 dark:text-stone-100">
                                            {d.name}
                                        </strong>
                                    </div>
                                </td>
                                <td className="px-3 py-2 align-top text-stone-600 dark:text-stone-300">
                                    {d.owner ?? "—"}
                                </td>
                                <td className="px-3 py-2 align-top">
                                    <VisibilityBadge isPublic={d.public} />
                                </td>
                                <td className="px-3 py-2 align-top text-right tabular-nums text-stone-700 dark:text-stone-300">
                                    {d.spec?.widgets?.length ?? 0}
                                </td>
                                <td
                                    className="px-3 py-2 align-top text-stone-500 dark:text-stone-400"
                                    title={d.updated_at}
                                >
                                    {timeAgo(d.updated_at)}
                                </td>
                                <td className="px-3 py-2 align-top whitespace-nowrap text-right">
                                    <div className="inline-flex items-center gap-2">
                                        <button
                                            type="button"
                                            className="p-1 rounded text-orange-600 dark:text-orange-400 opacity-0 group-hover:opacity-100 hover:bg-orange-100 dark:hover:bg-orange-900/40 transition-opacity"
                                            onClick={(e) => {
                                                e.stopPropagation();
                                                navigate(`/dashboards/${encodeURIComponent(d.id)}`);
                                            }}
                                            title="Open dashboard"
                                            aria-label={`open ${d.name}`}
                                        >
                                            <ExternalLink className="w-3.5 h-3.5" />
                                        </button>
                                        <button
                                            type="button"
                                            className="p-1 rounded text-stone-800 dark:text-stone-200 hover:text-red-700 dark:hover:text-red-300 hover:bg-red-50 dark:hover:bg-red-950/30"
                                            onClick={(e) => {
                                                e.stopPropagation();
                                                if (confirm(`Delete dashboard "${d.name}"?`))
                                                    void remove(d.id);
                                            }}
                                            title="Delete"
                                        >
                                            <Trash2 className="w-3.5 h-3.5" />
                                        </button>
                                    </div>
                                </td>
                            </tr>
                        ))}
                    </tbody>
                </table>
            </div>
        </div>
    );
}
