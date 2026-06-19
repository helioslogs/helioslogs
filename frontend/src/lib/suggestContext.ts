// Copyright 2026 Appbird LLC. Licensed under "GNU Affero General Public License v3.0"

// Pure cursor-context analyzer for the search input: from query text + caret position,
// decide what token the user is typing and what range an accepted suggestion replaces.

export type SuggestKind =
    // First token after `|` — a pipe command.
    | "command"
    // Bare word in the main segment — field names plus boolean operators.
    | "field"
    // Text after `<field>:` — value completion for that field.
    | "value"
    // Argument position of `stats` — agg functions plus `by`.
    | "agg"
    // After `stats … by` — field name(s) to group on.
    | "stats-field"
    // After `top` / `rare` / `sort` — primary field argument.
    | "arg-field"
    // Nothing to suggest (mid-quoted phrase, after `head`/`tail`, etc.).
    | "none";

export interface SuggestContext {
    kind: SuggestKind;
    // Lowercased prefix candidates must match; empty = show everything.
    prefix: string;
    // Half-open range `[start, end)` an accepted suggestion replaces.
    start: number;
    end: number;
    // For `kind: "value"` — the field whose value is being typed.
    field?: string;
    // Value typed inside an open `"..."` — popover skips auto-quoting on insert.
    quoted?: boolean;
    // Cursor follows a complete term, so AND/OR/NOT make sense (kind "field" only).
    atTermBoundary?: boolean;
}

// Walk `before` forward tracking quote state; return the index of the last
// top-level `|` (one not inside `"..."`), or -1.
function lastTopLevelPipe(before: string): number {
    let inQuote = false;
    let last = -1;
    for (let i = 0; i < before.length; i++) {
        const c = before[i];
        if (inQuote) {
            if (c === "\\" && i + 1 < before.length) {
                i++;
                continue;
            }
            if (c === '"') inQuote = false;
            continue;
        }
        if (c === '"') inQuote = true;
        else if (c === "|") last = i;
    }
    return last;
}

// If `before` ends inside an open quoted string, return the index of that
// opening `"`. Otherwise -1.
function openQuoteStart(before: string): number {
    let inQuote = false;
    let start = -1;
    for (let i = 0; i < before.length; i++) {
        const c = before[i];
        if (inQuote) {
            if (c === "\\" && i + 1 < before.length) {
                i++;
                continue;
            }
            if (c === '"') {
                inQuote = false;
                start = -1;
            }
            continue;
        }
        if (c === '"') {
            inQuote = true;
            start = i;
        }
    }
    return inQuote ? start : -1;
}

// Start of the current token, walking back from `cursor`. Stops at whitespace,
// `|`, `(`, `)`, or `"` (open-quote case is handled by `openQuoteStart`).
function tokenStart(text: string, cursor: number): number {
    let i = cursor;
    while (i > 0) {
        const c = text[i - 1];
        if (
            c === " " ||
            c === "\t" ||
            c === "\n" ||
            c === "|" ||
            c === "(" ||
            c === ")" ||
            c === '"'
        ) {
            break;
        }
        i--;
    }
    return i;
}

// True at a fresh token boundary after a completed term — decides whether
// AND/OR/NOT make sense.
function atTermBoundary(before: string): boolean {
    if (before.length === 0) return false;
    const last = before[before.length - 1];
    if (last !== " " && last !== "\t") return false;
    const trimmed = before.trimEnd();
    if (trimmed.length === 0) return false;
    // Don't suggest booleans straight after `AND`/`OR`/`NOT` themselves.
    const tail = trimmed.split(/\s+/).pop() ?? "";
    if (tail === "AND" || tail === "OR" || tail === "NOT") return false;
    return true;
}

