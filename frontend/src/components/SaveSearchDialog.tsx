// Copyright 2026 Appbird LLC. Licensed under "GNU Affero General Public License v3.0"

import { useEffect, useRef, useState } from "react";
import { createPortal } from "react-dom";
import { Globe, Lock, X } from "lucide-react";
import type { SearchInput } from "../state/url";

interface Props {
    // Suggested initial name. The user can edit before saving.
    initialName: string;
    // Starting visibility. Defaults to public (`true`) when omitted.
    initialPublic?: boolean;
    // The search state about to be saved; shown as a read-only confirmation preview.
    preview: SearchInput;
    // Called when the user confirms. Empty/whitespace names are blocked at
    // the dialog so the parent doesn't have to defend against them.
    onSave: (input: { name: string; public: boolean }) => void;
    onClose: () => void;
}

// Modal for "save current search" — captures name + visibility.
export function SaveSearchDialog({ initialName, initialPublic, preview, onSave, onClose }: Props) {
    const [name, setName] = useState(initialName);
    // New searches default to public; callers can override via initialPublic.
    const [isPublic, setIsPublic] = useState(initialPublic ?? true);
    const inputRef = useRef<HTMLInputElement>(null);

    useEffect(() => {
        inputRef.current?.select();
    }, []);

    useEffect(() => {
        const onKey = (e: KeyboardEvent) => {
            if (e.key === "Escape") onClose();
        };
        document.addEventListener("keydown", onKey);
        return () => document.removeEventListener("keydown", onKey);
    }, [onClose]);

    const submit = (e?: React.FormEvent) => {
        e?.preventDefault();
        const trimmed = name.trim();
        if (!trimmed) return;
        onSave({ name: trimmed, public: isPublic });
    };

    // Portal to <body> so this dialog's <form> isn't nested in the search form
    // (invalid HTML) — otherwise Save would submit the search form instead.
    return createPortal(
        <div
            className="fixed inset-0 z-50 flex items-center justify-center bg-black/40"
            onMouseDown={(e) => {
                if (e.target === e.currentTarget) onClose();
            }}
        >
            <form
                onSubmit={submit}
                className="w-full max-w-md mx-4 bg-white dark:bg-stone-900 border border-stone-200 dark:border-stone-700 rounded-xl shadow-xl overflow-hidden"
            >
                <header className="px-4 py-3 flex items-center justify-between border-b border-stone-200 dark:border-stone-800">
                    <h2 className="font-semibold text-stone-900 dark:text-stone-100">
                        Save search
                    </h2>
                    <button
                        type="button"
                        onClick={onClose}
                        className="p-1 rounded text-stone-400 hover:text-stone-700 dark:hover:text-stone-200 hover:bg-stone-100 dark:hover:bg-stone-800"
                        aria-label="close"
                    >
                        <X className="w-4 h-4" />
                    </button>
                </header>

                <div className="px-4 py-4 space-y-4">
                    <Preview input={preview} />

                    <label className="block">
                        <span className="block mb-1 text-stone-700 dark:text-stone-300">Name</span>
                        <input
                            ref={inputRef}
                            type="text"
                            value={name}
                            onChange={(e) => setName(e.target.value)}
                            className="w-full px-3 py-1.5 bg-stone-50 dark:bg-stone-950 border border-stone-200 dark:border-stone-700 rounded-md focus:outline-none focus:border-orange-500 focus:bg-white dark:focus:bg-stone-900 text-stone-900 dark:text-stone-100"
                            placeholder="Name this search…"
                            autoComplete="off"
                            spellCheck={false}
                        />
                    </label>

                    <fieldset className="space-y-2">
                        <legend className="mb-1 text-stone-700 dark:text-stone-300">
                            Visibility
                        </legend>
                        <Choice
                            checked={!isPublic}
                            onChange={() => setIsPublic(false)}
                            icon={<Lock className="w-3.5 h-3.5" />}
                            title="Private"
                            detail="Only you can see, edit, and delete this search."
                        />
                        <Choice
                            checked={isPublic}
                            onChange={() => setIsPublic(true)}
                            icon={<Globe className="w-3.5 h-3.5" />}
                            title="Public"
                            detail="Visible to all users — anyone can edit or delete it."
                        />
                    </fieldset>
                </div>

                <footer className="px-4 py-3 flex items-center justify-end gap-2 border-t border-stone-200 dark:border-stone-800 bg-stone-50/50 dark:bg-stone-950/40">
                    <button
                        type="button"
                        onClick={onClose}
                        className="px-3 py-1.5 rounded-md border border-stone-200 dark:border-stone-700 text-stone-700 dark:text-stone-300 hover:bg-stone-100 dark:hover:bg-stone-800"
                    >
                        Cancel
                    </button>
                    <button
                        type="submit"
                        disabled={!name.trim()}
                        className="px-3 py-1.5 rounded-md bg-orange-600 hover:bg-orange-500 text-white font-medium disabled:opacity-50 disabled:cursor-not-allowed"
                    >
                        Save
                    </button>
                </footer>
            </form>
        </div>,
        document.body,
    );
}

// Read-only summary of the search being saved. Each row is omitted when
// the underlying value is absent so the box stays compact.
function Preview({ input }: { input: SearchInput }) {
    const rangeLabel = input.start && input.end ? `${input.start}  →  ${input.end}` : input.range;
    return (
        <div className="rounded-md border border-stone-200 dark:border-stone-700 bg-stone-50/50 dark:bg-stone-950/40 px-3 py-2 space-y-1">
            <PreviewRow label="Query">
                <code className="font-mono text-stone-800 dark:text-stone-200 break-all">
                    {input.q?.trim() || "*"}
                </code>
            </PreviewRow>
            {input.index && (
                <PreviewRow label="Index">
                    <span className="text-stone-800 dark:text-stone-200">{input.index}</span>
                </PreviewRow>
            )}
            <PreviewRow label="Range">
                <span className="text-stone-800 dark:text-stone-200">{rangeLabel}</span>
                {input.follow && (
                    <span className="ml-2 px-1.5 py-0.5 rounded bg-green-50 text-green-700 dark:bg-green-950/40 dark:text-green-300">
                        live
                    </span>
                )}
            </PreviewRow>
        </div>
    );
}

function PreviewRow({ label, children }: { label: string; children: React.ReactNode }) {
    return (
        <div className="flex items-baseline gap-2 min-w-0">
            <span className="w-16 flex-shrink-0 uppercase tracking-wider text-stone-500 dark:text-stone-400">
                {label}
            </span>
            <span className="flex-grow min-w-0 truncate">{children}</span>
        </div>
    );
}

function Choice({
    checked,
    onChange,
    icon,
    title,
    detail,
}: {
    checked: boolean;
    onChange: () => void;
    icon: React.ReactNode;
    title: string;
    detail: string;
}) {
    return (
        <label
            className={`flex items-start gap-2 px-3 py-2 rounded-md border cursor-pointer ${
                checked
                    ? "border-orange-300 bg-orange-50/40 dark:border-orange-700 dark:bg-orange-950/30"
                    : "border-stone-200 dark:border-stone-700 hover:bg-stone-50 dark:hover:bg-stone-800/40"
            }`}
        >
            <input
                type="radio"
                checked={checked}
                onChange={onChange}
                className="mt-1 text-orange-500 focus:ring-orange-500"
            />
            <div className="flex-grow min-w-0">
                <div className="flex items-center gap-1.5 text-stone-900 dark:text-stone-100 font-medium">
                    {icon}
                    {title}
                </div>
                <div className="text-stone-500 dark:text-stone-400">{detail}</div>
            </div>
        </label>
    );
}
