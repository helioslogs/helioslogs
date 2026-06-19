// Copyright 2026 Appbird LLC. Licensed under "GNU Affero General Public License v3.0"

// HTTP client for the backend agent endpoints: CRUD over conversations
// plus the SSE-streaming message endpoint.

import type { ViewContext } from "../lib/viewContext";
import { apiFetch } from "./client";

export interface ConversationMeta {
    id: string;
    title: string;
    created_at: number;
    updated_at: number;
}

export interface StoredToolCall {
    id: string;
    name: string;
    arguments: Record<string, unknown>;
    status: "ok" | "error";
    result?: unknown;
    error?: string | null;
    duration_ms: number;
}

export interface StoredTurn {
    id: string;
    turn_idx: number;
    role: "user" | "assistant";
    payload: {
        content?: string;
        view_context?: ViewContext;
        tool_calls?: StoredToolCall[];
        reasoning?: string;
        reasoning_duration_ms?: number;
        duration_ms?: number;
    };
    created_at: number;
}

export interface ConversationDetail extends ConversationMeta {
    turns: StoredTurn[];
}

export async function listConversations(): Promise<ConversationMeta[]> {
    const r = await apiFetch("/api/agent/conversations");
    if (!r.ok) throw new Error(`list conversations: ${r.status}`);
    const j = await r.json();
    return j.conversations ?? [];
}

export async function createConversation(title?: string): Promise<ConversationMeta> {
    const r = await apiFetch("/api/agent/conversations", {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: JSON.stringify({ title: title ?? "" }),
    });
    if (!r.ok) throw new Error(`create conversation: ${r.status}`);
    const j = await r.json();
    return j.conversation;
}

export async function getConversation(id: string): Promise<ConversationDetail> {
    const r = await apiFetch(`/api/agent/conversations/${id}`);
    if (!r.ok) throw new Error(`get conversation: ${r.status}`);
    return r.json();
}

export async function renameConversation(id: string, title: string): Promise<void> {
    const r = await apiFetch(`/api/agent/conversations/${id}`, {
        method: "PATCH",
        headers: { "content-type": "application/json" },
        body: JSON.stringify({ title }),
    });
    if (!r.ok) throw new Error(`rename conversation: ${r.status}`);
}

export async function deleteConversation(id: string): Promise<void> {
    const r = await apiFetch(`/api/agent/conversations/${id}`, { method: "DELETE" });
    if (!r.ok) throw new Error(`delete conversation: ${r.status}`);
}

// ---- SSE event types matching backend AgentEvent ------------------------

export type AgentEvent =
    | { type: "turn_start"; turn_idx: number }
    | { type: "content_delta"; delta: string }
    | { type: "reasoning_delta"; delta: string }
    | {
          type: "tool_delta";
          index: number;
          id?: string;
          name?: string;
          arguments_delta?: string;
      }
    | {
          type: "tool_running";
          index: number;
          id: string;
          name: string;
          arguments: Record<string, unknown>;
      }
    | {
          type: "tool_result";
          index: number;
          id: string;
          name: string;
          status: "ok" | "error";
          result?: unknown;
          error?: string;
          duration_ms: number;
      }
    | {
          type: "turn_end";
          duration_ms: number;
          reasoning_duration_ms?: number;
          content: string;
      }
    | { type: "error"; message: string };

export interface SendHandlers {
    onEvent: (evt: AgentEvent) => void;
}

// Read an SSE response body, invoking `onMessage` with each parsed `data:` JSON
// event. Keepalive comments and malformed payloads are skipped.
async function consumeSse(r: Response, onMessage: (msg: unknown) => void): Promise<void> {
    if (!r.body) throw new Error("stream: no response body");
    const reader = r.body.getReader();
    const decoder = new TextDecoder();
    let buf = "";
    while (true) {
        const { value, done } = await reader.read();
        if (done) break;
        buf += decoder.decode(value, { stream: true });
        let nl: number;
        while ((nl = buf.indexOf("\n\n")) !== -1) {
            const block = buf.slice(0, nl);
            buf = buf.slice(nl + 2);
            const dataLines: string[] = [];
            for (const line of block.split("\n")) {
                if (line.startsWith("data:")) dataLines.push(line.slice(5).trimStart());
            }
            if (dataLines.length === 0) continue;
            try {
                onMessage(JSON.parse(dataLines.join("\n")));
            } catch {
                continue; // skip malformed
            }
        }
    }
}

