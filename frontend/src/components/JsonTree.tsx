// Copyright 2026 Appbird LLC. Licensed under "GNU Affero General Public License v3.0"

import { useState } from "react";
import { ChevronRight } from "lucide-react";
import { ClickableText } from "./ClickableText";
import { Highlight } from "./Highlight";

interface Common {
    terms: string[];
    // Current query — only used for click-to-search active state. Optional in
    // read-only mode (no `onPickTerm`).
    query?: string;
    // Toggle a word as a search term; omit for a read-only tree (plain highlighted leaves).
    onPickTerm?: (term: string) => void;
    // Nodes at a depth below this start expanded; deeper ones start collapsed.
    defaultOpenDepth: number;
}

// A leaf token — interactive click-to-search when `onPickTerm` is set,
// otherwise plain text with term highlighting.
function Leaf({ text, terms, query, onPickTerm }: { text: string } & Common) {
    return onPickTerm ? (
        <ClickableText text={text} terms={terms} query={query ?? ""} onPickTerm={onPickTerm} />
    ) : (
        <Highlight text={text} terms={terms} />
    );
}

// Hierarchical collapsible JSON viewer; leaves use ClickableText for click-to-search + tint.
export function JsonTree({
    data,
    ...common
}: { data: Record<string, unknown> | unknown[] } & Common) {
    const entries = Array.isArray(data)
        ? data.map((v, i) => [String(i), v] as const)
        : Object.entries(data);
    return (
        <div className="leading-snug">
            {entries.map(([k, v]) => (
                <JsonNode
                    key={k}
                    nodeKey={k}
                    isIndex={Array.isArray(data)}
                    value={v}
                    depth={0}
                    {...common}
                />
            ))}
        </div>
    );
}

function isContainer(v: unknown): v is Record<string, unknown> | unknown[] {
    return v !== null && typeof v === "object";
}

function JsonNode({
    nodeKey,
    isIndex,
    value,
    depth,
    ...common
}: {
    nodeKey: string;
    isIndex: boolean;
    value: unknown;
    depth: number;
} & Common) {
    const { defaultOpenDepth } = common;
    const [open, setOpen] = useState(depth < defaultOpenDepth);
    const pad = { paddingLeft: depth * 14 } as const;

    // Object keys are clickable search tokens; array indices are a dim label.
    const keyEl = isIndex ? (
        <span className="text-stone-400 dark:text-stone-500">{nodeKey}</span>
    ) : (
        <span className="text-sky-700 dark:text-sky-300">
            <Leaf text={nodeKey} {...common} />
        </span>
    );

    const entries =
        isContainer(value) &&
        (Array.isArray(value)
            ? value.map((v, i) => [String(i), v] as const)
            : Object.entries(value));

    if (!entries || entries.length === 0) {
        return (
            <div className="flex" style={pad}>
                <span className="inline-block w-3.5 shrink-0" aria-hidden="true" />
                <span className="whitespace-pre-wrap break-all min-w-0">
                    {keyEl}
                    <span className="text-stone-400 dark:text-stone-500">: </span>
                    {isContainer(value) ? (
                        <span className="text-stone-400 dark:text-stone-500">
                            {Array.isArray(value) ? "[]" : "{}"}
                        </span>
                    ) : (
                        <ValueLeaf value={value} {...common} />
                    )}
                </span>
            </div>
        );
    }

    const isArr = Array.isArray(value);
    return (
        <div>
            <div
                className="flex items-center cursor-pointer rounded-sm hover:bg-stone-200/50 dark:hover:bg-stone-700/40"
                style={pad}
                onClick={(e) => {
                    e.stopPropagation();
                    setOpen((o) => !o);
                }}
            >
                <ChevronRight
                    className={`w-3.5 h-3.5 shrink-0 text-stone-600 dark:text-stone-300 transition-transform ${
                        open ? "rotate-90" : ""
                    }`}
                    aria-hidden="true"
                />
                {keyEl}
                <span className="ml-1.5 text-stone-600 dark:text-stone-300">
                    {isArr ? `[${entries.length}]` : `{${entries.length}}`}
                </span>
            </div>
            {open &&
                entries.map(([k, v]) => (
                    <JsonNode
                        key={k}
                        nodeKey={k}
                        isIndex={isArr}
                        value={v}
                        depth={depth + 1}
                        {...common}
                    />
                ))}
        </div>
    );
}

function ValueLeaf({ value, ...common }: { value: unknown } & Common) {
    if (value === null) {
        return <span className="text-stone-400 dark:text-stone-500 italic">null</span>;
    }
    if (typeof value === "string") {
        return (
            <span className="text-emerald-700 dark:text-emerald-400">
                &quot;
                <Leaf text={value} {...common} />
                &quot;
            </span>
        );
    }
    const cls =
        typeof value === "number"
            ? "text-blue-700 dark:text-blue-400"
            : "text-purple-700 dark:text-purple-400";
    return (
        <span className={cls}>
            <Leaf text={String(value)} {...common} />
        </span>
    );
}
