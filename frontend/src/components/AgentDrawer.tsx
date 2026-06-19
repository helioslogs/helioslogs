// Copyright 2026 Appbird LLC. Licensed under "GNU Affero General Public License v3.0"

import { useEffect, useRef, useState } from "react";
import {
    ArrowUp,
    Brain,
    Check,
    ChevronDown,
    ChevronLeft,
    ChevronRight,
    Eye,
    History,
    Maximize2,
    Minimize2,
    PanelRightClose,
    Pencil,
    Plus,
    Sparkles,
    Square,
    Trash2,
    User,
    X,
} from "lucide-react";
import type { ConversationMeta } from "../api/agent";
import { onInvestigateLog, onOpenConversation, onRunMonitorLive } from "../api/events";
import { replayPendingAlertAction } from "../lib/alertActions";
import type { ViewContext } from "../lib/viewContext";
import { useAgentChat, type AgentToolCallUI, type AgentTurn } from "../state/useAgentChat";
import { useAgentEnabled } from "../state/useAgentEnabled";
import { useAuth } from "../state/useAuth";
import type { AgentPanelState } from "../state/useAgentPanel";
import { AgentToolArtifact } from "./AgentToolArtifact";
import { Markdown } from "./Markdown";
import { MonitorDraftCard } from "./MonitorDraftCard";
import { formatDuration } from "../lib/formatDuration";
import { LiveDuration } from "./LiveDuration";

interface Props {
    panel: AgentPanelState;
}

const STARTER_PROMPTS = [
    "Show me error spikes in the last hour",
    "Which services have the most 5xx responses today?",
    "What's the p95 latency by service?",
    "Find anything unusual in the last 24h",
];

