// Copyright 2026 Appbird LLC. Licensed under "GNU Affero General Public License v3.0"

import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { Link } from "react-router-dom";
import { ArrowRight, Database, FolderInput, Loader2, Search, Send, X } from "lucide-react";
import { loadSampleData } from "../api/client";
import { useAuth } from "../state/useAuth";
import { SearchBar } from "../components/SearchBar";
import { RefreshIntervalPicker } from "../components/RefreshIntervalPicker";
import { resolveRefreshSecs, type RefreshSetting } from "../lib/autoRefresh";
import { useAutoRefresh } from "../state/useAutoRefresh";
import { Histogram } from "../components/Histogram";
import { ResultsList } from "../components/ResultsList";
import { ResultsTable } from "../components/ResultsTable";
import { isTimechartTable, TimechartChart } from "../components/TimechartChart";
import { FieldPanel } from "../components/FieldPanel";
import { toggleFilter, toggleTerm } from "../lib/query";
import { useSearchQuery } from "../state/useSearchQuery";
import { useLiveTail } from "../state/useLiveTail";
import { LiveTailView } from "../components/LiveTailView";
import type { SearchInput } from "../state/url";

// Per-browser auto-refresh preference for the search page. `"auto"` scales a
// default to the active relative range; a number is an explicit interval.
const SEARCH_REFRESH_KEY = "helios-search-refresh";
function readSearchRefresh(): RefreshSetting {
    try {
        const v = localStorage.getItem(SEARCH_REFRESH_KEY);
        if (v === null || v === "auto") return "auto";
        const n = Number(v);
        return Number.isFinite(n) ? n : "auto";
    } catch {
        return "auto";
    }
}
function writeSearchRefresh(setting: RefreshSetting): void {
    try {
        localStorage.setItem(SEARCH_REFRESH_KEY, String(setting));
    } catch {
        // storage disabled — selection still applies for this session.
    }
}

