// Copyright 2026 Appbird LLC. Licensed under "GNU Affero General Public License v3.0"

// `/admin/indexes` — catalog readout, per-index summary with expandable partitions,
// and catalog-wide maintenance actions. Polls index-info every 5s.

import { useCallback, useEffect, useMemo, useState } from "react";
import { ChevronDown, ChevronRight, Database, HardDrive, RefreshCw } from "lucide-react";
import { forceCommit, gcFiles, getCatalogInfo, mergeSegments } from "../../api/client";
import type { CatalogInfo, PartitionSummary } from "../../api/types";
import { formatBytes } from "../../lib/format";
import { Card, ErrorBanner, Toast } from "../../components/admin";

interface IndexAggregate {
    env: string;
    name: string;
    partitions: PartitionSummary[];
    num_docs: number;
    num_segments: number;
    byte_size: number;
    oldest_day: string;
    newest_day: string;
}

interface EnvAggregate {
    env: string;
    indexes: IndexAggregate[];
    num_docs: number;
    num_segments: number;
    byte_size: number;
}

export function IndexesPanel() {
    const [info, setInfo] = useState<CatalogInfo | null>(null);
    const [busy, setBusy] = useState<string | null>(null);
    const [error, setError] = useState<string | null>(null);
    const [toast, setToast] = useState<string | null>(null);
    const [expanded, setExpanded] = useState<Set<string>>(new Set());

    const refresh = useCallback(async () => {
        try {
            setInfo(await getCatalogInfo());
            setError(null);
        } catch (e) {
            setError(e instanceof Error ? e.message : String(e));
        }
    }, []);

    useEffect(() => {
        void refresh();
    }, [refresh]);

    // Poll while the panel is mounted so segment counts update post-merge.
    useEffect(() => {
        const h = setInterval(() => {
            getCatalogInfo()
                .then(setInfo)
                .catch(() => {});
        }, 5000);
        return () => clearInterval(h);
    }, []);

    const flashToast = (msg: string) => {
        setToast(msg);
        setTimeout(() => setToast(null), 3000);
    };

    const handleMergeAll = useCallback(async () => {
        if (
            !confirm("Merge all partitions' segments into one each? This rewrites the index files.")
        )
            return;
        setBusy("merge");
        setError(null);
        try {
            const r = await mergeSegments();
            flashToast(
                `merged ${r.merged_segments_total ?? 0} segments across ${
                    r.partitions_touched ?? 0
                } partitions in ${r.took_ms ?? 0}ms`,
            );
            await refresh();
        } catch (e) {
            setError(e instanceof Error ? e.message : String(e));
        } finally {
            setBusy(null);
        }
    }, [refresh]);

    const handleMergePartition = useCallback(
        async (p: PartitionSummary) => {
            if (p.num_segments < 2) return;
            setBusy(`merge-${p.env}-${p.index}-${p.day}`);
            setError(null);
            try {
                const r = await mergeSegments({ env: p.env, index: p.index, day: p.day });
                flashToast(
                    `${p.env}/${p.index}/${p.day}: merged ${r.merged_segments_total ?? 0} segments in ${
                        r.took_ms ?? 0
                    }ms`,
                );
                await refresh();
            } catch (e) {
                setError(e instanceof Error ? e.message : String(e));
            } finally {
                setBusy(null);
            }
        },
        [refresh],
    );

    const handleForceCommit = useCallback(async () => {
        setBusy("commit");
        setError(null);
        try {
            const r = await forceCommit();
            flashToast(`committed ${r.committed.length} partitions in ${r.took_ms}ms`);
            await refresh();
        } catch (e) {
            setError(e instanceof Error ? e.message : String(e));
        } finally {
            setBusy(null);
        }
    }, [refresh]);

    const handleGc = useCallback(async () => {
        setBusy("gc");
        setError(null);
        try {
            const r = await gcFiles();
            flashToast(
                r.message ??
                    `cleanup: dropped ${r.partitions_dropped ?? 0} partition(s) in ${r.took_ms}ms`,
            );
            await refresh();
        } catch (e) {
            setError(e instanceof Error ? e.message : String(e));
        } finally {
            setBusy(null);
        }
    }, [refresh]);

    // Two-level rollup: env → indexes → partitions; `_system` sorts last, partitions newest-first.
    const envGroups = useMemo<EnvAggregate[]>(() => {
        if (!info) return [];
        const byEnvIdx = new Map<string, Map<string, PartitionSummary[]>>();
        for (const p of info.partitions) {
            if (!byEnvIdx.has(p.env)) byEnvIdx.set(p.env, new Map());
            const inner = byEnvIdx.get(p.env)!;
            if (!inner.has(p.index)) inner.set(p.index, []);
            inner.get(p.index)!.push(p);
        }
        const envs: EnvAggregate[] = [];
        for (const [env, inner] of byEnvIdx.entries()) {
            const indexes: IndexAggregate[] = [];
            for (const [name, parts] of inner.entries()) {
                const sorted = [...parts].sort((a, b) => (a.day < b.day ? 1 : -1));
                indexes.push({
                    env,
                    name,
                    partitions: sorted,
                    num_docs: sorted.reduce((s, p) => s + p.num_docs, 0),
                    num_segments: sorted.reduce((s, p) => s + p.num_segments, 0),
                    byte_size: sorted.reduce((s, p) => s + p.byte_size, 0),
                    oldest_day: sorted[sorted.length - 1].day,
                    newest_day: sorted[0].day,
                });
            }
            indexes.sort((a, b) => b.byte_size - a.byte_size);
            envs.push({
                env,
                indexes,
                num_docs: indexes.reduce((s, i) => s + i.num_docs, 0),
                num_segments: indexes.reduce((s, i) => s + i.num_segments, 0),
                byte_size: indexes.reduce((s, i) => s + i.byte_size, 0),
            });
        }
        // `_system` last, everything else alphabetical.
        envs.sort((a, b) => {
            const sa = a.env.startsWith("_") ? 1 : 0;
            const sb = b.env.startsWith("_") ? 1 : 0;
            if (sa !== sb) return sa - sb;
            return a.env.localeCompare(b.env);
        });
        return envs;
    }, [info]);

    const totalIndexes = useMemo(
        () => envGroups.reduce((s, e) => s + e.indexes.length, 0),
        [envGroups],
    );

    const toggleIndex = useCallback((envName: string, name: string) => {
        setExpanded((prev) => {
            const key = `${envName}/${name}`;
            const next = new Set(prev);
            if (next.has(key)) next.delete(key);
            else next.add(key);
            return next;
        });
    }, []);

    if (!info) {
        return (
            <div className="p-6">
                <ErrorBanner error={error} />
                {!error && <div className="text-stone-700 dark:text-stone-300">loading…</div>}
            </div>
        );
    }

    return (
        <div>
            <Toast message={toast} />

            <Card title="Index management">
                <div className="p-6 space-y-6 max-w-3xl">
                    <IndexesHelpFrame />

                    <ErrorBanner error={error} />

                    <Subheader title="Maintenance actions (global, across all envs)" />

                    <div className="text-stone-700 dark:text-stone-300 -mt-2">
                        These act on every partition in every env. Per-partition merges (env-scoped)
                        are available inline below when you expand an index.
                    </div>

                    <div className="flex flex-wrap items-center gap-2">
                        <ActionBtn
                            onClick={handleMergeAll}
                            busy={busy === "merge"}
                            title="Compact each partition's segments down to one across every env. Rewrites index files."
                        >
                            Merge all
                        </ActionBtn>
                        <ActionBtn
                            onClick={handleForceCommit}
                            busy={busy === "commit"}
                            title="Flush any buffered writes to disk immediately (every env)."
                        >
                            Force commit
                        </ActionBtn>
                        <ActionBtn
                            onClick={handleGc}
                            busy={busy === "gc"}
                            title="Drop day-partitions past their retention right now (every env)."
                        >
                            Run cleanup now
                        </ActionBtn>
                        <button
                            type="button"
                            onClick={refresh}
                            className="inline-flex items-center gap-1.5 px-3 py-1.5 rounded-md border border-stone-200 dark:border-stone-700 text-stone-700 dark:text-stone-200 hover:bg-stone-100 dark:hover:bg-stone-800 transition"
                            title="Re-fetch catalog info. Auto-polls every 5s anyway."
                        >
                            <RefreshCw className="w-3.5 h-3.5" />
                            Refresh
                        </button>
                    </div>

                    <Subheader title="Catalog summary" />

                    <div className="grid grid-cols-2 sm:grid-cols-4 gap-3">
                        <Stat label="Indexes" value={totalIndexes.toLocaleString()} />
                        <Stat label="Partitions" value={info.num_partitions.toLocaleString()} />
                        <Stat label="Live segments" value={info.num_segments.toLocaleString()} />
                        <Stat label="Total on-disk" value={formatBytes(info.total_bytes)} />
                    </div>
                    <div className="text-stone-700 dark:text-stone-300 flex items-center gap-1.5">
                        <HardDrive className="w-3.5 h-3.5 flex-shrink-0" />
                        <span>
                            Data directory: <code className="font-mono">{info.data_dir}</code>
                        </span>
                    </div>
                </div>

                <div className="border-t border-stone-200 dark:border-stone-800 px-6 py-4 bg-stone-50/50 dark:bg-stone-950/30">
                    <div className="font-semibold uppercase tracking-wider text-stone-700 dark:text-stone-300">
                        Indexes by environment
                    </div>
                </div>

                {envGroups.length === 0 ? (
                    <div className="px-6 py-8 text-center text-stone-700 dark:text-stone-300 italic">
                        No partitions yet — ingest some events.
                    </div>
                ) : (
                    <div>
                        {envGroups.map((env) => (
                            <div
                                key={env.env}
                                className="border-b border-stone-100 dark:border-stone-800 last:border-b-0"
                            >
                                <div className="px-6 py-2 bg-stone-50/60 dark:bg-stone-950/20 flex items-center gap-3 text-stone-700 dark:text-stone-300">
                                    <code className="font-mono font-semibold text-stone-900 dark:text-stone-100">
                                        {env.env}
                                    </code>
                                    {env.env.startsWith("_") && (
                                        <span className="px-1.5 py-0.5 text-stone-700 dark:text-stone-300 bg-stone-200/60 dark:bg-stone-800/60 rounded uppercase tracking-wider">
                                            system
                                        </span>
                                    )}
                                    <div className="flex-grow" />
                                    <div className="text-stone-700 dark:text-stone-300 tabular-nums">
                                        {env.indexes.length}{" "}
                                        {env.indexes.length === 1 ? "index" : "indexes"}
                                    </div>
                                    <div className="text-stone-700 dark:text-stone-300 tabular-nums">
                                        {env.num_docs.toLocaleString()} docs
                                    </div>
                                    <div className="font-mono tabular-nums text-stone-800 dark:text-stone-200">
                                        {formatBytes(env.byte_size)}
                                    </div>
                                </div>
                                <ul className="divide-y divide-stone-100 dark:divide-stone-800">
                                    {env.indexes.map((idx) => {
                                        const key = `${env.env}/${idx.name}`;
                                        return (
                                            <IndexRow
                                                key={key}
                                                idx={idx}
                                                expanded={expanded.has(key)}
                                                onToggle={() => toggleIndex(env.env, idx.name)}
                                                totalBytes={info.total_bytes}
                                                onMergePartition={handleMergePartition}
                                                busy={busy}
                                            />
                                        );
                                    })}
                                </ul>
                            </div>
                        ))}
                    </div>
                )}
            </Card>

            <Card title="Shared schema">
                <div className="px-6 py-4 max-w-3xl text-stone-700 dark:text-stone-300">
                    All partitions share one schema — universal-core fields plus a dynamic JSON
                    column for everything else (
                    <code className="font-mono">dynamic.&lt;name&gt;</code> at query time).
                </div>
                <table className="w-full">
                    <thead className="bg-stone-50 dark:bg-stone-950/40 text-stone-700 dark:text-stone-300 border-y border-stone-200 dark:border-stone-800">
                        <tr>
                            <th className="text-left font-semibold uppercase tracking-wider px-4 py-2">
                                Field
                            </th>
                            <th className="text-left font-semibold uppercase tracking-wider px-4 py-2">
                                Type
                            </th>
                        </tr>
                    </thead>
                    <tbody className="divide-y divide-stone-100 dark:divide-stone-800">
                        {(info.schema ?? []).map((f) => (
                            <tr key={f.name}>
                                <td className="px-4 py-2 font-mono text-stone-800 dark:text-stone-200">
                                    {f.name}
                                </td>
                                <td className="px-4 py-2 text-stone-700 dark:text-stone-300">
                                    {f.type}
                                </td>
                            </tr>
                        ))}
                    </tbody>
                </table>
            </Card>
        </div>
    );
}

