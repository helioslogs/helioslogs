// Copyright 2026 Appbird LLC. Licensed under "GNU Affero General Public License v3.0"

// Render a wall-clock duration compactly: ms under 1s, one-decimal seconds
// under 1m, then `Nm Ns`. Used by the agent UI for tool/turn timings.
export function formatDuration(ms: number): string {
    if (!isFinite(ms) || ms < 0) return "";
    if (ms < 1000) return `${Math.round(ms)}ms`;
    const seconds = ms / 1000;
    if (seconds < 60) return `${seconds.toFixed(1)}s`;
    const wholeMin = Math.floor(seconds / 60);
    const remSec = Math.round(seconds - wholeMin * 60);
    return `${wholeMin}m ${remSec}s`;
}
