// Copyright 2026 Appbird LLC. Licensed under "GNU Affero General Public License v3.0"

// Admin → Source management. Lists ingestion sources across all environments
// with CRUD, a "Run now" trigger, and per-source progress/checkpoint readout.

import { useCallback, useEffect, useRef, useState } from "react";
import {
    ChevronDown,
    ChevronRight,
    FolderInput,
    FolderOpen,
    Info,
    Pencil,
    Play,
    Plus,
    RotateCcw,
    Trash2,
    Webhook,
} from "lucide-react";
import {
    browseDir,
    createSource,
    deleteSource,
    getEnv,
    getIngestTokens,
    getSource,
    listEnvs,
    resetSource,
    runSourceNow,
    setIngestEndpoints,
    updateSource,
} from "../../api/client";
import { notifySourcesChanged } from "../../api/events";
import type {
    BrowseResult,
    IngestAuthConfig,
    Source,
    SourceDetail,
    SourceInput,
} from "../../api/types";
import { formatBytes } from "../../lib/format";
import { useSources } from "../../state/useSources";
import { Card, Empty, ErrorBanner, HelpFrame, Th, Toast } from "../../components/admin";

// HTTP push ingestion toggles (native API + compatibility shims). These gate the
// `/api/ingest*` and ES/OTLP/Loki/HEC endpoints; pull sources + syslog are separate.
function IngestEndpointsCard() {
    const [cfg, setCfg] = useState<IngestAuthConfig | null>(null);
    const [busy, setBusy] = useState(false);
    const [error, setError] = useState<string | null>(null);

    const load = useCallback(() => {
        getIngestTokens()
            .then(setCfg)
            .catch((e) => setError(e instanceof Error ? e.message : String(e)));
    }, []);
    useEffect(() => load(), [load]);

    async function toggle(patch: { api_enabled?: boolean; shims_enabled?: boolean }) {
        setBusy(true);
        setError(null);
        try {
            await setIngestEndpoints(patch);
            setCfg(await getIngestTokens());
        } catch (e) {
            setError(e instanceof Error ? e.message : String(e));
        } finally {
            setBusy(false);
        }
    }

    return (
        <Card title="HTTP ingestion endpoints">
            <div className="p-6 space-y-3">
                <HelpFrame icon={<Webhook className="w-4 h-4" />}>
                    <p className="max-w-3xl">
                        Push-based ingestion over HTTP, separate from the pull sources below.
                        Turning a class off makes HeliosLogs reject those requests with{" "}
                        <code className="text-orange-700 dark:text-orange-300">403</code>; pull
                        sources and the syslog listener are unaffected.
                    </p>
                </HelpFrame>
                <ErrorBanner error={error} />
                <label className="flex items-start gap-2 text-stone-700 dark:text-stone-200">
                    <input
                        type="checkbox"
                        className="mt-1 accent-orange-600"
                        checked={cfg?.api_enabled ?? true}
                        disabled={busy || !cfg}
                        onChange={(e) => void toggle({ api_enabled: e.target.checked })}
                    />
                    <span>
                        <span className="font-medium">HeliosLogs Ingest API</span> —{" "}
                        <code className="text-xs">/api/ingest</code> and{" "}
                        <code className="text-xs">/api/ingest/raw</code>
                    </span>
                </label>
                <label className="flex items-start gap-2 text-stone-700 dark:text-stone-200">
                    <input
                        type="checkbox"
                        className="mt-1 accent-orange-600"
                        checked={cfg?.shims_enabled ?? true}
                        disabled={busy || !cfg}
                        onChange={(e) => void toggle({ shims_enabled: e.target.checked })}
                    />
                    <span>
                        <span className="font-medium">Drop-in compatibility APIs</span> —
                        Elasticsearch <code className="text-xs">_bulk</code>, OTLP, Loki, and HEC
                    </span>
                </label>
                {cfg && !cfg.api_enabled && !cfg.shims_enabled && (
                    <p className="text-xs text-amber-700 dark:text-amber-400">
                        All HTTP push ingestion is disabled — only pull sources and the syslog
                        listener can bring in data.
                    </p>
                )}
            </div>
        </Card>
    );
}