// Stream events for one user → assistant exchange. Resolves on stream end,
// rejects on protocol/network error, throws AbortError on `signal.abort()`.
export async function sendMessage(
    conversationId: string,
    content: string,
    viewContext: ViewContext | undefined,
    handlers: SendHandlers,
    signal?: AbortSignal,
): Promise<void> {
    const r = await apiFetch(`/api/agent/conversations/${conversationId}/messages`, {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: JSON.stringify({
            content,
            view_context: viewContext ?? null,
        }),
        signal,
    });
    if (!r.ok) {
        const body = await r.text().catch(() => "");
        throw new Error(`send message ${r.status}: ${body || r.statusText}`);
    }
    await consumeSse(r, (msg) => handlers.onEvent(msg as AgentEvent));
}

export interface MonitorRunHandlers extends SendHandlers {
    // Fired once with the trace conversation id before the agent events stream.
    onConversation: (id: string) => void;
}

// Run an AI monitor and stream its live investigation trace. The first SSE
// event carries the conversation id; the rest are normal `AgentEvent`s.
export async function streamMonitorRun(
    monitorId: string,
    handlers: MonitorRunHandlers,
    signal?: AbortSignal,
): Promise<void> {
    const r = await apiFetch(`/api/monitors/${encodeURIComponent(monitorId)}/run_live`, {
        method: "POST",
        signal,
    });
    if (!r.ok) {
        const body = await r.text().catch(() => "");
        throw new Error(`run monitor ${r.status}: ${body || r.statusText}`);
    }
    await consumeSse(r, (msg) => {
        const m = msg as { type?: string; id?: string };
        if (m.type === "conversation" && typeof m.id === "string") {
            handlers.onConversation(m.id);
        } else {
            handlers.onEvent(msg as AgentEvent);
        }
    });
}

// ---- LLM admin settings -------------------------------------------------

export interface LlmSettings {
    // Master on/off switch for chat + AI monitors.
    enabled: boolean;
    provider: "openai" | "anthropic" | "bedrock";
    // Resolved model for the active provider; per-provider models below are the source of truth.
    model: string;
    openai_model: string;
    anthropic_model: string;
    bedrock_model: string;
    openai_endpoint: string;
    openai_api_key_set: boolean;
    anthropic_endpoint: string;
    anthropic_api_key_set: boolean;
    bedrock_region: string;
    bedrock_auth_mode: "default_chain" | "bearer_token";
    bedrock_access_key_id_set: boolean;
    bedrock_secret_access_key_set: boolean;
    bedrock_session_token_set: boolean;
    bedrock_bearer_token_set: boolean;
}

export async function getLlmSettings(): Promise<LlmSettings> {
    const r = await apiFetch("/api/admin/agent");
    if (!r.ok) throw new Error(`get llm settings: ${r.status}`);
    return r.json();
}

// Public agent availability — readable by any logged-in user (not admin-only),
// so chat/monitor UIs can reflect the disabled state.
export async function getAgentStatus(): Promise<{ enabled: boolean }> {
    const r = await apiFetch("/api/agent/status");
    if (!r.ok) throw new Error(`agent status: ${r.status}`);
    return r.json();
}

// Patch LLM settings (send only changed keys). For API keys: empty string
// clears, non-empty sets, `undefined` leaves unchanged.
export async function updateLlmSettings(
    patch: Partial<{
        enabled: boolean;
        provider: LlmSettings["provider"];
        openai_model: string;
        anthropic_model: string;
        bedrock_model: string;
        openai_endpoint: string;
        openai_api_key: string;
        anthropic_endpoint: string;
        anthropic_api_key: string;
        bedrock_region: string;
        bedrock_auth_mode: LlmSettings["bedrock_auth_mode"];
        bedrock_access_key_id: string;
        bedrock_secret_access_key: string;
        bedrock_session_token: string;
        bedrock_bearer_token: string;
    }>,
): Promise<LlmSettings> {
    const r = await apiFetch("/api/admin/agent", {
        method: "PUT",
        headers: { "content-type": "application/json" },
        body: JSON.stringify(patch),
    });
    if (!r.ok) throw new Error(`update llm settings: ${r.status}`);
    return r.json();
}

export interface LlmTestMessage {
    role: string;
    content: string;
}

export interface LlmTestResult {
    ok: boolean;
    provider?: string;
    model?: string;
    // The exact messages HeliosLogs submitted to the model.
    request?: LlmTestMessage[];
    reply?: string;
    error?: string;
}

// Connectivity check. The optional patch (same shape as updateLlmSettings)
// is overlaid on saved settings server-side, so unsaved form edits are tested;
// blank API-key fields fall back to the stored key.
export async function testLlmSettings(
    overrides?: Parameters<typeof updateLlmSettings>[0],
): Promise<LlmTestResult> {
    const r = await apiFetch("/api/admin/agent/test", {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: JSON.stringify(overrides ?? {}),
    });
    if (!r.ok) throw new Error(`test llm settings: ${r.status}`);
    return r.json();
}
