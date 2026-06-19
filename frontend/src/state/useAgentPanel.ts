// Copyright 2026 Appbird LLC. Licensed under "GNU Affero General Public License v3.0"

import { useCallback, useEffect, useState } from "react";

// Width of the collapsed vertical rail.
export const RAIL_WIDTH = 44;
// Minimum width of the expanded panel.
export const MIN_WIDTH = 340;

const COLLAPSED_KEY = "helios-agent-collapsed";
const WIDTH_KEY = "helios-agent-width";
const DEFAULT_WIDTH = 420;

function readBool(key: string, fallback: boolean): boolean {
    try {
        const v = localStorage.getItem(key);
        if (v === "1") return true;
        if (v === "0") return false;
    } catch {
        // ignore
    }
    return fallback;
}

function readNum(key: string, fallback: number): number {
    try {
        const v = parseInt(localStorage.getItem(key) ?? "", 10);
        if (Number.isFinite(v)) return v;
    } catch {
        // ignore
    }
    return fallback;
}

function persist(key: string, value: string) {
    try {
        localStorage.setItem(key, value);
    } catch {
        // ignore
    }
}

// Clamp a desired panel width to [MIN_WIDTH, viewport - 200] so the page
// behind the panel always keeps a usable strip.
function clampWidth(w: number): number {
    const max = Math.max(MIN_WIDTH, window.innerWidth - 200);
    return Math.max(MIN_WIDTH, Math.min(w, max));
}

export interface AgentPanelState {
    collapsed: boolean;
    maximized: boolean;
    // On-screen width: rail when collapsed, viewport when maximized, else chosen width.
    effectiveWidth: number;
    // True during a drag-resize — callers disable width transitions to track the cursor 1:1.
    resizing: boolean;
    expand: () => void;
    collapse: () => void;
    toggleMaximize: () => void;
    beginResize: (e: React.MouseEvent) => void;
}

// Owns the Investigate panel's layout state. Width + collapsed persist to
// localStorage; maximized is transient (resets on reload).
export function useAgentPanel(): AgentPanelState {
    const [collapsed, setCollapsed] = useState(() => readBool(COLLAPSED_KEY, false));
    const [maximized, setMaximized] = useState(false);
    const [width, setWidth] = useState(() => clampWidth(readNum(WIDTH_KEY, DEFAULT_WIDTH)));
    const [resizing, setResizing] = useState(false);
    const [winWidth, setWinWidth] = useState(() => window.innerWidth);

    useEffect(() => {
        const onResize = () => setWinWidth(window.innerWidth);
        window.addEventListener("resize", onResize);
        return () => window.removeEventListener("resize", onResize);
    }, []);

    const expand = useCallback(() => {
        setCollapsed(false);
        persist(COLLAPSED_KEY, "0");
    }, []);

    const collapse = useCallback(() => {
        setCollapsed(true);
        setMaximized(false);
        persist(COLLAPSED_KEY, "1");
    }, []);

    const toggleMaximize = useCallback(() => {
        setCollapsed(false);
        persist(COLLAPSED_KEY, "0");
        setMaximized((m) => !m);
    }, []);

    const beginResize = useCallback((e: React.MouseEvent) => {
        e.preventDefault();
        setResizing(true);
        document.body.style.userSelect = "none";
        document.body.style.cursor = "col-resize";
        let latest = clampWidth(window.innerWidth - e.clientX);

        const onMove = (ev: MouseEvent) => {
            latest = clampWidth(window.innerWidth - ev.clientX);
            setWidth(latest);
        };
        const onUp = () => {
            document.removeEventListener("mousemove", onMove);
            document.removeEventListener("mouseup", onUp);
            document.body.style.userSelect = "";
            document.body.style.cursor = "";
            setResizing(false);
            persist(WIDTH_KEY, String(latest));
        };
        document.addEventListener("mousemove", onMove);
        document.addEventListener("mouseup", onUp);
    }, []);

    const effectiveWidth = collapsed ? RAIL_WIDTH : maximized ? winWidth : width;

    return {
        collapsed,
        maximized,
        effectiveWidth,
        resizing,
        expand,
        collapse,
        toggleMaximize,
        beginResize,
    };
}