export function SourcesPanel() {
    const { items, error, refresh } = useSources();
    const [busyId, setBusyId] = useState<string | null>(null);
    const [actionError, setActionError] = useState<string | null>(null);
    const [toast, setToast] = useState<string | null>(null);
    const [expanded, setExpanded] = useState<string | null>(null);
    const [dialog, setDialog] = useState<
        { kind: "create" } | { kind: "edit"; source: Source } | null
    >(null);

    function flash(msg: string) {
        setToast(msg);
        setTimeout(() => setToast(null), 2500);
    }

    async function withBusy(id: string, fn: () => Promise<void>) {
        setBusyId(id);
        setActionError(null);
        try {
            await fn();
            notifySourcesChanged();
        } catch (e) {
            setActionError(e instanceof Error ? e.message : String(e));
        } finally {
            setBusyId(null);
        }
    }

    return (
        <div>
            <IngestEndpointsCard />
            <Card title="Source management">
                <div className="p-6 space-y-4">
                    <HelpFrame icon={<FolderInput className="w-4 h-4" />}>
                        <p className="max-w-3xl">
                            Point HeliosLogs at logs and let the server ingest them. A{" "}
                            <strong>filesystem</strong> source polls a glob on the server's own
                            disk/mounts (e.g.{" "}
                            <code className="text-orange-700 dark:text-orange-300">
                                /var/log/**/*.log
                            </code>
                            ), tailing new lines as they're written. An <strong>S3</strong> source
                            polls a bucket prefix (e.g.{" "}
                            <code className="text-orange-700 dark:text-orange-300">
                                s3://bucket/logs/**/*.gz
                            </code>
                            ) and ingests each object once. This screen lists sources across{" "}
                            <strong>all environments</strong>; each source ingests into the
                            environment shown.
                        </p>
                    </HelpFrame>
                    <ErrorBanner error={error ?? actionError} />
                    <div className="flex items-center gap-3">
                        <button
                            type="button"
                            onClick={() => setDialog({ kind: "create" })}
                            className="px-3 py-1.5 font-medium text-white bg-orange-600 hover:bg-orange-500 rounded-md transition flex items-center gap-1.5"
                        >
                            <Plus className="w-4 h-4" />
                            New source
                        </button>
                        <span className="text-stone-700 dark:text-stone-300">
                            {items.length} source{items.length === 1 ? "" : "s"}
                        </span>
                    </div>
                </div>

                {items.length === 0 ? (
                    <Empty>No sources yet — create one to start ingesting.</Empty>
                ) : (
                    <table className="w-full">
                        <thead className="bg-stone-50 dark:bg-stone-950/40 border-y border-stone-200 dark:border-stone-800">
                            <tr>
                                <Th>Name</Th>
                                <Th>Environment</Th>
                                <Th>Kind</Th>
                                <Th>Path</Th>
                                <Th>Index</Th>
                                <Th>Status</Th>
                                <Th align="right">Actions</Th>
                            </tr>
                        </thead>
                        <tbody className="divide-y divide-stone-100 dark:divide-stone-800">
                            {items.map((s) => (
                                <SourceRow
                                    key={s.id}
                                    source={s}
                                    busy={busyId === s.id}
                                    expanded={expanded === s.id}
                                    onToggle={() =>
                                        setExpanded((cur) => (cur === s.id ? null : s.id))
                                    }
                                    onRun={() =>
                                        withBusy(s.id, async () => {
                                            await runSourceNow(s.id);
                                            flash("Queued — next poll within ~5s");
                                        })
                                    }
                                    onToggleEnabled={() =>
                                        withBusy(s.id, () =>
                                            updateSource(s.id, { enabled: !s.enabled }).then(
                                                () => {},
                                            ),
                                        )
                                    }
                                    onReset={() => {
                                        if (
                                            !confirm(
                                                `Reset "${s.name}"?\n\nThis deletes its ingestion checkpoint and zeroes its counters. On the next run it re-ingests every matching file from the start — use this after deleting the index data underneath.${
                                                    s.enabled
                                                        ? "\n\nTip: disable it first so it doesn't immediately re-run."
                                                        : ""
                                                }`,
                                            )
                                        )
                                            return;
                                        void withBusy(s.id, async () => {
                                            await resetSource(s.id);
                                            flash("Ingestion state reset");
                                        });
                                    }}
                                    onEdit={() => setDialog({ kind: "edit", source: s })}
                                    onDelete={() => {
                                        if (!confirm(`Delete source "${s.name}"?`)) return;
                                        void withBusy(s.id, () => deleteSource(s.id));
                                    }}
                                />
                            ))}
                        </tbody>
                    </table>
                )}
            </Card>

            {dialog && (
                <SourceDialog
                    source={dialog.kind === "edit" ? dialog.source : null}
                    onClose={() => setDialog(null)}
                    onSaved={() => {
                        setDialog(null);
                        notifySourcesChanged();
                        refresh();
                    }}
                />
            )}
            <Toast message={toast} />
        </div>
    );
}

