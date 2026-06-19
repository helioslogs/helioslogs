// Copyright 2026 Appbird LLC. Licensed under "GNU Affero General Public License v3.0"

import { useEffect, useRef, useState } from "react";
import { createPortal } from "react-dom";
import { Globe, Lock, X } from "lucide-react";
import type { SavedSearch } from "../api/types";
import { TimeRangePicker } from "./TimeRangePicker";
import { QueryInput } from "./widgets/QueryInput";
import { useQuerySuggestData } from "../state/useQuerySuggestData";

// Editable fields emitted on save; index is absent (expressed inline via `index:`).
export interface SavedSearchFormValues {
    name: string;
    q: string;
    range: string;
    start?: string;
    end?: string;
    follow: boolean;
    public: boolean;
}

interface Props {
    // The search being edited. Omit to open the dialog in "create" mode.
    search?: SavedSearch;
    // Called on save with the full field set. The page owns the network call
    // + notification so this dialog stays a pure form.
    onSave: (values: SavedSearchFormValues) => void;
    onClose: () => void;
}

// Modal form for creating or editing a saved search. One form drives both
// flows — `search` present = edit, absent = create.
export function EditSearchDialog({ search, onSave, onClose }: Props) {
    const isCreate = !search;
    const [name, setName] = useState(search?.name ?? "");
    const [q, setQ] = useState(search?.q ?? "*");
    const [range, setRange] = useState(search?.range ?? "-6h");
    const [start, setStart] = useState<string | undefined>(search?.start ?? undefined);
    const [end, setEnd] = useState<string | undefined>(search?.end ?? undefined);
    const [follow, setFollow] = useState(search?.follow ?? false);
    // New searches default to public; edits keep the search's current visibility.
    const [isPublic, setIsPublic] = useState(search?.public ?? true);
    const nameRef = useRef<HTMLInputElement>(null);
    const suggest = useQuerySuggestData();

    useEffect(() => {
        nameRef.current?.select();
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
        onSave({ name: trimmed, q, range, start, end, follow, public: isPublic });
    };

    // Portal to <body> so the modal never lands inside another form (a nested
    // <form> is invalid HTML and the inner submit gets hijacked).
    return createPortal(
        <div
            className="fixed inset-0 z-50 flex items-center justify-center bg-black/40"
            onMouseDown={(e) => {
                if (e.target === e.currentTarget) onClose();
            }}
        >
            <form
                onSubmit={submit}
                className="w-full max-w-xl mx-4 bg-white dark:bg-stone-900 border border-stone-200 dark:border-stone-700 rounded-xl shadow-xl overflow-visible"
            >
                <header className="px-4 py-3 flex items-center justify-between border-b border-stone-200 dark:border-stone-800">
                    <h2 className="font-semibold text-stone-900 dark:text-stone-100">
                        {isCreate ? "New saved search" : "Edit saved search"}
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
                    <Field label="Name">
                        <input
                            ref={nameRef}
                            type="text"
                            value={name}
                            onChange={(e) => setName(e.target.value)}
                            className={FIELD_INPUT}
                            autoComplete="off"
                            spellCheck={false}
                        />
                    </Field>

                    <Field label="Query">
                        <QueryInput
                            value={q}
                            onChange={setQ}
                            fields={suggest.fields}
                            indexes={suggest.indexes}
                            start={suggest.start}
                            end={suggest.end}
                            placeholder="*"
                            className={`${FIELD_INPUT} font-mono`}
                        />
                    </Field>

                    <Field label="Time range">
                        <TimeRangePicker
                            range={range}
                            start={start}
                            end={end}
                            onChange={(next) => {
                                if (next.range !== undefined) {
                                    setRange(next.range);
                                    setStart(undefined);
                                    setEnd(undefined);
                                } else if (next.start && next.end) {
                                    setStart(next.start);
                                    setEnd(next.end);
                                }
                            }}
                        />
                    </Field>

                    <label className="flex items-center gap-2 text-stone-700 dark:text-stone-300 cursor-pointer select-none">
                        <input
                            type="checkbox"
                            checked={follow}
                            onChange={(e) => setFollow(e.target.checked)}
                            className="rounded border-stone-300 text-orange-500 focus:ring-orange-500"
                        />
                        Follow live
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
                        {isCreate ? "Create" : "Save"}
                    </button>
                </footer>
            </form>
        </div>,
        document.body,
    );
}

const FIELD_INPUT =
    "w-full px-3 py-1.5 bg-stone-50 dark:bg-stone-950 border border-stone-200 dark:border-stone-700 rounded-md focus:outline-none focus:border-orange-500 focus:bg-white dark:focus:bg-stone-900 text-stone-900 dark:text-stone-100";

function Field({ label, children }: { label: string; children: React.ReactNode }) {
    return (
        <label className="block">
            <span className="block mb-1 text-stone-700 dark:text-stone-300">{label}</span>
            {children}
        </label>
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
