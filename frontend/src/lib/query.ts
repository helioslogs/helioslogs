// Copyright 2026 Appbird LLC. Licensed under "GNU Affero General Public License v3.0"

import type { SavedSearch } from "../api/types";
import type { SearchInput } from "../state/url";

// Canonical `field:value` clause — numbers bare, strings quoted so hyphens/dots
// don't trip the parser. Shared so add/toggle/active checks agree exactly.
export function filterClause(field: string, value: string | number): string {
    const v = typeof value === "number" ? String(value) : `"${String(value).replace(/"/g, '\\"')}"`;
    return `${field}:${v}`;
}

// Append `field:value` to a query, AND-combining with whatever's there.
export function addFilter(query: string, field: string, value: string | number): string {
    const clause = filterClause(field, value);
    const q = query.trim();
    if (q === "" || q === "*") return clause;
    // Don't re-add an identical clause.
    if (q.includes(clause)) return q;
    return `${q} AND ${clause}`;
}

// True when `field:value` is already an exact top-level AND-clause of the
// query — i.e. clicking that facet value again would remove it.
export function isFilterActive(query: string, field: string, value: string | number): boolean {
    const clause = filterClause(field, value);
    return query
        .trim()
        .split(/\s+AND\s+/)
        .some((p) => p.trim() === clause);
}

// Toggle `field:value` as a top-level AND-clause (add/remove); last removal
// leaves `*`. Non-AND-chain queries just get an append.
export function toggleFilter(query: string, field: string, value: string | number): string {
    const clause = filterClause(field, value);
    const q = query.trim();
    if (q === "" || q === "*") return clause;
    const parts = q.split(/\s+AND\s+/);
    const idx = parts.findIndex((p) => p.trim() === clause);
    if (idx === -1) return q.includes(clause) ? q : `${q} AND ${clause}`;
    const remaining = parts.filter((_, i) => i !== idx);
    return remaining.length === 0 ? "*" : remaining.join(" AND ");
}

// Quoted clause for a bare word — always quoted so multi-token values
// (`com.example.auth.JwtFilter`, `auth-svc`) match as a phrase.
function termClause(term: string): string {
    return `"${term.replace(/"/g, '\\"')}"`;
}

// Toggle a bare word as a top-level AND-clause; last removal leaves `*`.
// Used by click-a-word-to-search in result details.
export function toggleTerm(query: string, term: string): string {
    const clause = termClause(term);
    const q = query.trim();
    if (q === "" || q === "*") return clause;
    const parts = q.split(/\s+AND\s+/);
    const idx = parts.findIndex((p) => p.trim() === clause);
    if (idx === -1) return `${q} AND ${clause}`;
    const remaining = parts.filter((_, i) => i !== idx);
    return remaining.length === 0 ? "*" : remaining.join(" AND ");
}

// True when `term` is already an exact top-level AND-clause (so a click would
// remove it). Drives the active highlight on clickable words.
export function isTermActive(query: string, term: string): boolean {
    const clause = termClause(term);
    return query
        .trim()
        .split(/\s+AND\s+/)
        .some((p) => p.trim() === clause);
}

// True if a saved search's parameters match the user's current view exactly.
// Used to show the "★ active" indicator and filled star button.
export function sameAsCurrent(saved: SavedSearch, current: SearchInput): boolean {
    return (
        saved.q === current.q &&
        saved.range === current.range &&
        saved.follow === current.follow &&
        (saved.index ?? "") === (current.index ?? "") &&
        (saved.start ?? "") === (current.start ?? "") &&
        (saved.end ?? "") === (current.end ?? "")
    );
}

// Suggest a sensible default for the "Name this search" prompt.
export function suggestName(s: SearchInput): string {
    const q = s.q?.trim();
    if (q && q !== "*") return q.slice(0, 60);
    if (s.index) return `${s.index} · ${s.range}`;
    return s.range;
}