function SourceRow({
    source: s,
    busy,
    expanded,
    onToggle,
    onRun,
    onToggleEnabled,
    onReset,
    onEdit,
    onDelete,
}: {
    source: Source;
    busy: boolean;
    expanded: boolean;
    onToggle: () => void;
    onRun: () => void;
    onToggleEnabled: () => void;
    onReset: () => void;
    onEdit: () => void;
    onDelete: () => void;
}) {
    return (
        <>
            <tr className="text-stone-800 dark:text-stone-200 hover:bg-stone-50/60 dark:hover:bg-stone-800/30 transition-colors">
                <td className="px-3 py-2 font-medium">
                    <button
                        type="button"
                        onClick={onToggle}
                        className="inline-flex items-center gap-1.5 text-left hover:text-orange-600 dark:hover:text-orange-400"
                        title="Show progress & checkpoint"
                    >
                        {expanded ? (
                            <ChevronDown className="w-3.5 h-3.5 text-stone-500 dark:text-stone-400" />
                        ) : (
                            <ChevronRight className="w-3.5 h-3.5 text-stone-500 dark:text-stone-400" />
                        )}
                        {s.name}
                    </button>
                    {!s.enabled && (
                        <span className="ml-2 text-xs px-1.5 py-0.5 rounded bg-stone-200 dark:bg-stone-700 text-stone-600 dark:text-stone-300">
                            disabled
                        </span>
                    )}
                </td>
                <td className="px-3 py-2">
                    <code className="font-mono text-xs px-1.5 py-0.5 rounded bg-stone-100 dark:bg-stone-800 text-stone-700 dark:text-stone-300">
                        {s.env}
                    </code>
                </td>
                <td className="px-3 py-2 text-stone-700 dark:text-stone-300 whitespace-nowrap">
                    {s.kind} · {s.mode} · {fmtInterval(s.interval_seconds)}
                </td>
                <td className="px-3 py-2 font-mono text-xs max-w-xs truncate" title={s.path}>
                    {s.path}
                </td>
                <td className="px-3 py-2">{s.index}</td>
                <td className="px-3 py-2 whitespace-nowrap">
                    <StatusCell source={s} />
                </td>
                <td className="px-3 py-2">
                    <div className="flex items-center justify-end gap-1">
                        <IconBtn title="Run now" busy={busy} onClick={onRun}>
                            <Play className="w-4 h-4" />
                        </IconBtn>
                        <IconBtn
                            title={s.enabled ? "Disable" : "Enable"}
                            busy={busy}
                            onClick={onToggleEnabled}
                        >
                            <span className="text-xs font-medium px-1">
                                {s.enabled ? "Off" : "On"}
                            </span>
                        </IconBtn>
                        <IconBtn
                            title="Reset ingestion state (clear checkpoint + counters)"
                            busy={busy}
                            onClick={onReset}
                        >
                            <RotateCcw className="w-4 h-4" />
                        </IconBtn>
                        <IconBtn title="Edit" busy={false} onClick={onEdit}>
                            <Pencil className="w-4 h-4" />
                        </IconBtn>
                        <IconBtn title="Delete" busy={busy} onClick={onDelete}>
                            <Trash2 className="w-4 h-4 text-red-600 dark:text-red-400" />
                        </IconBtn>
                    </div>
                </td>
            </tr>
            {expanded && (
                <tr>
                    <td colSpan={7} className="bg-stone-50/60 dark:bg-stone-950/40 px-0 py-0">
                        <SourceDetails id={s.id} />
                    </td>
                </tr>
            )}
        </>
    );
}

