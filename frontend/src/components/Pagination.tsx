// Copyright 2026 Appbird LLC. Licensed under "GNU Affero General Public License v3.0"

import { ChevronLeft, ChevronRight } from "lucide-react";

interface Props {
    page: number; // 1-based
    totalPages: number;
    onChange: (page: number) => void;
    disabled?: boolean;
}

// Pages to render given current page + total. Always shows first, last,
// current ± 2, with "…" for any gap.
function pageList(current: number, total: number, span = 2): Array<number | "…"> {
    if (total <= 1) return [];
    if (total <= 7 + span * 2) {
        return Array.from({ length: total }, (_, i) => i + 1);
    }
    const set = new Set<number>([1, total, current]);
    for (let i = current - span; i <= current + span; i++) {
        if (i >= 1 && i <= total) set.add(i);
    }
    const sorted = [...set].sort((a, b) => a - b);
    const result: Array<number | "…"> = [];
    for (let i = 0; i < sorted.length; i++) {
        if (i > 0 && sorted[i] - sorted[i - 1] > 1) result.push("…");
        result.push(sorted[i]);
    }
    return result;
}

const BTN_BASE =
    "inline-flex items-center justify-center min-w-[28px] h-7 px-2 rounded-md border border-stone-200 dark:border-stone-700 bg-white dark:bg-stone-900 text-stone-700 dark:text-stone-300 hover:border-orange-300 hover:bg-orange-50/40 dark:hover:bg-orange-950/30 disabled:opacity-40 disabled:cursor-not-allowed disabled:hover:bg-white dark:disabled:hover:bg-stone-900 disabled:hover:border-stone-200 dark:disabled:hover:border-stone-700 transition";
const BTN_ACTIVE =
    "inline-flex items-center justify-center min-w-[28px] h-7 px-2 rounded-md bg-orange-600 text-white border border-orange-600";

export function Pagination({ page, totalPages, onChange, disabled }: Props) {
    if (totalPages <= 1) return null;
    const items = pageList(page, totalPages);
    const go = (p: number) => {
        if (disabled) return;
        if (p < 1 || p > totalPages || p === page) return;
        onChange(p);
    };
    return (
        <div className="flex items-center gap-1" role="navigation" aria-label="pagination">
            <button
                type="button"
                className={BTN_BASE}
                onClick={() => go(page - 1)}
                disabled={disabled || page <= 1}
                aria-label="previous page"
            >
                <ChevronLeft className="w-3 h-3" />
            </button>
            {items.map((it, i) =>
                it === "…" ? (
                    <span key={`gap-${i}`} className="px-1 text-stone-400">
                        …
                    </span>
                ) : (
                    <button
                        type="button"
                        key={it}
                        className={it === page ? BTN_ACTIVE : BTN_BASE}
                        onClick={() => go(it)}
                        disabled={disabled || it === page}
                        aria-label={`page ${it}`}
                        aria-current={it === page ? "page" : undefined}
                    >
                        {it}
                    </button>
                ),
            )}
            <button
                type="button"
                className={BTN_BASE}
                onClick={() => go(page + 1)}
                disabled={disabled || page >= totalPages}
                aria-label="next page"
            >
                <ChevronRight className="w-3 h-3" />
            </button>
        </div>
    );
}