export function AgentDrawer({ panel }: Props) {
    const {
        collapsed,
        maximized,
        effectiveWidth,
        resizing,
        expand,
        collapse,
        toggleMaximize,
        beginResize,
    } = panel;
    // System-wide agent switch. `undefined` while loading → treat as enabled to avoid a flash.
    const { enabled: agentEnabledRaw } = useAgentEnabled();
    const agentEnabled = agentEnabledRaw !== false;
    const isAdmin = !!useAuth().user?.is_admin;
    // The chat hook stays mounted across collapse/expand so the conversation
    // survives — the panel just stops rendering the transcript when railed.
    const {
        turns,
        ready,
        busy,
        error,
        send,
        investigate,
        runMonitorLive,
        cancel,
        conversations,
        conversationId,
        conversationMeta,
        selectConversation,
        newConversation,
        renameConversation,
        deleteConversation,
    } = useAgentChat();
    // History overlay state — when true, the message list is replaced by
    // the conversation picker.
    const [historyOpen, setHistoryOpen] = useState(false);

    // External "open this conversation" requests (e.g. alert inbox Investigate):
    // expand + switch, but don't pop history — land the user IN the conversation.
    useEffect(() => {
        return onOpenConversation((id) => {
            expand();
            setHistoryOpen(false);
            void selectConversation(id);
        });
    }, [expand, selectConversation]);

    // Per-row "Investigate" requests from the search results: expand the
    // drawer, then spin up a fresh thread seeded with the log entry.
    useEffect(() => {
        return onInvestigateLog((prompt) => {
            expand();
            setHistoryOpen(false);
            void investigate(prompt);
        });
    }, [expand, investigate]);

    // "Run & watch" a monitor: expand the drawer and stream the live trace.
    useEffect(() => {
        return onRunMonitorLive(({ monitorId, name }) => {
            expand();
            setHistoryOpen(false);
            void runMonitorLive(monitorId, name);
        });
    }, [expand, runMonitorLive]);

    // Replay a stashed cross-env alert action, but only once chat has booted
    // so the initial conversation load doesn't clobber the new thread.
    useEffect(() => {
        if (!ready) return;
        replayPendingAlertAction();
    }, [ready]);
    const scrollRef = useRef<HTMLDivElement | null>(null);
    const textareaRef = useRef<HTMLTextAreaElement | null>(null);
    // Follow-tail: auto-scroll only while pinned to bottom. Unpin on any upward
    // scroll delta; re-arm only within a tight 4px of the bottom.
    const atBottomRef = useRef(true);
    const lastScrollTopRef = useRef(0);

    useEffect(() => {
        const el = scrollRef.current;
        if (!el || !atBottomRef.current) return;
        el.scrollTop = el.scrollHeight;
        // Record the position so our own scrollTop write isn't read as an upward gesture.
        lastScrollTopRef.current = el.scrollTop;
    }, [turns]);

    const handleScroll = () => {
        const el = scrollRef.current;
        if (!el) return;
        const top = el.scrollTop;
        const prev = lastScrollTopRef.current;
        lastScrollTopRef.current = top;
        if (top < prev) {
            // Any upward delta = user took the wheel. Stop following.
            atBottomRef.current = false;
            return;
        }
        // Re-arm only when they've come back to within 4px of the bottom.
        const dist = el.scrollHeight - top - el.clientHeight;
        if (dist < 4) atBottomRef.current = true;
    };

    const handleSubmit = (e?: React.FormEvent) => {
        e?.preventDefault();
        const text = textareaRef.current?.value ?? "";
        if (!text.trim() || busy) return;
        send(text);
        if (textareaRef.current) textareaRef.current.value = "";
    };

    return (
        <aside
            className={`fixed top-12 right-0 bottom-0 z-20 flex bg-white dark:bg-stone-900 border-l border-stone-200 dark:border-stone-800 ${
                resizing ? "" : "transition-[width] duration-200"
            }`}
            style={{ width: effectiveWidth }}
        >
            {collapsed ? (
                <Rail onExpand={expand} />
            ) : (
                <>
                    {!maximized && <ResizeHandle onMouseDown={beginResize} />}
                    <div className="flex-grow flex flex-col min-w-0">
                        <header className="h-11 flex-shrink-0 px-3 flex items-center gap-2 border-b border-stone-200 dark:border-stone-800">
                            <div className="w-6 h-6 flex-shrink-0 rounded-md bg-gradient-to-br from-orange-500 to-orange-700 flex items-center justify-center text-white">
                                <Sparkles className="w-3.5 h-3.5" />
                            </div>
                            <span className="font-medium text-stone-800 dark:text-stone-100 truncate">
                                {conversationMeta?.title || "Investigate"}
                            </span>
                            <div className="flex-grow" />
                            <button
                                type="button"
                                onClick={() => {
                                    cancel();
                                    void newConversation();
                                    setHistoryOpen(false);
                                }}
                                className="p-1.5 rounded hover:bg-stone-100 dark:hover:bg-stone-800 text-stone-900 dark:text-stone-100"
                                title="New conversation"
                                aria-label="new conversation"
                            >
                                <Plus className="w-3.5 h-3.5" />
                            </button>
                            <button
                                type="button"
                                onClick={() => setHistoryOpen((v) => !v)}
                                className={`p-1.5 rounded transition ${
                                    historyOpen
                                        ? "bg-stone-100 dark:bg-stone-800 text-stone-900 dark:text-stone-100"
                                        : "hover:bg-stone-100 dark:hover:bg-stone-800 text-stone-900 dark:text-stone-100"
                                }`}
                                title="Conversation history"
                                aria-label="conversation history"
                            >
                                <History className="w-3.5 h-3.5" />
                            </button>
                            <button
                                type="button"
                                onClick={toggleMaximize}
                                className="p-1.5 rounded hover:bg-stone-100 dark:hover:bg-stone-800 text-stone-900 dark:text-stone-100"
                                title={maximized ? "Restore" : "Maximize"}
                                aria-label={maximized ? "restore panel" : "maximize panel"}
                            >
                                {maximized ? (
                                    <Minimize2 className="w-3.5 h-3.5" />
                                ) : (
                                    <Maximize2 className="w-3.5 h-3.5" />
                                )}
                            </button>
                            <button
                                type="button"
                                onClick={collapse}
                                className="p-1.5 rounded hover:bg-stone-100 dark:hover:bg-stone-800 text-stone-900 dark:text-stone-100"
                                title="Collapse to side"
                                aria-label="collapse investigate panel"
                            >
                                <PanelRightClose className="w-4 h-4" />
                            </button>
                        </header>

                        {!agentEnabled ? (
                            <AgentDisabledNotice isAdmin={isAdmin} />
                        ) : (
                            <>
                                {historyOpen ? (
                                    <ConversationList
                                        conversations={conversations}
                                        activeId={conversationId}
                                        onPick={async (id) => {
                                            await selectConversation(id);
                                            setHistoryOpen(false);
                                        }}
                                        onNew={async () => {
                                            await newConversation();
                                            setHistoryOpen(false);
                                        }}
                                        onRename={renameConversation}
                                        onDelete={deleteConversation}
                                    />
                                ) : (
                                    <div
                                        ref={scrollRef}
                                        onScroll={handleScroll}
                                        className="flex-grow overflow-auto px-4 py-4 space-y-5"
                                    >
                                        {turns.length === 0 && <Welcome onPick={(p) => send(p)} />}
                                        {turns.map((t, i) => (
                                            <TurnView key={i} turn={t} onPick={send} busy={busy} />
                                        ))}
                                        {error && (
                                            <div className="px-3 py-2 rounded-md bg-red-50 text-red-800 border border-red-200 dark:bg-red-950 dark:text-red-200 dark:border-red-900">
                                                {error}
                                                {error.toLowerCase().includes("fetch") && (
                                                    <div className="mt-1 text-stone-600 dark:text-stone-400">
                                                        LLM unreachable — check{" "}
                                                        <a
                                                            href="/admin/agent"
                                                            className="underline hover:no-underline"
                                                        >
                                                            LLM Provider
                                                        </a>
                                                        .
                                                    </div>
                                                )}
                                            </div>
                                        )}
                                    </div>
                                )}

                                <form
                                    onSubmit={handleSubmit}
                                    className="border-t border-stone-200 dark:border-stone-800 p-3 flex-shrink-0"
                                >
                                    <div className="bg-stone-50 dark:bg-stone-950 border border-stone-200 dark:border-stone-700 rounded-xl focus-within:border-orange-500 focus-within:bg-white dark:focus-within:bg-stone-900 transition">
                                        <textarea
                                            ref={textareaRef}
                                            rows={2}
                                            placeholder="Ask HeliosLogs anything about your logs…"
                                            className="w-full px-3 py-2 bg-transparent border-0 focus:outline-none resize-none text-stone-900 dark:text-stone-100 placeholder:text-stone-400 dark:placeholder:text-stone-500"
                                            onKeyDown={(e) => {
                                                if (e.key === "Enter" && !e.shiftKey) {
                                                    e.preventDefault();
                                                    handleSubmit();
                                                }
                                            }}
                                        />
                                        <div className="flex items-center justify-between px-2 pb-2">
                                            <span className="text-stone-400 dark:text-stone-500">
                                                {busy
                                                    ? "thinking…"
                                                    : "Enter to send · Shift+Enter for newline"}
                                            </span>
                                            {busy ? (
                                                <button
                                                    type="button"
                                                    onClick={cancel}
                                                    className="px-2.5 py-1 bg-stone-700 dark:bg-stone-600 text-white font-medium rounded-md hover:bg-stone-600 dark:hover:bg-stone-500 flex items-center gap-1.5"
                                                    title="Stop"
                                                >
                                                    <Square className="w-3 h-3" />
                                                    Stop
                                                </button>
                                            ) : (
                                                <button
                                                    type="submit"
                                                    className="px-2.5 py-1 bg-orange-600 hover:bg-orange-500 text-white font-medium rounded-md flex items-center gap-1.5"
                                                    title="Send (Enter)"
                                                >
                                                    <ArrowUp className="w-3 h-3" />
                                                    Send
                                                </button>
                                            )}
                                        </div>
                                    </div>
                                </form>
                            </>
                        )}
                    </div>
                </>
            )}
        </aside>
    );
}

