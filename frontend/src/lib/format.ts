// Copyright 2026 Appbird LLC. Licensed under "GNU Affero General Public License v3.0"

// Display formatters shared across pages. Pure functions; no React.

export function formatBytes(n: number): string {
    if (n < 1024) return `${n} B`;
    if (n < 1024 * 1024) return `${(n / 1024).toFixed(1)} KB`;
    if (n < 1024 * 1024 * 1024) return `${(n / 1024 / 1024).toFixed(2)} MB`;
    return `${(n / 1024 / 1024 / 1024).toFixed(2)} GB`;
}

// Relative "X ago" — ISO in → human out.
export function timeAgo(iso: string): string {
    const t = new Date(iso).getTime();
    if (!Number.isFinite(t)) return iso;
    const secs = Math.max(0, Math.floor((Date.now() - t) / 1000));
    if (secs < 5) return "just now";
    if (secs < 60) return `${secs}s ago`;
    if (secs < 3600) return `${Math.floor(secs / 60)}m ago`;
    if (secs < 86400) return `${Math.floor(secs / 3600)}h ago`;
    return `${Math.floor(secs / 86400)}d ago`;
}

// "1.2k", "3.4M" — saves table cells from very long numbers.
export function compactNumber(n: number): string {
    if (n < 1000) return String(n);
    if (n < 1_000_000) return `${(n / 1000).toFixed(n < 10_000 ? 1 : 0)}k`;
    return `${(n / 1_000_000).toFixed(1)}M`;
}
