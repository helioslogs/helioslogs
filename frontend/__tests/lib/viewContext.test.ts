// Copyright 2026 Appbird LLC. Licensed under "GNU Affero General Public License v3.0"

import { describe, it, expect } from "vitest";
import { formatViewContext, getViewContext, type ViewContext } from "../../src/lib/viewContext";

describe("formatViewContext", () => {
    it("renders the search-route block with query/range/index", () => {
        const c: ViewContext = {
            route: "search",
            url: "http://x/search?q=level:error",
            query: "level:error",
            index: "web",
            timeRange: "-1h",
            follow: true,
            page: 3,
            timezone: "UTC",
            nowLocal: "2026-01-01 00:00:00 UTC",
        };
        const out = formatViewContext(c);
        expect(out).toContain("Search page");
        expect(out).toContain("Search query: level:error");
        expect(out).toContain("Time range: -1h");
        expect(out).toContain("Index filter: web");
        expect(out).toContain("Live-follow mode is ON");
        expect(out).toContain("Viewing results page 3");
    });

    it("describes a match-all query and default scopes", () => {
        const c: ViewContext = {
            route: "search",
            url: "http://x/search",
            query: "*",
            timezone: "UTC",
            nowLocal: "now",
        };
        const out = formatViewContext(c);
        expect(out).toContain("* (everything)");
        expect(out).toContain("all indexes");
    });

    it("renders saved and admin route blocks", () => {
        const saved = formatViewContext({
            route: "saved",
            url: "http://x/saved",
            timezone: "UTC",
            nowLocal: "now",
        });
        expect(saved).toContain("Saved Searches page");
        const admin = formatViewContext({
            route: "admin",
            url: "http://x/admin",
            timezone: "UTC",
            nowLocal: "now",
        });
        expect(admin).toContain("Admin page");
    });
});

describe("getViewContext", () => {
    it("captures the current route + a timezone snapshot", () => {
        // jsdom defaults the location to "/", which maps to the search route.
        const c = getViewContext();
        expect(c.route).toBe("search");
        expect(typeof c.timezone).toBe("string");
        expect(typeof c.nowLocal).toBe("string");
    });
});
