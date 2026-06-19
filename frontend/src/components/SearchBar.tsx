// Copyright 2026 Appbird LLC. Licensed under "GNU Affero General Public License v3.0"

import { useCallback, useEffect, useLayoutEffect, useRef, useState, type ReactNode } from "react";
import { Search as SearchIcon, Star } from "lucide-react";
import { createSearch } from "../api/client";
import type { DiscoveredField } from "../api/types";
import { notifySavedChanged } from "../api/events";
import { sameAsCurrent, suggestName } from "../lib/query";
import { useSavedSearches } from "../state/useSavedSearches";
import type { SearchInput } from "../state/url";
import { SaveSearchDialog } from "./SaveSearchDialog";
import { SavedSearchesMenu } from "./SavedSearchesMenu";
import { SearchSuggest, type SearchSuggestHandle } from "./SearchSuggest";
import { TimeRangePicker } from "./TimeRangePicker";

interface Props {
    initial: SearchInput;
    onSubmit: (s: SearchInput) => void;
    indexes: string[];
    // The currently-displayed state (not the form draft) for Save/popover actions.
    current: SearchInput;
    onLoadSaved: (s: SearchInput) => void;
    // Discovered fields for the autocomplete popover; reused from the sidebar fetch.
    fields: DiscoveredField[];
    // Effective time range scoping value lookups in the autocomplete popover.
    start: string;
    end: string;
    refreshControl?: ReactNode;
}

// Shared field-control classes — Atlas-style: stone-50 bg, focus orange ring.
const FIELD =
    "px-2.5 py-1.5 bg-stone-50 dark:bg-stone-950 border border-stone-200 dark:border-stone-700 rounded-md focus:outline-none focus:border-orange-500 focus:bg-white dark:focus:bg-stone-900 text-stone-900 dark:text-stone-100";