// Shown in the Investigate panel when an admin has turned off AI features.
function AgentDisabledNotice({ isAdmin }: { isAdmin: boolean }) {
    return (
        <div className="flex-grow flex flex-col items-center justify-center px-6 py-10 text-center">
            <div className="w-10 h-10 rounded-lg bg-stone-100 dark:bg-stone-800 flex items-center justify-center text-stone-400 dark:text-stone-500 mb-3">
                <Sparkles className="w-5 h-5" />
            </div>
            <div className="font-medium text-stone-800 dark:text-stone-100">
                Investigate is disabled
            </div>
            <p className="mt-1.5 max-w-xs text-stone-500 dark:text-stone-400 leading-relaxed">
                AI agent functionality is turned off until an administrator enables an LLM provider.
            </p>
            {isAdmin && (
                <a
                    href="/admin/agent"
                    className="mt-3 text-orange-600 hover:text-orange-500 dark:text-orange-400 underline"
                >
                    Configure the LLM provider
                </a>
            )}
        </div>
    );
}

// The collapsed state — a thin clickable rail along the screen's right
// edge. Whole rail is the expand affordance.
function Rail({ onExpand }: { onExpand: () => void }) {
    return (
        <button
            type="button"
            onClick={onExpand}
            className="w-full h-full flex flex-col items-center gap-3 pt-3 hover:bg-stone-50 dark:hover:bg-stone-800/50 group"
            title="Open Investigate panel"
            aria-label="open investigate panel"
        >
            <div className="w-7 h-7 rounded-md bg-gradient-to-br from-orange-500 to-orange-700 flex items-center justify-center text-white flex-shrink-0">
                <Sparkles className="w-4 h-4" />
            </div>
            <ChevronLeft className="w-4 h-4 text-stone-400 group-hover:text-orange-500 flex-shrink-0" />
            <span
                className="font-medium tracking-wide text-stone-900 dark:text-stone-100 group-hover:text-stone-700 dark:group-hover:text-stone-300"
                style={{ writingMode: "vertical-rl" }}
            >
                Investigate
            </span>
        </button>
    );
}

