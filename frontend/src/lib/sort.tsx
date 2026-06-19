// Copyright 2026 Appbird LLC. Licensed under "GNU Affero General Public License v3.0"

// Tiny reusable client-side table sorting: a `useSort` hook for the
// (column, direction) state, `sortRows` to apply it via per-column accessors,
// and a `SortableTh` header cell that toggles sort + shows the direction.

import { useState, type ReactNode } from "react";
import { ChevronDown, ChevronsUpDown, ChevronUp } from "lucide-react";

export type SortDir = "asc" | "desc";
export interface SortState {
    key: string | null;
    dir: SortDir;
}

export function useSort(initial: SortState = { key: null, dir: "asc" }) {
    const [sort, setSort] = useState<SortState>(initial);
    const toggle = (key: string) =>
        setSort((s) =>
            s.key === key ? { key, dir: s.dir === "asc" ? "desc" : "asc" } : { key, dir: "asc" },
        );
    return { sort, toggle };
}

type Cell = string | number | boolean | null | undefined;

// Return a new sorted array. Nullish values always sort to the bottom,
// regardless of direction, so empty cells don't crowd the top.
export function sortRows<T>(
    rows: T[],
    sort: SortState,
    accessors: Record<string, (r: T) => Cell>,
): T[] {
    if (!sort.key) return rows;
    const acc = accessors[sort.key];
    if (!acc) return rows;
    const dir = sort.dir === "asc" ? 1 : -1;
    return [...rows].sort((a, b) => {
        const av = acc(a);
        const bv = acc(b);
        const an = av == null || av === "";
        const bn = bv == null || bv === "";
        if (an && bn) return 0;
        if (an) return 1;
        if (bn) return -1;
        return (
            dir *
            compare(av as Exclude<Cell, null | undefined>, bv as Exclude<Cell, null | undefined>)
        );
    });
}

function compare(a: string | number | boolean, b: string | number | boolean): number {
    if (typeof a === "number" && typeof b === "number") return a - b;
    if (typeof a === "boolean" && typeof b === "boolean") return a === b ? 0 : a ? 1 : -1;
    return String(a).localeCompare(String(b), undefined, { numeric: true, sensitivity: "base" });
}

export function SortableTh({
    children,
    sortKey,
    sort,
    onSort,
    align = "left",
}: {
    children?: ReactNode;
    sortKey: string;
    sort: SortState;
    onSort: (key: string) => void;
    align?: "left" | "right";
}) {
    const active = sort.key === sortKey;
    return (
        <th
            onClick={() => onSort(sortKey)}
            className={`px-3 py-2 font-semibold uppercase tracking-wider text-stone-500 dark:text-stone-400 border-b border-stone-200 dark:border-stone-800 cursor-pointer select-none hover:text-stone-700 dark:hover:text-stone-200 ${
                align === "right" ? "text-right" : "text-left"
            }`}
            aria-sort={active ? (sort.dir === "asc" ? "ascending" : "descending") : "none"}
        >
            <span
                className={`inline-flex items-center gap-1 ${align === "right" ? "flex-row-reverse" : ""}`}
            >
                {children}
                {active ? (
                    sort.dir === "asc" ? (
                        <ChevronUp className="w-3 h-3" />
                    ) : (
                        <ChevronDown className="w-3 h-3" />
                    )
                ) : (
                    <ChevronsUpDown className="w-3 h-3 opacity-30" />
                )}
            </span>
        </th>
    );
}
