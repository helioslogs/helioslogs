// Copyright 2026 Appbird LLC. Licensed under "GNU Affero General Public License v3.0"

import type { ReactNode } from "react";

// Orange-tinted help banner shown at the top of an admin card — the shared
// treatment used across General / Index / Source / Environment / Token panels.
export function HelpFrame({ icon, children }: { icon: ReactNode; children: ReactNode }) {
    return (
        <div className="flex gap-3 p-4 rounded-lg bg-orange-50/60 dark:bg-orange-950/20 border border-orange-200/70 dark:border-orange-900/40">
            <div className="flex-shrink-0 mt-0.5 text-orange-600 dark:text-orange-400">{icon}</div>
            <div className="space-y-1.5 text-stone-700 dark:text-stone-200 leading-relaxed">
                {children}
            </div>
        </div>
    );
}