// Drag handle on the panel's left edge. Sits flush with the border;
// highlights on hover so it's discoverable.
function ResizeHandle({ onMouseDown }: { onMouseDown: (e: React.MouseEvent) => void }) {
    return (
        <div
            onMouseDown={onMouseDown}
            className="w-1.5 flex-shrink-0 cursor-col-resize bg-transparent hover:bg-orange-400/50 active:bg-orange-500/60 transition-colors"
            title="Drag to resize"
        />
    );
}

function Welcome({ onPick }: { onPick: (prompt: string) => void }) {
    return (
        <div className="space-y-3">
            <div className="flex gap-2">
                <div className="w-7 h-7 rounded-lg bg-gradient-to-br from-orange-500 to-orange-700 flex items-center justify-center text-white flex-shrink-0">
                    <Sparkles className="w-3.5 h-3.5" />
                </div>
                <div className="text-stone-700 dark:text-stone-300 leading-relaxed pt-0.5">
                    I can search, aggregate, and chart logs with you. Try:
                </div>
            </div>
            <div className="ml-9 space-y-1">
                {STARTER_PROMPTS.map((p) => (
                    <button
                        key={p}
                        type="button"
                        onClick={() => onPick(p)}
                        className="block w-full text-left px-3 py-1.5 bg-stone-50 dark:bg-stone-800 border border-stone-200 dark:border-stone-700 hover:border-orange-300 dark:hover:border-orange-600 hover:bg-orange-50/30 dark:hover:bg-orange-950/20 rounded-md text-stone-700 dark:text-stone-300"
                    >
                        "{p}"
                    </button>
                ))}
            </div>
        </div>
    );
}