// Per-source progress: run state + ingest checkpoint (per-file byte offsets, the
// "fishbucket"). Polls while expanded so a running source's offsets tick up live.
function SourceDetails({ id }: { id: string }) {
    const [detail, setDetail] = useState<SourceDetail | null>(null);
    const [err, setErr] = useState<string | null>(null);

    useEffect(() => {
        let alive = true;
        const load = () =>
            getSource(id)
                .then((d) => alive && (setDetail(d), setErr(null)))
                .catch((e) => alive && setErr(e instanceof Error ? e.message : String(e)));
        load();
        const h = setInterval(load, 4000);
        return () => {
            alive = false;
            clearInterval(h);
        };
    }, [id]);

    if (err) {
        return (
            <div className="px-12 py-4">
                <ErrorBanner error={err} />
            </div>
        );
    }
    if (!detail) {
        return <div className="px-12 py-4 text-stone-700 dark:text-stone-300">loading…</div>;
    }

    const s = detail.source;
    const files = Object.entries(detail.checkpoint.files ?? {}).sort((a, b) =>
        a[0] < b[0] ? -1 : 1,
    );
    const totalBytes = files.reduce((acc, [, m]) => acc + m.offset, 0);

    const liveRows = s.total_ingested + (s.running ? s.progress_ingested : 0);

    return (
        <div className="px-12 py-4 space-y-4 border-t border-stone-100 dark:border-stone-800">
            {s.running && (
                <div className="flex items-center gap-3 px-4 py-2.5 rounded-lg bg-orange-50/70 dark:bg-orange-950/30 border border-orange-200/70 dark:border-orange-900/40">
                    <span className="w-2 h-2 rounded-full bg-orange-500 animate-pulse flex-shrink-0" />
                    <div className="min-w-0 text-stone-800 dark:text-stone-200">
                        {s.progress_file ? (
                            <>
                                Indexing{" "}
                                <code className="font-mono text-xs break-all text-orange-800 dark:text-orange-300">
                                    {s.progress_file}
                                </code>{" "}
                                — {s.progress_ingested.toLocaleString()} rows this run
                            </>
                        ) : (
                            <>Scanning for files to ingest…</>
                        )}
                    </div>
                </div>
            )}
            <div className="grid grid-cols-2 sm:grid-cols-4 gap-3">
                <Metric label="State">
                    {!s.enabled ? (
                        <span className="text-stone-600 dark:text-stone-300">disabled</span>
                    ) : s.running ? (
                        <span className="text-orange-600 dark:text-orange-400">running</span>
                    ) : (
                        <span className="text-emerald-600 dark:text-emerald-400">idle</span>
                    )}
                </Metric>
                <Metric label={s.running ? "Rows ingested (live)" : "Rows ingested (lifetime)"}>
                    {liveRows.toLocaleString()}
                </Metric>
                <Metric label="Last run">{s.last_run_at ? fmtAgo(s.last_run_at) : "never"}</Metric>
                <Metric label="Poll interval">{fmtInterval(s.interval_seconds)}</Metric>
                <Metric label="Files tracked">{files.length.toLocaleString()}</Metric>
                <Metric label="Bytes consumed">{formatBytes(totalBytes)}</Metric>
                <Metric label="Last status">
                    {s.last_status ? (
                        <span
                            className={
                                s.last_status === "ok"
                                    ? "text-emerald-600 dark:text-emerald-400"
                                    : "text-red-600 dark:text-red-400"
                            }
                        >
                            {s.last_status}
                        </span>
                    ) : (
                        "—"
                    )}
                </Metric>
                <Metric label="Running since">
                    {s.enabled && s.running && s.running_since ? fmtAgo(s.running_since) : "—"}
                </Metric>
            </div>

            {s.last_error && (
                <div className="text-sm text-red-600 dark:text-red-400 font-mono break-all">
                    last error: {s.last_error}
                </div>
            )}

            <div>
                <div className="font-semibold uppercase tracking-wider text-stone-700 dark:text-stone-300 mb-1">
                    Checkpoint (per-file progress)
                </div>
                {files.length === 0 ? (
                    <div className="text-stone-700 dark:text-stone-300 italic">
                        No files checkpointed yet — nothing has matched the glob, or the source
                        hasn't run.
                    </div>
                ) : (
                    <div className="border border-stone-200 dark:border-stone-800 rounded-md overflow-hidden max-h-72 overflow-y-auto">
                        <table className="w-full">
                            <thead className="bg-stone-100/60 dark:bg-stone-900/40 text-stone-700 dark:text-stone-300 sticky top-0">
                                <tr>
                                    <th className="text-left font-semibold uppercase tracking-wider px-3 py-1.5">
                                        File
                                    </th>
                                    <th className="text-right font-semibold uppercase tracking-wider px-3 py-1.5">
                                        Offset
                                    </th>
                                    <th className="text-right font-semibold uppercase tracking-wider px-3 py-1.5">
                                        Modified
                                    </th>
                                </tr>
                            </thead>
                            <tbody className="divide-y divide-stone-100 dark:divide-stone-800">
                                {files.map(([path, m]) => (
                                    <tr key={path} className="text-stone-700 dark:text-stone-300">
                                        <td
                                            className="px-3 py-1.5 font-mono text-xs break-all"
                                            title={path}
                                        >
                                            {path}
                                        </td>
                                        <td className="px-3 py-1.5 text-right font-mono tabular-nums whitespace-nowrap">
                                            {formatBytes(m.offset)}
                                        </td>
                                        <td className="px-3 py-1.5 text-right font-mono tabular-nums whitespace-nowrap text-stone-700 dark:text-stone-300">
                                            {m.mtime_ms ? fmtAgo(m.mtime_ms) : "—"}
                                        </td>
                                    </tr>
                                ))}
                            </tbody>
                        </table>
                    </div>
                )}
            </div>
        </div>
    );
}

function Metric({ label, children }: { label: string; children: React.ReactNode }) {
    return (
        <div className="rounded-lg border border-stone-200 dark:border-stone-800 bg-white/60 dark:bg-stone-900/40 px-3 py-2">
            <div className="text-stone-700 dark:text-stone-300">{label}</div>
            <div className="font-mono tabular-nums font-semibold text-stone-900 dark:text-stone-100 mt-0.5">
                {children}
            </div>
        </div>
    );
}

