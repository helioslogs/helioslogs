// Copyright 2026 Appbird LLC. Licensed under "GNU Affero General Public License v3.0"

// Env-aware alert actions. Acting on a cross-env alert switches the active env
// (a hard reload, like EnvPicker) and replays the action after reboot.

import { getEnv, setEnv } from "../api/client";
import { notifyInvestigateLog, notifyOpenConversation } from "../api/events";

const KEY = "helios.pending-alert-action";

type Pending =
    | { env: string; kind: "investigate"; prompt: string }
    | { env: string; kind: "conversation"; id: string };

function stash(p: Pending): void {
    try {
        localStorage.setItem(KEY, JSON.stringify(p));
    } catch {
        /* ignore */
    }
}

function take(): Pending | null {
    const s = localStorage.getItem(KEY);
    if (!s) return null;
    localStorage.removeItem(KEY);
    try {
        return JSON.parse(s) as Pending;
    } catch {
        return null;
    }
}

// Persist the new env + sync `?env=` in the address bar, then hard-reload so
// every hook re-fetches against it — mirrors EnvPicker.handlePick.
function switchEnvAndReload(env: string): void {
    setEnv(env);
    const url = new URL(window.location.href);
    url.searchParams.set("env", env);
    window.history.replaceState(window.history.state, "", url);
    window.location.reload();
}

// Seed a fresh investigation in `env`. Same env → fire now; different env →
// stash + switch env (reload), then replay on startup.
export function investigateInEnv(env: string, prompt: string): void {
    if (env && env !== getEnv()) {
        stash({ env, kind: "investigate", prompt });
        switchEnvAndReload(env);
    } else {
        notifyInvestigateLog(prompt);
    }
}

// Open a monitor's trace conversation in `env` (so follow-ups run there).
export function openConversationInEnv(env: string, id: string): void {
    if (env && env !== getEnv()) {
        stash({ env, kind: "conversation", id });
        switchEnvAndReload(env);
    } else {
        notifyOpenConversation(id);
    }
}

// Replay a stashed action after an env-switch reload. Call once on app start,
// after the agent drawer's event listeners are registered.
export function replayPendingAlertAction(): void {
    const p = take();
    if (!p) return;
    if (p.kind === "investigate") notifyInvestigateLog(p.prompt);
    else notifyOpenConversation(p.id);
}
