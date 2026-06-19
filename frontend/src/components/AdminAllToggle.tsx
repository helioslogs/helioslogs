// Copyright 2026 Appbird LLC. Licensed under "GNU Affero General Public License v3.0"

import { ShieldCheck } from "lucide-react";

// Admin-only "view all" switch for the Saved/Dashboards/Monitors lists.
export function AdminAllToggle({
    checked,
    onChange,
    noun,
}: {
    checked: boolean;
    onChange: (v: boolean) => void;
    // Plural label for the tooltip, e.g. "searches", "dashboards", "monitors".
    noun: string;
}) {
    return (
        <label
            className="flex items-center gap-1.5 cursor-pointer select-none text-stone-600 dark:text-stone-300"
            title={`Admin: show all users' ${noun}, including private ones`}
        >
            <input
                type="checkbox"
                checked={checked}
                onChange={(e) => onChange(e.target.checked)}
                className="rounded border-stone-300 text-orange-500 focus:ring-orange-500"
            />
            <ShieldCheck className="w-3.5 h-3.5 text-orange-500" />
            View all
        </label>
    );
}
