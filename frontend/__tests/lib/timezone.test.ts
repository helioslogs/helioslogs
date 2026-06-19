// Copyright 2026 Appbird LLC. Licensed under "GNU Affero General Public License v3.0"

import { describe, it, expect, beforeEach, vi } from "vitest";
import {
    getStoredTimezone,
    setStoredTimezone,
    onTimezoneChange,
    formatTsForRow,
    formatTickShort,
    formatTooltipTs,
    formatTzLabel,
} from "../../src/lib/timezone";

beforeEach(() => localStorage.clear());

describe("stored timezone", () => {
    it("round-trips through localStorage", () => {
        setStoredTimezone("Asia/Tokyo");
        expect(getStoredTimezone()).toBe("Asia/Tokyo");
    });
    it("notifies subscribers on change and unsubscribes cleanly", () => {
        const seen: string[] = [];
        const off = onTimezoneChange((tz) => seen.push(tz));
        setStoredTimezone("Europe/Paris");
        off();
        setStoredTimezone("UTC");
        expect(seen).toEqual(["Europe/Paris"]);
    });
});

describe("formatTsForRow", () => {
    it("renders YYYY-MM-DD HH:MM:SS in the given zone", () => {
        expect(formatTsForRow("2026-01-01T12:00:00Z", "UTC")).toBe("2026-01-01 12:00:00");
        // 2026-01-01 is EST (UTC-5) in New York.
        expect(formatTsForRow("2026-01-01T12:00:00Z", "America/New_York")).toBe(
            "2026-01-01 07:00:00",
        );
    });
    it("handles empty and unparseable inputs", () => {
        expect(formatTsForRow(undefined, "UTC")).toBe("");
        expect(formatTsForRow("nope", "UTC")).toBe("nope");
    });
});

describe("formatTickShort", () => {
    it("uses MM-DD for day-scale intervals and HH:MM otherwise", () => {
        expect(formatTickShort("2026-03-04T00:00:00Z", 86_400_000, "UTC")).toBe("03-04");
        expect(formatTickShort("2026-03-04T09:30:00Z", 60_000, "UTC")).toBe("09:30");
    });
});

describe("formatTooltipTs / formatTzLabel", () => {
    it("includes a zone abbreviation in the tooltip", () => {
        const out = formatTooltipTs("2026-01-01T12:00:00Z", "UTC");
        expect(out.startsWith("2026-01-01 12:00:00")).toBe(true);
    });
    it("labels a zone with its name", () => {
        expect(formatTzLabel("UTC")).toContain("UTC");
    });
});

// Guard against accidental fake timers leaking from other suites.
vi.useRealTimers();
