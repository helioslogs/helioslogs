// Copyright 2026 Appbird LLC. Licensed under "GNU Affero General Public License v3.0"

// Autocomplete dropdown for the search input: `lib/suggestContext`
// classifies the current token and this renders matching candidates ranked by category.

import {
    forwardRef,
    useCallback,
    useEffect,
    useImperativeHandle,
    useMemo,
    useRef,
    useState,
} from "react";
import { aggregate } from "../api/client";
import type { DiscoveredField } from "../api/types";
import { analyzeContext, type SuggestContext } from "../lib/suggestContext";
import { AGG_FUNCS, BOOLEAN_OPS, COMMANDS, STATS_BY, SYSTEM_FIELDS } from "../lib/suggestData";

interface Item {
    label: string;
    // Short right-aligned hint (e.g. agg description, value count, "field").
    detail?: string;
    // Text to splice into the input on accept; may add structural punctuation.
    insert: string;
    key: string;
    // Section header above the first item in each group; empty string suppresses it.
    group: string;
}

export interface SearchSuggestProps {
    text: string;
    caret: number;
    // Reused from the sidebar's discover_fields fetch to avoid double-fetching per keystroke.
    fields: DiscoveredField[];
    // Feeds `index:` value suggestions without hitting /api/aggregate.
    indexes: string[];
    // Time scope for value suggestions, so `service:` only suggests services seen in-window.
    start: string;
    end: string;
    onAccept: (next: string, caret: number) => void;
    // Actively engaging (typed/clicked), distinct from DOM focus — autoFocus on remount
    // gives focus but should NOT pop the menu.
    active: boolean;
}

export interface SearchSuggestHandle {
    // Returns true if the popover consumed the key event (caller should preventDefault).
    handleKey: (e: React.KeyboardEvent<HTMLInputElement>) => boolean;
    dismiss: () => void;
}

export const SearchSuggest = forwardRef<SearchSuggestHandle, SearchSuggestProps>(
    function SearchSuggest({ text, caret, fields, indexes, start, end, onAccept, active }, ref) {
        const context = useMemo(() => analyzeContext(text, caret), [text, caret]);

        const values = useFieldValues({
            context,
            indexes,
            start,
            end,
        });

        const items = useMemo(
            () => buildItems(context, fields, values.values),
            [context, fields, values.values],
        );

        // -1 = nothing selected, so Enter with no ↑/↓ falls through to form submit.
        const [selIdx, setSelIdx] = useState(-1);
        const [dismissed, setDismissed] = useState(false);

        // Reset selection (and any prior Esc dismissal) when the item set
        // changes — typing a new char gives the user a fresh shot at the menu.
        useEffect(() => {
            setSelIdx(-1);
            setDismissed(false);
        }, [text, caret]);

        const accept = useCallback(
            (item: Item) => {
                const next = text.slice(0, context.start) + item.insert + text.slice(context.end);
                const newCaret = context.start + item.insert.length;
                onAccept(next, newCaret);
                setDismissed(true);
            },
            [text, context, onAccept],
        );

        const visible = active && !dismissed && items.length > 0 && context.kind !== "none";

        useImperativeHandle(
            ref,
            () => ({
                // Re-check visibility per invocation so the handler stays correct
                // even when useImperativeHandle's memoisation doesn't re-run.
                handleKey: (e) => {
                    if (!visible) return false;
                    if (e.key === "ArrowDown") {
                        // First ↓ moves from "nothing selected" to the first item; from
                        // there it advances normally, stopping at the last.
                        setSelIdx((i) => Math.min(items.length - 1, i + 1));
                        return true;
                    }
                    if (e.key === "ArrowUp") {
                        // ↑ from the first item (or from "nothing selected") leaves the
                        // selection at -1, releasing the menu so Enter submits.
                        setSelIdx((i) => (i <= 0 ? -1 : i - 1));
                        return true;
                    }
                    if (e.key === "Enter") {
                        // Enter accepts only after explicit ↑/↓ navigation; otherwise
                        // return false so the form submits (runs the search).
                        if (selIdx < 0) return false;
                        const it = items[selIdx];
                        if (it) {
                            accept(it);
                            return true;
                        }
                        return false;
                    }
                    if (e.key === "Tab") {
                        // Tab completes to the active item, or the top match if none selected.
                        const it = items[selIdx >= 0 ? selIdx : 0];
                        if (it) {
                            accept(it);
                            return true;
                        }
                        return false;
                    }
                    if (e.key === "Escape") {
                        setDismissed(true);
                        return true;
                    }
                    return false;
                },
                dismiss: () => setDismissed(true),
            }),
            [items, selIdx, accept, visible],
        );

        if (!visible) return null;

        return (
            <div
                className="absolute top-full left-0 right-0 mt-1 z-50 max-h-80 overflow-auto rounded-md border border-stone-200 dark:border-stone-700 bg-white dark:bg-stone-900 shadow-lg"
                // Don't steal focus on hover; clicks use mousedown handlers below.
                onMouseDown={(e) => e.preventDefault()}
            >
                {items.map((it, i) => {
                    const showGroup =
                        it.group !== "" && (i === 0 || items[i - 1].group !== it.group);
                    return (
                        <div key={it.key}>
                            {showGroup && (
                                <div className="px-3 pt-2 pb-0.5 text-[10px] font-semibold uppercase tracking-wider text-stone-400 dark:text-stone-500">
                                    {it.group}
                                </div>
                            )}
                            <button
                                type="button"
                                className={`w-full flex items-center justify-between gap-3 px-3 py-1.5 text-left font-mono text-sm ${
                                    i === selIdx
                                        ? "bg-orange-50 text-orange-900 dark:bg-orange-950/40 dark:text-orange-100"
                                        : "text-stone-800 dark:text-stone-200 hover:bg-stone-50 dark:hover:bg-stone-800/50"
                                }`}
                                onMouseEnter={() => setSelIdx(i)}
                                onMouseDown={(e) => {
                                    e.preventDefault();
                                    accept(it);
                                }}
                            >
                                <span className="truncate">{it.label}</span>
                                {it.detail && (
                                    <span className="shrink-0 text-xs text-stone-400 dark:text-stone-500 font-sans">
                                        {it.detail}
                                    </span>
                                )}
                            </button>
                        </div>
                    );
                })}
                {values.loading && (
                    <div className="px-3 py-1.5 text-xs text-stone-400 dark:text-stone-500 font-sans border-t border-stone-100 dark:border-stone-800">
                        looking up values…
                    </div>
                )}
            </div>
        );
    },
);

