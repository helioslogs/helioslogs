// Copyright 2026 Appbird LLC. Licensed under "GNU Affero General Public License v3.0"

import { useCallback, useEffect, useRef, useState } from "react";
import { useLocation, useNavigate } from "react-router-dom";
import {
    aggregate,
    discoverFields,
    getIndexes,
    histogram,
    listSearchPartitions,
    partitionToParam,
    search,
    searchHistogram,
} from "../api/client";
import type {
    AggregateResponse,
    DiscoveredField,
    Hit,
    HistogramBucket,
    HistogramResponse,
    SearchPartition,
    SearchResponse,
} from "../api/types";
import { readUrl, sameInput, searchHref, type SearchInput } from "./url";

// Live progress through the day-by-day streaming scan, ticked once per completed
// day; `null` while no scan is in flight. Surfaced as "scanning N of M days…".
export interface ScanProgress {
    totalDays: number;
    doneDays: number;
}

// Rough pipe detection: streaming gives no benefit for pipe queries (they need every
// doc) so they take the single-shot path. Conservative — a false positive is just slower.
function looksLikePipe(q: string): boolean {
    // Strip quoted strings so a literal "|" in a phrase doesn't trip the check.
    const stripped = q.replace(/"[^"]*"/g, "");
    return stripped.includes("|");
}

const PAGE_SIZE = 100;
// `index` is always pinned (catalog-layer partition breakdown); not user-removable.
const ALWAYS_PINNED = "index";

const PINNED_KEY = "helios-pinned-fields";
const SEEDED_KEY = "helios-pinned-seeded";
// How many auto-seeded categorical fields to pin on a first-ever visit.
const AUTOSEED_LIMIT = 3;

function readPinnedPref(): string[] | null {
    try {
        const v = localStorage.getItem(PINNED_KEY);
        return v ? (JSON.parse(v) as string[]) : null;
    } catch {
        return null;
    }
}

function writePinnedPref(list: string[]) {
    try {
        localStorage.setItem(PINNED_KEY, JSON.stringify(list));
        localStorage.setItem(SEEDED_KEY, "1");
    } catch {
        /* private mode / quota — pins just won't persist */
    }
}

// Cache key for FieldPanel remounting. When any input changes, panels reset and
// re-fetch on next expand with the new query context.
export function queryCacheKey(input: SearchInput): string {
    return [
        input.q,
        input.index ?? "",
        input.start ?? "",
        input.end ?? "",
        input.range,
        input.follow ? "1" : "0",
    ].join("|");
}

interface UseSearchQuery {
    input: SearchInput;
    setInput: (next: SearchInput) => void;
    searchResp: SearchResponse | null;
    histResp: HistogramResponse | null;
    loading: boolean;
    error: string | null;
    indexes: string[];
    pageSize: number;
    // Pinned facets — `index` first, then user picks; expanded with value breakdowns.
    pinnedFields: string[];
    // The rest of the groupable catalog; collapsed, breakdown loaded on click.
    availableFields: string[];
    // Pin/unpin a field (persists to localStorage). No-op for `index`.
    togglePin: (field: string) => void;
    // Per-field metadata for instant-preview rendering before the terms-agg lands.
    discovered: DiscoveredField[];
    // Lazy per-field agg fetcher. Reads the *current* input at call time (not render),
    // and always opts into `approximate=true` (backend no-ops it for narrow windows).
    fetchFieldBuckets: (field: string) => Promise<AggregateResponse>;
    // Changes whenever the query context changes; keys field panels for remount.
    cacheKey: string;
    // Re-run the current query without changing input or URL (refresh button).
    refresh: () => void;
    // Day-by-day scan progress while streaming; cleared when it settles or falls back.
    scanProgress: ScanProgress | null;
    // Cancels the in-flight query (search/histogram + trailing discover). Idempotent.
    cancel: () => void;
}

