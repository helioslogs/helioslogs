// Copyright 2026 Appbird LLC. Licensed under "GNU Affero General Public License v3.0"

import { Boxes } from "lucide-react";

// The environment a monitor targets / an alert was raised in. Neutral pill so
// it doesn't clash with severity or kind badges.
export function EnvBadge({ env }: { env: string | undefined }) {
    if (!env) return null;
    return (
        <span
            className="inline-flex items-center gap-1 px-1.5 py-0.5 rounded border uppercase tracking-wider border-stone-200 bg-stone-50 text-stone-600 dark:border-stone-700 dark:bg-stone-800/60 dark:text-stone-300"
            title={`Environment: ${env}`}
        >
            <Boxes className="w-3.5 h-3.5" />
            {env}
        </span>
    );
}
