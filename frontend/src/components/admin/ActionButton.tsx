// Copyright 2026 Appbird LLC. Licensed under "GNU Affero General Public License v3.0"

import type { ReactNode } from "react";

// Dark primary action button used in admin panels (Save, etc.).
export function ActionButton({
    onClick,
    busy,
    children,
}: {
    onClick: () => void;
    busy: boolean;
    children: ReactNode;
}) {
    return (
        <button
            type="button"
            onClick={onClick}
            disabled={busy}
            className="px-3 py-1.5 font-medium rounded-md bg-stone-900 hover:bg-stone-800 dark:bg-stone-800 dark:hover:bg-stone-700 text-white disabled:opacity-50 disabled:cursor-not-allowed transition"
        >
            {children}
        </button>
    );
}
