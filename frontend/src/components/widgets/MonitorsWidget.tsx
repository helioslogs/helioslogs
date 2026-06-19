// Copyright 2026 Appbird LLC. Licensed under "GNU Affero General Public License v3.0"

import { Link } from "react-router-dom";
import { CircleDot, Pause } from "lucide-react";
import type { Widget } from "../../api/types";
import { useMonitors } from "../../state/useMonitors";

interface Props {
    widget: Widget;
}

// Compact monitor status list. Reuses the shared `useMonitors` hook
// (self-polling) so run state stays live.
export function MonitorsWidget({ widget }: Props) {
    const { items, error } = useMonitors();
    const limit = widget.limit || 10;

    if (error) return <p className="text-sm text-red-600 dark:text-red-300">{error}</p>;
    if (items.length === 0) {
        return <p className="text-sm text-stone-400 dark:text-stone-500">no monitors configured</p>;
    }

    return (
        <ul className="divide-y divide-stone-100 dark:divide-stone-800 -my-1">
            {items.slice(0, limit).map((m) => {
                const status = !m.enabled
                    ? "disabled"
                    : m.last_status === "error"
                      ? "error"
                      : m.last_status === "ok"
                        ? "ok"
                        : "idle";
                const dot =
                    status === "error"
                        ? "text-red-500"
                        : status === "ok"
                          ? "text-emerald-500"
                          : "text-stone-400";
                return (
                    <li key={m.id}>
                        <Link
                            to="/alerts/monitors"
                            className="flex items-center gap-2 py-1.5 hover:bg-stone-50 dark:hover:bg-stone-800/60 rounded px-1 -mx-1"
                        >
                            {m.enabled ? (
                                <CircleDot className={`w-3.5 h-3.5 shrink-0 ${dot}`} />
                            ) : (
                                <Pause className="w-3.5 h-3.5 shrink-0 text-stone-400" />
                            )}
                            <span className="min-w-0 flex-1 text-sm text-stone-800 dark:text-stone-100 truncate">
                                {m.name}
                            </span>
                            <span className="shrink-0 text-xs text-stone-600 dark:text-stone-300">
                                {m.kind}
                                {m.running ? " · running" : ""}
                            </span>
                        </Link>
                    </li>
                );
            })}
        </ul>
    );
}
