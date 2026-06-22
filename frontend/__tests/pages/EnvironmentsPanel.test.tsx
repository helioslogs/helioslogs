// Copyright 2026 Appbird LLC. Licensed under "GNU Affero General Public License v3.0"

import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

const { listEnvsWithDefault, createEnv, deleteEnv, reorderEnvs, setDefaultEnv, setEnvRetention } =
    vi.hoisted(() => ({
        listEnvsWithDefault: vi.fn(),
        createEnv: vi.fn(),
        deleteEnv: vi.fn(),
        reorderEnvs: vi.fn(),
        setDefaultEnv: vi.fn(),
        setEnvRetention: vi.fn(),
    }));
vi.mock("../../src/api/client", () => ({
    listEnvsWithDefault,
    createEnv,
    deleteEnv,
    reorderEnvs,
    setDefaultEnv,
    setEnvRetention,
}));

import { EnvironmentsPanel } from "../../src/pages/admin/EnvironmentsPanel";

const env = (name: string) => ({ name, is_system: false, created_at: "2026-01-01T00:00:00Z" });
const catalog = (names: string[], defaultEnv: string | null = null) => ({
    envs: names.map(env),
    defaultEnv,
});

beforeEach(() => {
    listEnvsWithDefault.mockReset();
    createEnv.mockReset();
    deleteEnv.mockReset();
    reorderEnvs.mockReset();
    setDefaultEnv.mockReset();
    setEnvRetention.mockReset();
});

describe("<EnvironmentsPanel>", () => {
    it("shows the empty state when there are no user envs", async () => {
        listEnvsWithDefault.mockResolvedValue(catalog([]));
        render(<EnvironmentsPanel />);
        expect(screen.getByRole("heading", { name: "Environments" })).toBeInTheDocument();
        expect(await screen.findByText(/No user envs/)).toBeInTheDocument();
    });

    it("lists the fetched envs", async () => {
        listEnvsWithDefault.mockResolvedValue(catalog(["prod", "default"]));
        render(<EnvironmentsPanel />);
        expect(await screen.findByText("prod")).toBeInTheDocument();
    });

    it("creates an env from the input", async () => {
        listEnvsWithDefault.mockResolvedValue(catalog([]));
        createEnv.mockResolvedValue(env("dev"));
        render(<EnvironmentsPanel />);
        await screen.findByText(/No user envs/);
        await userEvent.type(screen.getByPlaceholderText(/new env name/i), "dev");
        await userEvent.click(screen.getByRole("button", { name: "Create" }));
        await waitFor(() => expect(createEnv).toHaveBeenCalledWith("dev"));
    });

    it("rejects an invalid env name before calling the API", async () => {
        listEnvsWithDefault.mockResolvedValue(catalog([]));
        render(<EnvironmentsPanel />);
        await screen.findByText(/No user envs/);
        await userEvent.type(screen.getByPlaceholderText(/new env name/i), "bad name");
        await userEvent.click(screen.getByRole("button", { name: "Create" }));
        expect(createEnv).not.toHaveBeenCalled();
        expect(screen.getByText(/only letters, digits/i)).toBeInTheDocument();
    });

    it("marks an env as the default for new users", async () => {
        listEnvsWithDefault.mockResolvedValue(catalog(["prod", "default"]));
        setDefaultEnv.mockResolvedValue("prod");
        render(<EnvironmentsPanel />);
        await screen.findByText("prod");
        // Both rows show a "set as default" star; the first is prod's.
        await userEvent.click(screen.getAllByTitle(/set as default for new users/i)[0]);
        await waitFor(() => expect(setDefaultEnv).toHaveBeenCalledWith("prod"));
    });

    it("reorders envs with the move buttons", async () => {
        listEnvsWithDefault.mockResolvedValue(catalog(["prod", "default"]));
        reorderEnvs.mockResolvedValue([]);
        render(<EnvironmentsPanel />);
        await screen.findByText("prod");
        await userEvent.click(screen.getByLabelText("Move default up"));
        await waitFor(() => expect(reorderEnvs).toHaveBeenCalledWith(["default", "prod"]));
    });
});