// ============================================================================
// Item construction — turn a SuggestContext + data into a ranked Item list
// ============================================================================

function buildItems(
    context: SuggestContext,
    fields: DiscoveredField[],
    values: SuggestValue[],
): Item[] {
    switch (context.kind) {
        case "command":
            return COMMANDS.filter((c) => c.label.toLowerCase().startsWith(context.prefix)).map(
                (c) => ({
                    label: c.label,
                    detail: c.detail,
                    insert: c.insert,
                    key: `cmd:${c.label}`,
                    group: "",
                }),
            );

        case "agg": {
            const aggs = AGG_FUNCS.filter((a) =>
                a.label.toLowerCase().startsWith(context.prefix),
            ).map((a) => ({
                label: a.label,
                detail: a.detail,
                insert: a.insert,
                key: `agg:${a.label}`,
                group: "aggregations",
            }));
            // Always offer `by` so the user can pivot to grouping after the first
            // agg without remembering the keyword.
            const showBy = STATS_BY.label.startsWith(context.prefix);
            return showBy
                ? [
                      ...aggs,
                      {
                          label: STATS_BY.label,
                          detail: STATS_BY.detail,
                          insert: STATS_BY.insert,
                          key: "agg:by",
                          group: "aggregations",
                      },
                  ]
                : aggs;
        }

        case "field":
        case "stats-field":
        case "arg-field": {
            // Merge known system fields (absent from discover_fields) ahead of
            // discovered ones, since they're the most commonly-typed filters.
            const seen = new Set<string>();
            const candidates: { name: string; detail: string }[] = [];
            for (const sf of SYSTEM_FIELDS) {
                if (seen.has(sf.name)) continue;
                seen.add(sf.name);
                candidates.push({ name: sf.name, detail: sf.detail });
            }
            for (const f of fields) {
                if (seen.has(f.name)) continue;
                seen.add(f.name);
                candidates.push({ name: f.name, detail: fieldDetail(f) });
            }

            // Match full name OR leaf segment so `qty` surfaces `items.qty`
            // (mirrors backend bare-leaf resolution); full-prefix matches rank first.
            const leafStarts = (name: string) => {
                const i = name.lastIndexOf(".");
                return i >= 0 && name.slice(i + 1).startsWith(context.prefix);
            };
            const matchedFields = candidates
                .filter((f) => {
                    const n = f.name.toLowerCase();
                    return n.startsWith(context.prefix) || leafStarts(n);
                })
                .sort(
                    (a, b) =>
                        (a.name.toLowerCase().startsWith(context.prefix) ? 0 : 1) -
                        (b.name.toLowerCase().startsWith(context.prefix) ? 0 : 1),
                )
                .slice(0, 20)
                .map<Item>((f) => ({
                    label: f.name,
                    detail: f.detail,
                    // Main search segment appends `:` to flow into a value;
                    // stats/arg-field positions want a bare name plus space.
                    insert: context.kind === "field" ? `${f.name}:` : `${f.name} `,
                    key: `field:${f.name}`,
                    group: "fields",
                }));

            if (context.kind === "field") {
                // Booleans only at clean term boundaries — never at the very start
                // of a query, and never as the very first chars of a token.
                const showBooleans =
                    context.atTermBoundary && context.prefix === ""
                        ? true
                        : context.prefix.length > 0;
                const booleans = showBooleans
                    ? BOOLEAN_OPS.filter((b) =>
                          b.label.toLowerCase().startsWith(context.prefix.toLowerCase()),
                      ).map<Item>((b) => ({
                          label: b.label,
                          detail: b.detail,
                          insert: b.insert,
                          key: `bool:${b.label}`,
                          group: "operators",
                      }))
                    : [];
                // Booleans first at a term boundary with no prefix — that's when
                // AND/OR are what the user is reaching for.
                if (context.atTermBoundary && context.prefix === "") {
                    return [...booleans, ...matchedFields];
                }
                return [...matchedFields, ...booleans];
            }
            return matchedFields;
        }

        case "value":
            return values
                .filter((v) => v.label.toLowerCase().startsWith(context.prefix))
                .slice(0, 20)
                .map<Item>((v) => ({
                    label: v.label,
                    detail: v.detail,
                    insert: valueInsert(v.label, context.quoted ?? false),
                    key: `val:${v.label}`,
                    group: "values",
                }));

        case "none":
            return [];
    }
}

