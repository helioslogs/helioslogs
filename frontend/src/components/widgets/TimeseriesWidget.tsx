// Copyright 2026 Appbird LLC. Licensed under "GNU Affero General Public License v3.0"

import { useEffect, useState } from "react";
import { Link } from "react-router-dom";
import { ExternalLink } from "lucide-react";
import {
    Area,
    AreaChart,
    Bar,
    BarChart,
    CartesianGrid,
    Line,
    LineChart,
    ResponsiveContainer,
    Tooltip,
    XAxis,
    YAxis,
} from "recharts";
import { histogram } from "../../api/client";
import type { Series, Widget } from "../../api/types";
import { formatTickShort, formatTooltipTs } from "../../lib/timezone";
import { useTimezone } from "../../state/timezone";
import { useTheme } from "../../state/theme";
import { colorAt, dashSearchHref, type DashRange } from "./util";

interface Props {
    widget: Widget;
    range: DashRange;
    refreshKey: number;
    onLoadingChange?: (loading: boolean) => void;
    onError?: (msg: string | null) => void;
}

interface Row {
    t: string;
    [seriesId: string]: string | number;
}

// Multi-series time chart: one histogram per series, merged by bucket
// timestamp. Each series links back to its results in the active env.
export function TimeseriesWidget({ widget, range, refreshKey, onLoadingChange, onError }: Props) {
    const series = widget.series ?? [];
    const [rows, setRows] = useState<Row[]>([]);
    const [intervalMs, setIntervalMs] = useState(0);
    const tz = useTimezone();
    const { theme } = useTheme();
    const isDark = theme === "dark";

    useEffect(() => {
        let cancelled = false;
        if (series.length === 0) {
            setRows([]);
            return;
        }
        onLoadingChange?.(true);
        Promise.all(
            series.map((s) =>
                histogram({ q: s.query || "*", start: range.start, end: range.end }).then((r) => ({
                    s,
                    r,
                })),
            ),
        )
            .then((results) => {
                if (cancelled) return;
                const byTs = new Map<string, Row>();
                let ims = 0;
                for (const { s, r } of results) {
                    ims = r.interval_ms || ims;
                    for (const b of r.buckets) {
                        let row = byTs.get(b.t);
                        if (!row) {
                            row = { t: b.t };
                            byTs.set(b.t, row);
                        }
                        row[s.id] = b.count;
                    }
                }
                const merged = [...byTs.values()].sort((a, b) => a.t.localeCompare(b.t));
                // Fill gaps with 0 so lines don't break across empty buckets.
                for (const row of merged)
                    for (const s of series) if (row[s.id] == null) row[s.id] = 0;
                setIntervalMs(ims);
                setRows(merged);
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
        return <p className="text-sm text-stone-400 dark:text-stone-500">No series configured.</p>;
    }

    const tooltipStyle = {
        background: isDark ? "#1c1917" : "#ffffff",
        border: `1px solid ${isDark ? "#44403c" : "#e7e5e4"}`,
        borderRadius: 8,
        fontSize: 13,
        color: isDark ? "#f5f5f4" : "#1c1917",
    };
    const chart = widget.chart ?? "line";

    return (
        <div className="h-full flex flex-col">
            <div className="flex-1 min-h-0">
                <ResponsiveContainer width="100%" height="100%">
                    {renderChart(chart, rows, series, intervalMs, tz, tooltipStyle)}
                </ResponsiveContainer>
            </div>
            <div className="flex flex-wrap gap-x-3 gap-y-1 pt-2">
                {series.map((s, i) => (
                    <Link
                        key={s.id}
                        to={dashSearchHref(s.query, range)}
                        title={`View results: ${s.query || "*"}`}
                        className="group inline-flex items-center gap-1.5 text-xs text-stone-600 dark:text-stone-300 hover:text-orange-600 dark:hover:text-orange-400"
                    >
                        <span
                            className="w-2.5 h-2.5 rounded-sm"
                            style={{ background: s.color || colorAt(i) }}
                        />
                        {s.label || s.query || "series"}
                        <ExternalLink
                            className="w-3 h-3 text-stone-400 dark:text-stone-500 group-hover:text-orange-500 dark:group-hover:text-orange-400"
                            aria-hidden="true"
                        />
                    </Link>
                ))}
            </div>
        </div>
    );
}

function renderChart(
    kind: "line" | "bar" | "area",
    rows: Row[],
    series: Series[],
    intervalMs: number,
    tz: string,
    tooltipStyle: object,
) {
    const xAxis = (
        <XAxis
            dataKey="t"
            tickFormatter={(v) => formatTickShort(v as string, intervalMs, tz)}
            tick={{ fontSize: 12 }}
            interval="preserveStartEnd"
            minTickGap={36}
        />
    );
    const yAxis = <YAxis allowDecimals={false} tick={{ fontSize: 12 }} width={40} />;
    const tooltip = (
        <Tooltip
            contentStyle={tooltipStyle}
            labelFormatter={(v) => formatTooltipTs(v as string, tz)}
            formatter={(v: number, name: string) => [v.toLocaleString(), name]}
        />
    );
    const grid = <CartesianGrid vertical={false} />;
    const margin = { top: 6, right: 10, left: 0, bottom: 0 };

    if (kind === "bar") {
        return (
            <BarChart data={rows} margin={margin}>
                {grid}
                {xAxis}
                {yAxis}
                {tooltip}
                {series.map((s, i) => (
                    <Bar
                        key={s.id}
                        dataKey={s.id}
                        name={s.label || s.query}
                        fill={s.color || colorAt(i)}
                        radius={[2, 2, 0, 0]}
                    />
                ))}
            </BarChart>
        );
    }
    if (kind === "area") {
        return (
            <AreaChart data={rows} margin={margin}>
                {grid}
                {xAxis}
                {yAxis}
                {tooltip}
                {series.map((s, i) => (
                    <Area
                        key={s.id}
                        type="monotone"
                        dataKey={s.id}
                        name={s.label || s.query}
                        stroke={s.color || colorAt(i)}
                        fill={s.color || colorAt(i)}
                        fillOpacity={0.18}
                        strokeWidth={2}
                        dot={false}
                    />
                ))}
            </AreaChart>
        );
    }
    return (
        <LineChart data={rows} margin={margin}>
            {grid}
            {xAxis}
            {yAxis}
            {tooltip}
            {series.map((s, i) => (
                <Line
                    key={s.id}
                    type="monotone"
                    dataKey={s.id}
                    name={s.label || s.query}
                    stroke={s.color || colorAt(i)}
                    strokeWidth={2}
                    dot={false}
                />
            ))}
        </LineChart>
    );
}
