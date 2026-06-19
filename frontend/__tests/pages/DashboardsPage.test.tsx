// Copyright 2026 Appbird LLC. Licensed under "GNU Affero General Public License v3.0"

import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen } from "@testing-library/react";
import { MemoryRouter } from "react-router-dom";

// Mock the data hook + auth so the page renders without the network or a
// real session — we're testing the page's rendering, not the hook.
const { useDashboards, useAuth } = vi.hoisted(() => ({
    useDashboards: vi.fn(),
    useAuth: vi.fn(),
}));
vi.mock("../../src/state/useDashboards", () => ({ useDashboards }));
vi.mock("../../src/state/useAuth", () => ({ useAuth }));

import { DashboardsPage } from "../../src/pages/DashboardsPage";

const dashboard = {
    id: "d1",
    name: "Ops Overview",
    description: "",
    spec: { widgets: [] },
    public: true,
    owner: null,
    created_at: "2026-01-01T00:00:00Z",
    updated_at: "2026-01-01T00:00:00Z",
};

const renderPage = () =>
    render(
        <MemoryRouter>
            <DashboardsPage />
        </MemoryRouter>,
    );

beforeEach(() => {
    useAuth.mockReturnValue({ user: { is_admin: false } });
    useDashboards.mockReturnValue({
        items: [dashboard],
        error: null,
        loading: false,
        remove: vi.fn(),
    });
});

describe("<DashboardsPage>", () => {
    it("renders the header and a dashboard row", () => {
        renderPage();
        expect(screen.getByRole("heading", { name: "Dashboards" })).toBeInTheDocument();
        expect(screen.getByText("Ops Overview")).toBeInTheDocument();
    });

    it("offers the New dashboard action", () => {
        renderPage();
        expect(screen.getByRole("button", { name: /New dashboard/i })).toBeInTheDocument();
    });
});