export function analyzeContext(text: string, cursor: number): SuggestContext {
    const before = text.slice(0, cursor);

    // Cursor inside an open quote: either `field:"…` (a value) or a bare
    // `"…` phrase (nothing to suggest). Everything below assumes outside quotes.
    const oq = openQuoteStart(before);
    if (oq >= 0) {
        // Is the char just before the quote a `:` preceded by an identifier?
        // If so it's a quoted value for that field.
        const justBefore = before.slice(0, oq);
        const m = justBefore.match(/([A-Za-z_][\w.]*):$/);
        if (m) {
            const field = m[1];
            const value = before.slice(oq + 1);
            return {
                kind: "value",
                prefix: value.toLowerCase(),
                start: oq + 1,
                end: cursor,
                field,
                quoted: true,
            };
        }
        return { kind: "none", prefix: "", start: cursor, end: cursor };
    }

    // --- Pipe segmentation ----------------------------------------------
    const pipeIdx = lastTopLevelPipe(before);
    const segStart = pipeIdx + 1;
    const segText = before.slice(segStart);
    const isPipeSegment = pipeIdx >= 0;

    const curStart = tokenStart(text, cursor);
    const curText = text.slice(curStart, cursor);

    if (isPipeSegment) {
        return analyzePipeSegment(segText, segStart, curStart, curText, cursor);
    }
    return analyzeMainSegment(before, curStart, curText, cursor);
}

function analyzeMainSegment(
    before: string,
    curStart: number,
    curText: string,
    cursor: number,
): SuggestContext {
    // `field:value` — split on the first `:` in the current token.
    const colon = curText.indexOf(":");
    if (colon >= 0) {
        const field = curText.slice(0, colon);
        let valueRaw = curText.slice(colon + 1);
        let valueStart = curStart + colon + 1;
        // Trim a leading comparison operator (`>=`/`<=`/`>`/`<`) from the
        // prefix match so values still surface for numeric `where` clauses.
        let opLen = 0;
        if (valueRaw.startsWith(">=") || valueRaw.startsWith("<=")) opLen = 2;
        else if (valueRaw.startsWith(">") || valueRaw.startsWith("<")) opLen = 1;
        valueRaw = valueRaw.slice(opLen);
        valueStart += opLen;
        // Opening quote handled in the `openQuoteStart` path above — by the time
        // we're here the value is unquoted bare text.
        return {
            kind: "value",
            prefix: valueRaw.toLowerCase(),
            start: valueStart,
            end: cursor,
            field,
        };
    }

    // Bare word — offer field names (rendered as `field:`) plus booleans at a
    // clean term boundary.
    const beforeCur = before.slice(0, curStart);
    return {
        kind: "field",
        prefix: curText.toLowerCase(),
        start: curStart,
        end: cursor,
        atTermBoundary: atTermBoundary(beforeCur),
    };
}

function analyzePipeSegment(
    segText: string,
    segStart: number,
    curStart: number,
    curText: string,
    cursor: number,
): SuggestContext {
    // First non-whitespace token of the segment is a command name: true when
    // everything between `segStart` and the current token is whitespace.
    const beforeCurInSeg = segText.slice(0, curStart - segStart);
    if (beforeCurInSeg.trim() === "") {
        return {
            kind: "command",
            prefix: curText.toLowerCase(),
            start: curStart,
            end: cursor,
        };
    }

    // Past the first token — context depends on which command opened the
    // segment. Extract the leading word case-insensitively.
    const cmdMatch = segText.match(/^\s*(\S+)/);
    const cmd = cmdMatch ? cmdMatch[1].toLowerCase() : "";

    if (cmd === "stats") {
        // In `stats`, `by` switches from agg suggestions to a group-by field list.
        const inSegBeforeCur = segText.slice(0, curStart - segStart);
        if (/\bby\b/i.test(inSegBeforeCur)) {
            return {
                kind: "stats-field",
                prefix: curText.toLowerCase(),
                start: curStart,
                end: cursor,
            };
        }
        return {
            kind: "agg",
            prefix: curText.toLowerCase(),
            start: curStart,
            end: cursor,
        };
    }

    if (cmd === "top" || cmd === "rare" || cmd === "sort") {
        // sort accepts a leading `-` or `+` to flip direction. Strip it for
        // prefix matching so `sort -lat` still finds `latency_ms`.
        let prefix = curText;
        let start = curStart;
        if (cmd === "sort" && (prefix.startsWith("-") || prefix.startsWith("+"))) {
            prefix = prefix.slice(1);
            start = curStart + 1;
        }
        return {
            kind: "arg-field",
            prefix: prefix.toLowerCase(),
            start,
            end: cursor,
        };
    }

    // `head` / `tail` take a number — no useful name suggestions.
    return { kind: "none", prefix: "", start: cursor, end: cursor };
}
