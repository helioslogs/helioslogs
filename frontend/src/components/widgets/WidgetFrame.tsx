// Copyright 2026 Appbird LLC. Licensed under "GNU Affero General Public License v3.0"

import type { ReactNode } from "react";
import { GripVertical, Loader2, Pencil, Trash2 } from "lucide-react";

interface Props {
    title: string;
    editing: boolean;
    onEdit?: () => void;
    onDelete?: () => void;
    // Spinner in the header while the widget refetches.
    loading?: boolean;
    // Per-widget error (failed fetch) shown in place of content.
    error?: string | null;
    // Optional right-aligned header content (e.g. a "view" link).
    headerRight?: ReactNode;
    children: ReactNode;
}

// Shared card chrome for dashboard widgets; the header doubles as the drag handle in edit mode.
export function WidgetFrame({
    title,
    editing,
    onEdit,
    onDelete,
    loading,
    error,
    headerRight,
    children,
}: Props) {
    return (
        <div
            className={`h-full flex flex-col rounded-xl border bg-white dark:bg-stone-900 overflow-hidden ${
                editing
                    ? "cursor-move border-stone-300 dark:border-stone-700 ring-1 ring-stone-200/60 dark:ring-stone-700/40"
                    : "border-stone-200 dark:border-stone-800"
            }`}
        >
            <div className="widget-drag-handle flex items-center gap-1.5 px-3 py-2 border-b border-stone-100 dark:border-stone-800">
                {editing && (
                    <GripVertical
                        className="w-4 h-4 text-stone-700 dark:text-stone-300 shrink-0"
                        aria-hidden="true"
                    />
                )}
                <span className="font-semibold text-sm text-stone-900 dark:text-stone-100 truncate flex-1">
                    {title}
                </span>
                {loading && (
                    <Loader2 className="w-3.5 h-3.5 animate-spin text-stone-400 shrink-0" />
                )}
                {headerRight}
                {editing && (
                    <span
                        className="widget-no-drag flex items-center gap-0.5 shrink-0"
                        // Buttons must not start a widget drag (also excluded via draggableCancel).
                        onMouseDown={(e) => e.stopPropagation()}
                    >
                        {onEdit && (
                            <button
                                type="button"
                                title="edit widget"
                                onClick={onEdit}
                                className="p-1 rounded text-stone-800 dark:text-stone-200 hover:text-orange-600 dark:hover:text-orange-400 hover:bg-orange-50 dark:hover:bg-orange-950/30"
                            >
                                <Pencil className="w-3.5 h-3.5" />
                            </button>
                        )}
                        {onDelete && (
                            <button
                                type="button"
                                title="remove widget"
                                onClick={onDelete}
                                className="p-1 rounded text-stone-800 dark:text-stone-200 hover:text-orange-600 dark:hover:text-orange-400 hover:bg-orange-50 dark:hover:bg-orange-950/30"
                            >
                                <Trash2 className="w-3.5 h-3.5" />
                            </button>
                        )}
                    </span>
                )}
            </div>
            <div className="flex-1 min-h-0 p-3 overflow-auto">
                {error ? (
                    <div className="text-sm text-red-600 dark:text-red-300">{error}</div>
                ) : (
                    children
                )}
            </div>
        </div>
    );
}
