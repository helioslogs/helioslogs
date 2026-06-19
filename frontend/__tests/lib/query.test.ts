// Copyright 2026 Appbird LLC. Licensed under "GNU Affero General Public License v3.0"

import { describe, it, expect } from "vitest";
import {
    filterClause,
    addFilter,
    isFilterActive,
    toggleFilter,
    toggleTerm,
    isTermActive,
    sameAsCurrent,
    suggestName,
} from "../../src/lib/query";
import type { SavedSearch } from "../../src/api/types";
import type { SearchInput } from "../../src/state/url";

describe("filterClause", () => {
    it("leaves numbers bare and quotes strings", () => {
        expect(filterClause("status", 500)).toBe("status:500");
        expect(filterClause("svc", "auth-svc")).toBe('svc:"auth-svc"');
    });
    it("escapes embedded quotes", () => {
        expect(filterClause("msg", 'say "hi"')).toBe('msg:"say \\"hi\\""');
    });
});

describe("addFilter", () => {
    it("replaces an empty/match-all query", () => {
        expect(addFilter("", "a", 1)).toBe("a:1");
        expect(addFilter("*", "a", 1)).toBe("a:1");
    });
    it("AND-combines with an existing query", () => {
        expect(addFilter("x:1", "a", 2)).toBe("x:1 AND a:2");
    });
    it("does not re-add an identical clause", () => {
        expect(addFilter("x:1 AND a:2", "a", 2)).toBe("x:1 AND a:2");
    });
});

describe("isFilterActive", () => {
    it("matches an exact top-level AND clause", () => {
        expect(isFilterActive("a:1 AND b:2", "a", 1)).toBe(true);
        expect(isFilterActive("a:1 AND b:2", "b", 2)).toBe(true);
        expect(isFilterActive("a:1", "b", 2)).toBe(false);
    });
});

describe("toggleFilter", () => {
    it("adds when absent", () => {
        expect(toggleFilter("a:1", "b", 2)).toBe("a:1 AND b:2");
        expect(toggleFilter("", "a", 1)).toBe("a:1");
    });
    it("removes when present", () => {
        expect(toggleFilter("a:1 AND b:2", "a", 1)).toBe("b:2");
    });
    it("leaves * when removing the last clause", () => {
        expect(toggleFilter("a:1", "a", 1)).toBe("*");
    });
    it("round-trips: add then toggle removes", () => {
        const added = addFilter("base:1", "x", "y");
        expect(toggleFilter(added, "x", "y")).toBe("base:1");
    });
});

describe("toggleTerm / isTermActive", () => {
    it("adds a quoted phrase term and detects it", () => {
        const q = toggleTerm("", "auth-svc");
        expect(q).toBe('"auth-svc"');
        expect(isTermActive(q, "auth-svc")).toBe(true);
    });
    it("removes the term on second toggle", () => {
        expect(toggleTerm('"foo"', "foo")).toBe("*");
    });
    it("AND-combines with existing clauses", () => {
        expect(toggleTerm("level:error", "timeout")).toBe('level:error AND "timeout"');
    });
});

describe("sameAsCurrent", () => {
    const base = {
        q: "level:error",
        range: "-1h",
        follow: false,
        index: "web",
        start: undefined,
        end: undefined,
    };
    const saved = base as unknown as SavedSearch;
    const current = base as unknown as SearchInput;
    it("is true for identical params", () => {
        expect(sameAsCurrent(saved, current)).toBe(true);
    });
    it("treats null/undefined index equivalently", () => {
        const s = { ...base, index: undefined } as unknown as SavedSearch;
        const c = { ...base, index: "" } as unknown as SearchInput;
        expect(sameAsCurrent(s, c)).toBe(true);
    });
    it("is false when the query differs", () => {
        const c = { ...base, q: "level:warn" } as unknown as SearchInput;
        expect(sameAsCurrent(saved, c)).toBe(false);
    });
});

describe("suggestName", () => {
    it("uses the query when it's not match-all", () => {
        expect(suggestName({ q: "status:500", range: "-1h", follow: false })).toBe("status:500");
    });
    it("falls back to index · range for match-all with an index", () => {
        expect(suggestName({ q: "*", range: "-6h", follow: false, index: "web" })).toBe(
            "web · -6h",
        );
    });
    it("falls back to the range alone", () => {
        expect(suggestName({ q: "*", range: "-6h", follow: false })).toBe("-6h");
    });
});