function StatusCell({ source: s }: { source: Source }) {
    if (!s.enabled) {
        return <span className="text-stone-700 dark:text-stone-300">disabled</span>;
    }
    if (s.running) {
        return (
            <span className="inline-flex items-center gap-1.5 text-orange-600 dark:text-orange-400">
                <span className="w-2 h-2 rounded-full bg-orange-500 animate-pulse" />
                running
                {s.progress_ingested > 0 && (
                    <span className="text-stone-700 dark:text-stone-300">
                        · {s.progress_ingested.toLocaleString()} rows
                    </span>
                )}
            </span>
        );
    }
    if (!s.last_run_at) {
        return <span className="text-stone-700 dark:text-stone-300">never run</span>;
    }
    const ok = s.last_status === "ok";
    return (
        <span className="inline-flex items-center gap-2">
            <span
                className={
                    ok ? "text-emerald-600 dark:text-emerald-400" : "text-red-600 dark:text-red-400"
                }
            >
                {ok ? "ok" : "error"}
            </span>
            <span className="text-stone-700 dark:text-stone-300">{fmtAgo(s.last_run_at)}</span>
            <span className="text-stone-700 dark:text-stone-300">
                · {s.total_ingested.toLocaleString()} rows
            </span>
            {!ok && s.last_error && (
                <span className="text-red-500/80 max-w-[16rem] truncate" title={s.last_error}>
                    {s.last_error}
                </span>
            )}
        </span>
    );
}

function IconBtn({
    title,
    busy,
    onClick,
    children,
}: {
    title: string;
    busy: boolean;
    onClick: () => void;
    children: React.ReactNode;
}) {
    return (
        <button
            type="button"
            title={title}
            disabled={busy}
            onClick={onClick}
            className="p-1.5 rounded-md hover:bg-stone-200 dark:hover:bg-stone-700 disabled:opacity-40 transition"
        >
            {children}
        </button>
    );
}

const FIELD =
    "w-full px-2.5 py-1.5 bg-stone-50 dark:bg-stone-950 border border-stone-200 dark:border-stone-700 rounded-md focus:outline-none focus:border-orange-500 focus:bg-white dark:focus:bg-stone-900 text-stone-900 dark:text-stone-100";
const LABEL = "block text-sm font-medium text-stone-600 dark:text-stone-300 mb-1";

// Field label with a hover-tooltip info icon, so per-field guidance doesn't
// take up vertical space under every input.
function FieldLabel({ help, children }: { help: string; children: React.ReactNode }) {
    return (
        <div className={`${LABEL} flex items-center gap-1`}>
            <span>{children}</span>
            <span
                title={help}
                aria-label={help}
                className="cursor-help text-stone-600 hover:text-stone-900 dark:text-stone-400 dark:hover:text-stone-200"
            >
                <Info className="w-3.5 h-3.5" />
            </span>
        </div>
    );
}

