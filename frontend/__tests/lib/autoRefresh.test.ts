// Copyright 2026 Appbird LLC. Licensed under "GNU Affero General Public License v3.0"

import { describe, it, expect } from "vitest";
import {
    relativeRangeSecs,
    smartRefreshSecs,
    resolveRefreshSecs,
    refreshSecsLabel,
} from "../../src/lib/autoRefresh";

describe("relativeRangeSecs", () => {
    it("parses relative ranges to seconds", () => {
        expect(relativeRangeSecs("-1h")).toBe(3600);
        expect(relativeRangeSecs("-30m")).toBe(1800);
        expect(relativeRangeSecs("-7d")).toBe(604800);
        expect(relativeRangeSecs("-2w")).toBe(1209600);
    });
    it("returns null for absolute / missing ranges", () => {
        expect(relativeRangeSecs(undefined)).toBeNull();
        expect(relativeRangeSecs("2026-01-01")).toBeNull();
        expect(relativeRangeSecs("-1y")).toBeNull();
    });
});

describe("smartRefreshSecs", () => {
    it("scales the cadence to the window", () => {
        expect(smartRefreshSecs("-30m")).toBe(300); // ≤ 1h → 5m
        expect(smartRefreshSecs("-1h")).toBe(300);
        expect(smartRefreshSecs("-12h")).toBe(900); // ≤ 24h → 15m
        expect(smartRefreshSecs("-7d")).toBe(1800); // > 24h → 30m
    });
    it("is off for absolute ranges", () => {
        expect(smartRefreshSecs("absolute")).toBe(0);
    });
});

describe("resolveRefreshSecs", () => {
    it("uses the smart default in auto mode", () => {
        expect(resolveRefreshSecs("auto", { range: "-30m", hasAbsolute: false })).toBe(300);
    });
    it("passes an explicit interval through", () => {
        expect(resolveRefreshSecs(60, { range: "-30m", hasAbsolute: false })).toBe(60);
    });
    it("disables refresh for absolute windows and live-follow", () => {
        expect(resolveRefreshSecs("auto", { range: "-30m", hasAbsolute: true })).toBe(0);
        expect(
            resolveRefreshSecs("auto", { range: "-30m", hasAbsolute: false, follow: true }),
        ).toBe(0);
    });
});

describe("refreshSecsLabel", () => {
    it("formats cadences", () => {
        expect(refreshSecsLabel(0)).toBe("Off");
        expect(refreshSecsLabel(30)).toBe("30s");
        expect(refreshSecsLabel(300)).toBe("5m");
        expect(refreshSecsLabel(3600)).toBe("1h");
    });
});
