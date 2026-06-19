// Copyright 2026 Appbird LLC. Licensed under "GNU Affero General Public License v3.0"

import type { ReactNode } from "react";

// Uppercase table header cell used by the admin list tables.
export function Th({
    children,
    align = "left",
}: {
    children?: ReactNode;
    align?: "left" | "right";
}) {
    return (
        <th
            className={`px-3 py-2 font-semibold uppercase tracking-wider text-stone-700 dark:text-stone-300 ${
                align === "right" ? "text-right" : "text-left"
            }`}
        >
            {children}
        </th>
    );
}
