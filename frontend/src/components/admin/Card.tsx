// Copyright 2026 Appbird LLC. Licensed under "GNU Affero General Public License v3.0"

import type { ReactNode } from "react";

// Bordered section with a heading — the frame every admin sub-panel sits in.
export function Card({ title, children }: { title: string; children: ReactNode }) {
    return (
        <section className="border-b border-stone-200 dark:border-stone-800">
            <h2 className="px-6 py-3 font-semibold text-stone-700 dark:text-stone-200 border-b border-stone-200 dark:border-stone-800">
                {title}
            </h2>
            {children}
        </section>
    );
}
