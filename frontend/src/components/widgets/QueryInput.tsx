// Copyright 2026 Appbird LLC. Licensed under "GNU Affero General Public License v3.0"

import { useCallback, useLayoutEffect, useRef, useState } from "react";
import type { DiscoveredField } from "../../api/types";
import { SearchSuggest, type SearchSuggestHandle } from "../SearchSuggest";

interface Props {
    value: string;
    onChange: (next: string) => void;
    // Discovered fields + known indexes, for the autocomplete popover (same
    // data the main search bar uses).
    fields: DiscoveredField[];
    indexes: string[];
    // Time window to scope value lookups to (the dashboard/widget range).
    start: string;
    end: string;
    placeholder?: string;
    className?: string;
}

// Query input wrapping `<input>` + `<SearchSuggest>` with the main search bar's autocomplete.
export function QueryInput({
    value,
    onChange,
    fields,
    indexes,
    start,
    end,
    placeholder,
    className,
}: Props) {
    const inputRef = useRef<HTMLInputElement>(null);
    const [caret, setCaret] = useState(value.length);
    const [engaged, setEngaged] = useState(false);
    const suggestRef = useRef<SearchSuggestHandle>(null);
    const pendingCaretRef = useRef<number | null>(null);

    useLayoutEffect(() => {
        if (pendingCaretRef.current !== null && inputRef.current) {
            const pos = pendingCaretRef.current;
            inputRef.current.setSelectionRange(pos, pos);
            setCaret(pos);
            pendingCaretRef.current = null;
        }
    }, [value]);

    const handleAccept = useCallback(
        (next: string, newCaret: number) => {
            pendingCaretRef.current = newCaret;
            onChange(next);
        },
        [onChange],
    );

    return (
        <div className="relative flex-1 min-w-0">
            <input
                ref={inputRef}
                type="text"
                value={value}
                placeholder={placeholder}
                className={className}
                autoComplete="off"
                spellCheck={false}
                onChange={(e) => {
                    onChange(e.target.value);
                    setCaret(e.target.selectionStart ?? e.target.value.length);
                    setEngaged(true);
                }}
                onKeyDown={(e) => {
                    if (suggestRef.current?.handleKey(e)) e.preventDefault();
                }}
                onKeyUp={(e) =>
                    setCaret(e.currentTarget.selectionStart ?? e.currentTarget.value.length)
                }
                onMouseDown={() => setEngaged(true)}
                onClick={(e) =>
                    setCaret(e.currentTarget.selectionStart ?? e.currentTarget.value.length)
                }
                onBlur={() => window.setTimeout(() => setEngaged(false), 120)}
            />
            <SearchSuggest
                ref={suggestRef}
                text={value}
                caret={caret}
                fields={fields}
                indexes={indexes}
                start={start}
                end={end}
                active={engaged}
                onAccept={handleAccept}
            />
        </div>
    );
}