export function SearchPage() {
    const {
        input,
        setInput,
        searchResp,
        histResp,
        loading,
        error,
        indexes,
        pageSize,
        pinnedFields,
        availableFields,
        togglePin,
        discovered,
        fetchFieldBuckets,
        cacheKey,
        refresh,
        scanProgress,
        cancel,
    } = useSearchQuery();

    const isAdmin = !!useAuth().user?.is_admin;
    // Offer the first-run sample-data action only on a broad (unfiltered) empty
    // result — a specific query that simply didn't match isn't a "no data" signal.
    const isBroadQuery = !input.q.trim() || input.q.trim() === "*";

    // Live tail replaces the results list while following (hits-only — pipe
    // queries keep the table view and ignore follow).
    const isPipeQuery = input.q.replace(/"[^"]*"/g, "").includes("|");
    const tailActive = !!input.follow && !isPipeQuery;
    const tail = useLiveTail(tailActive, input.q, input.index);

    // First-run sample seeding: ingested rows land in a buffered writer that flushes
    // on a ~5s timer, so a single post-load refresh often still sees nothing. While
    // seeding, keep re-running the search until rows appear (or we give up), showing a
    // loading state the whole time so the user never has to manually refresh.
    const [seeding, setSeeding] = useState(false);
    const total = searchResp?.total ?? 0;
    const seedTicksRef = useRef(0);
    useEffect(() => {
        if (!seeding) return;
        if (total > 0) {
            setSeeding(false);
            return;
        }
        // `total` stays 0 across empty refreshes (a stable number), so this effect
        // doesn't re-run on each poll — the interval lives until data lands or times out.
        seedTicksRef.current = 0;
        const id = setInterval(() => {
            seedTicksRef.current += 1;
            if (seedTicksRef.current > 12) {
                setSeeding(false);
                return;
            }
            refresh();
        }, 1800);
        return () => clearInterval(id);
    }, [seeding, total, refresh]);

    // Effective time range for the autocomplete popover's value lookups —
    // mirrors what `useSearchQuery.runQuery` computes for its own fetches.
    const suggestStart = input.start ?? (input.follow ? "-5m" : input.range);
    const suggestEnd = input.end ?? "now";

    const handleSubmit = useCallback(
        (s: SearchInput) => {
            // SearchBar owns all query fields, so trust its emission verbatim.
            // Page resets to 1 on any form-driven change.
            setInput({ ...s, page: 1 });
        },
        [setInput],
    );

    // Toggle: first click adds `field:value`, clicking the same value again
    // removes that exact clause. Drives both facet rows and result-row picks.
    const handleFieldPick = useCallback(
        (field: string, value: string | number) => {
            setInput({ ...input, q: toggleFilter(input.q, field, value), page: 1 });
        },
        [input, setInput],
    );

    // Click-a-word toggles it as a bare term. Bare terms match the verbatim `raw`
    // text, so any visible word resolves with no field-mapping.
    const handleTermPick = useCallback(
        (term: string) => {
            setInput({ ...input, q: toggleTerm(input.q, term), page: 1 });
        },
        [input, setInput],
    );

    const handleBucketSelect = useCallback(
        (startMs: number, endMs: number) => {
            setInput({
                ...input,
                start: new Date(startMs).toISOString(),
                end: new Date(endMs).toISOString(),
                follow: false,
                page: 1,
            });
        },
        [input, setInput],
    );

    const handleClearTimeSelection = useCallback(() => {
        setInput({ ...input, start: undefined, end: undefined, page: 1 });
    }, [input, setInput]);

    const handleLoadSaved = useCallback(
        (s: SearchInput) => setInput({ ...s, page: 1 }),
        [setInput],
    );

    // Footer-derived coverage/kind per field, for the sidebar header glance
    // (and shown on Available fields before any value breakdown is loaded).
    const fieldMeta = useMemo(() => {
        const m = new Map<string, { coverage: number; valueKind: string; cardinality: number }>();
        for (const f of discovered) {
            m.set(f.name, {
                coverage: f.coverage,
                valueKind: f.value_kind,
                cardinality: f.cardinality,
            });
        }
        return m;
    }, [discovered]);

    const handlePageChange = useCallback(
        (page: number) => {
            setInput({ ...input, page });
            window.scrollTo({ top: 0, behavior: "smooth" });
        },
        [input, setInput],
    );

    // Auto-refresh. Re-runs the active query on a cadence for relative ranges;
    // off for absolute windows and while following live (which polls every 2s).
    const [refreshSetting, setRefreshSetting] = useState<RefreshSetting>(readSearchRefresh);
    const refreshDisabled = !!(input.start && input.end) || !!input.follow;
    const refreshSecs = resolveRefreshSecs(refreshSetting, {
        range: input.range,
        hasAbsolute: !!(input.start && input.end),
        follow: input.follow,
    });
    useAutoRefresh(refreshSecs, refresh);
    const refreshControl = (
        <RefreshIntervalPicker
            onRefresh={refresh}
            refreshing={loading}
            setting={refreshSetting}
            onChange={(s) => {
                setRefreshSetting(s);
                writeSearchRefresh(s);
            }}
            effectiveSecs={refreshSecs}
            disabled={refreshDisabled}
            following={!!input.follow}
            disabledReason={
                input.follow
                    ? "Following live — the tail updates itself"
                    : "Auto-refresh applies to relative ranges"
            }
        />
    );

    // Sidebar quick-filter: case-insensitive substring match on field name. UI-only.
    const [fieldFilter, setFieldFilter] = useState("");
    const fieldNeedle = fieldFilter.trim().toLowerCase();
    const matchName = useCallback(
        (f: string) => !fieldNeedle || f.toLowerCase().includes(fieldNeedle),
        [fieldNeedle],
    );
    const shownPinned = useMemo(() => pinnedFields.filter(matchName), [pinnedFields, matchName]);
    const shownAvailable = useMemo(
        () => availableFields.filter(matchName),
        [availableFields, matchName],
    );

    return (
        <div className="h-full flex flex-col">
            <div className="bg-white dark:bg-stone-900 border-b border-stone-200 dark:border-stone-800 px-4 py-3 flex-shrink-0">
                <SearchBar
                    key={`${input.q}|${input.range}|${input.index ?? ""}`}
                    initial={input}
                    onSubmit={handleSubmit}
                    indexes={indexes}
                    current={input}
                    onLoadSaved={handleLoadSaved}
                    fields={discovered}
                    start={suggestStart}
                    end={suggestEnd}
                    refreshControl={refreshControl}
                />
                {error && (
                    <div className="mt-2 px-3 py-2 rounded-md bg-red-50 text-red-800 border border-red-200 dark:bg-red-950 dark:text-red-200 dark:border-red-900">
                        error: {error}
                    </div>
                )}
            </div>

            <div className="flex-grow flex overflow-hidden">
                <aside className="text-sm w-80 shrink-0 border-r border-stone-200 dark:border-stone-800 bg-white dark:bg-stone-900 overflow-auto">
                    <div className="sticky top-0 z-10 bg-white dark:bg-stone-900 border-b border-stone-200 dark:border-stone-800 px-3 py-2">
                        <div className="relative">
                            <Search className="absolute left-2 top-1/2 -translate-y-1/2 w-3.5 h-3.5 text-stone-400 pointer-events-none" />
                            <input
                                type="text"
                                value={fieldFilter}
                                onChange={(e) => setFieldFilter(e.target.value)}
                                placeholder="Filter fields…"
                                className="w-full pl-7 pr-7 py-1.5 rounded-md border border-stone-200 dark:border-stone-700 bg-stone-50 dark:bg-stone-800 text-stone-800 dark:text-stone-200 placeholder-stone-400 dark:placeholder-stone-500 focus:outline-none focus:ring-1 focus:ring-blue-400 dark:focus:ring-blue-500"
                            />
                            {fieldFilter && (
                                <button
                                    type="button"
                                    onClick={() => setFieldFilter("")}
                                    className="absolute right-1.5 top-1/2 -translate-y-1/2 p-0.5 rounded text-stone-400 hover:text-stone-600 dark:hover:text-stone-200 hover:bg-stone-200 dark:hover:bg-stone-700"
                                    title="Clear filter"
                                    aria-label="Clear field filter"
                                >
                                    <X className="w-3.5 h-3.5" />
                                </button>
                            )}
                        </div>
                    </div>
                    {shownPinned.length > 0 && (
                        <>
                            <div className="px-4 pt-3 pb-1 flex items-center gap-2">
                                <span className="font-semibold uppercase tracking-wider text-stone-700 dark:text-stone-300">
                                    Pinned
                                </span>
                                <span className="text-[11px] text-stone-400 dark:text-stone-500 normal-case tracking-normal">
                                    value breakdowns
                                </span>
                            </div>
                            <div className="px-1">
                                {shownPinned.map((f) => (
                                    <FieldPanel
                                        // Stable key (no cacheKey) so the panel refetches in place
                                        // instead of remounting, keeping old buckets visible (dimmed).
                                        key={`pin|${f}`}
                                        field={f}
                                        fetch={fetchFieldBuckets}
                                        cacheKey={cacheKey}
                                        // Pinned panels load their breakdown up front, deferred behind
                                        // the main search (mainLoading) so they don't steal cores.
                                        defaultOpen
                                        onPick={handleFieldPick}
                                        query={input.q}
                                        mainLoading={loading}
                                        meta={fieldMeta.get(f)}
                                        pinned
                                        // `index` is always pinned (no unpin affordance).
                                        onTogglePin={f === "index" ? undefined : togglePin}
                                    />
                                ))}
                            </div>
                        </>
                    )}

                    <div className="px-4 pt-4 pb-1 flex items-center gap-2">
                        <span className="font-semibold uppercase tracking-wider text-stone-700 dark:text-stone-300">
                            Available fields
                        </span>
                        {shownAvailable.length > 0 && (
                            <span className="text-[11px] text-stone-400 dark:text-stone-500">
                                {shownAvailable.length}
                            </span>
                        )}
                    </div>
                    <div className="px-1 pb-2">
                        {shownAvailable.length === 0 ? (
                            <p className="px-3 py-2 text-stone-400 dark:text-stone-500 italic">
                                {fieldNeedle
                                    ? "no fields match"
                                    : loading
                                      ? "loading…"
                                      : "no other fields in this window"}
                            </p>
                        ) : (
                            shownAvailable.map((f) => (
                                <FieldPanel
                                    // Available fields stay collapsed until clicked; then one field's
                                    // breakdown loads. Stable key (see Pinned) so it doesn't remount.
                                    key={`avail|${f}`}
                                    field={f}
                                    fetch={fetchFieldBuckets}
                                    cacheKey={cacheKey}
                                    onPick={handleFieldPick}
                                    query={input.q}
                                    mainLoading={loading}
                                    meta={fieldMeta.get(f)}
                                    pinned={false}
                                    onTogglePin={togglePin}
                                />
                            ))
                        )}
                    </div>
                </aside>

                <main className="flex-grow overflow-auto px-5 py-4 space-y-4">
                    {!tailActive && (
                        <Histogram
                            buckets={histResp?.buckets ?? []}
                            intervalMs={histResp?.interval_ms ?? 0}
                            loading={loading}
                            selectedRange={
                                input.start && input.end
                                    ? { start: input.start, end: input.end }
                                    : undefined
                            }
                            onSelectBucket={handleBucketSelect}
                            onClearSelection={handleClearTimeSelection}
                            scanProgress={scanProgress}
                            onCancel={cancel}
                        />
                    )}

                    {tailActive ? (
                        <LiveTailView tail={tail} query={input.q} onPickTerm={handleTermPick} />
                    ) : searchResp?.table ? (
                        <>
                            {isTimechartTable(searchResp.table) && (
                                <TimechartChart table={searchResp.table} />
                            )}
                            <ResultsTable table={searchResp.table} loading={loading} />
                        </>
                    ) : (
                        <ResultsList
                            hits={searchResp?.hits ?? []}
                            total={searchResp?.total ?? 0}
                            tookUs={searchResp?.took_us ?? 0}
                            loading={loading || seeding}
                            loadingLabel={seeding ? "Loading sample data…" : undefined}
                            highlightTerms={searchResp?.highlight_terms ?? []}
                            page={input.page ?? 1}
                            pageSize={pageSize}
                            onPageChange={handlePageChange}
                            onPick={handleFieldPick}
                            onPickTimeRange={handleBucketSelect}
                            onPickTerm={handleTermPick}
                            query={input.q}
                            emptyExtra={
                                isAdmin && isBroadQuery ? (
                                    <EmptyStateGuide onSeeded={() => setSeeding(true)} />
                                ) : undefined
                            }
                        />
                    )}
                </main>
            </div>
        </div>
    );
}

