// Copyright 2026 Appbird LLC. Licensed under "GNU Affero General Public License v3.0"

import { Fragment, type ReactNode } from "react";

interface Props {
    text: string | null | undefined;
    terms: string[];
}

// Renders `text` with `<mark>` around case-insensitive `terms`; overlaps merged to avoid nesting.
export function Highlight({ text, terms }: Props): ReactNode {
    if (!text) return text ?? null;
    if (terms.length === 0) return text;

    const lower = text.toLowerCase();
    const ranges: Array<[number, number]> = [];
    for (const term of terms) {
        if (!term) continue;
        const t = term.toLowerCase();
        if (!t) continue;
        let i = 0;
        while ((i = lower.indexOf(t, i)) !== -1) {
            ranges.push([i, i + t.length]);
            i += t.length;
        }
    }
    if (ranges.length === 0) return text;

    ranges.sort((a, b) => a[0] - b[0] || a[1] - b[1]);
    const merged: Array<[number, number]> = [ranges[0]];
    for (let i = 1; i < ranges.length; i++) {
        const last = merged[merged.length - 1];
        if (ranges[i][0] <= last[1]) {
            last[1] = Math.max(last[1], ranges[i][1]);
        } else {
            merged.push(ranges[i]);
        }
    }

    const parts: ReactNode[] = [];
    let pos = 0;
    for (let i = 0; i < merged.length; i++) {
        const [s, e] = merged[i];
        if (pos < s) parts.push(<Fragment key={`p${i}`}>{text.slice(pos, s)}</Fragment>);
        parts.push(
            <mark
                key={`m${i}`}
                className="bg-blue-100 dark:bg-blue-900/50 text-stone-900 dark:text-blue-100 rounded-sm px-0.5"
            >
                {text.slice(s, e)}
            </mark>,
        );
        pos = e;
    }
    if (pos < text.length) parts.push(<Fragment key="end">{text.slice(pos)}</Fragment>);
    return <>{parts}</>;
}
