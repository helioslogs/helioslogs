// Copyright 2026 Appbird LLC. Licensed under "GNU Affero General Public License v3.0"

import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

const { listEnvs, createEnv, deleteEnv } = vi.hoisted(() => ({
    listEnvs: vi.fn(),
    createEnv: vi.fn(),
    deleteEnv: vi.fn(),
}));
vi.mock("../../src/api/client", () => ({ listEnvs, createEnv, deleteEnv }));

import { EnvironmentsPanel } from "../../src/pages/admin/EnvironmentsPanel";

const env = (name: string) => ({ name, created_at: "2026-01-01T00:00:00Z" });

beforeEach(() => {
    listEnvs.mockReset();
    createEnv.mockReset();
    deleteEnv.mockReset();
});

describe("<EnvironmentsPanel>", () => {
    it("shows the empty state when there are no user envs", async () => {
        listEnvs.mockResolvedValue([]);
        render(<EnvironmentsPanel />);
        expect(screen.getByRole("heading", { name: "Environments" })).toBeInTheDocument();
        expect(await screen.findByText(/No user envs/)).toBeInTheDocument();
    });

    it("lists the fetched envs", async () => {
        listEnvs.mockResolvedValue([env("prod"), env("default")]);
        render(<EnvironmentsPanel />);
        expect(await screen.findByText("prod")).toBeInTheDocument();
    });

    it("creates an env from the input", async () => {
        listEnvs.mockResolvedValue([]);
        createEnv.mockResolvedValue(env("dev"));
        render(<EnvironmentsPanel />);
        await screen.findByText(/No user envs/);
        await userEvent.type(screen.getByPlaceholderText(/new env name/i), "dev");
        await userEvent.click(screen.getByRole("button", { name: "Create" }));
        await waitFor(() => expect(createEnv).toHaveBeenCalledWith("dev"));
    });

    it("rejects an invalid env name before calling the API", async () => {
        listEnvs.mockResolvedValue([]);
        render(<EnvironmentsPanel />);
        await screen.findByText(/No user envs/);
        await userEvent.type(screen.getByPlaceholderText(/new env name/i), "bad name");
        await userEvent.click(screen.getByRole("button", { name: "Create" }));
        expect(createEnv).not.toHaveBeenCalled();
        expect(screen.getByText(/only letters, digits/i)).toBeInTheDocument();
    });
});
