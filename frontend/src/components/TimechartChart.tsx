// Copyright 2026 Appbird LLC. Licensed under "GNU Affero General Public License v3.0"

import { useMemo } from "react";
import {
    CartesianGrid,
    Line,
    LineChart,
    ResponsiveContainer,
    Tooltip,
    XAxis,
    YAxis,
} from "recharts";
import type { TableResult } from "../api/types";
import { formatTickShort, formatTooltipTs } from "../lib/timezone";
import { useTimezone } from "../state/timezone";
import { useTheme } from "../state/theme";
import { colorAt } from "./widgets/util";

const MAX_SERIES = 20;

interface Props {
    table: TableResult;
}

interface Row {
    t: string;
    [series: string]: string | number;
}

export function isTimechartTable(table: TableResult): boolean {
    return table.columns[0] === "_time";
}

// Pivot the long-format timechart table (one row per bucket × group) into one
// recharts row per bucket with a column per series. Numeric columns are the
// aggregations; string columns are the group-by values.
function pivot(table: TableResult): { rows: Row[]; series: string[]; truncated: number } {
    const { columns, rows } = table;
    const aggIdx: number[] = [];
    const groupIdx: number[] = [];
    for (let i = 1; i < columns.length; i++) {
        const numeric = rows.some((r) => typeof r[i] === "number");
        (numeric ? aggIdx : groupIdx).push(i);
    }
    const byTs = new Map<string, Row>();
    const totals = new Map<string, number>();
    for (const r of rows) {
        const t = String(r[0]);
        const group = groupIdx.map((i) => r[i] ?? "—").join(" / ");
        for (const ai of aggIdx) {
            const name =
                aggIdx.length === 1 && group
                    ? group
                    : group
                      ? `${group}: ${columns[ai]}`
                      : columns[ai];
            let row = byTs.get(t);
            if (!row) {
                row = { t };
                byTs.set(t, row);
            }
            const v = typeof r[ai] === "number" ? r[ai] : 0;
            row[name] = v;
            totals.set(name, (totals.get(name) ?? 0) + v);
        }
    }
    const ranked = [...totals.entries()].sort((a, b) => b[1] - a[1]).map(([k]) => k);
    const series = ranked.slice(0, MAX_SERIES);
    const out = [...byTs.values()].sort((a, b) => a.t.localeCompare(b.t));
    for (const row of out) for (const s of series) if (row[s] == null) row[s] = 0;
    return { rows: out, series, truncated: ranked.length - series.length };
}

export function TimechartChart({ table }: Props) {
    const tz = useTimezone();
    const { theme } = useTheme();
    const isDark = theme === "dark";
    const { rows, series, truncated } = useMemo(() => pivot(table), [table]);

    if (rows.length === 0) return null;

    const intervalMs = rows.length > 1 ? Date.parse(rows[1].t) - Date.parse(rows[0].t) : 60_000;
    const tooltipStyle = {
        background: isDark ? "#1c1917" : "#ffffff",
        border: `1px solid ${isDark ? "#44403c" : "#e7e5e4"}`,
        borderRadius: 8,
        fontSize: 13,
        color: isDark ? "#f5f5f4" : "#1c1917",
    };

    return (
        <div className="rounded-xl border border-stone-200 dark:border-stone-800 bg-white dark:bg-stone-900 px-3 pt-3 pb-1">
            <div className="h-56">
                <ResponsiveContainer width="100%" height="100%">
                    <LineChart data={rows} margin={{ top: 6, right: 10, left: 0, bottom: 0 }}>
                        <CartesianGrid vertical={false} />
                        <XAxis
                            dataKey="t"
                            tickFormatter={(v) => formatTickShort(v as string, intervalMs, tz)}
                            tick={{ fontSize: 12 }}
                            interval="preserveStartEnd"
                            minTickGap={36}
                        />
                        <YAxis tick={{ fontSize: 12 }} width={48} />
                        <Tooltip
                            contentStyle={tooltipStyle}
                            labelFormatter={(v) => formatTooltipTs(v as string, tz)}
                            formatter={(v: number, name: string) => [v.toLocaleString(), name]}
                        />
                        {series.map((s, i) => (
                            <Line
                                key={s}
                                type="monotone"
                                dataKey={s}
                                name={s}
                                stroke={colorAt(i)}
                                strokeWidth={2}
                                dot={false}
                            />
                        ))}
                    </LineChart>
                </ResponsiveContainer>
            </div>
            <div className="flex flex-wrap gap-x-3 gap-y-1 px-1 py-2 text-xs text-stone-600 dark:text-stone-300">
                {series.map((s, i) => (
                    <span key={s} className="inline-flex items-center gap-1.5">
                        <span
                            className="w-2.5 h-2.5 rounded-sm"
                            style={{ background: colorAt(i) }}
                        />
                        {s}
                    </span>
                ))}
                {truncated > 0 && (
                    <span className="text-stone-400 dark:text-stone-500">
                        +{truncated} more series not shown
                    </span>
                )}
            </div>
        </div>
    );
}
