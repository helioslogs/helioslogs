// Copyright 2026 Appbird LLC. Licensed under "GNU Affero General Public License v3.0"

import { useEffect, useRef } from "react";

// Periodic auto-refresh; `secs <= 0`/`paused` disables it. Pauses while the tab is
// hidden and fires once on return if an interval elapsed while away.
export function useAutoRefresh(secs: number, onRefresh: () => void, paused = false): void {
    const cb = useRef(onRefresh);
    cb.current = onRefresh;

    useEffect(() => {
        if (secs <= 0 || paused) return;
        const ms = secs * 1000;
        let last = Date.now();
        const tick = () => {
            last = Date.now();
            cb.current();
        };
        const timer = window.setInterval(() => {
            if (!document.hidden) tick();
        }, ms);
        const onVisible = () => {
            if (!document.hidden && Date.now() - last >= ms) tick();
        };
        document.addEventListener("visibilitychange", onVisible);
        return () => {
            window.clearInterval(timer);
            document.removeEventListener("visibilitychange", onVisible);
        };
    }, [secs, paused]);
}