// The search view's orchestration hook: URL-seeded input, the parallel fetches,
// request-id deduping, polling, and popstate. `setInput` is the single mutation entry.
export function useSearchQuery(): UseSearchQuery {
    const [input, setInputState] = useState<SearchInput>(() => readUrl());
    const [searchResp, setSearchResp] = useState<SearchResponse | null>(null);
    const [histResp, setHistResp] = useState<HistogramResponse | null>(null);
    const [discovered, setDiscovered] = useState<DiscoveredField[]>([]);
    const [loading, setLoading] = useState(false);
    const [error, setError] = useState<string | null>(null);
    const [indexes, setIndexes] = useState<string[]>([]);
    const [scanProgress, setScanProgress] = useState<ScanProgress | null>(null);

    // User-pinned facet names (`index` is pinned separately); null pref ⇒ auto-seed once.
    const [pinned, setPinned] = useState<string[]>(() => readPinnedPref() ?? []);
    const seededRef = useRef(readPinnedPref() !== null);

    const togglePin = useCallback((field: string) => {
        if (field === ALWAYS_PINNED) return;
        seededRef.current = true;
        setPinned((prev) => {
            const next = prev.includes(field) ? prev.filter((f) => f !== field) : [...prev, field];
            writePinnedPref(next);
            return next;
        });
    }, []);

    // Single AbortController per in-flight runQuery. New runs abort the
    // previous, and the public `cancel` hook aborts the current one.
    const abortRef = useRef<AbortController | null>(null);

    const reqIdRef = useRef(0);

    // Latest-input ref keeps `fetchFieldBuckets` stable while reading fresh context at call time.
    const inputRef = useRef(input);
    inputRef.current = input;

    const runQuery = useCallback(async (s: SearchInput) => {
        const id = ++reqIdRef.current;

        // Abort any in-flight run first — saves cores and prevents stale results racing.
        abortRef.current?.abort();
        const ctrl = new AbortController();
        abortRef.current = ctrl;
        const signal = ctrl.signal;

        setLoading(true);
        setError(null);
        setScanProgress(null);

        // Absolute start/end win when set (e.g. from a histogram-bucket click).
        // Otherwise: follow → rolling 5m window, else the range dropdown.
        const start = s.start ?? (s.follow ? "-5m" : s.range);
        const end = s.end ?? "now";

        const page = Math.max(1, s.page ?? 1);
        const offset = (page - 1) * PAGE_SIZE;

        // Streaming applies to page 1 of a non-pipe query. Pipe queries and page > 1
        // need a full cross-partition pass, so they fall back to the single-shot path.
        const useStreaming = page === 1 && !looksLikePipe(s.q);

        // ---- Phase 1: primary content (search + histogram) ----
        try {
            if (!useStreaming) {
                if (looksLikePipe(s.q)) {
                    // Pipe queries produce a table (not a hits+histogram pair) and the
                    // combined endpoint rejects them — keep the separate calls.
                    const [searchResult, histResult] = await Promise.all([
                        search(
                            { q: s.q, index: s.index, start, end, offset, limit: PAGE_SIZE },
                            { signal },
                        ),
                        histogram({ q: s.q, index: s.index, start, end }, { signal }),
                    ]);
                    if (id !== reqIdRef.current || signal.aborted) return;
                    setSearchResp(searchResult);
                    setHistResp(histResult);
                } else {
                    // Pagination (page > 1): one combined request for both halves.
                    const r = await searchHistogram(
                        { q: s.q, index: s.index, start, end, offset, limit: PAGE_SIZE },
                        { signal },
                    );
                    if (id !== reqIdRef.current || signal.aborted) return;
                    setSearchResp({
                        total: r.total,
                        took_us: r.took_us,
                        hits: r.hits,
                        highlight_terms: r.highlight_terms,
                        partitions_scanned: r.partitions_scanned,
                        offset: r.offset,
                        limit: r.limit,
                    });
                    setHistResp({
                        interval_ms: r.interval_ms,
                        took_us: r.took_us,
                        buckets: r.buckets,
                    });
                }
            } else {
                // Plan: list partitions in scan order (newest day first).
                const plan = await listSearchPartitions(
                    { q: s.q, index: s.index, start, end },
                    { signal },
                );
                if (id !== reqIdRef.current || signal.aborted) return;

                // Group by day (preserving the backend's most-recent-first order); each
                // group is one round-trip, scanned in parallel server-side.
                const dayGroups: SearchPartition[][] = [];
                let currentDay = "";
                for (const p of plan.partitions) {
                    if (p.day !== currentDay) {
                        dayGroups.push([]);
                        currentDay = p.day;
                    }
                    dayGroups[dayGroups.length - 1].push(p);
                }

                // Empty plan — no partitions match the query+range. Emit empty
                // responses so the UI clears any stale results from a prior query.
                if (dayGroups.length === 0) {
                    setSearchResp({
                        total: 0,
                        took_us: 0,
                        hits: [],
                        highlight_terms: [],
                        partitions_scanned: 0,
                        offset: 0,
                        limit: PAGE_SIZE,
                    });
                    setHistResp({ interval_ms: 0, took_us: 0, buckets: [] });
                    setScanProgress(null);
                } else {
                    setScanProgress({ totalDays: dayGroups.length, doneDays: 0 });

                    // Accumulators. Hits arrive newest-day-first and time-desc within a day,
                    // so concat preserves order; capped at PAGE_SIZE while `total` keeps the sum.
                    let totalHits = 0;
                    let allHits: Hit[] = [];
                    const bucketMap = new Map<string, number>();
                    let totalTookUs = 0;
                    let highlightTerms: string[] = [];
                    let intervalMs = 0;
                    let partitionsScanned = 0;

                    for (let i = 0; i < dayGroups.length; i++) {
                        if (id !== reqIdRef.current || signal.aborted) return;
                        const partitionsParam = dayGroups[i].map(partitionToParam).join(",");
                        // One combined call per day: the server evaluates the filter once
                        // for both the hits and the histogram instead of twice.
                        const r = await searchHistogram(
                            {
                                q: s.q,
                                index: s.index,
                                start,
                                end,
                                offset: 0,
                                limit: PAGE_SIZE,
                                partitions: partitionsParam,
                            },
                            { signal },
                        );
                        if (id !== reqIdRef.current || signal.aborted) return;

                        totalHits += r.total;
                        if (allHits.length < PAGE_SIZE) {
                            allHits = allHits.concat(r.hits).slice(0, PAGE_SIZE);
                        }
                        if (highlightTerms.length === 0) highlightTerms = r.highlight_terms;
                        partitionsScanned += r.partitions_scanned;
                        totalTookUs += r.took_us;

                        if (intervalMs === 0) intervalMs = r.interval_ms;
                        for (const b of r.buckets) {
                            bucketMap.set(b.t, (bucketMap.get(b.t) ?? 0) + b.count);
                        }
                        // Buckets re-sorted by timestamp ascending each tick — the
                        // chart component expects that ordering.
                        const allBuckets: HistogramBucket[] = Array.from(bucketMap.entries())
                            .sort(([a], [b]) => a.localeCompare(b))
                            .map(([t, count]) => ({ t, count }));

                        setSearchResp({
                            total: totalHits,
                            took_us: totalTookUs,
                            hits: allHits,
                            highlight_terms: highlightTerms,
                            partitions_scanned: partitionsScanned,
                            offset: 0,
                            limit: PAGE_SIZE,
                        });
                        setHistResp({
                            interval_ms: intervalMs,
                            took_us: totalTookUs,
                            buckets: allBuckets,
                        });
                        setScanProgress({ totalDays: dayGroups.length, doneDays: i + 1 });
                    }
                }
            }
        } catch (e: unknown) {
            // Aborts are expected (new query / explicit cancel); suppress them.
            if (signal.aborted || (e instanceof DOMException && e.name === "AbortError")) {
                return;
            }
            if (id !== reqIdRef.current) return;
            setError(e instanceof Error ? e.message : String(e));
            return;
        } finally {
            if (id === reqIdRef.current && !signal.aborted) {
                setLoading(false);
                setScanProgress(null);
            }
        }

        // ---- Phase 2: discover_fields ----
        try {
            const discResult = await discoverFields(
                {
                    q: s.q,
                    index: s.index,
                    start,
                    end,
                    top: 60,
                },
                { signal },
            );
            if (id !== reqIdRef.current || signal.aborted) return;
            setDiscovered(discResult.fields);
        } catch (e: unknown) {
            if (signal.aborted || (e instanceof DOMException && e.name === "AbortError")) {
                return;
            }
            if (id !== reqIdRef.current) return;
            // Phase 2 errors don't undo phase 1 results; surface in the error
            // banner so it's not silently swallowed.
            setError(e instanceof Error ? e.message : String(e));
        }
    }, []);

    const cancel = useCallback(() => {
        abortRef.current?.abort();
        abortRef.current = null;
        setLoading(false);
        setScanProgress(null);
    }, []);

    // Lazy per-field aggregation, coalesced: many panels open at once, so we batch
    // every request in the same microtask into one multi-field `/api/aggregate` scan.
    const aggBatchRef = useRef<
        Map<string, { resolve: (r: AggregateResponse) => void; reject: (e: unknown) => void }[]>
    >(new Map());
    const aggFlushScheduledRef = useRef(false);

    const flushFieldBuckets = useCallback(() => {
        aggFlushScheduledRef.current = false;
        const pending = aggBatchRef.current;
        if (pending.size === 0) return;
        const fields = Array.from(pending.keys());
        const waiters = Array.from(pending.values()).flat();
        pending.clear();

        const s = inputRef.current;
        const start = s.start ?? (s.follow ? "-5m" : s.range);
        const end = s.end ?? "now";
        aggregate({
            q: s.q,
            index: s.index,
            start,
            end,
            // One scan for every field the open panels need.
            fields: fields.join(","),
            // 20 leaves headroom for "Show more" (default-visible cap is 5) without a refetch.
            size: 20,
            // Always opt into sampling; backend no-ops it for narrow windows (≤16 partitions).
            approximate: true,
        })
            .then((r) => waiters.forEach((w) => w.resolve(r)))
            .catch((e) => waiters.forEach((w) => w.reject(e)));
    }, []);

    const fetchFieldBuckets = useCallback(
        (field: string): Promise<AggregateResponse> =>
            new Promise((resolve, reject) => {
                const pending = aggBatchRef.current;
                const arr = pending.get(field) ?? [];
                arr.push({ resolve, reject });
                pending.set(field, arr);
                if (!aggFlushScheduledRef.current) {
                    aggFlushScheduledRef.current = true;
                    queueMicrotask(flushFieldBuckets);
                }
            }),
        [flushFieldBuckets],
    );

    const navigate = useNavigate();
    const setInput = useCallback(
        (next: SearchInput) => {
            setInputState(next);
            runQuery(next);
            // Route the URL update through React Router; raw pushState would corrupt RR's
            // history state shape and break back/forward.
            const href = searchHref(next);
            if (href !== window.location.pathname + window.location.search) {
                navigate(href);
            }
        },
        [navigate, runQuery],
    );

    const refresh = useCallback(() => {
        runQuery(inputRef.current);
    }, [runQuery]);

    // Fetch on mount and every RR navigation (location.key bump); skip when the URL
    // already matches current input (setInput's own navigate already ran the query).
    const location = useLocation();
    const initRanRef = useRef(false);
    useEffect(() => {
        const fromUrl = readUrl();
        // Canonicalize the address bar so the range is always visible; replaceState
        // keeps it out of back/forward history.
        const canonical = searchHref(fromUrl);
        if (canonical !== window.location.pathname + window.location.search) {
            window.history.replaceState(window.history.state, "", canonical);
        }
        if (initRanRef.current && sameInput(fromUrl, inputRef.current)) return;
        initRanRef.current = true;
        setInputState(fromUrl);
        runQuery(fromUrl);
        // eslint-disable-next-line react-hooks/exhaustive-deps
    }, [location.key]);

    // Listen to popstate directly — RR's location.key sync has been seen to miss it.
    // Diff against current input so the RR-driven path doesn't double-fetch.
    useEffect(() => {
        const onPop = () => {
            const fromUrl = readUrl();
            if (sameInput(fromUrl, inputRef.current)) return;
            setInputState(fromUrl);
            runQuery(fromUrl);
        };
        window.addEventListener("popstate", onPop);
        return () => window.removeEventListener("popstate", onPop);
    }, [runQuery]);

    // Refresh the index list every 10s so new indexes appear. Env-scoped server-side.
    useEffect(() => {
        const refresh = () =>
            getIndexes()
                .then(setIndexes)
                .catch(() => {});
        refresh();
        const h = setInterval(refresh, 10000);
        return () => clearInterval(h);
    }, []);

    // Follow mode is handled by the live-tail view (`useLiveTail` polls its own
    // since-cursor); no page-level 2s re-query loop anymore.

    // First-ever visit: seed Pinned with `source` + a few categorical fields, once when
    // the catalog arrives. Clearing pins counts as "user took control" so we don't re-seed.
    useEffect(() => {
        if (seededRef.current || discovered.length === 0) return;
        seededRef.current = true;
        const seed = [
            "source",
            ...discovered
                .filter((f) => f.interesting)
                .slice(0, AUTOSEED_LIMIT)
                .map((f) => f.name),
        ];
        const uniq = Array.from(new Set(seed));
        setPinned(uniq);
        writePinnedPref(uniq);
    }, [discovered]);

    // Two stable lists: Pinned (user-curated) and Available (rest of the catalog).
    // Pins drop silently if a field leaves the catalog; `source` is always offerable.
    const catalogNames = new Set(discovered.map((f) => f.name));
    const livePins = pinned.filter((f) => f === "source" || catalogNames.has(f));
    const pinnedFields = [ALWAYS_PINNED, ...livePins];
    const pinnedSet = new Set(pinnedFields);
    const availableFields = discovered
        .filter((f) => f.groupable && !pinnedSet.has(f.name))
        .map((f) => f.name);

    return {
        input,
        setInput,
        searchResp,
        histResp,
        loading,
        error,
        indexes,
        pageSize: PAGE_SIZE,
        pinnedFields,
        availableFields,
        togglePin,
        discovered,
        fetchFieldBuckets,
        cacheKey: queryCacheKey(input),
        refresh,
        scanProgress,
        cancel,
    };
}
