// Copyright 2026 Appbird LLC. Licensed under "GNU Affero General Public License v3.0"

import { useState } from "react";
import { createPortal } from "react-dom";
import { Volume2, VolumeX, X } from "lucide-react";
import type { Alert } from "../api/types";
import { useAlertToasts } from "../state/useAlertToasts";
import { timeAgo } from "../lib/format";
import { SeverityIcon } from "./AlertDetail";
import { AlertDetailModal } from "./AlertDetailModal";

// Newest toasts shown; the rest collapse into a "+N more" line.
const MAX_VISIBLE = 5;

// Global foreground alert toasts, bottom-right. Mounted once in the app shell.
export function AlertToasts() {
    const { toasts, dismiss, dismissAll, muted, toggleMuted } = useAlertToasts();
    const [detail, setDetail] = useState<Alert | null>(null);

    if (toasts.length === 0) return null;

    // Newest first; persistent until dismissed or acknowledged elsewhere.
    const ordered = [...toasts].sort((a, b) => b.created_at - a.created_at);
    const visible = ordered.slice(0, MAX_VISIBLE);
    const hidden = ordered.length - visible.length;

    return createPortal(
        <>
            <div className="fixed bottom-4 right-4 z-50 flex flex-col gap-2 w-[22rem] max-w-[calc(100vw-2rem)]">
                <div className="flex items-center justify-between px-1 text-xs text-stone-500 dark:text-stone-400">
                    <button
                        type="button"
                        onClick={toggleMuted}
                        className="inline-flex items-center gap-1 hover:text-stone-700 dark:hover:text-stone-200 transition"
                        title={muted ? "Unmute alert sound" : "Mute alert sound"}
                    >
                        {muted ? (
                            <VolumeX className="w-3.5 h-3.5" />
                        ) : (
                            <Volume2 className="w-3.5 h-3.5" />
                        )}
                    </button>
                    <button
                        type="button"
                        onClick={dismissAll}
                        className="hover:text-stone-700 dark:hover:text-stone-200 transition font-medium"
                    >
                        Dismiss all ({ordered.length})
                    </button>
                </div>

                {visible.map((a) => (
                    <ToastCard
                        key={a.id}
                        alert={a}
                        onDismiss={() => dismiss(a.id)}
                        onOpen={() => setDetail(a)}
                    />
                ))}

                {hidden > 0 && (
                    <div className="text-center text-xs text-stone-500 dark:text-stone-400 py-1">
                        +{hidden} more in the inbox
                    </div>
                )}
            </div>

            {detail && <AlertDetailModal alert={detail} onClose={() => setDetail(null)} />}
        </>,
        document.body,
    );
}

function ToastCard({
    alert,
    onDismiss,
    onOpen,
}: {
    alert: Alert;
    onDismiss: () => void;
    onOpen: () => void;
}) {
    return (
        <div className="relative bg-white dark:bg-stone-900 border border-stone-200 dark:border-stone-700 rounded-lg shadow-lg overflow-hidden">
            <div
                className="flex items-start gap-2.5 p-3 pr-8 cursor-pointer hover:bg-stone-50 dark:hover:bg-stone-800/60 transition"
                onClick={onOpen}
                title="View alert details"
            >
                <div className="mt-0.5 flex-shrink-0">
                    <SeverityIcon severity={alert.severity} />
                </div>
                <div className="min-w-0 flex-grow">
                    <div className="font-medium text-stone-900 dark:text-stone-100 truncate">
                        {alert.title}
                    </div>
                    {alert.summary && (
                        <div className="text-stone-600 dark:text-stone-400 mt-0.5 line-clamp-2">
                            {alert.summary}
                        </div>
                    )}
                    <div className="text-stone-400 dark:text-stone-500 mt-1 truncate">
                        {alert.monitor_name} · {timeAgo(new Date(alert.created_at).toISOString())}
                    </div>
                </div>
            </div>
            <button
                type="button"
                onClick={onDismiss}
                className="absolute top-2 right-2 p-1 rounded text-stone-400 hover:text-stone-700 hover:bg-stone-100 dark:hover:text-stone-200 dark:hover:bg-stone-800 transition"
                title="Dismiss"
                aria-label="dismiss alert"
            >
                <X className="w-4 h-4" />
            </button>
        </div>
    );
}
