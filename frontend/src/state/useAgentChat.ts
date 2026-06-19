// Copyright 2026 Appbird LLC. Licensed under "GNU Affero General Public License v3.0"

// React state for the agent investigation panel. Talks to the backend agent API
// (`/api/agent/*`); owns in-flight render state and applies SSE events to it.

import { useCallback, useEffect, useRef, useState } from "react";
import {
    createConversation as apiCreateConversation,
    deleteConversation as apiDeleteConversation,
    getConversation,
    listConversations,
    renameConversation as apiRenameConversation,
    sendMessage,
    streamMonitorRun,
    type AgentEvent,
    type ConversationMeta,
    type StoredToolCall,
    type StoredTurn,
} from "../api/agent";
import { getViewContext, type ViewContext } from "../lib/viewContext";

export interface AgentToolCallUI {
    id: string;
    name: string;
    arguments: Record<string, unknown>;
    status: "streaming" | "running" | "ok" | "error";
    result?: unknown;
    error?: string;
    startedAt?: number;
    durationMs?: number;
}

export interface AgentTurn {
    role: "user" | "assistant";
    content: string;
    viewContext?: ViewContext;
    toolCalls?: AgentToolCallUI[];
    reasoning?: string;
    reasoningStartedAt?: number;
    reasoningDurationMs?: number;
    durationMs?: number;
    streaming?: boolean;
}

interface UseAgentChat {
    // Active conversation id; `null` when none selected.
    conversationId: string | null;
    // Metadata for the active conversation; `null` until `conversationId` is set.
    conversationMeta: ConversationMeta | null;
    // All conversations for this user, newest first. Loaded on mount.
    conversations: ConversationMeta[];
    // Turns of the active conversation. Empty when none is selected.
    turns: AgentTurn[];
    // True once the initial conversation load has settled.
    ready: boolean;
    busy: boolean;
    error: string | null;
    send: (userMessage: string) => Promise<void>;
    // Start a new conversation seeded with `seedMessage` and run it.
    investigate: (seedMessage: string) => Promise<void>;
    // Run an AI monitor and stream its investigation trace live.
    runMonitorLive: (monitorId: string, monitorName: string) => Promise<void>;
    cancel: () => void;
    // Clear the active conversation by switching to a fresh, server-persisted one.
    clear: () => Promise<void>;
    selectConversation: (id: string) => Promise<void>;
    newConversation: () => Promise<void>;
    renameConversation: (id: string, title: string) => Promise<void>;
    deleteConversation: (id: string) => Promise<void>;
}

function toolFromStored(c: StoredToolCall): AgentToolCallUI {
    return {
        id: c.id,
        name: c.name,
        arguments: c.arguments,
        status: c.status,
        result: c.result,
        error: c.error ?? undefined,
        durationMs: c.duration_ms,
    };
}

function turnFromStored(t: StoredTurn): AgentTurn {
    if (t.role === "user") {
        return {
            role: "user",
            content: t.payload.content ?? "",
            viewContext: t.payload.view_context,
        };
    }
    return {
        role: "assistant",
        content: t.payload.content ?? "",
        toolCalls: (t.payload.tool_calls ?? []).map(toolFromStored),
        reasoning: t.payload.reasoning,
        reasoningDurationMs: t.payload.reasoning_duration_ms,
        durationMs: t.payload.duration_ms,
    };
}