// Subtle line under a user message showing what view it was asked from.
function ViewContextChip({ ctx }: { ctx: ViewContext }) {
    const parts: string[] =
        ctx.route === "search"
            ? [
                  ctx.query && ctx.query !== "*" ? ctx.query : "*",
                  ctx.timeRange ?? "-6h",
                  ...(ctx.index ? [ctx.index] : []),
              ]
            : [`${ctx.route} page`];
    return (
        <div className="mt-1 flex flex-wrap items-center gap-1 text-stone-400 dark:text-stone-500">
            <Eye className="w-3 h-3 flex-shrink-0" />
            {parts.map((p, i) => (
                <span key={i} className="flex items-center gap-1">
                    {i > 0 && <span className="text-stone-300 dark:text-stone-600">·</span>}
                    <span className="font-mono">{p}</span>
                </span>
            ))}
        </div>
    );
}

const TOOL_LABEL_PREFIX: Record<string, string> = {
    query_logs: "Q",
    histogram: "H",
    aggregate: "A",
    list_sources: "L",
};

function TurnView({
    turn,
    onPick,
    busy,
}: {
    turn: AgentTurn;
    onPick: (text: string) => void;
    busy: boolean;
}) {
    if (turn.role === "user") {
        return (
            <div className="flex gap-2">
                <div className="w-7 h-7 rounded-full bg-stone-200 dark:bg-stone-700 flex items-center justify-center flex-shrink-0">
                    <User className="w-3.5 h-3.5 text-stone-600 dark:text-stone-300" />
                </div>
                <div className="flex-grow min-w-0">
                    <div className="text-stone-900 dark:text-stone-100 leading-relaxed pt-0.5 whitespace-pre-wrap break-words">
                        {turn.content}
                    </div>
                    {turn.viewContext && <ViewContextChip ctx={turn.viewContext} />}
                </div>
            </div>
        );
    }

    // Per-type counters for Q1/H1/A1 labelling within this turn.
    const counts: Record<string, number> = {};
    const hasReasoning = !!turn.reasoning && turn.reasoning.length > 0;
    // Split out tool calls with custom UI: suggest_followups → buttons,
    // create_monitor → confirmation card; the rest render as default artifacts.
    const dataToolCalls = (turn.toolCalls ?? []).filter(
        (c) => c.name !== "suggest_followups" && c.name !== "create_monitor",
    );
    const monitorDrafts = (turn.toolCalls ?? []).filter((c) => c.name === "create_monitor");
    const lastFollowup = [...(turn.toolCalls ?? [])]
        .reverse()
        .find((c) => c.name === "suggest_followups");
    const followupPrompts = extractFollowupPrompts(lastFollowup);
    const followupLabel = extractFollowupLabel(lastFollowup);
    return (
        <div className="flex gap-2">
            <div className="w-7 h-7 rounded-lg bg-gradient-to-br from-orange-500 to-orange-700 flex items-center justify-center text-white flex-shrink-0">
                <Sparkles className="w-3.5 h-3.5" />
            </div>
            <div className="flex-grow min-w-0 space-y-2">
                {dataToolCalls.map((c) => {
                    const prefix = TOOL_LABEL_PREFIX[c.name] ?? "T";
                    counts[prefix] = (counts[prefix] ?? 0) + 1;
                    const label = `${prefix}${counts[prefix]}`;
                    return <AgentToolArtifact key={c.id || label} call={c} label={label} />;
                })}
                {monitorDrafts.map((c, i) => (
                    <MonitorDraftCard key={c.id || `monitor-draft-${i}`} call={c} />
                ))}
                {/* Thinking pill sits BELOW tool calls so it stays visible at the
                    turn bottom as more tools land above it. */}
                {hasReasoning && (
                    <ThinkingPill
                        text={turn.reasoning!}
                        streaming={!!turn.streaming}
                        startedAt={turn.reasoningStartedAt}
                        durationMs={turn.reasoningDurationMs}
                    />
                )}
                {turn.content && (
                    <div>
                        <Markdown>{turn.content}</Markdown>
                        {turn.streaming && (
                            <span className="ml-0.5 inline-block w-1.5 h-3 bg-stone-400 dark:bg-stone-500 align-middle animate-pulse" />
                        )}
                    </div>
                )}
                {followupPrompts.length > 0 && (
                    <FollowupButtons
                        prompts={followupPrompts}
                        label={followupLabel}
                        onPick={onPick}
                        disabled={busy}
                    />
                )}
                {/* Turn footer: total wall-clock time, shown only once settled
                    (streaming would jitter and add nothing). */}
                {!turn.streaming && turn.durationMs !== undefined && (
                    <div className="text-right text-stone-400 dark:text-stone-500 tabular-nums">
                        Took {formatDuration(turn.durationMs)}
                    </div>
                )}
                {turn.streaming &&
                    !turn.content &&
                    !hasReasoning &&
                    (!turn.toolCalls || turn.toolCalls.length === 0) && (
                        <div className="text-stone-400 dark:text-stone-500 italic">
                            thinking
                            <AnimatedDots />
                        </div>
                    )}
            </div>
        </div>
    );
}

