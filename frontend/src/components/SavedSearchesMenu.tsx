// Copyright 2026 Appbird LLC. Licensed under "GNU Affero General Public License v3.0"

import { useEffect, useRef, useState } from "react";
import { Link } from "react-router-dom";
import { ChevronDown, ListTree } from "lucide-react";
import { useSavedSearches } from "../state/useSavedSearches";
import type { SearchInput } from "../state/url";
import { SavedSearchList } from "./SavedSearchList";

interface Props {
    current: SearchInput;
    onLoad: (s: SearchInput) => void;
}

export function SavedSearchesMenu({ current, onLoad }: Props) {
    const [open, setOpen] = useState(false);
    const [filter, setFilter] = useState("");
    const wrapRef = useRef<HTMLDivElement | null>(null);
    const { items } = useSavedSearches();

    // Click outside / escape closes.
    useEffect(() => {
        if (!open) return;
        const onDown = (e: MouseEvent) => {
            if (!wrapRef.current?.contains(e.target as Node)) setOpen(false);
        };
        const onKey = (e: KeyboardEvent) => {
            if (e.key === "Escape") setOpen(false);
        };
        document.addEventListener("mousedown", onDown);
        document.addEventListener("keydown", onKey);
        return () => {
            document.removeEventListener("mousedown", onDown);
            document.removeEventListener("keydown", onKey);
        };
    }, [open]);

    const handleLoad = (s: SearchInput) => {
        onLoad(s);
        setOpen(false);
    };

    return (
        <div className="relative" ref={wrapRef}>
            <button
                type="button"
                className="px-2.5 py-1.5 rounded-md border border-stone-200 dark:border-stone-700 text-stone-700 dark:text-stone-300 hover:border-orange-300 hover:bg-orange-50/40 dark:hover:bg-orange-950/30 transition flex items-center gap-1.5"
                onClick={() => setOpen((v) => !v)}
                aria-haspopup="menu"
                aria-expanded={open}
                title="Saved searches"
            >
                <ListTree className="w-3.5 h-3.5" />
                <span>Saved</span>
                {items.length > 0 && (
                    <span className="text-stone-400 dark:text-stone-500">{items.length}</span>
                )}
                <ChevronDown className="w-3 h-3 text-stone-400" />
            </button>
            {open && (
                <div
                    className="absolute right-0 mt-1 w-80 bg-white dark:bg-stone-900 border border-stone-200 dark:border-stone-700 rounded-lg shadow-lg z-30 overflow-hidden"
                    role="menu"
                >
                    {items.length > 5 && (
                        <div className="px-2 pt-2">
                            <input
                                type="text"
                                className="w-full px-2 py-1 bg-stone-50 dark:bg-stone-950 border border-stone-200 dark:border-stone-700 rounded focus:outline-none focus:border-orange-500"
                                placeholder="filter…"
                                value={filter}
                                onChange={(e) => setFilter(e.target.value)}
                                autoFocus
                            />
                        </div>
                    )}
                    <div className="max-h-96 overflow-auto py-1">
                        <SavedSearchList
                            current={current}
                            onLoad={handleLoad}
                            filter={filter}
                            limit={50}
                            emptyText="no saved searches yet — click ☆ to save"
                        />
                    </div>
                    <Link
                        className="block text-center text-orange-700 dark:text-orange-300 hover:bg-orange-50 dark:hover:bg-orange-950/30 border-t border-stone-200 dark:border-stone-800 py-2"
                        to="/saved"
                        onClick={() => setOpen(false)}
                    >
                        manage all →
                    </Link>
                </div>
            )}
        </div>
    );
}