export function SearchBar({
    initial,
    onSubmit,
    indexes,
    current,
    onLoadSaved,
    fields,
    start,
    end,
    refreshControl,
}: Props) {
    const [q, setQ] = useState(initial.q);
    const [range, setRange] = useState(initial.range);
    const [follow, setFollow] = useState(initial.follow);
    // No picker (filtering lives in the query as `index:foo`); thread it through so it survives submits.
    const index = initial.index ?? "";
    // Absolute bounds: both set (wins over `range`) or both undefined (relative mode).
    const [absStart, setAbsStart] = useState<string | undefined>(initial.start);
    const [absEnd, setAbsEnd] = useState<string | undefined>(initial.end);
    const { items: saved } = useSavedSearches();

    // Caret tracking — the autocomplete popover keys off (text, caret). We
    // update it on every selection-changing event the input emits.
    const inputRef = useRef<HTMLInputElement>(null);
    const [caret, setCaret] = useState(0);
    // "Engaged" not "focused": true only on real interaction, since autoFocus
    // on remount would otherwise re-pop the menu on programmatic refocus.
    const [engaged, setEngaged] = useState(false);
    const suggestRef = useRef<SearchSuggestHandle>(null);
    // Caret queued by an accepted suggestion, applied post-render once `q` flushes to the DOM.
    const pendingCaretRef = useRef<number | null>(null);

    useEffect(() => {
        setQ(initial.q);
    }, [initial.q]);
    // Sync absolute bounds on parent rewrites; absolute-only changes don't remount via `key`.
    useEffect(() => {
        setAbsStart(initial.start);
        setAbsEnd(initial.end);
    }, [initial.start, initial.end]);

    // Auto-submit on "live" control changes (no submit button); fingerprint-guarded
    // to skip the initial mount, StrictMode double-invokes, and same-value remounts.
    const lastLiveFingerprintRef = useRef<string | null>(null);
    useEffect(() => {
        const fingerprint = `${range}|${follow ? 1 : 0}|${index}|${absStart ?? ""}|${absEnd ?? ""}`;
        if (lastLiveFingerprintRef.current === null) {
            lastLiveFingerprintRef.current = fingerprint;
            return;
        }
        if (lastLiveFingerprintRef.current === fingerprint) return;
        lastLiveFingerprintRef.current = fingerprint;
        onSubmit({
            q,
            range,
            follow,
            index: index || undefined,
            start: absStart,
            end: absEnd,
        });
        // eslint-disable-next-line react-hooks/exhaustive-deps
    }, [range, follow, index, absStart, absEnd]);

    useLayoutEffect(() => {
        if (pendingCaretRef.current !== null && inputRef.current) {
            const pos = pendingCaretRef.current;
            inputRef.current.setSelectionRange(pos, pos);
            setCaret(pos);
            pendingCaretRef.current = null;
        }
    }, [q]);

    const handleAccept = useCallback((next: string, newCaret: number) => {
        pendingCaretRef.current = newCaret;
        setQ(next);
    }, []);

    // If the current view matches an existing saved search, mark the star
    // filled and label it with that name.
    const matchedSaved = saved.find((s) => sameAsCurrent(s, current));

    // null when closed; pins a `current` snapshot so an in-flight update can't shift the save.
    const [saveDraft, setSaveDraft] = useState<SearchInput | null>(null);

    const handleSaveClick = useCallback(() => {
        setSaveDraft(current);
    }, [current]);

    const handleSaveConfirm = useCallback(
        async ({ name, public: isPublic }: { name: string; public: boolean }) => {
            if (!saveDraft) return;
            try {
                await createSearch({
                    name,
                    q: saveDraft.q,
                    index: saveDraft.index,
                    range: saveDraft.range,
                    start: saveDraft.start,
                    end: saveDraft.end,
                    follow: saveDraft.follow,
                    public: isPublic,
                });
                notifySavedChanged();
                setSaveDraft(null);
            } catch (e: unknown) {
                window.alert(e instanceof Error ? e.message : String(e));
            }
        },
        [saveDraft],
    );

    return (
        <form
            className="flex items-center gap-2 flex-wrap"
            onSubmit={(e) => {
                e.preventDefault();
                suggestRef.current?.dismiss();
                onSubmit({
                    q,
                    range,
                    follow,
                    index: index || undefined,
                    start: absStart,
                    end: absEnd,
                });
            }}
        >
            {/* Anchors the absolute popover under the input; flex-grow here (not the
          input) lets it stretch to full width without measuring. */}
            <div className="relative flex-grow min-w-[280px]">
                <SearchIcon
                    className="pointer-events-none absolute left-2.5 top-1/2 -translate-y-1/2 w-4 h-4 text-stone-400 dark:text-stone-500"
                    aria-hidden="true"
                />
                <input
                    ref={inputRef}
                    className={`${FIELD} w-full font-mono pl-9`}
                    type="text"
                    placeholder='try: level:ERROR  |  index:*webhooks  |  status:500  |  "upstream call failed"'
                    value={q}
                    onChange={(e) => {
                        setQ(e.target.value);
                        setCaret(e.target.selectionStart ?? e.target.value.length);
                        setEngaged(true);
                    }}
                    onKeyDown={(e) => {
                        if (suggestRef.current?.handleKey(e)) {
                            e.preventDefault();
                        }
                    }}
                    onKeyUp={(e) => {
                        // ←/→/Home/End/Cmd-A move the caret without firing onChange;
                        // sync our local caret on every keyup to stay aligned.
                        const t = e.currentTarget;
                        setCaret(t.selectionStart ?? t.value.length);
                    }}
                    // mousedown fires before focus, so a click arms the popover on the same event.
                    onMouseDown={() => setEngaged(true)}
                    onClick={(e) => {
                        const t = e.currentTarget;
                        setCaret(t.selectionStart ?? t.value.length);
                    }}
                    onFocus={(e) => {
                        // Don't engage here: autoFocus on remount lands here too, and the
                        // user didn't ask for suggestions. Engagement comes from typing/mousedown/Tab.
                        setCaret(e.currentTarget.selectionStart ?? e.currentTarget.value.length);
                    }}
                    // Blur delay so a mousedown-accept on a popover row lands before we hide it.
                    onBlur={() => window.setTimeout(() => setEngaged(false), 100)}
                    autoComplete="off"
                    spellCheck={false}
                    autoFocus
                />
                <SearchSuggest
                    ref={suggestRef}
                    text={q}
                    caret={caret}
                    fields={fields}
                    indexes={indexes}
                    start={start}
                    end={end}
                    active={engaged}
                    onAccept={handleAccept}
                />
            </div>

            <TimeRangePicker
                range={range}
                start={absStart}
                end={absEnd}
                disabled={follow}
                onChange={(next) => {
                    // Relative pick clears absolute; absolute pick installs bounds and
                    // clears `follow` (fixed window and live-tail are mutually exclusive).
                    if (next.range !== undefined) {
                        setRange(next.range);
                        setAbsStart(undefined);
                        setAbsEnd(undefined);
                    } else if (next.start && next.end) {
                        setAbsStart(next.start);
                        setAbsEnd(next.end);
                        setFollow(false);
                    }
                }}
            />

            <button
                type="submit"
                className="px-3 py-1.5 font-medium text-white bg-orange-600 hover:bg-orange-500 rounded-md transition"
            >
                Search
            </button>

            <button
                type="button"
                onClick={handleSaveClick}
                className={`px-2.5 py-1.5 rounded-md border transition flex items-center gap-1.5 ${
                    matchedSaved
                        ? "border-orange-300 bg-orange-50 text-orange-900 dark:border-orange-700 dark:bg-orange-950/40 dark:text-orange-200"
                        : "border-stone-200 dark:border-stone-700 hover:border-orange-300 hover:bg-orange-50/40 dark:hover:bg-orange-950/30 text-stone-700 dark:text-stone-300"
                }`}
                title={
                    matchedSaved
                        ? `Currently viewing "${matchedSaved.name}" — click to save as new`
                        : "Save current view"
                }
            >
                <Star className="w-3.5 h-3.5" fill={matchedSaved ? "currentColor" : "none"} />
                <span className="max-w-[140px] truncate">
                    {matchedSaved ? matchedSaved.name : "Save"}
                </span>
            </button>

            <SavedSearchesMenu current={current} onLoad={onLoadSaved} />

            {/* Live controls grouped right: refresh next to the follow-live toggle. */}
            <div className="ml-auto flex items-center gap-2">
                {refreshControl}

                <label
                    className={`flex items-center gap-1.5 px-2.5 py-1.5 rounded-md border cursor-pointer select-none transition ${
                        follow
                            ? "border-orange-500 bg-orange-600 text-white shadow-sm"
                            : "border-stone-200 dark:border-stone-700 text-stone-600 dark:text-stone-400 hover:border-orange-300 hover:bg-orange-50/40 dark:hover:bg-orange-950/30"
                    }`}
                    title={
                        follow
                            ? "Following live — tailing new results every 2s. Click to stop."
                            : "Follow live — tail new results in real time"
                    }
                >
                    <input
                        type="checkbox"
                        checked={follow}
                        onChange={(e) => setFollow(e.target.checked)}
                        className="sr-only"
                    />
                    <span className="relative flex h-2 w-2" aria-hidden="true">
                        {follow && (
                            <span className="absolute inline-flex h-full w-full rounded-full bg-white opacity-75 animate-ping" />
                        )}
                        <span
                            className={`relative inline-flex h-2 w-2 rounded-full ${
                                follow ? "bg-white" : "bg-stone-400 dark:bg-stone-500"
                            }`}
                        />
                    </span>
                    <span className={follow ? "font-medium" : ""}>
                        {follow ? "Following live" : "Follow live"}
                    </span>
                </label>
            </div>

            {saveDraft && (
                <SaveSearchDialog
                    initialName={suggestName(saveDraft)}
                    preview={saveDraft}
                    onSave={handleSaveConfirm}
                    onClose={() => setSaveDraft(null)}
                />
            )}
        </form>
    );
}