// Pull prompts from a settled suggest_followups call; prefer the validated
// result, fall back to raw arguments so they render before the result persists.
function extractFollowupPrompts(call: AgentToolCallUI | undefined): string[] {
    if (!call) return [];
    const fromResult =
        call.result && typeof call.result === "object"
            ? (call.result as { prompts?: unknown }).prompts
            : undefined;
    const candidate = Array.isArray(fromResult) ? fromResult : call.arguments?.prompts;
    if (!Array.isArray(candidate)) return [];
    return candidate
        .map((p) => (typeof p === "string" ? p.trim() : ""))
        .filter((p) => p.length > 0)
        .slice(0, 4);
}

function extractFollowupLabel(call: AgentToolCallUI | undefined): string | undefined {
    if (!call) return undefined;
    const fromResult =
        call.result && typeof call.result === "object"
            ? (call.result as { label?: unknown }).label
            : undefined;
    const fromArgs = call.arguments?.label;
    const v = typeof fromResult === "string" ? fromResult : fromArgs;
    return typeof v === "string" && v.trim().length > 0 ? v.trim() : undefined;
}

// Clickable suggest_followups buttons; disabled while busy so a click can't
// race a still-streaming turn.
function FollowupButtons({
    prompts,
    label,
    onPick,
    disabled,
}: {
    prompts: string[];
    label?: string;
    onPick: (text: string) => void;
    disabled: boolean;
}) {
    return (
        <div className="space-y-1 pt-1">
            <div className="text-stone-500 dark:text-stone-400">{label ?? "Try next"}</div>
            {prompts.map((p) => (
                <button
                    key={p}
                    type="button"
                    onClick={() => onPick(p)}
                    disabled={disabled}
                    className="block w-full text-left px-3 py-1.5 bg-stone-50 dark:bg-stone-800 border border-stone-200 dark:border-stone-700 hover:border-orange-300 dark:hover:border-orange-600 hover:bg-orange-50/30 dark:hover:bg-orange-950/20 rounded-md text-stone-700 dark:text-stone-300 disabled:opacity-50 disabled:cursor-not-allowed"
                >
                    {p}
                </button>
            ))}
        </div>
    );
}

