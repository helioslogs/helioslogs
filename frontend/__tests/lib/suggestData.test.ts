// Copyright 2026 Appbird LLC. Licensed under "GNU Affero General Public License v3.0"

import { describe, it, expect } from "vitest";
import {
    COMMANDS,
    AGG_FUNCS,
    BOOLEAN_OPS,
    STATS_BY,
    SYSTEM_FIELDS,
} from "../../src/lib/suggestData";

// These static catalogs drive the autocomplete popover; the `insert` strings
// in particular are load-bearing (trailing space vs opening paren), so pin them.

describe("COMMANDS", () => {
    it("inserts a trailing space so args can follow", () => {
        const stats = COMMANDS.find((c) => c.label === "stats");
        expect(stats?.insert).toBe("stats ");
        expect(COMMANDS.map((c) => c.label)).toEqual([
            "stats",
            "top",
            "rare",
            "sort",
            "head",
            "tail",
        ]);
    });
});

describe("AGG_FUNCS", () => {
    it("inserts bare count but open-parens the field aggs", () => {
        expect(AGG_FUNCS.find((a) => a.label === "count")?.insert).toBe("count");
        expect(AGG_FUNCS.find((a) => a.label === "sum")?.insert).toBe("sum(");
        expect(AGG_FUNCS.find((a) => a.label === "p95")?.insert).toBe("p95(");
    });
});

describe("BOOLEAN_OPS / STATS_BY", () => {
    it("offers the three boolean keywords and a by keyword", () => {
        expect(BOOLEAN_OPS.map((b) => b.label)).toEqual(["AND", "OR", "NOT"]);
        expect(STATS_BY.insert).toBe("by ");
    });
});

describe("SYSTEM_FIELDS", () => {
    it("includes the universal-core fields", () => {
        const names = SYSTEM_FIELDS.map((f) => f.name);
        expect(names).toEqual(["index", "source", "message", "timestamp", "raw"]);
    });
});
