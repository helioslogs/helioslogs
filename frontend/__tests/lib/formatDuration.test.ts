// Copyright 2026 Appbird LLC. Licensed under "GNU Affero General Public License v3.0"

import { describe, it, expect } from "vitest";
import { formatDuration } from "../../src/lib/formatDuration";

describe("formatDuration", () => {
    it("renders sub-second as rounded ms", () => {
        expect(formatDuration(342)).toBe("342ms");
        expect(formatDuration(0)).toBe("0ms");
    });
    it("renders seconds with one decimal", () => {
        expect(formatDuration(1200)).toBe("1.2s");
        expect(formatDuration(59_900)).toBe("59.9s");
    });
    it("renders minutes + whole seconds past a minute", () => {
        expect(formatDuration(83_000)).toBe("1m 23s");
        expect(formatDuration(600_000)).toBe("10m 0s");
    });
    it("returns empty for invalid/negative input", () => {
        expect(formatDuration(-1)).toBe("");
        expect(formatDuration(NaN)).toBe("");
    });
});
