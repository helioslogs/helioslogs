// Copyright 2026 Appbird LLC. Licensed under "GNU Affero General Public License v3.0"

import {
    useCallback,
    useEffect,
    useLayoutEffect,
    useMemo,
    useRef,
    useState,
    type Ref,
} from "react";
import { Link, useNavigate, useParams, useSearchParams } from "react-router-dom";
import { GridLayout, useContainerWidth, verticalCompactor, type Layout } from "react-grid-layout";
import "react-grid-layout/css/styles.css";
import { ArrowLeft, Check, Globe, GripVertical, Loader2, Lock, Pencil, Plus } from "lucide-react";
import {
    createDashboard,
    discoverFields,
    getDashboard,
    getEnv,
    getIndexes,
    updateDashboard,
} from "../api/client";
import type { Dashboard, DashboardSpec, DiscoveredField, Widget, WidgetKind } from "../api/types";
import { TimeRangePicker } from "../components/TimeRangePicker";
import { RefreshIntervalPicker } from "../components/RefreshIntervalPicker";
import { resolveRefreshSecs, type RefreshSetting } from "../lib/autoRefresh";
import { useAutoRefresh } from "../state/useAutoRefresh";
import { WidgetView } from "../components/widgets/WidgetView";
import { WidgetEditor } from "../components/widgets/WidgetEditor";
import { colorAt, newWidgetId, specRange } from "../components/widgets/util";
import { ErrorBanner } from "../components/admin";

const COLS = 12;

// Sensible default footprint per widget kind (grid units).
function defaultSize(kind: WidgetKind): { w: number; h: number } {
    switch (kind) {
        case "timeseries":
            return { w: 6, h: 6 };
        case "stat":
            return { w: 3, h: 4 };
        case "topn":
            return { w: 4, h: 6 };
        case "search_results":
            return { w: 8, h: 7 };
        default:
            return { w: 4, h: 6 }; // alerts / saved
    }
}

