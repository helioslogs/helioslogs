// Copyright 2026 Appbird LLC. Licensed under "GNU Affero General Public License v3.0"

// Time range picker: relative-window presets plus an absolute
// `start`/`end` path and a freeform relative input.

import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { ChevronDown, Clock } from "lucide-react";
import { useTimezone } from "../state/timezone";
import { formatTooltipTs } from "../lib/timezone";

interface Props {
    // Current relative range (e.g. `-15m`). Always set — fallback when no
    // absolute bounds are present.
    range: string;
    // Current absolute bounds (ISO 8601). When both are present they take
    // precedence over `range` everywhere downstream.
    start?: string;
    end?: string;
    // Emits `range` (relative; caller clears start/end) or `start`+`end` (absolute).
    onChange: (next: { range?: string; start?: string; end?: string }) => void;
    // Following live pins a fixed 5-minute window, so the picker is locked and
    // shows "Last 5 min" regardless of the underlying range.
    disabled?: boolean;
}

const PRESETS: { label: string; range: string }[] = [
    { label: "Last 5 min", range: "-5m" },
    { label: "Last 15 min", range: "-15m" },
    { label: "Last 30 min", range: "-30m" },
    { label: "Last 1 hour", range: "-1h" },
    { label: "Last 3 hours", range: "-3h" },
    { label: "Last 6 hours", range: "-6h" },
    { label: "Last 12 hours", range: "-12h" },
    { label: "Last 24 hours", range: "-24h" },
    { label: "Last 2 days", range: "-2d" },
    { label: "Last 7 days", range: "-7d" },
    { label: "Last 14 days", range: "-14d" },
    { label: "Last 30 days", range: "-30d" },
];

// Day-anchored presets computed as absolute bounds so they stay fixed across midnight.
const DAY_PRESETS: { label: string; compute: () => { start: string; end: string } }[] = [
    {
        label: "Today",
        compute: () => {
            const now = new Date();
            const start = new Date(now);
            start.setHours(0, 0, 0, 0);
            return { start: start.toISOString(), end: now.toISOString() };
        },
    },
    {
        label: "Yesterday",
        compute: () => {
            const start = new Date();
            start.setDate(start.getDate() - 1);
            start.setHours(0, 0, 0, 0);
            const end = new Date(start);
            end.setHours(23, 59, 59, 999);
            return { start: start.toISOString(), end: end.toISOString() };
        },
    },
];

// Parses `-Nm/h/d/s/w` to canonical `-N<unit>` (weeks → days, since backend lacks `w`); null if bad.
function normalizeRelative(raw: string): string | null {
    const s = raw.trim().toLowerCase();
    if (!s) return null;
    const m = s.match(/^-?(\d+)\s*([smhdw])$/);
    if (!m) return null;
    const n = parseInt(m[1], 10);
    if (!Number.isFinite(n) || n <= 0) return null;
    let unit = m[2];
    let count = n;
    if (unit === "w") {
        unit = "d";
        count = n * 7;
    }
    return `-${count}${unit}`;
}

// ISO → `YYYY-MM-DDTHH:mm` in local tz for `<input type="datetime-local">`.
function isoToLocalInput(iso?: string): string {
    if (!iso) return "";
    const d = new Date(iso);
    if (Number.isNaN(d.getTime())) return "";
    const pad = (n: number) => String(n).padStart(2, "0");
    return (
        `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())}` +
        `T${pad(d.getHours())}:${pad(d.getMinutes())}`
    );
}

// Local datetime string → ISO 8601. `new Date("YYYY-MM-DDTHH:mm")` is
// interpreted as local time by the JS engine, which is what we want here.
function localInputToIso(s: string): string | null {
    if (!s) return null;
    const d = new Date(s);
    if (Number.isNaN(d.getTime())) return null;
    return d.toISOString();
}

// Index of the matching preset for the label/highlight; -1 in absolute mode.
function activePresetIndex(range: string, hasAbsolute: boolean): number {
    if (hasAbsolute) return -1;
    return PRESETS.findIndex((p) => p.range === range);
}

