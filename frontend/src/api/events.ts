// Copyright 2026 Appbird LLC. Licensed under "GNU Affero General Public License v3.0"

// Cross-component sync — components subscribed via `onSavedChanged` re-fetch
// after any mutation. Lets the popover, page, and active-saved indicator
// stay consistent without prop-drilling a refetch callback.

const SAVED_CHANGED_EVENT = "helios-saved-changed";

export function notifySavedChanged(): void {
    window.dispatchEvent(new Event(SAVED_CHANGED_EVENT));
}

export function onSavedChanged(handler: () => void): () => void {
    window.addEventListener(SAVED_CHANGED_EVENT, handler);
    return () => window.removeEventListener(SAVED_CHANGED_EVENT, handler);
}

const MONITORS_CHANGED_EVENT = "helios-monitors-changed";

export function notifyMonitorsChanged(): void {
    window.dispatchEvent(new Event(MONITORS_CHANGED_EVENT));
}

export function onMonitorsChanged(handler: () => void): () => void {
    window.addEventListener(MONITORS_CHANGED_EVENT, handler);
    return () => window.removeEventListener(MONITORS_CHANGED_EVENT, handler);
}

const ALERTS_CHANGED_EVENT = "helios-alerts-changed";

export function notifyAlertsChanged(): void {
    window.dispatchEvent(new Event(ALERTS_CHANGED_EVENT));
}

export function onAlertsChanged(handler: () => void): () => void {
    window.addEventListener(ALERTS_CHANGED_EVENT, handler);
    return () => window.removeEventListener(ALERTS_CHANGED_EVENT, handler);
}

const SOURCES_CHANGED_EVENT = "helios-sources-changed";

export function notifySourcesChanged(): void {
    window.dispatchEvent(new Event(SOURCES_CHANGED_EVENT));
}

export function onSourcesChanged(handler: () => void): () => void {
    window.addEventListener(SOURCES_CHANGED_EVENT, handler);
    return () => window.removeEventListener(SOURCES_CHANGED_EVENT, handler);
}

// Request to open a specific agent conversation in the Investigate drawer.
const OPEN_CONVERSATION_EVENT = "helios-open-conversation";

export function notifyOpenConversation(id: string): void {
    window.dispatchEvent(new CustomEvent(OPEN_CONVERSATION_EVENT, { detail: id }));
}

export function onOpenConversation(handler: (id: string) => void): () => void {
    const wrapped = (e: Event) => {
        const ce = e as CustomEvent<string>;
        if (typeof ce.detail === "string") handler(ce.detail);
    };
    window.addEventListener(OPEN_CONVERSATION_EVENT, wrapped);
    return () => window.removeEventListener(OPEN_CONVERSATION_EVENT, wrapped);
}

// Request to investigate a single log entry: opens a fresh conversation
// seeded with `prompt`. Fired by the per-row Investigate action.
const INVESTIGATE_LOG_EVENT = "helios-investigate-log";

export function notifyInvestigateLog(prompt: string): void {
    window.dispatchEvent(new CustomEvent(INVESTIGATE_LOG_EVENT, { detail: prompt }));
}

export function onInvestigateLog(handler: (prompt: string) => void): () => void {
    const wrapped = (e: Event) => {
        const ce = e as CustomEvent<string>;
        if (typeof ce.detail === "string") handler(ce.detail);
    };
    window.addEventListener(INVESTIGATE_LOG_EVENT, wrapped);
    return () => window.removeEventListener(INVESTIGATE_LOG_EVENT, wrapped);
}

// Request to run an AI monitor and watch its execution live in the drawer.
const RUN_MONITOR_LIVE_EVENT = "helios-run-monitor-live";

export interface RunMonitorLiveDetail {
    monitorId: string;
    name: string;
}

export function notifyRunMonitorLive(detail: RunMonitorLiveDetail): void {
    window.dispatchEvent(new CustomEvent(RUN_MONITOR_LIVE_EVENT, { detail }));
}

export function onRunMonitorLive(handler: (detail: RunMonitorLiveDetail) => void): () => void {
    const wrapped = (e: Event) => {
        const ce = e as CustomEvent<RunMonitorLiveDetail>;
        if (ce.detail && typeof ce.detail.monitorId === "string") handler(ce.detail);
    };
    window.addEventListener(RUN_MONITOR_LIVE_EVENT, wrapped);
    return () => window.removeEventListener(RUN_MONITOR_LIVE_EVENT, wrapped);
}
