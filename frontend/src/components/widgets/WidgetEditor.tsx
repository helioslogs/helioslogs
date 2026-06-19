// Copyright 2026 Appbird LLC. Licensed under "GNU Affero General Public License v3.0"

import { useState } from "react";
import { Plus, Trash2, X } from "lucide-react";
import type { DiscoveredField, Series, Widget, WidgetKind } from "../../api/types";
import { TimeRangePicker } from "../TimeRangePicker";
import { QueryInput } from "./QueryInput";
import { FieldNameInput } from "./FieldNameInput";
import { colorAt, newWidgetId, type DashRange } from "./util";

interface Props {
    // The widget being edited (or a fresh blank for "add").
    initial: Widget;
    isNew: boolean;
    onSave: (w: Widget) => void;
    onCancel: () => void;
    // Discovered fields + indexes for query/field autocomplete.
    fields: DiscoveredField[];
    indexes: string[];
    // Dashboard time window — scopes autocomplete value lookups and seeds the
    // per-widget override picker.
    range: DashRange;
}

const KINDS: { kind: WidgetKind; label: string; hint: string }[] = [
    {
        kind: "timeseries",
        label: "Time series",
        hint: "match counts over time, one line per query",
    },
    { kind: "stat", label: "Stat", hint: "big number — total matches per query" },
    { kind: "topn", label: "Top values", hint: "breakdown of a field by match count" },
    { kind: "search_results", label: "Search results", hint: "table of latest matching events" },
    { kind: "alerts", label: "Alerts", hint: "alert inbox with acknowledge / investigate" },
    { kind: "saved_searches", label: "Saved searches", hint: "saved-search shortcuts" },
];

const usesSeries = (k: WidgetKind) => k === "timeseries" || k === "stat";
const usesBaseQuery = (k: WidgetKind) => k === "topn" || k === "search_results";
const usesField = (k: WidgetKind) => k === "topn";
const usesLimit = (k: WidgetKind) =>
    k === "alerts" || k === "saved_searches" || k === "search_results";
const usesTime = (k: WidgetKind) =>
    k === "timeseries" || k === "stat" || k === "topn" || k === "search_results";

const inputBase =
    "px-2.5 py-1.5 bg-white dark:bg-stone-900 border border-stone-200 dark:border-stone-700 rounded-md focus:outline-none focus:border-orange-500 text-sm";
const input = `w-full ${inputBase}`;
const label =
    "block text-xs font-semibold uppercase tracking-wider text-stone-900 dark:text-stone-100 mb-1.5";

