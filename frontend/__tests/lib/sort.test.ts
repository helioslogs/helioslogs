// Copyright 2026 Appbird LLC. Licensed under "GNU Affero General Public License v3.0"

import { describe, it, expect } from "vitest";
import { sortRows, type SortState } from "../../src/lib/sort";

interface Row {
    name: string;
    n: number | null;
}

const rows: Row[] = [
    { name: "b", n: 2 },
    { name: "a", n: 10 },
    { name: "c", n: 5 },
];
const acc = {
    name: (r: Row) => r.name,
    n: (r: Row) => r.n,
};

describe("sortRows", () => {
    it("sorts ascending and descending by a numeric accessor", () => {
        const asc = sortRows(rows, { key: "n", dir: "asc" }, acc).map((r) => r.n);
        expect(asc).toEqual([2, 5, 10]);
        const desc = sortRows(rows, { key: "n", dir: "desc" }, acc).map((r) => r.n);
        expect(desc).toEqual([10, 5, 2]);
    });

    it("sorts strings with natural/numeric collation", () => {
        const out = sortRows(rows, { key: "name", dir: "asc" }, acc).map((r) => r.name);
        expect(out).toEqual(["a", "b", "c"]);
    });

    it("keeps nullish/empty values at the bottom regardless of direction", () => {
        const withNull: Row[] = [
            { name: "x", n: null },
            { name: "y", n: 1 },
        ];
        const asc = sortRows(withNull, { key: "n", dir: "asc" }, acc).map((r) => r.n);
        expect(asc).toEqual([1, null]);
        const desc = sortRows(withNull, { key: "n", dir: "desc" }, acc).map((r) => r.n);
        expect(desc).toEqual([1, null]);
    });

    it("returns the input unchanged when no key or unknown key", () => {
        const noKey: SortState = { key: null, dir: "asc" };
        expect(sortRows(rows, noKey, acc)).toBe(rows);
        expect(sortRows(rows, { key: "missing", dir: "asc" }, acc)).toBe(rows);
    });

    it("does not mutate the input array", () => {
        const copy = [...rows];
        sortRows(rows, { key: "n", dir: "desc" }, acc);
        expect(rows).toEqual(copy);
    });
});
