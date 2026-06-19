// Copyright 2026 Appbird LLC. Licensed under "GNU Affero General Public License v3.0"

// URL ↔ SearchInput plumbing. The URL is the source of truth for the
// search view; this module owns the read/write boundary so the rest of
// the app deals in `SearchInput` objects only.

import { getEnv } from "../api/client";

export interface SearchInput {
    q: string;
    range: string; // "-15m", "-1h", "-6h", "-24h" — used when start/end aren't set
    follow: boolean;
    // Absolute time bounds (ISO 8601). Set by clicking a histogram bucket.
    start?: string;
    end?: string;
    // Index filter. Empty string / undefined = scatter-gather across all indexes.
    index?: string;
    // 1-based page number; reset to 1 on any non-pagination state change.
    page?: number;
    // Optional env override for shareable links; unset = active env. Not read back from URL.
    env?: string;
}

export const DEFAULTS: SearchInput = { q: "*", range: "-6h", follow: false };

export function readUrl(): SearchInput {
    const p = new URLSearchParams(window.location.search);
    const start = p.get("start") ?? undefined;
    const end = p.get("end") ?? undefined;
    const pageRaw = parseInt(p.get("page") ?? "", 10);
    const page = Number.isFinite(pageRaw) && pageRaw > 0 ? pageRaw : 1;
    // Accept legacy `source=` query param so existing bookmarks still work.
    const indexParam = p.get("index") ?? p.get("source") ?? undefined;
    return {
        q: p.get("q") ?? DEFAULTS.q,
        range: p.get("range") ?? DEFAULTS.range,
        follow: p.get("follow") === "1",
        start,
        end,
        index: indexParam,
        page,
    };
}

export function sameInput(a: SearchInput, b: SearchInput): boolean {
    return (
        a.q === b.q &&
        a.range === b.range &&
        a.follow === b.follow &&
        (a.start ?? "") === (b.start ?? "") &&
        (a.end ?? "") === (b.end ?? "") &&
        (a.index ?? "") === (b.index ?? "") &&
        (a.page ?? 1) === (b.page ?? 1)
    );
}

// Path of the search route. The router redirects "/" here.
export const SEARCH_PATH = "/search";

// Serialize an input to a query string. Defaults are omitted to keep URLs tidy;
// when absolute start/end are set they win and `range` is omitted.
function toQueryString(s: SearchInput): string {
    const p = new URLSearchParams();
    // Always pin an env so the URL is self-describing and shareable (startup adopts
    // `?env=`). Callers can override to open results in a specific env.
    p.set("env", s.env ?? getEnv());
    const hasAbsolute = !!(s.start && s.end);
    if (s.q !== DEFAULTS.q) p.set("q", s.q);
    if (s.index) p.set("index", s.index);
    if (hasAbsolute) {
        p.set("start", s.start!);
        p.set("end", s.end!);
    } else {
        // range is always explicit — it's a primary part of the search, not
        // noise worth hiding, so it stays in the URL even at the default.
        p.set("range", s.range);
        if (s.follow) p.set("follow", "1");
    }
    if ((s.page ?? 1) > 1) p.set("page", String(s.page));
    return p.toString();
}

// The `/search?…` URL that represents this input — for <Link>s and
// cross-route navigation (e.g. loading a saved search from the saved page).
export function searchHref(s: SearchInput): string {
    const qs = toQueryString(s);
    return qs ? `${SEARCH_PATH}?${qs}` : SEARCH_PATH;
}

// Remember the user's last search across the session so the top-nav "Search"
// item restores it instead of resetting to defaults. Env is intentionally not
// stored — restoring always uses the currently active env (see searchHref).
const LAST_SEARCH_KEY = "helios.lastSearch";

export function saveLastSearch(s: SearchInput): void {
    try {
        const { env: _env, ...rest } = s;
        void _env;
        sessionStorage.setItem(LAST_SEARCH_KEY, JSON.stringify(rest));
    } catch {
        // sessionStorage disabled / quota — restore just won't be available.
    }
}

export function loadLastSearch(): SearchInput | null {
    try {
        const raw = sessionStorage.getItem(LAST_SEARCH_KEY);
        if (!raw) return null;
        const o = JSON.parse(raw);
        if (typeof o !== "object" || o === null) return null;
        // Merge over DEFAULTS so a partial/old payload still yields a valid input.
        return { ...DEFAULTS, ...(o as Partial<SearchInput>) };
    } catch {
        return null;
    }
}