// Reasoning-trace pill, persisted in the transcript. Default collapsed; the
// last user toggle becomes the sticky initial state for new pills (per-pill toggle still local).
function ThinkingPill({
    text,
    streaming,
    startedAt,
    durationMs,
}: {
    text: string;
    streaming: boolean;
    startedAt?: number;
    durationMs?: number;
}) {
    const [open, setOpen] = useState(() => getThinkingPref());
    const toggle = () => {
        const next = !open;
        setOpen(next);
        setThinkingPref(next);
    };

    return (
        // Borderless tinted background (unlike bordered tool pills) marks this as
        // meta-commentary rather than a concrete action.
        <div className="rounded-lg bg-stone-100/70 dark:bg-stone-800/40 overflow-hidden">
            <button
                type="button"
                onClick={toggle}
                className="w-full px-3 py-2 flex items-center gap-2 hover:bg-stone-100 dark:hover:bg-stone-800/60 text-left"
            >
                {open ? (
                    <ChevronDown className="w-3 h-3 text-stone-400 flex-shrink-0" />
                ) : (
                    <ChevronRight className="w-3 h-3 text-stone-400 flex-shrink-0" />
                )}
                <Brain className="w-3.5 h-3.5 text-stone-400 dark:text-stone-500 flex-shrink-0" />
                <span className="italic text-stone-600 dark:text-stone-300">Thinking</span>
                {streaming ? (
                    <span className="text-stone-400 dark:text-stone-500 flex items-center gap-1">
                        <AnimatedDots />
                        {startedAt !== undefined && (
                            <span>
                                · <LiveDuration startedAt={startedAt} />
                            </span>
                        )}
                    </span>
                ) : (
                    <span className="text-stone-400 dark:text-stone-500 truncate">
                        · {summarizeReasoning(text)}
                        {durationMs !== undefined && (
                            <span className="tabular-nums"> · {formatDuration(durationMs)}</span>
                        )}
                    </span>
                )}
            </button>
            {open && (
                <div className="border-t border-stone-100 dark:border-stone-800 bg-stone-50/50 dark:bg-stone-950/40 px-3 py-2">
                    <pre className="font-mono whitespace-pre-wrap break-words text-stone-700 dark:text-stone-300 max-h-96 overflow-auto">
                        {text}
                    </pre>
                </div>
            )}
        </div>
    );
}

// Persisted ThinkingPill initial open/closed state, updated on each toggle.
const THINKING_PREF_KEY = "helios-thinking-expanded";
function getThinkingPref(): boolean {
    try {
        return localStorage.getItem(THINKING_PREF_KEY) === "1";
    } catch {
        return false;
    }
}
function setThinkingPref(open: boolean): void {
    try {
        localStorage.setItem(THINKING_PREF_KEY, open ? "1" : "0");
    } catch {
        // storage disabled / quota — toggling still works for this session.
    }
}

// Collapsed-pill summary: just the char count, so closed state doesn't leak content.
function summarizeReasoning(text: string): string {
    const chars = text.length;
    if (chars < 1000) return `${chars} chars`;
    return `${(chars / 1000).toFixed(1)}k chars`;
}

// Cycles ".", "..", "..." on a fixed-width slot so the label doesn't jitter.
function AnimatedDots() {
    const [n, setN] = useState(1);
    useEffect(() => {
        const id = setInterval(() => setN((x) => (x % 3) + 1), 400);
        return () => clearInterval(id);
    }, []);
    return <span className="inline-block w-4 text-left align-baseline">{".".repeat(n)}</span>;
}

