// Copyright 2026 Appbird LLC. Licensed under "GNU Affero General Public License v3.0"

import { BarChart, Bar, XAxis, YAxis, Tooltip, ResponsiveContainer, CartesianGrid } from "recharts";
import { Loader2, Square, X } from "lucide-react";
import type { HistogramBucket } from "../api/types";
import type { ScanProgress } from "../state/useSearchQuery";
import { formatTickShort, formatTooltipTs } from "../lib/timezone";
import { useTimezone } from "../state/timezone";
import { useTheme } from "../state/theme";

interface Props {
    buckets: HistogramBucket[];
    intervalMs: number;
    loading?: boolean;
    // Currently-active absolute time window (set by clicking a bucket). When
    // present, a clearable chip is shown in the summary line.
    selectedRange?: { start: string; end: string };
    // Click handler for a bucket → narrow the search to [start, start+interval).
    onSelectBucket?: (startMs: number, endMs: number) => void;
    // Clear the absolute time selection.
    onClearSelection?: () => void;
    // Day-by-day streaming progress. When set, the loading pill shows
    // "scanning X of Y days" instead of the generic "refreshing…".
    scanProgress?: ScanProgress | null;
    // Abort the in-flight scan. Surfaced as a "stop" button next to the
    // progress text when present.
    onCancel?: () => void;
}

export function Histogram({
    buckets,
    intervalMs,
    loading,
    selectedRange,
    onSelectBucket,
    onClearSelection,
    scanProgress,
    onCancel,
}: Props) {
    const tz = useTimezone();
    const { theme } = useTheme();
    const isDark = theme === "dark";

    if (loading && buckets.length === 0) {
        return (
            <div className="rounded-xl border border-stone-200 dark:border-stone-800 bg-white dark:bg-stone-900 p-8 flex items-center justify-center gap-3 text-stone-500 dark:text-stone-400">
                <Loader2 className="w-4 h-4 animate-spin" aria-hidden="true" />
                <span>
                    {scanProgress
                        ? `scanning ${scanProgress.doneDays} of ${scanProgress.totalDays} days…`
                        : "loading…"}
                </span>
                {onCancel && scanProgress && (
                    <button
                        type="button"
                        onClick={onCancel}
                        className="inline-flex items-center gap-1 px-2 py-0.5 rounded border border-stone-300 dark:border-stone-700 hover:bg-stone-100 dark:hover:bg-stone-800 text-stone-600 dark:text-stone-300"
                        title="stop scanning — keep results gathered so far"
                    >
                        <Square className="w-3 h-3" aria-hidden="true" />
                        stop
                    </button>
                )}
            </div>
        );
    }
    if (buckets.length === 0) {
        return (
            <div className="rounded-xl border border-stone-200 dark:border-stone-800 bg-white dark:bg-stone-900 p-8 text-center text-stone-500 dark:text-stone-400">
                no events in range
            </div>
        );
    }
    const total = buckets.reduce((acc, b) => acc + b.count, 0);

    const handleBarClick = (data: HistogramBucket | undefined) => {
        if (!onSelectBucket || !data?.t) return;
        const start = new Date(data.t).getTime();
        if (Number.isNaN(start)) return;
        onSelectBucket(start, start + intervalMs);
    };

    return (
        <div className="relative rounded-xl border border-stone-200 dark:border-stone-800 bg-white dark:bg-stone-900 p-3">
            {loading && (
                <div
                    className="absolute top-2 right-3 z-10 flex items-center gap-2 px-2 py-0.5 rounded-md bg-stone-100/90 dark:bg-stone-800/90 text-stone-600 dark:text-stone-300 backdrop-blur-sm"
                    title={scanProgress ? "scanning — partial results below" : "refreshing"}
                >
                    <Loader2 className="w-3.5 h-3.5 animate-spin" aria-hidden="true" />
                    <span>
                        {scanProgress
                            ? `scanning ${scanProgress.doneDays} of ${scanProgress.totalDays} days…`
                            : "refreshing…"}
                    </span>
                    {onCancel && scanProgress && (
                        <button
                            type="button"
                            onClick={onCancel}
                            className="inline-flex items-center gap-1 px-1.5 py-0.5 rounded border border-stone-300 dark:border-stone-700 hover:bg-stone-200 dark:hover:bg-stone-700 text-stone-700 dark:text-stone-200"
                            title="stop scanning — keep results gathered so far"
                        >
                            <Square className="w-3 h-3" aria-hidden="true" />
                            stop
                        </button>
                    )}
                </div>
            )}
            <div className="flex items-center justify-between mb-2 px-1 text-stone-500 dark:text-stone-400">
                <span>
                    <span className="font-medium text-stone-700 dark:text-stone-200">
                        {total.toLocaleString()}
                    </span>{" "}
                    events · {buckets.length} buckets ·{" "}
                    {intervalMs >= 60000
                        ? `${(intervalMs / 60000).toFixed(0)}m interval`
                        : `${(intervalMs / 1000).toFixed(0)}s interval`}
                </span>
                {selectedRange ? (
                    <span className="inline-flex items-center gap-1.5 px-2 py-0.5 rounded-md bg-blue-50 text-blue-800 border border-blue-200 dark:bg-blue-950/50 dark:text-blue-200 dark:border-blue-900">
                        zoomed: {formatTooltipTs(selectedRange.start, tz)} →{" "}
                        {formatTooltipTs(selectedRange.end, tz)}
                        {onClearSelection && (
                            <button
                                type="button"
                                aria-label="clear time selection"
                                onClick={onClearSelection}
                                title="clear"
                                className="p-0.5 rounded hover:bg-blue-100 dark:hover:bg-blue-900/50"
                            >
                                <X className="w-3 h-3" />
                            </button>
                        )}
                    </span>
                ) : (
                    onSelectBucket && (
                        <span className="text-stone-400 dark:text-stone-500">
                            click a bar to zoom
                        </span>
                    )
                )}
            </div>
            <ResponsiveContainer width="100%" height={120}>
                <BarChart data={buckets} margin={{ top: 4, right: 8, left: 0, bottom: 0 }}>
                    <CartesianGrid vertical={false} />
                    <XAxis
                        dataKey="t"
                        tickFormatter={(v) => formatTickShort(v as string, intervalMs, tz)}
                        tick={{ fontSize: 14 }}
                        interval="preserveStartEnd"
                        minTickGap={36}
                    />
                    <YAxis allowDecimals={false} tick={{ fontSize: 14 }} width={48} />
                    <Tooltip
                        contentStyle={{
                            background: isDark ? "#1c1917" : "#ffffff",
                            border: `1px solid ${isDark ? "#44403c" : "#e7e5e4"}`,
                            borderRadius: 8,
                            fontSize: 15,
                            color: isDark ? "#f5f5f4" : "#1c1917",
                        }}
                        labelFormatter={(v) => formatTooltipTs(v as string, tz)}
                        formatter={(v: number) => [v.toLocaleString(), "count"]}
                        cursor={{
                            fill: isDark ? "rgba(96, 165, 250, 0.12)" : "rgba(91, 157, 255, 0.08)",
                        }}
                    />
                    <Bar
                        dataKey="count"
                        fill={isDark ? "#60a5fa" : "#5b9dff"}
                        radius={[2, 2, 0, 0]}
                        onClick={(data) => handleBarClick(data as HistogramBucket)}
                        style={onSelectBucket ? { cursor: "pointer" } : undefined}
                    />
                </BarChart>
            </ResponsiveContainer>
        </div>
    );
}
