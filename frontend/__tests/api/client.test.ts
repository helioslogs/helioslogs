// Copyright 2026 Appbird LLC. Licensed under "GNU Affero General Public License v3.0"

import { describe, it, expect, beforeEach, afterEach, vi } from "vitest";
import {
    getToken,
    setToken,
    clearToken,
    getEnv,
    setEnv,
    apiFetch,
    search,
    partitionToParam,
} from "../../src/api/client";

// A stubbed fetch that records calls and returns a configurable Response.
function stubFetch(make: () => Response) {
    const fn = vi.fn(async (..._args: unknown[]) => make());
    vi.stubGlobal("fetch", fn);
    return fn;
}

const calledUrl = (fn: ReturnType<typeof vi.fn>): string => String(fn.mock.calls[0][0]);
const calledHeaders = (fn: ReturnType<typeof vi.fn>): Headers =>
    (fn.mock.calls[0][1] as RequestInit).headers as Headers;

beforeEach(() => localStorage.clear());
afterEach(() => vi.unstubAllGlobals());

describe("token + env storage", () => {
    it("round-trips the auth token", () => {
        expect(getToken()).toBeNull();
        setToken("jwt-abc");
        expect(getToken()).toBe("jwt-abc");
        clearToken();
        expect(getToken()).toBeNull();
    });
    it("defaults the env to `default`", () => {
        expect(getEnv()).toBe("default");
        setEnv("prod");
        expect(getEnv()).toBe("prod");
    });
});

describe("apiFetch", () => {
    it("attaches the bearer token when present", async () => {
        const fn = stubFetch(() => new Response("{}", { status: 200 }));
        setToken("jwt-abc");
        await apiFetch("/api/stats");
        expect(calledHeaders(fn).get("Authorization")).toBe("Bearer jwt-abc");
    });

    it("omits the auth header when there's no token", async () => {
        const fn = stubFetch(() => new Response("{}", { status: 200 }));
        await apiFetch("/api/stats");
        expect(calledHeaders(fn).get("Authorization")).toBeNull();
    });

    it("appends the active env as ?env=", async () => {
        const fn = stubFetch(() => new Response("{}", { status: 200 }));
        setEnv("prod");
        await apiFetch("/api/stats");
        expect(calledUrl(fn)).toContain("env=prod");
    });

    it("respects an env already present on the URL", async () => {
        const fn = stubFetch(() => new Response("{}", { status: 200 }));
        setEnv("prod");
        await apiFetch("/api/stats?env=_system");
        const url = calledUrl(fn);
        expect(url).toContain("env=_system");
        expect(url).not.toContain("env=prod");
    });

    it("swaps in a refreshed token from the response header", async () => {
        stubFetch(
            () =>
                new Response("{}", {
                    status: 200,
                    headers: { "X-Helios-Token-Refresh": "fresh-jwt" },
                }),
        );
        setToken("old-jwt");
        await apiFetch("/api/stats");
        expect(getToken()).toBe("fresh-jwt");
    });

    it("clears the token and emits helios-401 on a 401", async () => {
        stubFetch(() => new Response("nope", { status: 401 }));
        setToken("stale");
        let fired = false;
        const onEvt = () => {
            fired = true;
        };
        window.addEventListener("helios-401", onEvt);
        await apiFetch("/api/stats");
        window.removeEventListener("helios-401", onEvt);
        expect(getToken()).toBeNull();
        expect(fired).toBe(true);
    });
});

describe("search() endpoint wrapper", () => {
    it("builds the query string (omitting empty/undefined) and returns parsed JSON", async () => {
        const fn = stubFetch(
            () => new Response(JSON.stringify({ total: 3, hits: [] }), { status: 200 }),
        );
        const res = await search({ q: "level:error", index: "", limit: 50 });
        const url = calledUrl(fn);
        expect(url).toContain("/api/search?");
        expect(url).toContain("q=level%3Aerror");
        expect(url).toContain("limit=50");
        expect(url).not.toContain("index="); // empty string is dropped
        expect(res.total).toBe(3);
    });

    it("throws with the status + body on a non-2xx", async () => {
        stubFetch(() => new Response("bad query", { status: 400 }));
        await expect(search({ q: "(" })).rejects.toThrow(/search 400: bad query/);
    });
});

describe("partitionToParam", () => {
    it("formats env:index:day triples", () => {
        expect(partitionToParam({ env: "prod", index: "web", day: "2026-01-01" })).toBe(
            "prod:web:2026-01-01",
        );
    });
});
