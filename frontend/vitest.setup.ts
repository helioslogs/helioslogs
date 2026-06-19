// Copyright 2026 Appbird LLC. Licensed under "GNU Affero General Public License v3.0"

import "@testing-library/jest-dom/vitest";
import { afterEach } from "vitest";
import { cleanup } from "@testing-library/react";

// Unmount React trees between tests so queries don't bleed across cases.
afterEach(() => cleanup());

// jsdom (as wired by vitest here) doesn't expose Web Storage, and several
// modules (`api/client`, `lib/timezone`, `pages/LoginPage`) persist UI prefs to
// `localStorage` / `sessionStorage`. Install a minimal in-memory shim for both
// so those code paths run under test.

class MemStorage implements Storage {
    private m = new Map<string, string>();
    get length(): number {
        return this.m.size;
    }
    clear(): void {
        this.m.clear();
    }
    getItem(key: string): string | null {
        return this.m.has(key) ? (this.m.get(key) as string) : null;
    }
    setItem(key: string, value: string): void {
        this.m.set(String(key), String(value));
    }
    removeItem(key: string): void {
        this.m.delete(key);
    }
    key(index: number): string | null {
        return Array.from(this.m.keys())[index] ?? null;
    }
}

function installStorage(name: "localStorage" | "sessionStorage") {
    const store = new MemStorage();
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    (globalThis as any)[name] = store;
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    if (typeof window !== "undefined") (window as any)[name] = store;
}
installStorage("localStorage");
installStorage("sessionStorage");