export function TimeRangePicker({ range, start, end, onChange, disabled = false }: Props) {
    const [open, setOpen] = useState(false);
    const [customRel, setCustomRel] = useState("");
    const [customRelError, setCustomRelError] = useState<string | null>(null);
    // Local draft state for the absolute inputs — we only emit on Apply,
    // not on every keystroke, so half-edited values don't fire a search.
    const [absStart, setAbsStart] = useState("");
    const [absEnd, setAbsEnd] = useState("");
    const [absError, setAbsError] = useState<string | null>(null);
    const tz = useTimezone();
    const wrapRef = useRef<HTMLDivElement>(null);

    const hasAbsolute = !!(start && end);
    const activeIdx = useMemo(() => activePresetIndex(range, hasAbsolute), [range, hasAbsolute]);

    // Sync draft fields on open so editing starts from the current absolute bounds.
    useEffect(() => {
        if (!open) return;
        setAbsStart(isoToLocalInput(start));
        setAbsEnd(isoToLocalInput(end));
        setCustomRel("");
        setCustomRelError(null);
        setAbsError(null);
    }, [open, start, end]);

    // Close on outside click / ESC. mousedown (not click) so the close fires
    // before any other onClick on the page steals focus first.
    useEffect(() => {
        if (!open) return;
        const onDown = (e: MouseEvent) => {
            if (!wrapRef.current) return;
            if (!wrapRef.current.contains(e.target as Node)) setOpen(false);
        };
        const onKey = (e: KeyboardEvent) => {
            if (e.key === "Escape") setOpen(false);
        };
        document.addEventListener("mousedown", onDown);
        document.addEventListener("keydown", onKey);
        return () => {
            document.removeEventListener("mousedown", onDown);
            document.removeEventListener("keydown", onKey);
        };
    }, [open]);

    const pickPreset = useCallback(
        (r: string) => {
            onChange({ range: r });
            setOpen(false);
        },
        [onChange],
    );

    const pickDayPreset = useCallback(
        (compute: () => { start: string; end: string }) => {
            const { start: s, end: e } = compute();
            onChange({ start: s, end: e });
            setOpen(false);
        },
        [onChange],
    );

    const applyCustomRelative = useCallback(() => {
        const norm = normalizeRelative(customRel);
        if (!norm) {
            setCustomRelError("Expected e.g. -3h, -2d, -45m");
            return;
        }
        onChange({ range: norm });
        setOpen(false);
    }, [customRel, onChange]);

    const applyAbsolute = useCallback(() => {
        const sIso = localInputToIso(absStart);
        const eIso = localInputToIso(absEnd);
        if (!sIso || !eIso) {
            setAbsError("Fill in both start and end");
            return;
        }
        if (new Date(sIso).getTime() >= new Date(eIso).getTime()) {
            setAbsError("Start must be before end");
            return;
        }
        onChange({ start: sIso, end: eIso });
        setOpen(false);
    }, [absStart, absEnd, onChange]);

    // Trigger label: absolute window if set, matched preset name otherwise,
    // or raw range string (e.g. `-45m`) when no preset matches.
    const triggerLabel = useMemo(() => {
        if (disabled) return "Last 5 min";
        if (hasAbsolute) {
            return `${formatTooltipTs(start, tz)}  →  ${formatTooltipTs(end, tz)}`;
        }
        if (activeIdx >= 0) return PRESETS[activeIdx].label;
        return range;
    }, [disabled, hasAbsolute, start, end, tz, activeIdx, range]);

    return (
        <div ref={wrapRef} className="relative">
            <button
                type="button"
                onClick={() => setOpen((v) => !v)}
                disabled={disabled}
                className="px-2.5 py-1.5 bg-stone-50 dark:bg-stone-950 border border-stone-200 dark:border-stone-700 rounded-md focus:outline-none focus:border-orange-500 focus:bg-white dark:focus:bg-stone-900 text-stone-900 dark:text-stone-100 flex items-center gap-1.5 min-w-[140px] disabled:opacity-60 disabled:cursor-not-allowed"
                title={
                    disabled
                        ? "Following live — fixed 5-minute window"
                        : hasAbsolute
                          ? "Absolute time range — click to change"
                          : "Time range — click to change"
                }
            >
                <Clock className="w-3.5 h-3.5 shrink-0 text-stone-400 dark:text-stone-500" />
                <span className="truncate text-left flex-grow">{triggerLabel}</span>
                <ChevronDown
                    className={`w-3.5 h-3.5 shrink-0 text-stone-400 dark:text-stone-500 transition-transform ${
                        open ? "rotate-180" : ""
                    }`}
                />
            </button>

            {open && (
                <div className="absolute right-0 z-30 mt-1 w-[520px] rounded-md border border-stone-200 dark:border-stone-700 bg-white dark:bg-stone-900 shadow-lg overflow-hidden">
                    <div className="grid grid-cols-2 divide-x divide-stone-200 dark:divide-stone-800">
                        {/* Left column: absolute + custom relative */}
                        <div className="px-3 py-2 space-y-3">
                            <div>
                                <div className="pt-1 pb-1.5 text-stone-500 dark:text-stone-400 uppercase tracking-wider font-semibold">
                                    Absolute range
                                </div>
                                <label className="block">
                                    <span className="block text-stone-600 dark:text-stone-400 mb-0.5">
                                        From
                                    </span>
                                    <input
                                        type="datetime-local"
                                        value={absStart}
                                        onChange={(e) => {
                                            setAbsStart(e.target.value);
                                            setAbsError(null);
                                        }}
                                        className="w-full px-2 py-1 bg-stone-50 dark:bg-stone-950 border border-stone-200 dark:border-stone-700 rounded focus:outline-none focus:border-orange-500 text-stone-900 dark:text-stone-100"
                                    />
                                </label>
                                <label className="block mt-1.5">
                                    <span className="block text-stone-600 dark:text-stone-400 mb-0.5">
                                        To
                                    </span>
                                    <input
                                        type="datetime-local"
                                        value={absEnd}
                                        onChange={(e) => {
                                            setAbsEnd(e.target.value);
                                            setAbsError(null);
                                        }}
                                        className="w-full px-2 py-1 bg-stone-50 dark:bg-stone-950 border border-stone-200 dark:border-stone-700 rounded focus:outline-none focus:border-orange-500 text-stone-900 dark:text-stone-100"
                                    />
                                </label>
                                {absError && (
                                    <div className="mt-1 text-red-600 dark:text-red-400">
                                        {absError}
                                    </div>
                                )}
                                <button
                                    type="button"
                                    onClick={applyAbsolute}
                                    className="mt-2 px-3 py-1 font-medium text-white bg-orange-600 hover:bg-orange-500 rounded transition"
                                >
                                    Apply
                                </button>
                            </div>

                            <div>
                                <div className="pt-1 pb-1.5 text-stone-500 dark:text-stone-400 uppercase tracking-wider font-semibold">
                                    Custom relative
                                </div>
                                <div className="flex gap-1.5">
                                    <input
                                        type="text"
                                        value={customRel}
                                        onChange={(e) => {
                                            setCustomRel(e.target.value);
                                            setCustomRelError(null);
                                        }}
                                        onKeyDown={(e) => {
                                            if (e.key === "Enter") {
                                                e.preventDefault();
                                                applyCustomRelative();
                                            }
                                        }}
                                        placeholder="-3h, -2d, -45m"
                                        className="flex-grow min-w-0 px-2 py-1 bg-stone-50 dark:bg-stone-950 border border-stone-200 dark:border-stone-700 rounded focus:outline-none focus:border-orange-500 font-mono text-stone-900 dark:text-stone-100"
                                    />
                                    <button
                                        type="button"
                                        onClick={applyCustomRelative}
                                        className="px-3 py-1 border border-stone-200 dark:border-stone-700 rounded hover:border-orange-300 hover:bg-orange-50/40 dark:hover:bg-orange-950/30 text-stone-700 dark:text-stone-300 transition"
                                    >
                                        Set
                                    </button>
                                </div>
                                {customRelError && (
                                    <div className="mt-1 text-red-600 dark:text-red-400">
                                        {customRelError}
                                    </div>
                                )}
                                <div className="mt-1 text-stone-500 dark:text-stone-500">
                                    Units: <span className="font-mono">s, m, h, d, w</span>
                                </div>
                            </div>
                        </div>

                        {/* Right column: quick presets */}
                        <div className="px-2 py-2 max-h-[360px] overflow-y-auto">
                            <div className="px-1.5 pt-1 pb-1.5 text-stone-500 dark:text-stone-400 uppercase tracking-wider font-semibold">
                                Quick
                            </div>
                            {PRESETS.map((p, i) => (
                                <button
                                    key={p.range}
                                    type="button"
                                    onClick={() => pickPreset(p.range)}
                                    className={`w-full text-left px-2 py-1 rounded transition-colors ${
                                        i === activeIdx
                                            ? "bg-orange-50 text-orange-900 dark:bg-orange-950/40 dark:text-orange-200"
                                            : "hover:bg-stone-100 dark:hover:bg-stone-800 text-stone-800 dark:text-stone-200"
                                    }`}
                                >
                                    {p.label}
                                </button>
                            ))}
                            <div className="border-t border-stone-200 dark:border-stone-800 my-1.5" />
                            {DAY_PRESETS.map((p) => (
                                <button
                                    key={p.label}
                                    type="button"
                                    onClick={() => pickDayPreset(p.compute)}
                                    className="w-full text-left px-2 py-1 rounded transition-colors hover:bg-stone-100 dark:hover:bg-stone-800 text-stone-800 dark:text-stone-200"
                                >
                                    {p.label}
                                </button>
                            ))}
                        </div>
                    </div>
                </div>
            )}
        </div>
    );
}
