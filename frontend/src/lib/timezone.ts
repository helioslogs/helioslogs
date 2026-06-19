// Copyright 2026 Appbird LLC. Licensed under "GNU Affero General Public License v3.0"

// Timezone is a UI-only display preference (localStorage per browser; backend is UTC).
// Owns the get/set plus the timestamp formatter helpers.

const STORAGE_KEY = "helios-timezone";
const CHANGE_EVENT = "helios-tz-change";

export function getStoredTimezone(): string {
    try {
        const stored = localStorage.getItem(STORAGE_KEY);
        if (stored) return stored;
    } catch {
        // localStorage unavailable (private mode, etc.) — fall through.
    }
    try {
        return Intl.DateTimeFormat().resolvedOptions().timeZone || "UTC";
    } catch {
        return "UTC";
    }
}

export function setStoredTimezone(tz: string): void {
    try {
        localStorage.setItem(STORAGE_KEY, tz);
    } catch {
        // ignore
    }
    window.dispatchEvent(new CustomEvent(CHANGE_EVENT, { detail: tz }));
}

export function onTimezoneChange(handler: (tz: string) => void): () => void {
    const wrapped = (e: Event) => handler((e as CustomEvent<string>).detail);
    window.addEventListener(CHANGE_EVENT, wrapped as EventListener);
    return () => window.removeEventListener(CHANGE_EVENT, wrapped as EventListener);
}

const FALLBACK_TIMEZONES = [
    "UTC",
    "America/New_York",
    "America/Chicago",
    "America/Denver",
    "America/Los_Angeles",
    "America/Sao_Paulo",
    "Europe/London",
    "Europe/Berlin",
    "Europe/Paris",
    "Europe/Moscow",
    "Africa/Johannesburg",
    "Asia/Dubai",
    "Asia/Kolkata",
    "Asia/Shanghai",
    "Asia/Tokyo",
    "Australia/Sydney",
];

export function getAvailableTimezones(): string[] {
    // `Intl.supportedValuesOf` is ES2022; widely supported but typed weakly.
    const intlAny = Intl as unknown as { supportedValuesOf?: (k: string) => string[] };
    if (typeof intlAny.supportedValuesOf === "function") {
        try {
            const list = intlAny.supportedValuesOf("timeZone");
            if (Array.isArray(list) && list.length > 0) return list;
        } catch {
            /* fall through */
        }
    }
    return FALLBACK_TIMEZONES;
}

// Compact GMT offset string for a timezone — e.g. "GMT-5", "GMT+9:30".
export function formatOffset(tz: string): string {
    try {
        const parts = new Intl.DateTimeFormat("en-US", {
            timeZone: tz,
            timeZoneName: "shortOffset",
        }).formatToParts(new Date());
        return parts.find((p) => p.type === "timeZoneName")?.value ?? "";
    } catch {
        return "";
    }
}

// Label for the timezone select: `America/New_York (GMT-5)`.
export function formatTzLabel(tz: string): string {
    const offset = formatOffset(tz);
    return offset ? `${tz} (${offset})` : tz;
}

function partsToMap(parts: Intl.DateTimeFormatPart[]): Record<string, string> {
    const map: Record<string, string> = {};
    for (const p of parts) map[p.type] = p.value;
    return map;
}

// `YYYY-MM-DD HH:MM:SS` in the given tz — primary format for result rows.
export function formatTsForRow(iso: string | undefined, tz: string): string {
    if (!iso) return "";
    const d = new Date(iso);
    if (Number.isNaN(d.getTime())) return iso;
    try {
        const parts = new Intl.DateTimeFormat("en-CA", {
            timeZone: tz,
            year: "numeric",
            month: "2-digit",
            day: "2-digit",
            hour: "2-digit",
            minute: "2-digit",
            second: "2-digit",
            hour12: false,
        }).formatToParts(d);
        const m = partsToMap(parts);
        const hour = m.hour === "24" ? "00" : m.hour;
        return `${m.year}-${m.month}-${m.day} ${hour}:${m.minute}:${m.second}`;
    } catch {
        return d.toISOString().replace("T", " ").slice(0, 19);
    }
}

// Short label used as a histogram x-axis tick. `MM-DD` for ranges >= 1 day,
// `HH:MM` otherwise.
export function formatTickShort(iso: string, intervalMs: number, tz: string): string {
    if (!iso) return "";
    const d = new Date(iso);
    if (Number.isNaN(d.getTime())) return iso;
    try {
        if (intervalMs >= 86_400_000) {
            const parts = new Intl.DateTimeFormat("en-CA", {
                timeZone: tz,
                month: "2-digit",
                day: "2-digit",
            }).formatToParts(d);
            const m = partsToMap(parts);
            return `${m.month}-${m.day}`;
        }
        const parts = new Intl.DateTimeFormat("en-CA", {
            timeZone: tz,
            hour: "2-digit",
            minute: "2-digit",
            hour12: false,
        }).formatToParts(d);
        const m = partsToMap(parts);
        const hour = m.hour === "24" ? "00" : m.hour;
        return `${hour}:${m.minute}`;
    } catch {
        if (intervalMs >= 86_400_000) return d.toISOString().slice(5, 10);
        return d.toISOString().slice(11, 16);
    }
}

// Verbose timestamp for tooltips and the histogram zoom chip; includes the
// short offset so the user always knows what tz the number is in.
export function formatTooltipTs(iso: string | undefined, tz: string): string {
    if (!iso) return "";
    const d = new Date(iso);
    if (Number.isNaN(d.getTime())) return iso;
    try {
        const parts = new Intl.DateTimeFormat("en-CA", {
            timeZone: tz,
            year: "numeric",
            month: "2-digit",
            day: "2-digit",
            hour: "2-digit",
            minute: "2-digit",
            second: "2-digit",
            hour12: false,
            timeZoneName: "shortOffset",
        }).formatToParts(d);
        const m = partsToMap(parts);
        const hour = m.hour === "24" ? "00" : m.hour;
        return `${m.year}-${m.month}-${m.day} ${hour}:${m.minute}:${m.second} ${m.timeZoneName ?? ""}`.trim();
    } catch {
        return iso;
    }
}