// Build an SSE event handler that folds streaming deltas into the last
// (assistant) turn via `patchLast`. Shared by interactive chat and live
// monitor runs — both produce the same `AgentEvent` stream.
function makeTurnFolder(
    patchLast: (patch: Partial<AgentTurn>) => void,
    setError: (msg: string) => void,
): (evt: AgentEvent) => void {
    let turnContent = "";
    let turnReasoning = "";
    let firstReasoningAt: number | undefined;
    let lastReasoningAt: number | undefined;
    const toolCalls: AgentToolCallUI[] = [];

    return (evt: AgentEvent) => {
        switch (evt.type) {
            case "content_delta": {
                turnContent += evt.delta;
                patchLast({ content: turnContent });
                break;
            }
            case "reasoning_delta": {
                turnReasoning += evt.delta;
                const now = performance.now();
                if (firstReasoningAt === undefined) firstReasoningAt = now;
                lastReasoningAt = now;
                patchLast({
                    reasoning: turnReasoning,
                    reasoningStartedAt: firstReasoningAt,
                    reasoningDurationMs:
                        lastReasoningAt !== undefined && firstReasoningAt !== undefined
                            ? lastReasoningAt - firstReasoningAt
                            : undefined,
                });
                break;
            }
            case "tool_delta": {
                // Backend re-sends index-deltas with whatever fragments arrived
                // from the model; track the latest known fields.
                while (toolCalls.length <= evt.index) {
                    toolCalls.push({ id: "", name: "", arguments: {}, status: "streaming" });
                }
                const slot = toolCalls[evt.index];
                if (evt.id) slot.id = evt.id;
                if (evt.name) slot.name = evt.name;
                if (evt.arguments_delta !== undefined) {
                    const acc = (slot.arguments as { _argsRaw?: string })._argsRaw ?? "";
                    const next = acc + evt.arguments_delta;
                    try {
                        const parsed = JSON.parse(next);
                        if (parsed && typeof parsed === "object") {
                            slot.arguments = parsed as Record<string, unknown>;
                        } else {
                            slot.arguments = { _argsRaw: next };
                        }
                    } catch {
                        slot.arguments = { _argsRaw: next };
                    }
                }
                patchLast({ toolCalls: [...toolCalls] });
                break;
            }
            case "tool_running": {
                while (toolCalls.length <= evt.index) {
                    toolCalls.push({
                        id: evt.id,
                        name: evt.name,
                        arguments: evt.arguments,
                        status: "running",
                    });
                }
                toolCalls[evt.index] = {
                    ...toolCalls[evt.index],
                    id: evt.id,
                    name: evt.name,
                    arguments: evt.arguments,
                    status: "running",
                    startedAt: performance.now(),
                };
                patchLast({ toolCalls: [...toolCalls] });
                break;
            }
            case "tool_result": {
                const slot = toolCalls[evt.index] ?? {
                    id: evt.id,
                    name: evt.name,
                    arguments: {},
                    status: "ok",
                };
                toolCalls[evt.index] = {
                    ...slot,
                    status: evt.status,
                    result: evt.result,
                    error: evt.error,
                    durationMs: evt.duration_ms,
                };
                patchLast({ toolCalls: [...toolCalls] });
                break;
            }
            case "turn_end": {
                patchLast({
                    content: evt.content,
                    streaming: false,
                    durationMs: evt.duration_ms,
                    reasoningDurationMs: evt.reasoning_duration_ms,
                });
                break;
            }
            case "error": {
                setError(evt.message);
                break;
            }
            case "turn_start":
                break;
        }
    };
}

