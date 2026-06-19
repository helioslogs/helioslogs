// Copyright 2026 Appbird LLC. Licensed under "GNU Affero General Public License v3.0"

// Severity-detection heuristic. The UI decides whether a discovered field gets the
// log-level color treatment, based on its name and whether values match the severity vocabulary.

// Field names that, by convention across logging libraries, hold a log level.
const SEVERITY_FIELD_NAMES = new Set([
    "severity",
    "level",
    "lvl",
    "log.level",
    "loglevel",
    "log_level",
    "@level",
]);

// Canonical level names this UI colors; matched case-insensitively.
const KNOWN_LEVELS = ["DEBUG", "INFO", "WARN", "WARNING", "ERROR", "FATAL", "CRITICAL", "TRACE"];

// Quick name-only check — does this field look like a log-level field?
// Used by the sidebar to pick up the colored value chips treatment.
export function isSeverityShapedField(name: string): boolean {
    return SEVERITY_FIELD_NAMES.has(name.toLowerCase());
}

// Normalize a free-form severity to a canonical badge level, or null. Collapses
// `WARNING`→`WARN` and `CRITICAL`→`FATAL` so there's one badge set.
export function normalizeLevel(value: unknown): string | null {
    if (typeof value !== "string") return null;
    const up = value.toUpperCase();
    if (!KNOWN_LEVELS.includes(up)) return null;
    if (up === "WARNING") return "WARN";
    if (up === "CRITICAL") return "FATAL";
    return up;
}

// Per-row severity from a parsed event: first known field name that
// normalizes cleanly, else null.
export function detectRowSeverity(
    event: Record<string, unknown> | null | undefined,
): string | null {
    if (!event) return null;
    for (const key of SEVERITY_FIELD_NAMES) {
        if (key in event) {
            const lvl = normalizeLevel(event[key]);
            if (lvl) return lvl;
        }
    }
    return null;
}
