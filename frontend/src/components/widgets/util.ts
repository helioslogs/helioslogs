// Copyright 2026 Appbird LLC. Licensed under "GNU Affero General Public License v3.0"

import type { DashboardSpec } from "../../api/types";
import { searchHref } from "../../state/url";

// Default series palette, cycled by index. Picked to read on both themes.
export const SERIES_COLORS = [
    "#f97316", // orange-500
    "#0ea5e9", // sky-500
    "#10b981", // emerald-500
    "#8b5cf6", // violet-500
    "#f59e0b", // amber-500
    "#ec4899", // pink-500
    "#14b8a6", // teal-500
    "#ef4444", // red-500
];

export function colorAt(i: number): string {
    return SERIES_COLORS[i % SERIES_COLORS.length];
}

// The time window the widgets query. Absolute bounds win; otherwise the
// relative `time_range` against "now" (backend parses both).
export interface DashRange {
    start: string;
    end: string;
    // The relative shortcut (when no absolute bounds), for building search links.
    range?: string;
}

export function specRange(spec: DashboardSpec): DashRange {
    if (spec.start && spec.end) return { start: spec.start, end: spec.end };
    return { start: spec.time_range || "-24h", end: "now", range: spec.time_range || "-24h" };
}

// Resolve a per-widget time override (or fall back to the dashboard range).
export function overrideRange(
    t: { range?: string; start?: string; end?: string } | undefined,
    fallback: DashRange,
): DashRange {
    if (!t) return fallback;
    if (t.start && t.end) return { start: t.start, end: t.end };
    if (t.range) return { start: t.range, end: "now", range: t.range };
    return fallback;
}

// Build the `/search?…` link for a widget query over the dashboard window,
// so clicking a chart/stat opens the underlying results in the active env.
export function dashSearchHref(query: string, r: DashRange): string {
    const q = query.trim() === "" ? "*" : query;
    if (r.range) return searchHref({ q, range: r.range, follow: false });
    return searchHref({ q, range: "-24h", follow: false, start: r.start, end: r.end });
}

let widgetSeq = 0;
// Short client-side id for a freshly-added widget/series. Avoids Date.now()
// churn in keys and is unique within a session.
export function newWidgetId(prefix: string): string {
    widgetSeq += 1;
    return `${prefix}_${widgetSeq}_${Math.floor(performance.now())}`;
}
