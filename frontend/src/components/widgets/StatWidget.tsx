// Copyright 2026 Appbird LLC. Licensed under "GNU Affero General Public License v3.0"

import { useEffect, useState } from "react";
import { Link } from "react-router-dom";
import { search } from "../../api/client";
import type { Widget } from "../../api/types";
import { colorAt, dashSearchHref, type DashRange } from "./util";

interface Props {
    widget: Widget;
    range: DashRange;
    refreshKey: number;
    onLoadingChange?: (loading: boolean) => void;
    onError?: (msg: string | null) => void;
}

// One or more big-number totals — `total` match count per series over the
// dashboard window (limit=0, no hits fetched). Each number links to results.
export function StatWidget({ widget, range, refreshKey, onLoadingChange, onError }: Props) {
    const series = widget.series ?? [];
    const [counts, setCounts] = useState<(number | null)[]>([]);

    useEffect(() => {
        let cancelled = false;
        if (series.length === 0) {
            setCounts([]);
            return;
        }
        onLoadingChange?.(true);
        Promise.all(
            series.map((s) =>
                search({ q: s.query || "*", start: range.start, end: range.end, limit: 0 }).then(
                    (r) => r.total,
                ),
            ),
        )
            .then((totals) => {
                if (cancelled) return;
                setCounts(totals);
                onError?.(null);
            })
            .catch((e: unknown) => onError?.(e instanceof Error ? e.message : String(e)))
            .finally(() => {
                if (!cancelled) onLoadingChange?.(false);
            });
        return () => {
            cancelled = true;
        };
        // eslint-disable-next-line react-hooks/exhaustive-deps
    }, [JSON.stringify(series), range.start, range.end, refreshKey]);

    if (series.length === 0) {
        return <p className="text-sm text-stone-400 dark:text-stone-500">No query configured.</p>;
    }

    return (
        <div className="h-full flex flex-wrap items-center justify-around gap-4">
            {series.map((s, i) => (
                <Link
                    key={s.id}
                    to={dashSearchHref(s.query, range)}
                    title={`view results: ${s.query || "*"}`}
                    className="flex flex-col items-center group"
                >
                    <span
                        className="text-3xl font-semibold tabular-nums text-stone-900 dark:text-stone-100 group-hover:text-orange-600 dark:group-hover:text-orange-400 transition"
                        style={series.length > 1 ? { color: s.color || colorAt(i) } : undefined}
                    >
                        {counts[i] == null ? "—" : counts[i]!.toLocaleString()}
                    </span>
                    {(s.label || series.length > 1) && (
                        <span className="text-xs text-stone-500 dark:text-stone-400 mt-0.5">
                            {s.label || s.query}
                        </span>
                    )}
                </Link>
            ))}
        </div>
    );
}
