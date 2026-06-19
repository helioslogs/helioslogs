// Copyright 2026 Appbird LLC. Licensed under "GNU Affero General Public License v3.0"

import { Fragment, type ReactNode } from "react";
import { isTermActive } from "../lib/query";

// A clickable "word": alphanumeric run with inner dots/dashes/underscores kept
// whole; colons/slashes split, to avoid emitting query-confusing `key:value` tokens.
const WORD_RE = /[A-Za-z0-9][A-Za-z0-9._-]*/g;

interface Props {
    text: string;
    // Active search terms — matching words get a highlight tint.
    terms: string[];
    // Current query, to mark words already added (so a click removes them).
    query: string;
    // Append/remove this word as a bare search term.
    onPickTerm: (term: string) => void;
}

// Renders text with each word as a click-to-search token; words already in the
// query show active (click removes), highlight-term matches get a tint.
export function ClickableText({ text, terms, query, onPickTerm }: Props) {
    const termSet = new Set(terms.map((t) => t.toLowerCase()));
    const out: ReactNode[] = [];
    let last = 0;
    let i = 0;
    let m: RegExpExecArray | null;
    WORD_RE.lastIndex = 0;
    while ((m = WORD_RE.exec(text)) !== null) {
        if (m.index > last) {
            out.push(<Fragment key={`g${i}`}>{text.slice(last, m.index)}</Fragment>);
        }
        const word = m[0];
        const active = isTermActive(query, word);
        const highlighted = !active && termSet.has(word.toLowerCase());
        out.push(
            <span
                key={`w${i}`}
                onClick={(e) => {
                    e.stopPropagation();
                    onPickTerm(word);
                }}
                title={active ? `remove from search: ${word}` : `add to search: ${word}`}
                className={`cursor-pointer rounded-sm hover:bg-blue-200/70 dark:hover:bg-blue-800/50 hover:underline ${
                    active
                        ? "bg-blue-200/80 dark:bg-blue-800/60 underline decoration-blue-500"
                        : highlighted
                          ? "bg-amber-200/70 dark:bg-amber-700/40"
                          : ""
                }`}
            >
                {word}
            </span>,
        );
        last = m.index + word.length;
        i += 1;
    }
    if (last < text.length) {
        out.push(<Fragment key="tail">{text.slice(last)}</Fragment>);
    }
    return <>{out}</>;
}
