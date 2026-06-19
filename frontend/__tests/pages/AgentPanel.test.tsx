// Copyright 2026 Appbird LLC. Licensed under "GNU Affero General Public License v3.0"

import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen } from "@testing-library/react";

const { getLlmSettings, updateLlmSettings } = vi.hoisted(() => ({
    getLlmSettings: vi.fn(),
    updateLlmSettings: vi.fn(),
}));
vi.mock("../../src/api/agent", () => ({ getLlmSettings, updateLlmSettings }));

import { AgentPanel } from "../../src/pages/admin/AgentPanel";

const settings = {
    enabled: true,
    provider: "openai" as const,
    model: "local",
    openai_model: "local",
    anthropic_model: "claude-sonnet-4-6",
    bedrock_model: "anthropic.claude-sonnet-4-6",
    openai_endpoint: "http://localhost:8080/v1",
    openai_api_key_set: false,
    anthropic_endpoint: "https://api.anthropic.com/v1",
    anthropic_api_key_set: false,
    bedrock_region: "us-east-1",
    bedrock_auth_mode: "default_chain" as const,
    bedrock_access_key_id_set: false,
    bedrock_secret_access_key_set: false,
    bedrock_session_token_set: false,
    bedrock_bearer_token_set: false,
};

beforeEach(() => {
    getLlmSettings.mockReset();
    updateLlmSettings.mockReset();
});

describe("<AgentPanel>", () => {
    it("loads and renders the provider configuration", async () => {
        getLlmSettings.mockResolvedValue(settings);
        render(<AgentPanel />);
        // The panel shows "Loading…" until settings resolve; wait for content.
        expect(await screen.findAllByText("OpenAI-compatible")).not.toHaveLength(0);
        expect(
            screen.getByRole("heading", { name: "LLM Provider Configuration" }),
        ).toBeInTheDocument();
        expect(screen.getByText("AWS Bedrock")).toBeInTheDocument();
    });
});
