// Copyright 2026 Appbird LLC. Licensed under "GNU Affero General Public License v3.0"

import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen } from "@testing-library/react";

const { getRuntimeConfig, getTunables, getSettings, updateSettings, updateTunable } = vi.hoisted(
    () => ({
        getRuntimeConfig: vi.fn(),
        getTunables: vi.fn(),
        getSettings: vi.fn(),
        updateSettings: vi.fn(),
        updateTunable: vi.fn(),
    }),
);
vi.mock("../../src/api/client", () => ({
    getRuntimeConfig,
    getTunables,
    getSettings,
    updateSettings,
    updateTunable,
}));

import { GeneralPanel } from "../../src/pages/admin/GeneralPanel";

beforeEach(() => {
    getRuntimeConfig.mockReset();
    getTunables.mockReset();
    getTunables.mockResolvedValue([]);
    getSettings.mockReset();
    getSettings.mockResolvedValue({
        theme_default_appearance: "dark",
        theme_default_palette: "slate",
    });
});

describe("<GeneralPanel>", () => {
    it("renders the runtime config grouped by category", async () => {
        getRuntimeConfig.mockResolvedValue([
            {
                name: "HELIOS_DATA_DIR",
                value: "./data",
                description: "Where blocks live",
                category: "Storage",
                overridden: true,
            },
        ]);
        render(<GeneralPanel />);
        expect(screen.getByRole("heading", { name: "General settings" })).toBeInTheDocument();
        expect(await screen.findByText("HELIOS_DATA_DIR")).toBeInTheDocument();
        expect(screen.getByText("Storage")).toBeInTheDocument();
        expect(screen.getByText("./data")).toBeInTheDocument();
        expect(screen.getByText("configured")).toBeInTheDocument(); // overridden flag
    });

    it("renders the appearance defaults from settings", async () => {
        getRuntimeConfig.mockResolvedValue([]);
        render(<GeneralPanel />);
        expect(await screen.findByLabelText("Default appearance")).toHaveValue("dark");
        expect(screen.getByLabelText("Default theme")).toHaveValue("slate");
    });
});