export function useAgentChat(): UseAgentChat {
    const [conversations, setConversations] = useState<ConversationMeta[]>([]);
    const [conversationId, setConversationId] = useState<string | null>(null);
    const [conversationMeta, setConversationMeta] = useState<ConversationMeta | null>(null);
    const [turns, setTurns] = useState<AgentTurn[]>([]);
    const [busy, setBusy] = useState(false);
    const [error, setError] = useState<string | null>(null);
    // True once the initial load settled; the post-env-switch alert replay waits on this.
    const [ready, setReady] = useState(false);
    const abortRef = useRef<AbortController | null>(null);

    // Refresh the sidebar list. Cheap; called after any mutation.
    const refreshConversations = useCallback(async () => {
        try {
            const list = await listConversations();
            setConversations(list);
        } catch (e) {
            // List failures are non-fatal — the user can still chat.
            console.warn("listConversations:", e);
        }
    }, []);

    // Boot: load conversations, then either reopen the most recent one or
    // create a fresh one to slot into.
    useEffect(() => {
        let cancelled = false;
        (async () => {
            try {
                const list = await listConversations();
                if (cancelled) return;
                setConversations(list);
                if (list.length > 0) {
                    await openConversation(list[0].id);
                } else {
                    await createFresh();
                }
            } catch (e) {
                if (!cancelled) {
                    setError(e instanceof Error ? e.message : String(e));
                }
            } finally {
                if (!cancelled) setReady(true);
            }
        })();
        return () => {
            cancelled = true;
            abortRef.current?.abort();
        };
        // eslint-disable-next-line react-hooks/exhaustive-deps
    }, []);

    const openConversation = useCallback(async (id: string) => {
        setError(null);
        const detail = await getConversation(id);
        setConversationId(detail.id);
        setConversationMeta({
            id: detail.id,
            title: detail.title,
            created_at: detail.created_at,
            updated_at: detail.updated_at,
        });
        setTurns(detail.turns.map(turnFromStored));
    }, []);

    const createFresh = useCallback(async () => {
        const meta = await apiCreateConversation();
        setConversationId(meta.id);
        setConversationMeta(meta);
        setTurns([]);
        setConversations((prev) => [meta, ...prev.filter((c) => c.id !== meta.id)]);
        return meta;
    }, []);

    const selectConversation = useCallback(
        async (id: string) => {
            abortRef.current?.abort();
            await openConversation(id);
        },
        [openConversation],
    );

    const newConversation = useCallback(async () => {
        abortRef.current?.abort();
        await createFresh();
    }, [createFresh]);

    const renameConversation = useCallback(
        async (id: string, title: string) => {
            await apiRenameConversation(id, title);
            if (conversationId === id) {
                setConversationMeta((m) => (m ? { ...m, title } : m));
            }
            await refreshConversations();
        },
        [conversationId, refreshConversations],
    );

    const deleteConversation = useCallback(
        async (id: string) => {
            abortRef.current?.abort();
            await apiDeleteConversation(id);
            // If we deleted the active conversation, slot into a fresh one or
            // the next-newest if there's any left.
            const next = conversations.filter((c) => c.id !== id);
            setConversations(next);
            if (conversationId === id) {
                if (next.length > 0) {
                    await openConversation(next[0].id);
                } else {
                    await createFresh();
                }
            }
        },
        [conversationId, conversations, openConversation, createFresh],
    );

    const clear = useCallback(async () => {
        await newConversation();
    }, [newConversation]);

    const cancel = useCallback(() => {
        abortRef.current?.abort();
    }, []);

    const patchLast = useCallback((patch: Partial<AgentTurn>) => {
        setTurns((prev) => {
            const next = [...prev];
            const i = next.length - 1;
            if (i < 0 || next[i].role !== "assistant") return prev;
            next[i] = { ...next[i], ...patch };
            return next;
        });
    }, []);

    // Core send loop, parameterised on the target conversation so a freshly-created
    // one can be driven without waiting for `conversationId` state to flush.
    const runSend = useCallback(
        async (convId: string, userMessage: string) => {
            const trimmed = userMessage.trim();
            if (!trimmed) return;

            setError(null);
            setBusy(true);

            const viewContext = getViewContext();
            const turnStartedAt = performance.now();

            // Optimistically render the user turn + streaming assistant shell; don't
            // refetch until the stream settles so the UI doesn't flicker mid-turn.
            setTurns((prev) => [
                ...prev,
                { role: "user", content: trimmed, viewContext },
                { role: "assistant", content: "", streaming: true, toolCalls: [] },
            ]);

            abortRef.current = new AbortController();
            try {
                await sendMessage(
                    convId,
                    trimmed,
                    viewContext,
                    { onEvent: makeTurnFolder(patchLast, setError) },
                    abortRef.current.signal,
                );
            } catch (e: unknown) {
                if (e instanceof Error && e.name === "AbortError") {
                    patchLast({ streaming: false, durationMs: performance.now() - turnStartedAt });
                } else {
                    const msg = e instanceof Error ? e.message : String(e);
                    setError(msg);
                    patchLast({ streaming: false, durationMs: performance.now() - turnStartedAt });
                }
            } finally {
                setBusy(false);
                abortRef.current = null;
                // Bring the sidebar list in sync (the conversation's
                // updated_at + auto-title may have changed).
                refreshConversations();
            }
        },
        [patchLast, refreshConversations],
    );

    const send = useCallback(
        async (userMessage: string) => {
            if (busy) return;
            if (conversationId === null) return;
            await runSend(conversationId, userMessage);
        },
        [busy, conversationId, runSend],
    );

    // New conversation seeded with one message (per-row "Investigate" action).
    // Aborts any in-flight turn first so the new thread starts clean.
    const investigate = useCallback(
        async (seedMessage: string) => {
            abortRef.current?.abort();
            const meta = await createFresh();
            await runSend(meta.id, seedMessage);
        },
        [createFresh, runSend],
    );

    // Run an AI monitor and render its trace live. The backend creates the
    // conversation and streams the agent loop; we fold events as they arrive.
    const runMonitorLive = useCallback(
        async (monitorId: string, monitorName: string) => {
            abortRef.current?.abort();
            setError(null);
            setBusy(true);
            const nowMs = Date.now();
            setConversationId(null);
            setConversationMeta({
                id: "",
                title: `[monitor] ${monitorName}`,
                created_at: nowMs,
                updated_at: nowMs,
            });
            setTurns([
                { role: "user", content: `Running monitor “${monitorName}”…` },
                { role: "assistant", content: "", streaming: true, toolCalls: [] },
            ]);

            const onEvent = makeTurnFolder(patchLast, setError);
            abortRef.current = new AbortController();
            try {
                await streamMonitorRun(
                    monitorId,
                    {
                        onConversation: (id) => {
                            setConversationId(id);
                            setConversationMeta((m) => (m ? { ...m, id } : m));
                        },
                        onEvent,
                    },
                    abortRef.current.signal,
                );
            } catch (e: unknown) {
                if (e instanceof Error && e.name === "AbortError") {
                    patchLast({ streaming: false });
                } else {
                    setError(e instanceof Error ? e.message : String(e));
                    patchLast({ streaming: false });
                }
            } finally {
                setBusy(false);
                abortRef.current = null;
                refreshConversations();
            }
        },
        [patchLast, refreshConversations],
    );

    return {
        conversationId,
        conversationMeta,
        conversations,
        turns,
        ready,
        busy,
        error,
        send,
        investigate,
        runMonitorLive,
        cancel,
        clear,
        selectConversation,
        newConversation,
        renameConversation,
        deleteConversation,
    };
}
