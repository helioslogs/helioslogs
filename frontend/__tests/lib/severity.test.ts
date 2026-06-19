// Copyright 2026 Appbird LLC. Licensed under "GNU Affero General Public License v3.0"

import { describe, it, expect } from "vitest";
import { isSeverityShapedField, normalizeLevel, detectRowSeverity } from "../../src/lib/severity";

describe("isSeverityShapedField", () => {
    it("recognizes known level field names case-insensitively", () => {
        expect(isSeverityShapedField("level")).toBe(true);
        expect(isSeverityShapedField("LeVeL")).toBe(true);
        expect(isSeverityShapedField("log.level")).toBe(true);
        expect(isSeverityShapedField("message")).toBe(false);
    });
});

describe("normalizeLevel", () => {
    it("uppercases known levels", () => {
        expect(normalizeLevel("error")).toBe("ERROR");
        expect(normalizeLevel("Info")).toBe("INFO");
    });
    it("collapses WARNING→WARN and CRITICAL→FATAL", () => {
        expect(normalizeLevel("warning")).toBe("WARN");
        expect(normalizeLevel("critical")).toBe("FATAL");
    });
    it("returns null for non-levels and non-strings", () => {
        expect(normalizeLevel("banana")).toBeNull();
        expect(normalizeLevel(42)).toBeNull();
        expect(normalizeLevel(null)).toBeNull();
    });
});

describe("detectRowSeverity", () => {
    it("returns the normalized level from a known field", () => {
        expect(detectRowSeverity({ level: "warn" })).toBe("WARN");
        expect(detectRowSeverity({ severity: "ERROR" })).toBe("ERROR");
    });
    it("prefers `severity` over `level` (field priority order)", () => {
        expect(detectRowSeverity({ level: "info", severity: "error" })).toBe("ERROR");
    });
    it("returns null when nothing severity-shaped is present", () => {
        expect(detectRowSeverity({ msg: "hi" })).toBeNull();
        expect(detectRowSeverity({})).toBeNull();
        expect(detectRowSeverity(null)).toBeNull();
    });
});