// One index row in the catalog list. Collapsed by default; click to
// expand into a per-day breakdown with per-partition merge actions.
function IndexRow({
    idx,
    expanded,
    onToggle,
    totalBytes,
    onMergePartition,
    busy,
}: {
    idx: IndexAggregate;
    expanded: boolean;
    onToggle: () => void;
    totalBytes: number;
    onMergePartition: (p: PartitionSummary) => void;
    busy: string | null;
}) {
    const pct = totalBytes ? (idx.byte_size / totalBytes) * 100 : 0;
    const dayRange =
        idx.oldest_day === idx.newest_day
            ? idx.newest_day
            : `${idx.oldest_day} → ${idx.newest_day}`;
    return (
        <li>
            <button
                type="button"
                onClick={onToggle}
                className="w-full text-left px-6 py-3 hover:bg-stone-50/60 dark:hover:bg-stone-800/30 transition-colors flex items-center gap-3"
            >
                <span className="text-stone-700 dark:text-stone-300 flex-shrink-0">
                    {expanded ? (
                        <ChevronDown className="w-4 h-4" />
                    ) : (
                        <ChevronRight className="w-4 h-4" />
                    )}
                </span>
                <div className="font-mono font-semibold text-stone-900 dark:text-stone-100 truncate min-w-[10rem]">
                    {idx.name}
                </div>
                <div className="text-stone-700 dark:text-stone-300 tabular-nums flex-shrink-0">
                    {idx.partitions.length} {idx.partitions.length === 1 ? "day" : "days"}
                </div>
                <div className="text-stone-700 dark:text-stone-300 tabular-nums flex-shrink-0 hidden md:block">
                    {dayRange}
                </div>
                <div className="flex-grow" />
                <div className="text-stone-700 dark:text-stone-300 tabular-nums flex-shrink-0 hidden sm:block">
                    {idx.num_docs.toLocaleString()} docs
                </div>
                <div className="text-stone-700 dark:text-stone-300 tabular-nums flex-shrink-0">
                    {idx.num_segments} seg
                </div>
                <div className="font-mono tabular-nums text-stone-800 dark:text-stone-200 flex-shrink-0">
                    {formatBytes(idx.byte_size)}
                </div>
                <div
                    className="h-1.5 rounded bg-stone-100 dark:bg-stone-800 overflow-hidden w-24 flex-shrink-0"
                    title={`${pct.toFixed(1)}% of catalog`}
                >
                    <div
                        className="h-full bg-orange-500 dark:bg-orange-600"
                        style={{ width: `${pct.toFixed(1)}%` }}
                    />
                </div>
            </button>
            {expanded && (
                <div className="bg-stone-50/60 dark:bg-stone-950/40 border-t border-stone-100 dark:border-stone-800">
                    <table className="w-full">
                        <thead className="text-stone-700 dark:text-stone-300">
                            <tr className="border-b border-stone-200 dark:border-stone-800">
                                <th className="text-left font-semibold uppercase tracking-wider px-6 py-2 pl-12">
                                    Day
                                </th>
                                <th className="text-right font-semibold uppercase tracking-wider px-3 py-2">
                                    Docs
                                </th>
                                <th className="text-right font-semibold uppercase tracking-wider px-3 py-2">
                                    Segments
                                </th>
                                <th className="text-right font-semibold uppercase tracking-wider px-3 py-2">
                                    Size
                                </th>
                                <th className="text-right font-semibold uppercase tracking-wider px-6 py-2">
                                    Actions
                                </th>
                            </tr>
                        </thead>
                        <tbody className="divide-y divide-stone-100 dark:divide-stone-800">
                            {idx.partitions.map((p) => {
                                const partBusy = busy === `merge-${idx.env}-${idx.name}-${p.day}`;
                                return (
                                    <tr
                                        key={p.day}
                                        className="text-stone-700 dark:text-stone-300 hover:bg-stone-50 dark:hover:bg-stone-900/40"
                                    >
                                        <td className="px-6 py-1.5 pl-12 font-mono">{p.day}</td>
                                        <td className="px-3 py-1.5 text-right font-mono tabular-nums">
                                            {p.num_docs.toLocaleString()}
                                        </td>
                                        <td className="px-3 py-1.5 text-right font-mono tabular-nums">
                                            {p.num_segments}
                                        </td>
                                        <td className="px-3 py-1.5 text-right font-mono tabular-nums">
                                            {formatBytes(p.byte_size)}
                                        </td>
                                        <td className="px-6 py-1.5 text-right">
                                            <button
                                                type="button"
                                                className="px-2 py-1 rounded-md border border-stone-200 dark:border-stone-700 text-stone-700 dark:text-stone-300 hover:border-orange-300 hover:bg-orange-50/40 dark:hover:bg-orange-950/30 disabled:opacity-40 disabled:cursor-not-allowed transition"
                                                onClick={() => onMergePartition(p)}
                                                disabled={p.num_segments < 2 || partBusy}
                                                title={
                                                    p.num_segments < 2
                                                        ? "Nothing to merge"
                                                        : `Merge ${p.num_segments} segments → 1`
                                                }
                                            >
                                                {partBusy ? "merging…" : "merge"}
                                            </button>
                                        </td>
                                    </tr>
                                );
                            })}
                        </tbody>
                    </table>
                </div>
            )}
        </li>
    );
}

