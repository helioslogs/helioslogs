// Copyright 2026 Appbird LLC. Licensed under "GNU Affero General Public License v3.0"

import { describe, it, expect } from "vitest";
import { render, screen } from "@testing-library/react";
import { ResultsTable } from "../../src/components/ResultsTable";
import type { TableResult } from "../../src/api/types";

const table: TableResult = {
    columns: ["service", "count"],
    rows: [
        ["api", 5],
        ["web", 3],
    ],
    search: "*",
    stages: ["stats count by service"],
    took_us: 1234,
    scanned_docs: 100,
    partitions_scanned: 2,
};

describe("<ResultsTable>", () => {
    it("renders column headers and row cells", () => {
        render(<ResultsTable table={table} />);
        expect(screen.getByText("service")).toBeInTheDocument();
        expect(screen.getByText("count")).toBeInTheDocument();
        expect(screen.getByText("api")).toBeInTheDocument();
        expect(screen.getByText("web")).toBeInTheDocument();
        // Two data rows + the header row.
        expect(document.querySelectorAll("tbody tr")).toHaveLength(2);
    });

    it("echoes the search expression and pipe stages", () => {
        render(<ResultsTable table={table} />);
        expect(screen.getByText("stats count by service")).toBeInTheDocument();
    });

    it("shows an empty state when there are no rows", () => {
        render(<ResultsTable table={{ ...table, rows: [] }} />);
        expect(screen.getByText("no results")).toBeInTheDocument();
    });
});
