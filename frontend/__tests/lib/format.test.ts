// Copyright 2026 Appbird LLC. Licensed under "GNU Affero General Public License v3.0"

import { describe, it, expect, vi, afterEach } from "vitest";
import { formatBytes, compactNumber, timeAgo } from "../../src/lib/format";

describe("formatBytes", () => {
    it("scales across units", () => {
        expect(formatBytes(512)).toBe("512 B");
        expect(formatBytes(2048)).toBe("2.0 KB");
        expect(formatBytes(5 * 1024 * 1024)).toBe("5.00 MB");
        expect(formatBytes(3 * 1024 * 1024 * 1024)).toBe("3.00 GB");
    });
});

describe("compactNumber", () => {
    it("formats thousands and millions", () => {
        expect(compactNumber(999)).toBe("999");
        expect(compactNumber(1500)).toBe("1.5k");
        expect(compactNumber(15000)).toBe("15k");
        expect(compactNumber(2_000_000)).toBe("2.0M");
    });
});

describe("timeAgo", () => {
    afterEach(() => vi.useRealTimers());
    it("renders relative ages from a fixed now", () => {
        vi.useFakeTimers();
        vi.setSystemTime(new Date("2026-01-01T12:00:00Z"));
        expect(timeAgo("2026-01-01T11:59:58Z")).toBe("just now");
        expect(timeAgo("2026-01-01T11:59:30Z")).toBe("30s ago");
        expect(timeAgo("2026-01-01T11:30:00Z")).toBe("30m ago");
        expect(timeAgo("2026-01-01T09:00:00Z")).toBe("3h ago");
        expect(timeAgo("2025-12-30T12:00:00Z")).toBe("2d ago");
    });
    it("echoes back an unparseable input", () => {
        expect(timeAgo("not-a-date")).toBe("not-a-date");
    });
});