function SourceDialog({
    source,
    onClose,
    onSaved,
}: {
    source: Source | null;
    onClose: () => void;
    onSaved: () => void;
}) {
    const isEdit = !!source;
    const firstRef = useRef<HTMLInputElement>(null);
    const [name, setName] = useState(source?.name ?? "");
    const [env, setEnvVal] = useState(source?.env ?? getEnv());
    const [envs, setEnvs] = useState<string[]>([]);
    const [kind, setKind] = useState(source?.kind ?? "fs");
    const [index, setIndex] = useState(source?.index ?? "");
    const [path, setPath] = useState(source?.path ?? "");
    const [format, setFormat] = useState(source?.format ?? "auto");
    const [compression, setCompression] = useState(source?.compression ?? "auto");
    const [interval, setInterval] = useState(String(source?.interval_seconds ?? 10));
    const [exclude, setExclude] = useState((source?.exclude ?? []).join("\n"));
    const [multiline, setMultiline] = useState(source?.multiline_pattern ?? "");
    const [grok, setGrok] = useState(source?.grok_pattern ?? "");
    const [sourceTag, setSourceTag] = useState(source?.source_tag ?? "");
    const [enabled, setEnabled] = useState(source?.enabled ?? true);
    const [browsing, setBrowsing] = useState(false);
    const [saving, setSaving] = useState(false);
    const [err, setErr] = useState<string | null>(null);

    useEffect(() => {
        firstRef.current?.focus();
    }, []);

    // Env dropdown options — keep the source's current env in the list even if
    // it's no longer a registered env, so editing never silently moves it.
    useEffect(() => {
        listEnvs()
            .then((rows) => {
                const names = rows.map((r) => r.name);
                if (!names.includes(env)) names.unshift(env);
                setEnvs(names);
            })
            .catch(() => setEnvs([env]));
    }, [env]);

    async function submit(e: React.FormEvent) {
        e.preventDefault();
        setSaving(true);
        setErr(null);
        const excludeList = exclude
            .split("\n")
            .map((x) => x.trim())
            .filter(Boolean);
        const payload: SourceInput = {
            name: name.trim(),
            index: index.trim(),
            kind,
            mode: "pull",
            path: path.trim(),
            exclude: excludeList,
            format,
            compression,
            interval_seconds: Math.max(5, Number(interval) || 10),
            multiline_pattern: multiline.trim() || null,
            grok_pattern: grok.trim() || null,
            source_tag: sourceTag.trim() || null,
            enabled,
        };
        try {
            if (isEdit && source) {
                await updateSource(source.id, { ...payload, env });
            } else {
                await createSource(payload, env);
            }
            onSaved();
        } catch (e) {
            setErr(e instanceof Error ? e.message : String(e));
        } finally {
            setSaving(false);
        }
    }

    const pathPlaceholder = kind === "s3" ? "s3://my-bucket/logs/**/*.gz" : "/var/log/app/**/*.log";

    return (
        <div
            className="fixed inset-0 z-50 flex items-center justify-center bg-stone-900/50 dark:bg-black/60"
            onClick={onClose}
        >
            <form
                onSubmit={submit}
                onClick={(e) => e.stopPropagation()}
                className="bg-white dark:bg-stone-900 rounded-xl border border-stone-200 dark:border-stone-700 shadow-2xl w-full max-w-2xl mx-4 max-h-[90vh] overflow-auto"
            >
                <div className="px-5 py-3 border-b border-stone-200 dark:border-stone-800 flex items-center justify-between">
                    <h2 className="font-semibold text-stone-900 dark:text-stone-100">
                        {isEdit ? "Edit source" : "New source"}
                    </h2>
                    <button
                        type="button"
                        onClick={onClose}
                        className="text-stone-400 hover:text-stone-700 dark:hover:text-stone-200"
                    >
                        ✕
                    </button>
                </div>

                <div className="p-5 space-y-4">
                    {err && <ErrorBanner error={err} />}
                    <div>
                        <FieldLabel help="A label for this source in the list — purely cosmetic.">
                            Name
                        </FieldLabel>
                        <input
                            ref={firstRef}
                            className={FIELD}
                            value={name}
                            onChange={(e) => setName(e.target.value)}
                            placeholder="prod app logs"
                        />
                    </div>

                    <div className="grid grid-cols-2 gap-4">
                        <div>
                            <FieldLabel
                                help={`Workspace the events land in. Defaults to your active env${
                                    source ? "" : ` (${getEnv()})`
                                }.`}
                            >
                                Environment
                            </FieldLabel>
                            <select
                                className={FIELD}
                                value={env}
                                onChange={(e) => setEnvVal(e.target.value)}
                            >
                                {envs.map((n) => (
                                    <option key={n} value={n}>
                                        {n}
                                    </option>
                                ))}
                            </select>
                        </div>
                        <div>
                            <FieldLabel help="Where the logs live: the server's own disk/mounts, or an S3 bucket (creds from the server's AWS environment).">
                                Kind
                            </FieldLabel>
                            <select
                                className={FIELD}
                                value={kind}
                                onChange={(e) => setKind(e.target.value)}
                            >
                                <option value="fs">Filesystem (local / mounted)</option>
                                <option value="s3">S3 bucket</option>
                            </select>
                        </div>
                    </div>

                    <div>
                        <FieldLabel help="Storage partition (queried as index:app). Use {{ field }} to route per-event by a parsed field, e.g. app-{{ host }}.">
                            Index
                        </FieldLabel>
                        <input
                            className={FIELD}
                            value={index}
                            onChange={(e) => setIndex(e.target.value)}
                            placeholder="app  or  app-{{ host }}"
                        />
                    </div>

                    <div>
                        <div className="flex items-center justify-between">
                            <FieldLabel
                                help={
                                    kind === "s3"
                                        ? "An s3://bucket/prefix/ URL. Glob wildcards work in the key: ** spans sub-prefixes, * matches within one level — e.g. s3://logs/app/**/*.gz."
                                        : "A file path or glob: ** matches any depth of sub-folders, * any filename chars, {a,b} alternates — e.g. /var/log/**/*.log. Point at a folder with Browse, then append a pattern."
                                }
                            >
                                {kind === "s3" ? "S3 URL + glob" : "Path glob"}
                            </FieldLabel>
                            {kind === "fs" && (
                                <button
                                    type="button"
                                    onClick={() => setBrowsing(true)}
                                    className="inline-flex items-center gap-1 text-xs text-orange-700 dark:text-orange-300 hover:underline"
                                >
                                    <FolderOpen className="w-3.5 h-3.5" />
                                    Browse folders…
                                </button>
                            )}
                        </div>
                        <input
                            className={`${FIELD} font-mono text-sm`}
                            value={path}
                            onChange={(e) => setPath(e.target.value)}
                            placeholder={pathPlaceholder}
                        />
                    </div>

                    <div className="grid grid-cols-3 gap-4">
                        <div>
                            <FieldLabel help="How to parse each record. auto sniffs JSON vs. plain text.">
                                Format
                            </FieldLabel>
                            <select
                                className={FIELD}
                                value={format}
                                onChange={(e) => setFormat(e.target.value)}
                            >
                                {[
                                    "auto",
                                    "ndjson",
                                    "json",
                                    "text",
                                    "syslog",
                                    "logfmt",
                                    "csv",
                                    "grok",
                                    "cef",
                                    "w3c",
                                ].map((f) => (
                                    <option key={f} value={f}>
                                        {f}
                                    </option>
                                ))}
                            </select>
                        </div>
                        <div>
                            <FieldLabel help="auto uses the file extension. Compressed files are read whole, not tailed.">
                                Compression
                            </FieldLabel>
                            <select
                                className={FIELD}
                                value={compression}
                                onChange={(e) => setCompression(e.target.value)}
                            >
                                {["auto", "none", "gzip", "zstd"].map((c) => (
                                    <option key={c} value={c}>
                                        {c}
                                    </option>
                                ))}
                            </select>
                        </div>
                        <div>
                            <FieldLabel help="How often to re-scan for new bytes. Floor 5s.">
                                Poll interval (s)
                            </FieldLabel>
                            <input
                                className={FIELD}
                                type="number"
                                min={5}
                                value={interval}
                                onChange={(e) => setInterval(e.target.value)}
                            />
                        </div>
                    </div>

                    <div>
                        <FieldLabel help="Paths matching any of these globs are skipped — same wildcard rules as the path glob.">
                            Exclude globs (one per line)
                        </FieldLabel>
                        <textarea
                            className={`${FIELD} font-mono text-sm h-20`}
                            value={exclude}
                            onChange={(e) => setExclude(e.target.value)}
                            placeholder={"**/debug/*\n**/*.tmp"}
                        />
                    </div>

                    {format === "grok" && (
                        <div>
                            <FieldLabel help="Presets: nginx_access, apache_combined, apache_common, log4j, cri. A capture named timestamp/message is used as the event's time/message.">
                                Grok pattern (preset name or %{"{...}"} pattern)
                            </FieldLabel>
                            <input
                                className={`${FIELD} font-mono text-sm`}
                                value={grok}
                                onChange={(e) => setGrok(e.target.value)}
                                placeholder="nginx_access  ·  log4j  ·  cri  ·  %{IP:client} %{WORD:verb} ..."
                            />
                        </div>
                    )}

                    <div className="grid grid-cols-2 gap-4">
                        <div>
                            <FieldLabel help="Lines not matching this regex are joined onto the previous event — for stack traces that span lines.">
                                Multiline start regex (optional)
                            </FieldLabel>
                            <input
                                className={`${FIELD} font-mono text-sm`}
                                value={multiline}
                                onChange={(e) => setMultiline(e.target.value)}
                                placeholder="^\\d{4}-\\d{2}-\\d{2}"
                            />
                        </div>
                        <div>
                            <FieldLabel help="Value stamped on each event's source field. Defaults to the file path.">
                                Source tag (optional)
                            </FieldLabel>
                            <input
                                className={FIELD}
                                value={sourceTag}
                                onChange={(e) => setSourceTag(e.target.value)}
                                placeholder="defaults to the file path"
                            />
                        </div>
                    </div>

                    <label className="flex items-center gap-2 text-stone-700 dark:text-stone-300">
                        <input
                            type="checkbox"
                            checked={enabled}
                            onChange={(e) => setEnabled(e.target.checked)}
                            className="accent-orange-600"
                        />
                        Enabled
                        <span
                            title="When off, the supervisor skips this source — no polling, no ingest."
                            aria-label="When off, the supervisor skips this source — no polling, no ingest."
                            className="cursor-help text-stone-600 hover:text-stone-900 dark:text-stone-400 dark:hover:text-stone-200"
                        >
                            <Info className="w-3.5 h-3.5" />
                        </span>
                    </label>
                </div>

                <div className="px-5 py-3 border-t border-stone-200 dark:border-stone-800 flex items-center justify-end gap-2">
                    <button
                        type="button"
                        onClick={onClose}
                        className="px-3 py-1.5 rounded-md border border-stone-300 dark:border-stone-600 text-stone-700 dark:text-stone-200 hover:bg-stone-100 dark:hover:bg-stone-800 transition"
                    >
                        Cancel
                    </button>
                    <button
                        type="submit"
                        disabled={saving}
                        className="px-3 py-1.5 rounded-md bg-orange-600 hover:bg-orange-500 text-white font-medium disabled:opacity-50 transition"
                    >
                        {saving ? "Saving…" : isEdit ? "Save changes" : "Create"}
                    </button>
                </div>
            </form>

            {browsing && (
                <FolderPicker
                    start={deriveStartDir(path)}
                    onClose={() => setBrowsing(false)}
                    onPick={(dir) => {
                        const base = dir.replace(/\/+$/, "");
                        setPath(`${base}/**/*.log`);
                        setBrowsing(false);
                    }}
                />
            )}
        </div>
    );
}

