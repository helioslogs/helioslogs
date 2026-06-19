// Copyright 2026 Appbird LLC. Licensed under "GNU Affero General Public License v3.0"

// Transient bottom-right confirmation toast. Renders nothing when `message`
// is null.
export function Toast({ message }: { message: string | null }) {
    if (!message) return null;
    return (
        <div className="fixed bottom-4 right-4 px-3 py-2 rounded-lg shadow-lg bg-stone-900 dark:bg-stone-800 text-white z-50">
            {message}
        </div>
    );
}
