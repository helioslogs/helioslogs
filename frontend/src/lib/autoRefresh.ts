// Copyright 2026 Appbird LLC. Licensed under "GNU Affero General Public License v3.0"

// Auto-refresh cadence for search + dashboards: an explicit interval in seconds
// (0 = off) or `"auto"`, which scales to the active relative range.

export type RefreshSetting = "auto" | number;

export interface RefreshOption {
    label: string;
    value: RefreshSetting;
}

// Picker presets; `Auto` leads as the smart default.
export const REFRESH_OPTIONS: RefreshOption[] = [
    { label: "Auto", value: "auto" },
    { label: "Off", value: 0 },
    { label: "1m", value: 60 },
    { label: "5m", value: 300 },
    { label: "15m", value: 900 },
    { label: "30m", value: 1800 },
    { label: "1h", value: 3600 },
];

// Parse a relative range (`-1h`/`-30m`/`-7d`) into seconds; null for anything
// that isn't a simple relative range (absolute windows, etc.).
export function relativeRangeSecs(range: string | undefined): number | null {
    if (!range) return null;
    const m = /^-(\d+)([smhdw])$/.exec(range.trim());
    if (!m) return null;
    const unit: Record<string, number> = { s: 1, m: 60, h: 3600, d: 86400, w: 604800 };
    return Number(m[1]) * unit[m[2]];
}

// Smart default cadence for a relative range — leans to 5m / 15m / 30m so it
// stays useful without hammering the backend. 0 (off) for absolute ranges.
export function smartRefreshSecs(range: string | undefined): number {
    const secs = relativeRangeSecs(range);
    if (secs === null) return 0;
    if (secs <= 3600) return 300; // ≤ 1h  → every 5m
    if (secs <= 86400) return 900; // ≤ 24h → every 15m
    return 1800; // > 24h → every 30m
}

// Resolve the effective cadence (seconds; 0 = no auto-refresh) from the
// user's setting and the active time context.
export function resolveRefreshSecs(
    setting: RefreshSetting,
    opts: { range?: string; hasAbsolute: boolean; follow?: boolean },
): number {
    if (opts.hasAbsolute || opts.follow) return 0;
    return setting === "auto" ? smartRefreshSecs(opts.range) : setting;
}

// Compact label for a cadence in seconds: `Off` / `30s` / `5m` / `1h`.
export function refreshSecsLabel(secs: number): string {
    if (secs <= 0) return "Off";
    if (secs % 3600 === 0) return `${secs / 3600}h`;
    if (secs % 60 === 0) return `${secs / 60}m`;
    return `${secs}s`;
}