// Strip a trailing glob from a path so the picker opens near where the user
// already pointed (`/var/log/**/*.log` → `/var/log`).
function deriveStartDir(path: string): string | undefined {
    const trimmed = path.trim();
    if (!trimmed || trimmed.startsWith("s3://")) return undefined;
    const globIdx = trimmed.search(/[*?{[]/);
    const head = globIdx === -1 ? trimmed : trimmed.slice(0, globIdx);
    const lastSlash = head.lastIndexOf("/");
    return lastSlash > 0 ? head.slice(0, lastSlash) : "/";
}

// Server-side folder browser. Drills through directories and hands back the
// chosen folder; the dialog appends a default glob to it.
function FolderPicker({
    start,
    onClose,
    onPick,
}: {
    start?: string;
    onClose: () => void;
    onPick: (dir: string) => void;
}) {
    const [data, setData] = useState<BrowseResult | null>(null);
    const [err, setErr] = useState<string | null>(null);
    const [loading, setLoading] = useState(false);

    function go(path?: string) {
        setLoading(true);
        setErr(null);
        browseDir(path)
            .then((d) => setData(d))
            .catch((e) => setErr(e instanceof Error ? e.message : String(e)))
            .finally(() => setLoading(false));
    }

    useEffect(() => {
        go(start);
        // eslint-disable-next-line react-hooks/exhaustive-deps
    }, []);

    return (
        <div
            className="fixed inset-0 z-[60] flex items-center justify-center bg-stone-900/50 dark:bg-black/60"
            onClick={onClose}
        >
            <div
                onClick={(e) => e.stopPropagation()}
                className="bg-white dark:bg-stone-900 rounded-xl border border-stone-200 dark:border-stone-700 shadow-2xl w-full max-w-lg mx-4 max-h-[80vh] flex flex-col"
            >
                <div className="px-5 py-3 border-b border-stone-200 dark:border-stone-800 flex items-center justify-between">
                    <h3 className="font-semibold text-stone-900 dark:text-stone-100">
                        Browse server folders
                    </h3>
                    <button
                        type="button"
                        onClick={onClose}
                        className="text-stone-400 hover:text-stone-700 dark:hover:text-stone-200"
                    >
                        ✕
                    </button>
                </div>
                <div className="px-5 py-2 border-b border-stone-100 dark:border-stone-800 flex items-center gap-2">
                    <button
                        type="button"
                        disabled={!data?.parent || loading}
                        onClick={() => data?.parent && go(data.parent)}
                        className="px-2 py-1 rounded-md border border-stone-200 dark:border-stone-700 text-stone-600 dark:text-stone-300 hover:bg-stone-100 dark:hover:bg-stone-800 disabled:opacity-40 transition"
                        title="Up one level"
                    >
                        ↑ Up
                    </button>
                    <code className="font-mono text-xs text-stone-600 dark:text-stone-300 break-all flex-1">
                        {data?.path ?? "…"}
                    </code>
                </div>
                <div className="flex-1 overflow-y-auto min-h-[8rem]">
                    {err && (
                        <div className="p-4">
                            <ErrorBanner error={err} />
                        </div>
                    )}
                    {loading && !data ? (
                        <div className="p-4 text-stone-700 dark:text-stone-300">loading…</div>
                    ) : data && data.dirs.length === 0 ? (
                        <div className="p-4 text-stone-700 dark:text-stone-300 italic">
                            No sub-folders here.
                        </div>
                    ) : (
                        <ul className="divide-y divide-stone-100 dark:divide-stone-800">
                            {data?.dirs.map((d) => (
                                <li key={d.path}>
                                    <button
                                        type="button"
                                        onClick={() => go(d.path)}
                                        className="w-full text-left px-5 py-2 flex items-center gap-2 text-stone-700 dark:text-stone-300 hover:bg-stone-50 dark:hover:bg-stone-800/40 transition"
                                    >
                                        <FolderOpen className="w-4 h-4 text-stone-500 dark:text-stone-400 flex-shrink-0" />
                                        <span className="font-mono text-sm truncate">{d.name}</span>
                                    </button>
                                </li>
                            ))}
                        </ul>
                    )}
                </div>
                <div className="px-5 py-3 border-t border-stone-200 dark:border-stone-800 flex items-center justify-between gap-2">
                    <span className="text-xs text-stone-700 dark:text-stone-300">
                        Picks this folder + <code>/**/*.log</code> — tweak the glob after.
                    </span>
                    <button
                        type="button"
                        disabled={!data}
                        onClick={() => data && onPick(data.path)}
                        className="px-3 py-1.5 rounded-md bg-orange-600 hover:bg-orange-500 text-white font-medium disabled:opacity-50 transition"
                    >
                        Use this folder
                    </button>
                </div>
            </div>
        </div>
    );
}

function fmtInterval(secs: number): string {
    if (secs % 3600 === 0) return `${secs / 3600}h`;
    if (secs % 60 === 0) return `${secs / 60}m`;
    return `${secs}s`;
}

function fmtAgo(ms: number): string {
    const d = Date.now() - ms;
    if (d < 0) return "just now";
    if (d < 60_000) return "just now";
    if (d < 3_600_000) return `${Math.floor(d / 60_000)}m ago`;
    if (d < 86_400_000) return `${Math.floor(d / 3_600_000)}h ago`;
    return `${Math.floor(d / 86_400_000)}d ago`;
}
