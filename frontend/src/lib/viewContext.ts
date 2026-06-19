// Copyright 2026 Appbird LLC. Licensed under "GNU Affero General Public License v3.0"

// Snapshot of what the user is looking at when they send an agent message.
// Captured at send-time and woven into the prompt so the agent answers in
// the scope of the current view (query, time range, source filter).

import { getStoredTimezone } from "./timezone";
import { readUrl } from "../state/url";

export interface ViewContext {
    route: "search" | "saved" | "admin";
    url: string;
    // Search-route fields — undefined on other routes.
    query?: string;
    index?: string;
    // Either a relative range ("-6h") or an absolute "start → end" window.
    timeRange?: string;
    follow?: boolean;
    page?: number;
    // IANA display tz, so the agent reads wall-clock references in the user's frame.
    timezone: string;
    // User's "now", pre-formatted client-side so the server needs no chrono-tz.
    nowLocal: string;
}

export function getViewContext(): ViewContext {
    const path = window.location.pathname;
    const route: ViewContext["route"] = path.startsWith("/admin")
        ? "admin"
        : path.startsWith("/saved")
          ? "saved"
          : "search";

    const timezone = getStoredTimezone();
    const ctx: ViewContext = {
        route,
        url: window.location.href,
        timezone,
        nowLocal: formatNowInTimezone(timezone),
    };
    if (route === "search") {
        const u = readUrl();
        ctx.query = u.q;
        ctx.index = u.index;
        ctx.follow = u.follow;
        ctx.page = u.page;
        ctx.timeRange = u.start && u.end ? `${u.start} → ${u.end}` : u.range;
    }
    return ctx;
}

// "2026-05-25 14:30:42 EDT" — fixed year-first 24h shape so the agent needn't
// parse locales; the abbreviation is appended in a second Intl pass.
function formatNowInTimezone(tz: string): string {
    const now = new Date();
    try {
        const fmt = new Intl.DateTimeFormat("en-CA", {
            timeZone: tz,
            year: "numeric",
            month: "2-digit",
            day: "2-digit",
            hour: "2-digit",
            minute: "2-digit",
            second: "2-digit",
            hour12: false,
        });
        // en-CA renders as "YYYY-MM-DD, HH:mm:ss" — strip the comma.
        const base = fmt.format(now).replace(",", "");
        const abbr = new Intl.DateTimeFormat("en-US", {
            timeZone: tz,
            timeZoneName: "short",
        })
            .formatToParts(now)
            .find((p) => p.type === "timeZoneName")?.value;
        return abbr ? `${base} ${abbr}` : base;
    } catch {
        return now.toISOString();
    }
}

// Render the context as a compact block for the prompt preamble.
export function formatViewContext(c: ViewContext): string {
    const lines: string[] = [];
    if (c.route === "search") {
        lines.push("The user is on the Search page, looking at live log results.");
        lines.push(`- Search query: ${c.query && c.query !== "*" ? c.query : "* (everything)"}`);
        lines.push(`- Time range: ${c.timeRange ?? "-6h"}`);
        lines.push(`- Index filter: ${c.index ? c.index : "all indexes"}`);
        if (c.follow) lines.push("- Live-follow mode is ON");
        if (c.page && c.page > 1) lines.push(`- Viewing results page ${c.page}`);
    } else if (c.route === "saved") {
        lines.push("The user is on the Saved Searches page.");
    } else {
        lines.push("The user is on the Admin page (catalog stats, partitions, settings).");
    }
    lines.push(`- URL: ${c.url}`);
    return lines.join("\n");
}