export function DashboardViewPage() {
    const { id = "" } = useParams();
    const [params, setParams] = useSearchParams();
    const navigate = useNavigate();

    // `/dashboards/new` is an in-memory draft: nothing is persisted until the
    // user hits Save (which validates the name and creates the dashboard).
    const isDraft = id === "new";

    const [dash, setDash] = useState<Dashboard | null>(() => (isDraft ? blankDraft() : null));
    const [spec, setSpec] = useState<DashboardSpec | null>(() =>
        isDraft ? { time_range: "-24h", widgets: [] } : null,
    );
    const [error, setError] = useState<string | null>(null);
    const [editing, setEditing] = useState(params.get("edit") === "1" || isDraft);
    const [refreshKey, setRefreshKey] = useState(0);
    const [saving, setSaving] = useState(false);
    const [nameError, setNameError] = useState<string | null>(null);
    const nameRef = useRef<HTMLInputElement>(null);
    const [editorFor, setEditorFor] = useState<{ widget: Widget; isNew: boolean } | null>(null);
    // Field catalog + indexes for the editor's query/field autocomplete.
    const [fields, setFields] = useState<DiscoveredField[]>([]);
    const [indexes, setIndexes] = useState<string[]>([]);

    // Reload when the active env changes (query widgets follow it).
    const env = getEnv();

    // react-grid-layout v2 needs an explicit width; this hook measures the
    // container (the modern replacement for WidthProvider).
    const { width, containerRef, mounted, measureWidth } = useContainerWidth();

    // The container only mounts after loading, so the hook's mount-time measure
    // misses it. Re-measure + observe once `spec` and the container exist.
    useLayoutEffect(() => {
        const node = containerRef.current;
        if (!spec || !node) return;
        measureWidth();
        const ro = new ResizeObserver(() => measureWidth());
        ro.observe(node);
        return () => ro.disconnect();
    }, [spec, measureWidth, containerRef]);

    // Editor autocomplete catalog, scoped to the dashboard window + env. Keyed on
    // window/env (not the whole spec) so widget edits don't refetch.
    useEffect(() => {
        if (!spec) return;
        const r = specRange(spec);
        let cancelled = false;
        discoverFields({ q: "*", start: r.start, end: r.end, top: 200 })
            .then((resp) => {
                if (!cancelled) setFields(resp.fields);
            })
            .catch(() => {});
        getIndexes()
            .then((ix) => {
                if (!cancelled) setIndexes(ix);
            })
            .catch(() => {});
        return () => {
            cancelled = true;
        };
        // eslint-disable-next-line react-hooks/exhaustive-deps
    }, [spec?.time_range, spec?.start, spec?.end, env, refreshKey]);

    useEffect(() => {
        if (isDraft) return; // draft is seeded in local state; nothing to fetch
        let cancelled = false;
        getDashboard(id)
            .then((d) => {
                if (cancelled) return;
                setDash(d);
                setSpec({
                    ...d.spec,
                    time_range: d.spec?.time_range ?? "-24h",
                    widgets: d.spec?.widgets ?? [],
                });
                setError(null);
            })
            .catch((e: unknown) => setError(e instanceof Error ? e.message : String(e)));
        return () => {
            cancelled = true;
        };
    }, [id, isDraft]);

    // Debounced persistence for layout drags; immediate for structural edits.
    const saveTimer = useRef<ReturnType<typeof setTimeout> | null>(null);
    const persist = useCallback(
        (next: DashboardSpec, immediate = false) => {
            if (isDraft) return; // a draft only writes on explicit Save
            if (saveTimer.current) clearTimeout(saveTimer.current);
            const run = () => {
                setSaving(true);
                updateDashboard(id, { spec: next })
                    .catch((e: unknown) => setError(e instanceof Error ? e.message : String(e)))
                    .finally(() => setSaving(false));
            };
            if (immediate) run();
            else saveTimer.current = setTimeout(run, 700);
        },
        [id, isDraft],
    );

    // Save a draft: require a name (force it if missing), then create + go to it.
    const saveDraft = useCallback(async () => {
        if (!dash || !spec) return;
        const name = dash.name.trim();
        if (!name) {
            setNameError("Enter a name to save");
            nameRef.current?.focus();
            return;
        }
        setSaving(true);
        try {
            const created = await createDashboard({ name, spec, public: dash.public });
            navigate(`/dashboards/${encodeURIComponent(created.id)}`, { replace: true });
        } catch (e: unknown) {
            setError(e instanceof Error ? e.message : String(e));
            setSaving(false);
        }
    }, [dash, spec, navigate]);

    const updateSpec = useCallback(
        (next: DashboardSpec, immediate = false) => {
            setSpec(next);
            persist(next, immediate);
        },
        [persist],
    );

    const layout: Layout = useMemo(
        () =>
            (spec?.widgets ?? []).map((w) => ({
                i: w.id,
                x: w.layout.x,
                y: w.layout.y,
                w: w.layout.w,
                h: w.layout.h,
                minW: 2,
                minH: 3,
            })),
        [spec?.widgets],
    );

    const onLayoutChange = useCallback(
        (next: Layout) => {
            if (!editing || !spec) return;
            const byId = new Map(next.map((l) => [l.i, l]));
            let changed = false;
            const widgets = spec.widgets.map((w) => {
                const l = byId.get(w.id);
                if (!l) return w;
                if (
                    l.x !== w.layout.x ||
                    l.y !== w.layout.y ||
                    l.w !== w.layout.w ||
                    l.h !== w.layout.h
                ) {
                    changed = true;
                    return { ...w, layout: { x: l.x, y: l.y, w: l.w, h: l.h } };
                }
                return w;
            });
            if (changed) updateSpec({ ...spec, widgets });
        },
        [editing, spec, updateSpec],
    );

    const nextY = () =>
        (spec?.widgets ?? []).reduce((m, w) => Math.max(m, w.layout.y + w.layout.h), 0);

    const openAdd = () => {
        const kind: WidgetKind = "timeseries";
        const size = defaultSize(kind);
        const widget: Widget = {
            id: newWidgetId("w"),
            kind,
            title: "",
            chart: "line",
            series: [{ id: newWidgetId("s"), label: "", query: "*", color: colorAt(0) }],
            layout: { x: 0, y: nextY(), w: size.w, h: size.h },
        };
        setEditorFor({ widget, isNew: true });
    };

    const saveWidget = (w: Widget) => {
        if (!spec) return;
        // Resize the footprint to the kind's default when the kind changed and the
        // widget is new-ish; otherwise keep the user's current placement.
        const exists = spec.widgets.some((x) => x.id === w.id);
        const widgets = exists
            ? spec.widgets.map((x) => (x.id === w.id ? w : x))
            : [...spec.widgets, { ...w, layout: { ...w.layout, ...sizeFor(w) } }];
        updateSpec({ ...spec, widgets }, true);
        setEditorFor(null);
    };

    const deleteWidget = (wid: string) => {
        if (!spec) return;
        updateSpec({ ...spec, widgets: spec.widgets.filter((w) => w.id !== wid) }, true);
    };

    const onPickRange = (next: { range?: string; start?: string; end?: string }) => {
        if (!spec) return;
        const updated: DashboardSpec =
            next.start && next.end
                ? { ...spec, start: next.start, end: next.end }
                : {
                      ...spec,
                      time_range: next.range ?? spec.time_range,
                      start: undefined,
                      end: undefined,
                  };
        updateSpec(updated, true);
        setRefreshKey((k) => k + 1);
    };

    const toggleEdit = () => {
        const nextEditing = !editing;
        setEditing(nextEditing);
        const p = new URLSearchParams(params);
        if (nextEditing) p.set("edit", "1");
        else p.delete("edit");
        setParams(p, { replace: true });
    };

    const toggleVisibility = () => {
        if (!dash) return;
        const next = !dash.public;
        if (isDraft) {
            setDash({ ...dash, public: next });
            return;
        }
        updateDashboard(id, { public: next })
            .then(setDash)
            .catch((e: unknown) => setError(e instanceof Error ? e.message : String(e)));
    };

    // Auto-refresh. `refresh_secs` undefined = Auto (a smart default scaled to
    // the range); 0 = off; >0 = explicit. Absolute ranges resolve to off.
    const refreshSetting: RefreshSetting =
        spec?.refresh_secs === undefined ? "auto" : spec.refresh_secs;
    const refreshSecs = resolveRefreshSecs(refreshSetting, {
        range: spec?.time_range,
        hasAbsolute: !!(spec?.start && spec?.end),
    });
    const refreshDisabled = !!(spec?.start && spec?.end);
    useAutoRefresh(refreshSecs, () => setRefreshKey((k) => k + 1), editing);

    if (error && !spec) {
        return (
            <div className="max-w-5xl mx-auto px-6 py-8 space-y-4">
                <Link
                    to="/dashboards"
                    className="inline-flex items-center gap-1 text-sm text-stone-500 hover:text-stone-800 dark:hover:text-stone-200"
                >
                    <ArrowLeft className="w-4 h-4" /> Dashboards
                </Link>
                <ErrorBanner error={error} />
            </div>
        );
    }
    if (!dash || !spec) {
        return (
            <div className="flex items-center justify-center gap-2 text-stone-500 dark:text-stone-400 py-20">
                <Loader2 className="w-4 h-4 animate-spin" /> loading…
            </div>
        );
    }

    const range = specRange(spec);

    return (
        <div className="h-full flex flex-col">
            {/* Header */}
            <div className="flex items-center gap-3 px-5 py-3 border-b border-stone-200 dark:border-stone-800 bg-white dark:bg-stone-900 flex-wrap">
                <button
                    type="button"
                    onClick={() => {
                        if (
                            isDraft &&
                            (dash.name.trim() || spec.widgets.length > 0) &&
                            !window.confirm("Discard this unsaved dashboard?")
                        )
                            return;
                        navigate("/dashboards");
                    }}
                    title="back to dashboards"
                    className="p-1.5 rounded-md text-stone-500 hover:text-stone-800 dark:hover:text-stone-200 hover:bg-stone-100 dark:hover:bg-stone-800"
                >
                    <ArrowLeft className="w-4 h-4" />
                </button>
                {editing ? (
                    isDraft ? (
                        <input
                            ref={nameRef}
                            value={dash.name}
                            autoFocus
                            onChange={(e) => {
                                setDash({ ...dash, name: e.target.value });
                                if (nameError) setNameError(null);
                            }}
                            placeholder="Dashboard name"
                            className={`text-lg font-semibold bg-transparent border-b border-dashed focus:outline-none text-stone-900 dark:text-stone-100 ${
                                nameError
                                    ? "border-red-500 focus:border-red-500 placeholder-red-400"
                                    : "border-stone-300 dark:border-stone-600 focus:border-orange-500"
                            }`}
                        />
                    ) : (
                        <input
                            defaultValue={dash.name}
                            onBlur={(e) => {
                                const name = e.target.value.trim();
                                if (name && name !== dash.name) {
                                    updateDashboard(id, { name })
                                        .then((d) => setDash(d))
                                        .catch(() => {});
                                }
                            }}
                            className="text-lg font-semibold bg-transparent border-b border-dashed border-stone-300 dark:border-stone-600 focus:outline-none focus:border-orange-500 text-stone-900 dark:text-stone-100"
                        />
                    )
                ) : (
                    <h1 className="text-lg font-semibold text-stone-900 dark:text-stone-100 truncate">
                        {dash.name}
                    </h1>
                )}
                {isDraft && nameError && (
                    <span className="text-xs font-medium text-red-600 dark:text-red-400">
                        {nameError}
                    </span>
                )}
                <span className="text-xs text-stone-900 dark:text-stone-100 px-1.5 py-0.5 rounded bg-stone-100 dark:bg-stone-800">
                    env: {env}
                </span>

                <button
                    type="button"
                    onClick={toggleVisibility}
                    title={
                        dash.public
                            ? "Public — visible to all users. Click to make private."
                            : "Private — only you. Click to make public."
                    }
                    className={`inline-flex items-center gap-1 px-1.5 py-0.5 rounded border text-xs transition ${
                        dash.public
                            ? "border-emerald-200 bg-emerald-50 text-emerald-700 dark:border-emerald-900 dark:bg-emerald-950/40 dark:text-emerald-300 hover:border-emerald-300"
                            : "border-stone-200 bg-stone-50 text-stone-700 dark:border-stone-700 dark:bg-stone-800/60 dark:text-stone-200 hover:border-orange-300"
                    }`}
                >
                    {dash.public ? <Globe className="w-3 h-3" /> : <Lock className="w-3 h-3" />}
                    {dash.public ? "Public" : "Private"}
                </button>

                <div className="flex-grow" />

                {isDraft ? (
                    <span className="text-xs text-amber-600 dark:text-amber-400 inline-flex items-center gap-1">
                        unsaved draft
                    </span>
                ) : saving ? (
                    <span className="text-xs text-stone-700 dark:text-stone-300 inline-flex items-center gap-1">
                        <Loader2 className="w-3 h-3 animate-spin" /> saving
                    </span>
                ) : (
                    editing && (
                        <span className="text-xs text-stone-700 dark:text-stone-300 inline-flex items-center gap-1">
                            <Check className="w-3 h-3" /> saved
                        </span>
                    )
                )}

                <TimeRangePicker
                    range={spec.time_range}
                    start={spec.start}
                    end={spec.end}
                    onChange={onPickRange}
                />

                <RefreshIntervalPicker
                    onRefresh={() => setRefreshKey((k) => k + 1)}
                    setting={refreshSetting}
                    onChange={(s) =>
                        updateSpec({ ...spec, refresh_secs: s === "auto" ? undefined : s }, true)
                    }
                    effectiveSecs={refreshSecs}
                    disabled={refreshDisabled || editing}
                    disabledReason={
                        editing
                            ? "Auto-refresh is paused while editing"
                            : "Auto-refresh applies to relative ranges"
                    }
                />

                {editing && (
                    <button
                        type="button"
                        onClick={openAdd}
                        className="inline-flex items-center gap-1.5 px-3 py-1.5 font-medium rounded-md bg-orange-600 hover:bg-orange-500 text-white transition"
                    >
                        <Plus className="w-4 h-4" /> Add widget
                    </button>
                )}
                {isDraft ? (
                    <button
                        type="button"
                        onClick={saveDraft}
                        disabled={saving}
                        className="inline-flex items-center gap-1.5 px-3 py-1.5 font-medium rounded-md bg-orange-600 hover:bg-orange-500 text-white transition disabled:opacity-60"
                    >
                        {saving ? (
                            <Loader2 className="w-4 h-4 animate-spin" />
                        ) : (
                            <Check className="w-4 h-4" />
                        )}
                        Save dashboard
                    </button>
                ) : (
                    <button
                        type="button"
                        onClick={toggleEdit}
                        className={`inline-flex items-center gap-1.5 px-3 py-1.5 font-medium rounded-md transition ${
                            editing
                                ? "bg-stone-900 hover:bg-stone-800 dark:bg-stone-700 dark:hover:bg-stone-600 text-white"
                                : "border border-stone-200 dark:border-stone-700 text-stone-700 dark:text-stone-200 hover:bg-stone-100 dark:hover:bg-stone-800"
                        }`}
                    >
                        {editing ? <Check className="w-4 h-4" /> : <Pencil className="w-4 h-4" />}
                        {editing ? "Done" : "Edit"}
                    </button>
                )}
            </div>

            {/* Grid */}
            <div className="flex-grow overflow-auto bg-stone-50 dark:bg-stone-950 p-3">
                {error && <ErrorBanner error={error} />}
                {editing && spec.widgets.length > 0 && (
                    <div className="mb-3 flex items-center gap-2 px-3 py-1.5 rounded-lg text-sm bg-orange-50/70 dark:bg-orange-950/20 border border-orange-200/70 dark:border-orange-900/40 text-stone-700 dark:text-stone-200">
                        <GripVertical className="w-4 h-4 text-orange-500 shrink-0" />
                        <span>
                            Drag a widget to move it, drag its bottom-right corner to resize, and
                            click the <Pencil className="inline w-3.5 h-3.5 mb-0.5" /> icon to set
                            its queries. Changes save automatically.
                        </span>
                    </div>
                )}
                {spec.widgets.length === 0 ? (
                    <div className="text-center py-20 text-stone-500 dark:text-stone-400">
                        <p className="mb-3">This dashboard is empty.</p>
                        <button
                            type="button"
                            onClick={() => {
                                if (!editing) toggleEdit();
                                openAdd();
                            }}
                            className="inline-flex items-center gap-1.5 px-3 py-1.5 font-medium rounded-md bg-orange-600 hover:bg-orange-500 text-white"
                        >
                            <Plus className="w-4 h-4" /> Add your first widget
                        </button>
                    </div>
                ) : (
                    <div ref={containerRef as Ref<HTMLDivElement>}>
                        {mounted && (
                            <GridLayout
                                width={width}
                                layout={layout}
                                onLayoutChange={onLayoutChange}
                                gridConfig={{ cols: COLS, rowHeight: 48, margin: [12, 12] }}
                                // Whole card is the handle in edit mode; the action buttons and
                                // the resize grip are excluded so they don't start a move.
                                dragConfig={{
                                    enabled: editing,
                                    cancel: ".widget-no-drag,.react-resizable-handle",
                                }}
                                resizeConfig={{ enabled: editing }}
                                compactor={verticalCompactor}
                            >
                                {spec.widgets.map((w) => (
                                    <div key={w.id}>
                                        <WidgetView
                                            widget={w}
                                            range={range}
                                            refreshKey={refreshKey}
                                            editing={editing}
                                            onEdit={() => setEditorFor({ widget: w, isNew: false })}
                                            onDelete={() => deleteWidget(w.id)}
                                        />
                                    </div>
                                ))}
                            </GridLayout>
                        )}
                    </div>
                )}
            </div>

            {editorFor && (
                <WidgetEditor
                    initial={editorFor.widget}
                    isNew={editorFor.isNew}
                    onSave={saveWidget}
                    onCancel={() => setEditorFor(null)}
                    fields={fields}
                    indexes={indexes}
                    range={range}
                />
            )}
        </div>
    );
}

// On a brand-new widget, snap its footprint to the kind default so a stat
// isn't the size of a chart. (Existing widgets keep their placement.)
function sizeFor(w: Widget): { w: number; h: number } {
    return defaultSize(w.kind);
}

// Blank in-memory dashboard for the `/dashboards/new` draft flow — public by
// default, like the create form was. Persisted only when the user hits Save.
function blankDraft(): Dashboard {
    return {
        id: "",
        name: "",
        description: "",
        spec: { time_range: "-24h", widgets: [] },
        public: true,
        created_at: "",
        updated_at: "",
    };
}
