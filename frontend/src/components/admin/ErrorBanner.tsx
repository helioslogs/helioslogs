// Copyright 2026 Appbird LLC. Licensed under "GNU Affero General Public License v3.0"

// Red inline error banner. Renders nothing when `error` is null.
export function ErrorBanner({ error }: { error: string | null }) {
    if (!error) return null;
    return (
        <div className="px-3 py-2 rounded-md bg-red-50 text-red-800 border border-red-200 dark:bg-red-950 dark:text-red-200 dark:border-red-900">
            {error}
        </div>
    );
}
