// Copyright 2026 Appbird LLC. Licensed under "GNU Affero General Public License v3.0"

import { useEffect } from "react";
import { createPortal } from "react-dom";
import { Bell, X } from "lucide-react";
import { Link } from "react-router-dom";
import type { Alert } from "../api/types";
import { EnvBadge } from "./EnvBadge";
import { AlertActions, AlertDetailBody, SeverityBadge, SeverityIcon } from "./AlertDetail";
import { timeAgo } from "../lib/format";

// Alert detail in a modal (from the dashboard Alerts widget); navigating actions close it via onActed.
export function AlertDetailModal({ alert, onClose }: { alert: Alert; onClose: () => void }) {
    useEffect(() => {
        const onKey = (e: KeyboardEvent) => {
            if (e.key === "Escape") onClose();
        };
        document.addEventListener("keydown", onKey);
        return () => document.removeEventListener("keydown", onKey);
    }, [onClose]);

    return createPortal(
        <div
            className="fixed inset-0 z-50 flex items-center justify-center bg-black/40 p-4"
            onMouseDown={(e) => {
                if (e.target === e.currentTarget) onClose();
            }}
        >
            <div className="w-full max-w-2xl bg-white dark:bg-stone-900 border border-stone-200 dark:border-stone-700 rounded-xl shadow-xl max-h-[85vh] flex flex-col">
                <header className="px-4 py-3 flex items-start gap-3 border-b border-stone-200 dark:border-stone-800">
                    <SeverityIcon severity={alert.severity} />
                    <div className="flex-grow min-w-0">
                        <div className="flex items-center gap-2 flex-wrap">
                            <strong className="text-stone-900 dark:text-stone-100">
                                {alert.title}
                            </strong>
                            <SeverityBadge severity={alert.severity} />
                            <EnvBadge env={alert.env} />
                            {alert.acknowledged && (
                                <span className="uppercase tracking-wider px-1.5 py-0.5 rounded border border-stone-200 bg-stone-50 text-stone-500 dark:border-stone-700 dark:bg-stone-800/60 dark:text-stone-400">
                                    acked
                                </span>
                            )}
                        </div>
                        <div className="text-stone-500 dark:text-stone-400 mt-0.5 flex items-center gap-2">
                            <Bell className="w-3 h-3 flex-shrink-0" />
                            <Link
                                to={`/alerts/monitors?monitor=${encodeURIComponent(alert.monitor_id)}`}
                                onClick={onClose}
                                className="hover:text-orange-600 dark:hover:text-orange-400 hover:underline truncate"
                                title={`View the “${alert.monitor_name}” monitor`}
                            >
                                {alert.monitor_name}
                            </Link>
                            <span>·</span>
                            <span
                                className="flex-shrink-0"
                                title={new Date(alert.created_at).toLocaleString()}
                            >
                                {timeAgo(new Date(alert.created_at).toISOString())}
                            </span>
                        </div>
                    </div>
                    <button
                        type="button"
                        onClick={onClose}
                        className="p-1 rounded text-stone-400 hover:text-stone-700 dark:hover:text-stone-200 hover:bg-stone-100 dark:hover:bg-stone-800"
                        aria-label="close"
                    >
                        <X className="w-4 h-4" />
                    </button>
                </header>

                <div className="px-4 py-4 overflow-auto">
                    <AlertDetailBody alert={alert} />
                </div>

                <footer className="px-4 py-2.5 border-t border-stone-200 dark:border-stone-800 bg-stone-50/50 dark:bg-stone-950/40">
                    <AlertActions alert={alert} onActed={onClose} />
                </footer>
            </div>
        </div>,
        document.body,
    );
}
