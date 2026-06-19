// Copyright 2026 Appbird LLC. Licensed under "GNU Affero General Public License v3.0"

import { useMemo, useRef, useState } from "react";
import type { DiscoveredField } from "../../api/types";

interface Props {
    value: string;
    onChange: (next: string) => void;
    fields: DiscoveredField[];
    placeholder?: string;
    className?: string;
}

// Field-name input with a type-ahead dropdown over discovered fields.
export function FieldNameInput({ value, onChange, fields, placeholder, className }: Props) {
    const [open, setOpen] = useState(false);
    const blurTimer = useRef<ReturnType<typeof setTimeout> | null>(null);

    const matches = useMemo(() => {
        const q = value.trim().toLowerCase();
        return fields.filter((f) => f.name.toLowerCase().includes(q)).slice(0, 30);
    }, [fields, value]);

    return (
        <div className="relative flex-1 min-w-0">
            <input
                type="text"
                value={value}
                placeholder={placeholder}
                className={className}
                autoComplete="off"
                spellCheck={false}
                onChange={(e) => {
                    onChange(e.target.value);
                    setOpen(true);
                }}
                onFocus={() => setOpen(true)}
                onBlur={() => {
                    blurTimer.current = setTimeout(() => setOpen(false), 120);
                }}
            />
            {open && matches.length > 0 && (
                <div
                    className="absolute top-full left-0 right-0 mt-1 z-50 max-h-72 overflow-auto rounded-md border border-stone-200 dark:border-stone-700 bg-white dark:bg-stone-900 shadow-lg"
                    onMouseDown={(e) => e.preventDefault()}
                >
                    {matches.map((f) => (
                        <button
                            key={f.name}
                            type="button"
                            className="w-full flex items-center justify-between gap-3 px-3 py-1.5 text-left font-mono text-sm text-stone-800 dark:text-stone-200 hover:bg-orange-50 dark:hover:bg-orange-950/40"
                            onMouseDown={(e) => {
                                e.preventDefault();
                                onChange(f.name);
                                if (blurTimer.current) clearTimeout(blurTimer.current);
                                setOpen(false);
                            }}
                        >
                            <span className="truncate">{f.name}</span>
                            <span className="shrink-0 text-xs text-stone-500 dark:text-stone-400 font-sans">
                                {f.value_kind}
                                {f.cardinality > 0 ? ` · ~${f.cardinality}` : ""}
                            </span>
                        </button>
                    ))}
                </div>
            )}
        </div>
    );
}