// First-run guide shown in the empty results state (admins only): try sample data,
// or get real data in via a local file source or a log shipper. The sample button
// hands off to the page's seeding loop, which polls until the buffered rows appear.
function EmptyStateGuide({ onSeeded }: { onSeeded: () => void }) {
    const [busy, setBusy] = useState(false);
    const [error, setError] = useState<string | null>(null);

    async function run() {
        if (busy) return;
        setBusy(true);
        setError(null);
        try {
            await loadSampleData();
            onSeeded();
        } catch (e) {
            setError(e instanceof Error ? e.message : String(e));
        } finally {
            setBusy(false);
        }
    }

    return (
        <div className="flex flex-col items-center gap-4 max-w-md">
            <p className="text-sm text-stone-500 dark:text-stone-400">
                This environment is empty. Load sample logs to explore, or send your own data.
            </p>
            <button
                type="button"
                onClick={run}
                disabled={busy}
                className="inline-flex items-center gap-2 px-3 py-1.5 font-medium text-white bg-orange-600 hover:bg-orange-500 rounded-md transition disabled:opacity-50 disabled:cursor-not-allowed"
            >
                {busy ? (
                    <Loader2 className="w-4 h-4 animate-spin" aria-hidden="true" />
                ) : (
                    <Database className="w-4 h-4" aria-hidden="true" />
                )}
                {busy ? "Loading sample data…" : "Load sample data"}
            </button>
            {error && <span className="text-sm text-red-600 dark:text-red-400">{error}</span>}

            <div className="w-full pt-2 mt-1 border-t border-stone-200 dark:border-stone-800">
                <p className="text-xs font-medium uppercase tracking-wider text-stone-400 dark:text-stone-500 mb-2 text-left">
                    Bring your own data
                </p>
                <ul className="flex flex-col gap-1 text-left">
                    <li>
                        <Link
                            to="/admin/ingestion/sources"
                            className="group flex items-start gap-2.5 rounded-md px-2 py-2 -mx-2 hover:bg-stone-50 dark:hover:bg-stone-800/60"
                        >
                            <FolderInput className="w-4 h-4 mt-0.5 shrink-0 text-stone-400 dark:text-stone-500 group-hover:text-orange-500" />
                            <span className="min-w-0">
                                <span className="block text-sm font-medium text-stone-800 dark:text-stone-100">
                                    Tail a local file or directory
                                </span>
                                <span className="block text-xs text-stone-500 dark:text-stone-400">
                                    Point HeliosLogs at logs on this machine in Admin → Ingestion →
                                    Sources.
                                </span>
                            </span>
                            <ArrowRight className="w-4 h-4 mt-0.5 shrink-0 text-stone-300 dark:text-stone-600 group-hover:text-orange-500" />
                        </Link>
                    </li>
                    <li>
                        <Link
                            to="/admin/ingestion/tokens"
                            className="group flex items-start gap-2.5 rounded-md px-2 py-2 -mx-2 hover:bg-stone-50 dark:hover:bg-stone-800/60"
                        >
                            <Send className="w-4 h-4 mt-0.5 shrink-0 text-stone-400 dark:text-stone-500 group-hover:text-orange-500" />
                            <span className="min-w-0">
                                <span className="block text-sm font-medium text-stone-800 dark:text-stone-100">
                                    Ship logs from your stack
                                </span>
                                <span className="block text-xs text-stone-500 dark:text-stone-400">
                                    Fluent Bit, Elasticsearch, Loki, or OTLP can point straight at
                                    HeliosLogs — or POST NDJSON to{" "}
                                    <code className="font-mono text-[11px] text-stone-600 dark:text-stone-300">
                                        /api/ingest
                                    </code>
                                    . Create an ingest token in Admin → Ingestion → Ingest tokens.
                                </span>
                            </span>
                            <ArrowRight className="w-4 h-4 mt-0.5 shrink-0 text-stone-300 dark:text-stone-600 group-hover:text-orange-500" />
                        </Link>
                    </li>
                </ul>
            </div>
        </div>
    );
}