function fieldDetail(f: DiscoveredField): string {
    const pct = Math.round(f.coverage * 100);
    const card = f.cardinality > 0 ? ` · ~${f.cardinality} distinct` : "";
    return `${f.value_kind} · ${pct}%${card}`;
}

// Quote values with whitespace or lexer-structural chars; bare if already quoted.
function valueInsert(value: string, alreadyQuoted: boolean): string {
    if (alreadyQuoted) return value;
    if (/[\s"():|]/.test(value)) {
        return `"${value.replace(/"/g, '\\"')}"`;
    }
    return value;
}

// ============================================================================
// Field-value lookup hook — debounced + cached
// ============================================================================

interface SuggestValue {
    label: string;
    detail?: string;
}

interface FetchKey {
    field: string;
    start: string;
    end: string;
}

function keyOf(k: FetchKey): string {
    return `${k.field}|${k.start}|${k.end}`;
}

// Lazy-loads + caches top values per (field, time-range) for the field being
// completed. Special-cases `index:` to the in-memory partition list.
function useFieldValues(args: {
    context: SuggestContext;
    indexes: string[];
    start: string;
    end: string;
}): { values: SuggestValue[]; loading: boolean } {
    const { context, indexes, start, end } = args;
    const [cache, setCache] = useState<Record<string, SuggestValue[]>>({});
    const [loading, setLoading] = useState(false);
    const debounceRef = useRef<number | null>(null);

    const field = context.kind === "value" ? (context.field ?? "") : "";
    const key = field ? keyOf({ field, start, end }) : "";

    useEffect(() => {
        if (!field) return;
        // `index:` is the partition key; the list is already in memory, no /api/aggregate.
        if (field === "index") {
            setCache((c) => (c[key] ? c : { ...c, [key]: indexes.map((i) => ({ label: i })) }));
            return;
        }
        if (cache[key]) return;

        if (debounceRef.current !== null) {
            window.clearTimeout(debounceRef.current);
        }
        setLoading(true);
        debounceRef.current = window.setTimeout(async () => {
            try {
                const resp = await aggregate({
                    q: "*",
                    start,
                    end,
                    fields: field,
                    size: 50,
                });
                const buckets = resp.aggs[field] ?? [];
                const vals: SuggestValue[] = buckets.map((b) => ({
                    label: String(b.key),
                    detail: formatCount(b.count),
                }));
                setCache((c) => ({ ...c, [key]: vals }));
            } catch {
                // Silent: a failed lookup shouldn't be a visible error; empty slot retries later.
                setCache((c) => ({ ...c, [key]: [] }));
            } finally {
                setLoading(false);
            }
        }, 150);

        return () => {
            if (debounceRef.current !== null) {
                window.clearTimeout(debounceRef.current);
                debounceRef.current = null;
            }
        };
    }, [field, key, start, end, indexes, cache]);

    const values = key ? (cache[key] ?? []) : [];
    return { values, loading: !!field && !cache[key] && loading };
}

function formatCount(n: number): string {
    if (n >= 1_000_000) return `${(n / 1_000_000).toFixed(1)}M`;
    if (n >= 1_000) return `${(n / 1_000).toFixed(1)}k`;
    return String(n);
}
