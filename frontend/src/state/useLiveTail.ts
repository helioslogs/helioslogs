// Copyright 2026 Appbird LLC. Licensed under "GNU Affero General Public License v3.0"

import { useCallback, useEffect, useRef, useState } from "react";
import { search } from "../api/client";
import type { Hit } from "../api/types";

const POLL_MS = 1500;
const POLL_LIMIT = 500;
const BUFFER_CAP = 2000;
const BACKFILL_WINDOW = "-5m";

export interface LiveTail {
    rows: Hit[];
    paused: boolean;
    // Rows accumulated while paused, appended on resume.
    bufferedCount: number;
    error: string | null;
    pause: () => void;
    resume: () => void;
}

function rowKey(h: Hit): string {
    return `${h.timestamp ?? ""}|${h.raw ?? h.message ?? ""}`;
}

// Polling tail with an inclusive ms cursor: each poll re-reads the cursor
// millisecond (same-ms stragglers aren't missed) and dedupes that boundary
// by (timestamp, raw) key. Ring-buffered at BUFFER_CAP rows.
export function useLiveTail(active: boolean, q: string, index?: string): LiveTail {
    const [rows, setRows] = useState<Hit[]>([]);
    const [paused, setPaused] = useState(false);
    const [bufferedCount, setBufferedCount] = useState(0);
    const [error, setError] = useState<string | null>(null);

    const cursorRef = useRef<number | null>(null);
    const boundaryKeysRef = useRef<Set<string>>(new Set());
    const pausedBufferRef = useRef<Hit[]>([]);
    const pausedRef = useRef(false);
    pausedRef.current = paused;

    useEffect(() => {
        if (!active) return;
        let cancelled = false;
        const ctrl = new AbortController();
        cursorRef.current = null;
        boundaryKeysRef.current = new Set();
        pausedBufferRef.current = [];
        setRows([]);
        setBufferedCount(0);
        setPaused(false);
        setError(null);

        const advance = (incoming: Hit[]) => {
            // Response is time-DESC; the tail renders ascending.
            const asc = [...incoming].reverse();
            const fresh = asc.filter((h) => !boundaryKeysRef.current.has(rowKey(h)));
            if (fresh.length === 0) return;
            const prevCursor = cursorRef.current ?? 0;
            const maxTs = fresh.reduce((m, h) => {
                const t = h.timestamp ? Date.parse(h.timestamp) : NaN;
                return Number.isFinite(t) && t > m ? t : m;
            }, prevCursor);
            cursorRef.current = maxTs;
            // Dedupe keys cover only the boundary millisecond: carry the old set
            // when the cursor didn't move, start fresh when it advanced.
            const boundary = new Set<string>(maxTs === prevCursor ? boundaryKeysRef.current : []);
            for (const h of fresh) {
                if (h.timestamp && Date.parse(h.timestamp) === maxTs) boundary.add(rowKey(h));
            }
            boundaryKeysRef.current = boundary;
            if (pausedRef.current) {
                pausedBufferRef.current = [...pausedBufferRef.current, ...fresh].slice(-BUFFER_CAP);
                setBufferedCount(pausedBufferRef.current.length);
            } else {
                setRows((prev) => [...prev, ...fresh].slice(-BUFFER_CAP));
            }
        };

        const poll = async () => {
            const start =
                cursorRef.current === null
                    ? BACKFILL_WINDOW
                    : new Date(cursorRef.current).toISOString();
            try {
                const r = await search(
                    { q, index, start, end: "now", offset: 0, limit: POLL_LIMIT },
                    { signal: ctrl.signal },
                );
                if (cancelled) return;
                setError(null);
                advance(r.hits);
            } catch (e) {
                if (cancelled || ctrl.signal.aborted) return;
                setError(e instanceof Error ? e.message : String(e));
            }
        };

        void poll();
        const handle = setInterval(() => void poll(), POLL_MS);
        return () => {
            cancelled = true;
            ctrl.abort();
            clearInterval(handle);
        };
        // eslint-disable-next-line react-hooks/exhaustive-deps
    }, [active, q, index]);

    const pause = useCallback(() => setPaused(true), []);
    const resume = useCallback(() => {
        const buffered = pausedBufferRef.current;
        pausedBufferRef.current = [];
        setBufferedCount(0);
        if (buffered.length > 0) {
            setRows((prev) => [...prev, ...buffered].slice(-BUFFER_CAP));
        }
        setPaused(false);
    }, []);

    return { rows, paused, bufferedCount, error, pause, resume };
}
