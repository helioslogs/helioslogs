// Copyright 2026 Appbird LLC. Licensed under "GNU Affero General Public License v3.0"

import { Globe, Lock } from "lucide-react";

// Public/private pill shared by the Saved-searches and Dashboards lists so
// they look identical. Globe = public, Lock = private.
export function VisibilityBadge({ isPublic }: { isPublic: boolean }) {
    const Icon = isPublic ? Globe : Lock;
    return (
        <span
            className={`inline-flex items-center gap-1 px-1.5 py-0.5 rounded border text-xs ${
                isPublic
                    ? "border-emerald-200 bg-emerald-50 text-emerald-700 dark:border-emerald-900 dark:bg-emerald-950/40 dark:text-emerald-300"
                    : "border-stone-200 bg-stone-50 text-stone-600 dark:border-stone-700 dark:bg-stone-800/60 dark:text-stone-300"
            }`}
            title={isPublic ? "Public — visible to all users." : "Private — only you."}
        >
            <Icon className="w-3 h-3" />
            {isPublic ? "Public" : "Private"}
        </span>
    );
}
