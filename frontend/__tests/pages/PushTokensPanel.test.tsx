// Copyright 2026 Appbird LLC. Licensed under "GNU Affero General Public License v3.0"

import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen } from "@testing-library/react";

const mocks = vi.hoisted(() => ({
    getIngestTokens: vi.fn(),
    listEnvs: vi.fn(),
    createIngestToken: vi.fn(),
    deleteIngestToken: vi.fn(),
    setIngestRequire: vi.fn(),
    setIngestTokenEnabled: vi.fn(),
}));
vi.mock("../../src/api/client", () => mocks);

import { PushTokensPanel } from "../../src/pages/admin/PushTokensPanel";

beforeEach(() => {
    Object.values(mocks).forEach((m) => m.mockReset());
    mocks.listEnvs.mockResolvedValue([{ name: "default", created_at: "2026-01-01T00:00:00Z" }]);
});

describe("<PushTokensPanel>", () => {
    it("lists existing tokens with their masked hint", async () => {
        mocks.getIngestTokens.mockResolvedValue({
            require: false,
            tokens: [
                {
                    id: "t1",
                    name: "shipper",
                    token_hint: "…abcd",
                    env: "default",
                    indexes: [],
                    enabled: true,
                    last_used_at: null,
                    created_at: "2026-01-01T00:00:00Z",
                    updated_at: "2026-01-01T00:00:00Z",
                },
            ],
        });
        render(<PushTokensPanel />);
        expect(screen.getByRole("heading", { name: "Ingest tokens" })).toBeInTheDocument();
        expect(await screen.findByText("shipper")).toBeInTheDocument();
        expect(screen.getByText("…abcd")).toBeInTheDocument();
    });
});
