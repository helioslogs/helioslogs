// Copyright 2026 Appbird LLC. Licensed under "GNU Affero General Public License v3.0"

import { useEffect, useRef, useState } from "react";
import { Check, ChevronDown, RefreshCw } from "lucide-react";
import { REFRESH_OPTIONS, refreshSecsLabel, type RefreshSetting } from "../lib/autoRefresh";

interface Props {
    // Manual refresh — the left half of the split button. Always available,
    // even when the interval part is disabled (absolute ranges still refetch).
    onRefresh: () => void;
    // Spin the icon while a fetch is in flight.
    refreshing?: boolean;
    setting: RefreshSetting;
    onChange: (setting: RefreshSetting) => void;
    // Resolved cadence in seconds (0 = inactive) — drives the live dot and the
    // "Auto · 5m" hint.
    effectiveSecs: number;
    // Absolute range / live-follow / editing — auto-refresh is meaningless, so
    // the interval half is greyed out (manual refresh still works).
    disabled?: boolean;
    disabledReason?: string;
    // Following live: the whole control (manual refresh included) is redundant
    // because results already poll every 2s, so grey both halves out.
    following?: boolean;
}

// Split-button refresh control: manual-refresh icon + auto-refresh interval dropdown.
export function RefreshIntervalPicker({
    onRefresh,
    refreshing = false,
    setting,
    onChange,
    effectiveSecs,
    disabled = false,
    disabledReason,
    following = false,
}: Props) {
    const [open, setOpen] = useState(false);
    const ref = useRef<HTMLDivElement>(null);

    useEffect(() => {
        if (!open) return;
        const handler = (e: MouseEvent) => {
            if (ref.current && !ref.current.contains(e.target as Node)) setOpen(false);
        };
        window.addEventListener("mousedown", handler);
        return () => window.removeEventListener("mousedown", handler);
    }, [open]);

    const active = !disabled && effectiveSecs > 0;
    const label = disabled ? "Off" : setting === "auto" ? "Auto" : refreshSecsLabel(setting);
    const intervalTitle = disabled
        ? (disabledReason ?? "Auto-refresh unavailable")
        : setting === "auto"
          ? effectiveSecs > 0
              ? `Auto-refresh every ${refreshSecsLabel(effectiveSecs)} (scaled to range)`
              : "Auto-refresh: off for this range"
          : effectiveSecs > 0
            ? `Auto-refresh every ${refreshSecsLabel(effectiveSecs)}`
            : "Auto-refresh: off";

    return (
        <div className="relative" ref={ref}>
            <div className="inline-flex items-stretch rounded-md border border-stone-200 dark:border-stone-700 overflow-hidden">
                <button
                    type="button"
                    onClick={onRefresh}
                    disabled={refreshing || following}
                    title={following ? "Following live — updating every 2s" : "Refresh now"}
                    aria-label="Refresh now"
                    className="px-2.5 py-1.5 text-stone-700 dark:text-stone-300 hover:bg-orange-50/40 dark:hover:bg-orange-950/30 transition disabled:opacity-60 disabled:cursor-not-allowed"
                >
                    <RefreshCw
                        className={`w-3.5 h-3.5 ${refreshing ? "animate-spin" : ""}`}
                        aria-hidden="true"
                    />
                </button>
                <span className="w-px bg-stone-200 dark:bg-stone-700" aria-hidden="true" />
                <button
                    type="button"
                    disabled={disabled}
                    onClick={() => setOpen((v) => !v)}
                    title={intervalTitle}
                    aria-label={intervalTitle}
                    className="inline-flex items-center gap-1 pl-2 pr-1.5 py-1.5 text-stone-700 dark:text-stone-300 hover:bg-orange-50/40 dark:hover:bg-orange-950/30 transition disabled:opacity-60 disabled:cursor-not-allowed"
                >
                    {active && (
                        <span
                            className="w-1.5 h-1.5 rounded-full bg-emerald-500"
                            aria-hidden="true"
                        />
                    )}
                    <span className="tabular-nums">{label}</span>
                    <ChevronDown
                        className="w-3 h-3 text-stone-400 dark:text-stone-500"
                        aria-hidden="true"
                    />
                </button>
            </div>
            {open && !disabled && (
                <div className="absolute right-0 top-full mt-1 z-20 min-w-[8rem] rounded-md border border-stone-200 dark:border-stone-700 bg-white dark:bg-stone-900 shadow-lg py-1">
                    <div className="px-3 py-1 text-stone-500 dark:text-stone-400 border-b border-stone-100 dark:border-stone-800">
                        Auto-refresh
                    </div>
                    {REFRESH_OPTIONS.map((opt) => {
                        const selected = opt.value === setting;
                        return (
                            <button
                                key={String(opt.value)}
                                type="button"
                                onClick={() => {
                                    onChange(opt.value);
                                    setOpen(false);
                                }}
                                className={`flex w-full items-center gap-2 px-3 py-1 text-left hover:bg-stone-100 dark:hover:bg-stone-800 ${
                                    selected
                                        ? "text-orange-600 dark:text-orange-400"
                                        : "text-stone-700 dark:text-stone-300"
                                }`}
                            >
                                <Check
                                    className={`w-3 h-3 shrink-0 ${selected ? "" : "invisible"}`}
                                />
                                <span className="flex-1">{opt.label}</span>
                                {opt.value === "auto" && effectiveSecs > 0 && (
                                    <span className="text-stone-400 dark:text-stone-500 tabular-nums">
                                        {refreshSecsLabel(effectiveSecs)}
                                    </span>
                                )}
                            </button>
                        );
                    })}
                </div>
            )}
        </div>
    );
}
