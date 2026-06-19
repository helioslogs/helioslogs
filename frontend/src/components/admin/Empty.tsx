// Copyright 2026 Appbird LLC. Licensed under "GNU Affero General Public License v3.0"

import type { ReactNode } from "react";

// Centered placeholder shown when an admin list has no rows.
export function Empty({ children }: { children: ReactNode }) {
    return (
        <div className="px-4 py-8 text-center text-stone-700 dark:text-stone-300">{children}</div>
    );
}
