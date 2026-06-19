// Copyright 2026 Appbird LLC. Licensed under "GNU Affero General Public License v3.0"

import { useEffect, useState } from "react";
import { useNavigate } from "react-router-dom";
import { Bar, BarChart, Cell, ResponsiveContainer, Tooltip, XAxis, YAxis } from "recharts";
import { aggregate } from "../../api/client";
import type { TopBucket, Widget } from "../../api/types";
import { searchHref } from "../../state/url";
import { addFilter } from "../../lib/query";
import { useTheme } from "../../state/theme";
import { colorAt, type DashRange } from "./util";

interface Props {
    widget: Widget;
    range: DashRange;
    refreshKey: number;
    onLoadingChange?: (loading: boolean) => void;
    onError?: (msg: string | null) => void;
}

// Top-N breakdown of a field. Clicking a bar drills into the underlying
// results filtered to that value (`field:value` appended to the base query).
export function TopNWidget({ widget, range, refreshKey, onLoadingChange, onError }: Props) {
    const baseQuery = widget.series?.[0]?.query || "*";
    const field = widget.field || "";
    const size = widget.size || 10;
    const [rows, setRows] = useState<TopBucket[]>([]);
    const navigate = useNavigate();
    const { theme } = useTheme();
    const isDark = theme === "dark";

    useEffect(() => {
        let cancelled = false;
        if (!field) {
            setRows([]);
            return;
        }
        onLoadingChange?.(true);
        aggregate({
            q: baseQuery,
            start: range.start,
            end: range.end,
            fields: field,
            size,
            approximate: true,
        })
            .then((r) => {
                if (cancelled) return;
                setRows(r.aggs[field] ?? []);
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
    }, [baseQuery, field, size, range.start, range.end, refreshKey]);

    if (!field) {
        return <p className="text-sm text-stone-400 dark:text-stone-500">No field configured.</p>;
    }
    if (rows.length === 0) {
        return <p className="text-sm text-stone-400 dark:text-stone-500">no values in range</p>;
    }

    const data = rows.map((b) => ({ key: String(b.key), count: b.count }));

    const drill = (key: string) => {
        const q = addFilter(baseQuery, field, key);
        navigate(
            range.range
                ? searchHref({ q, range: range.range, follow: false })
                : searchHref({
                      q,
                      range: "-24h",
                      follow: false,
                      start: range.start,
                      end: range.end,
                  }),
        );
    };

    return (
        <ResponsiveContainer width="100%" height="100%">
            <BarChart
                data={data}
                layout="vertical"
                margin={{ top: 2, right: 12, left: 4, bottom: 2 }}
            >
                <XAxis type="number" allowDecimals={false} tick={{ fontSize: 12 }} hide />
                <YAxis
                    type="category"
                    dataKey="key"
                    width={110}
                    tick={{ fontSize: 12 }}
                    interval={0}
                />
                <Tooltip
                    contentStyle={{
                        background: isDark ? "#1c1917" : "#ffffff",
                        border: `1px solid ${isDark ? "#44403c" : "#e7e5e4"}`,
                        borderRadius: 8,
                        fontSize: 13,
                        color: isDark ? "#f5f5f4" : "#1c1917",
                    }}
                    formatter={(v: number) => [v.toLocaleString(), "count"]}
                    cursor={{ fill: isDark ? "rgba(249,115,22,0.12)" : "rgba(249,115,22,0.08)" }}
                />
                <Bar
                    dataKey="count"
                    radius={[0, 2, 2, 0]}
                    onClick={(d) => drill(String((d as { key: string }).key))}
                    style={{ cursor: "pointer" }}
                >
                    {data.map((_, i) => (
                        <Cell key={i} fill={colorAt(i)} />
                    ))}
                </Bar>
            </BarChart>
        </ResponsiveContainer>
    );
}