// In-drawer conversation picker shown when the history toggle is open.
function ConversationList({
    conversations,
    activeId,
    onPick,
    onNew,
    onRename,
    onDelete,
}: {
    conversations: ConversationMeta[];
    activeId: string | null;
    onPick: (id: string) => void | Promise<void>;
    onNew: () => void | Promise<void>;
    onRename: (id: string, title: string) => Promise<void>;
    onDelete: (id: string) => Promise<void>;
}) {
    const [editingId, setEditingId] = useState<string | null>(null);
    const [editValue, setEditValue] = useState("");

    return (
        <div className="flex-grow overflow-auto px-3 py-3 space-y-1">
            <button
                type="button"
                onClick={() => void onNew()}
                className="w-full flex items-center gap-2 px-2.5 py-2 rounded-md text-stone-700 dark:text-stone-200 hover:bg-stone-100 dark:hover:bg-stone-800 border border-dashed border-stone-200 dark:border-stone-700"
            >
                <Plus className="w-3.5 h-3.5" />
                New conversation
            </button>
            {conversations.length === 0 && (
                <div className="px-2.5 py-4 text-stone-400 dark:text-stone-500">
                    No previous conversations yet.
                </div>
            )}
            {conversations.map((c) => {
                const active = c.id === activeId;
                const isEditing = editingId === c.id;
                return (
                    <div
                        key={c.id}
                        className={`group flex items-center gap-1.5 px-2.5 py-1.5 rounded-md ${
                            active
                                ? "bg-orange-50 dark:bg-orange-950/30 text-stone-900 dark:text-stone-100"
                                : "hover:bg-stone-100 dark:hover:bg-stone-800 text-stone-700 dark:text-stone-300"
                        }`}
                    >
                        {isEditing ? (
                            <>
                                <input
                                    autoFocus
                                    value={editValue}
                                    onChange={(e) => setEditValue(e.target.value)}
                                    onKeyDown={(e) => {
                                        if (e.key === "Enter") {
                                            void onRename(c.id, editValue.trim() || c.title).then(
                                                () => setEditingId(null),
                                            );
                                        } else if (e.key === "Escape") {
                                            setEditingId(null);
                                        }
                                    }}
                                    className="flex-grow min-w-0 px-1 py-0.5 bg-white dark:bg-stone-900 border border-stone-200 dark:border-stone-700 rounded text-stone-900 dark:text-stone-100 focus:outline-none focus:border-orange-500"
                                />
                                <button
                                    type="button"
                                    onClick={() =>
                                        void onRename(c.id, editValue.trim() || c.title).then(() =>
                                            setEditingId(null),
                                        )
                                    }
                                    className="p-1 text-stone-500 hover:text-stone-700 dark:hover:text-stone-200"
                                    title="Save"
                                >
                                    <Check className="w-3.5 h-3.5" />
                                </button>
                                <button
                                    type="button"
                                    onClick={() => setEditingId(null)}
                                    className="p-1 text-stone-500 hover:text-stone-700 dark:hover:text-stone-200"
                                    title="Cancel"
                                >
                                    <X className="w-3.5 h-3.5" />
                                </button>
                            </>
                        ) : (
                            <>
                                <button
                                    type="button"
                                    onClick={() => void onPick(c.id)}
                                    className="flex-grow min-w-0 text-left truncate"
                                    title={c.title || "(untitled)"}
                                >
                                    {c.title || (
                                        <span className="italic text-stone-400 dark:text-stone-500">
                                            (untitled)
                                        </span>
                                    )}
                                </button>
                                <button
                                    type="button"
                                    onClick={() => {
                                        setEditValue(c.title);
                                        setEditingId(c.id);
                                    }}
                                    className="p-1 text-stone-400 hover:text-stone-700 dark:hover:text-stone-200 opacity-0 group-hover:opacity-100 transition-opacity"
                                    title="Rename"
                                >
                                    <Pencil className="w-3.5 h-3.5" />
                                </button>
                                <button
                                    type="button"
                                    onClick={() => {
                                        if (
                                            window.confirm(
                                                `Delete conversation "${c.title || "(untitled)"}"?`,
                                            )
                                        ) {
                                            void onDelete(c.id);
                                        }
                                    }}
                                    className="p-1 text-stone-400 hover:text-red-600 dark:hover:text-red-400 opacity-0 group-hover:opacity-100 transition-opacity"
                                    title="Delete"
                                >
                                    <Trash2 className="w-3.5 h-3.5" />
                                </button>
                            </>
                        )}
                    </div>
                );
            })}
        </div>
    );
}