function IndexesHelpFrame() {
    return (
        <div className="flex gap-3 p-4 rounded-lg bg-orange-50/60 dark:bg-orange-950/20 border border-orange-200/70 dark:border-orange-900/40">
            <div className="flex-shrink-0 mt-0.5">
                <Database className="w-4 h-4 text-orange-600 dark:text-orange-400" />
            </div>
            <div className="space-y-1.5 text-stone-700 dark:text-stone-200 leading-relaxed">
                <p>
                    HeliosLogs stores events in per-<code className="font-mono">(index, day)</code>{" "}
                    partitions. Each partition holds its own segments — the unit of on-disk storage;
                    queries fan out across every segment in scope.
                </p>
                <p className="text-stone-700 dark:text-stone-300">
                    The maintenance actions are rarely needed in normal operation — ingest commits
                    periodically (see <strong>General settings</strong> → commit interval), and
                    merges happen on a background schedule. Use <strong>Merge all</strong> to
                    compact fragmented partitions ahead of large reads; use{" "}
                    <strong>GC orphans</strong> after a crash to reclaim space from interrupted
                    merges.
                </p>
            </div>
        </div>
    );
}

function Subheader({ title }: { title: string }) {
    return (
        <div className="flex items-center gap-3 pt-2">
            <div className="font-semibold uppercase tracking-wider text-stone-700 dark:text-stone-300">
                {title}
            </div>
            <div className="flex-grow h-px bg-stone-200 dark:bg-stone-800" />
        </div>
    );
}

function Stat({ label, value }: { label: string; value: string }) {
    return (
        <div className="rounded-lg border border-stone-200 dark:border-stone-800 bg-stone-50/50 dark:bg-stone-950/40 px-3 py-2">
            <div className="text-stone-700 dark:text-stone-300">{label}</div>
            <div className="font-mono tabular-nums font-semibold text-stone-900 dark:text-stone-100 mt-0.5">
                {value}
            </div>
        </div>
    );
}

function ActionBtn({
    onClick,
    busy,
    title,
    children,
}: {
    onClick: () => void;
    busy: boolean;
    title?: string;
    children: React.ReactNode;
}) {
    return (
        <button
            type="button"
            onClick={onClick}
            disabled={busy}
            title={title}
            className="px-3 py-1.5 font-medium rounded-md bg-stone-900 hover:bg-stone-800 dark:bg-stone-800 dark:hover:bg-stone-700 text-white disabled:opacity-50 disabled:cursor-not-allowed transition"
        >
            {busy ? "working…" : children}
        </button>
    );
}