export function WidgetEditor({ initial, isNew, onSave, onCancel, fields, indexes, range }: Props) {
    const [kind, setKind] = useState<WidgetKind>(initial.kind);
    const [title, setTitle] = useState(initial.title);
    const [chart, setChart] = useState(initial.chart ?? "line");
    const [series, setSeries] = useState<Series[]>(
        initial.series && initial.series.length > 0
            ? initial.series
            : [{ id: newWidgetId("s"), label: "", query: "*", color: colorAt(0) }],
    );
    const [field, setField] = useState(initial.field ?? "");
    const [size, setSize] = useState(initial.size ?? 10);
    const [limit, setLimit] = useState(
        initial.limit ?? (initial.kind === "search_results" ? 20 : 10),
    );
    const [timeOverride, setTimeOverride] = useState(!!initial.time);
    const [time, setTime] = useState<{ range?: string; start?: string; end?: string }>(
        initial.time ?? { range: range.range ?? "-24h" },
    );

    const addSeries = () =>
        setSeries((xs) => [
            ...xs,
            { id: newWidgetId("s"), label: "", query: "*", color: colorAt(xs.length) },
        ]);
    const updateSeries = (i: number, patch: Partial<Series>) =>
        setSeries((xs) => xs.map((s, j) => (j === i ? { ...s, ...patch } : s)));
    const removeSeries = (i: number) => setSeries((xs) => xs.filter((_, j) => j !== i));

    const save = () => {
        const w: Widget = {
            ...initial,
            kind,
            title: title.trim() || defaultTitle(kind),
            layout: initial.layout,
            series: undefined,
            chart: undefined,
            field: undefined,
            size: undefined,
            limit: undefined,
            time: undefined,
        };
        if (usesSeries(kind)) {
            w.series = series.map((s) => ({ ...s, query: s.query.trim() || "*" }));
            if (kind === "timeseries") w.chart = chart;
        }
        if (usesBaseQuery(kind)) {
            const base = series[0] ?? {
                id: newWidgetId("s"),
                label: "",
                query: "*",
                color: colorAt(0),
            };
            w.series = [{ ...base, query: base.query.trim() || "*" }];
        }
        if (usesField(kind)) {
            w.field = field.trim();
            w.size = size;
        }
        if (usesLimit(kind)) w.limit = limit;
        if (usesTime(kind) && timeOverride) {
            w.time =
                time.start && time.end
                    ? { start: time.start, end: time.end }
                    : { range: time.range };
        }
        onSave(w);
    };

    return (
        <div
            className="fixed inset-0 z-50 flex items-center justify-center bg-black/40 p-4"
            onClick={onCancel}
        >
            <div
                className="w-full max-w-lg max-h-[85vh] overflow-auto rounded-xl bg-white dark:bg-stone-900 border border-stone-200 dark:border-stone-800 shadow-xl"
                onClick={(e) => e.stopPropagation()}
            >
                <div className="flex items-center justify-between px-4 py-3 border-b border-stone-200 dark:border-stone-800">
                    <h2 className="font-semibold text-stone-900 dark:text-stone-100">
                        {isNew ? "Add widget" : "Edit widget"}
                    </h2>
                    <button
                        type="button"
                        onClick={onCancel}
                        className="p-1 rounded hover:bg-stone-100 dark:hover:bg-stone-800"
                    >
                        <X className="w-4 h-4 text-stone-500" />
                    </button>
                </div>

                <div className="p-4 space-y-4">
                    <div>
                        <span className={label}>Type</span>
                        <div className="grid grid-cols-2 gap-1.5">
                            {KINDS.map((k) => (
                                <button
                                    key={k.kind}
                                    type="button"
                                    onClick={() => setKind(k.kind)}
                                    title={k.hint}
                                    className={`text-left px-2.5 py-1.5 rounded-md border text-sm transition ${
                                        kind === k.kind
                                            ? "border-orange-500 bg-orange-50 dark:bg-orange-950/40 text-stone-900 dark:text-stone-100"
                                            : "border-stone-200 dark:border-stone-700 hover:bg-stone-50 dark:hover:bg-stone-800 text-stone-700 dark:text-stone-300"
                                    }`}
                                >
                                    {k.label}
                                </button>
                            ))}
                        </div>
                    </div>

                    <div>
                        <span className={label}>Title</span>
                        <input
                            value={title}
                            onChange={(e) => setTitle(e.target.value)}
                            placeholder={defaultTitle(kind)}
                            className={input}
                        />
                    </div>

                    {kind === "timeseries" && (
                        <div>
                            <span className={label}>Chart</span>
                            <div className="flex gap-1.5">
                                {(["line", "area", "bar"] as const).map((c) => (
                                    <button
                                        key={c}
                                        type="button"
                                        onClick={() => setChart(c)}
                                        className={`px-3 py-1 rounded-md border text-sm capitalize transition ${
                                            chart === c
                                                ? "border-orange-500 bg-orange-50 dark:bg-orange-950/40 text-stone-900 dark:text-stone-100"
                                                : "border-stone-200 dark:border-stone-700 hover:bg-stone-50 dark:hover:bg-stone-800 text-stone-700 dark:text-stone-300"
                                        }`}
                                    >
                                        {c}
                                    </button>
                                ))}
                            </div>
                        </div>
                    )}

                    {usesSeries(kind) && (
                        <div>
                            <span className={label}>{kind === "stat" ? "Metrics" : "Series"}</span>
                            <div className="flex items-center gap-1.5 mb-1 text-[11px] font-semibold uppercase tracking-wider text-stone-900 dark:text-stone-100">
                                <span className="w-8 shrink-0">Color</span>
                                <span className="w-24 shrink-0">Label</span>
                                <span className="flex-1 min-w-0">Search query</span>
                                {series.length > 1 && <span className="w-7 shrink-0" />}
                            </div>
                            <div className="space-y-2">
                                {series.map((s, i) => (
                                    <div key={s.id} className="flex items-center gap-1.5">
                                        <input
                                            type="color"
                                            value={s.color || colorAt(i)}
                                            onChange={(e) =>
                                                updateSeries(i, { color: e.target.value })
                                            }
                                            className="w-8 h-8 shrink-0 rounded border border-stone-200 dark:border-stone-700 bg-transparent cursor-pointer"
                                            title="series color"
                                        />
                                        <input
                                            value={s.label}
                                            onChange={(e) =>
                                                updateSeries(i, { label: e.target.value })
                                            }
                                            placeholder="label"
                                            className={`${inputBase} w-24 shrink-0`}
                                        />
                                        <QueryInput
                                            value={s.query}
                                            onChange={(q) => updateSeries(i, { query: q })}
                                            fields={fields}
                                            indexes={indexes}
                                            start={range.start}
                                            end={range.end}
                                            placeholder="query, e.g. level:error"
                                            className={`${input} font-mono`}
                                        />
                                        {series.length > 1 && (
                                            <button
                                                type="button"
                                                onClick={() => removeSeries(i)}
                                                className="p-1.5 shrink-0 rounded text-stone-400 hover:text-red-600 hover:bg-red-50 dark:hover:bg-red-950"
                                            >
                                                <Trash2 className="w-4 h-4" />
                                            </button>
                                        )}
                                    </div>
                                ))}
                            </div>
                            <button
                                type="button"
                                onClick={addSeries}
                                className="mt-2 inline-flex items-center gap-1 text-sm text-orange-600 hover:text-orange-500"
                            >
                                <Plus className="w-3.5 h-3.5" /> add{" "}
                                {kind === "stat" ? "metric" : "series"}
                            </button>
                        </div>
                    )}

                    {usesBaseQuery(kind) && (
                        <div>
                            <span className={label}>Search query</span>
                            <div className="flex">
                                <QueryInput
                                    value={series[0]?.query ?? "*"}
                                    onChange={(q) => updateSeries(0, { query: q })}
                                    fields={fields}
                                    indexes={indexes}
                                    start={range.start}
                                    end={range.end}
                                    placeholder="*"
                                    className={`${input} font-mono`}
                                />
                            </div>
                        </div>
                    )}

                    {usesField(kind) && (
                        <div className="flex gap-3">
                            <div className="flex-1 min-w-0">
                                <span className={label}>Field</span>
                                <div className="flex">
                                    <FieldNameInput
                                        value={field}
                                        onChange={setField}
                                        fields={fields}
                                        placeholder="e.g. service"
                                        className={`${input} font-mono`}
                                    />
                                </div>
                            </div>
                            <div className="w-24">
                                <span className={label}>Top N</span>
                                <input
                                    type="number"
                                    min={1}
                                    max={50}
                                    value={size}
                                    onChange={(e) =>
                                        setSize(Math.max(1, Number(e.target.value) || 10))
                                    }
                                    className={input}
                                />
                            </div>
                        </div>
                    )}

                    {usesLimit(kind) && (
                        <div className="w-32">
                            <span className={label}>Max rows</span>
                            <input
                                type="number"
                                min={1}
                                max={100}
                                value={limit}
                                onChange={(e) =>
                                    setLimit(Math.max(1, Number(e.target.value) || 10))
                                }
                                className={input}
                            />
                        </div>
                    )}

                    {usesTime(kind) && (
                        <div>
                            <span className={label}>Time range</span>
                            <label className="flex items-center gap-2 text-sm text-stone-700 dark:text-stone-300 mb-2 cursor-pointer select-none">
                                <input
                                    type="checkbox"
                                    checked={timeOverride}
                                    onChange={(e) => setTimeOverride(e.target.checked)}
                                    className="rounded border-stone-300 text-orange-500 focus:ring-orange-500"
                                />
                                Override the dashboard time range
                            </label>
                            {timeOverride ? (
                                <TimeRangePicker
                                    range={time.range ?? "-24h"}
                                    start={time.start}
                                    end={time.end}
                                    onChange={(next) => {
                                        if (next.range !== undefined)
                                            setTime({ range: next.range });
                                        else if (next.start && next.end)
                                            setTime({ start: next.start, end: next.end });
                                    }}
                                />
                            ) : (
                                <p className="text-xs text-stone-500 dark:text-stone-400">
                                    Uses the dashboard’s time range.
                                </p>
                            )}
                        </div>
                    )}
                </div>

                <div className="flex justify-end gap-2 px-4 py-3 border-t border-stone-200 dark:border-stone-800">
                    <button
                        type="button"
                        onClick={onCancel}
                        className="px-3 py-1.5 rounded-md text-stone-600 dark:text-stone-300 hover:bg-stone-100 dark:hover:bg-stone-800 transition"
                    >
                        Cancel
                    </button>
                    <button
                        type="button"
                        onClick={save}
                        disabled={usesField(kind) && !field.trim()}
                        className="px-3 py-1.5 font-medium rounded-md bg-orange-600 hover:bg-orange-500 text-white disabled:opacity-50 transition"
                    >
                        {isNew ? "Add widget" : "Save"}
                    </button>
                </div>
            </div>
        </div>
    );
}

function defaultTitle(kind: WidgetKind): string {
    return KINDS.find((k) => k.kind === kind)?.label ?? "Widget";
}
