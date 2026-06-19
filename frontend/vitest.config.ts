// Copyright 2026 Appbird LLC. Licensed under "GNU Affero General Public License v3.0"

import { defineConfig } from "vitest/config";
import react from "@vitejs/plugin-react";

// Unit-test config for the lib helpers and API client. jsdom gives us
// `window` / `localStorage` / `Intl` so the timezone, view-context, and
// client tests run without a browser. The react plugin lets `.tsx` modules
// (e.g. `lib/sort.tsx`) transform even though Tier 1–2 tests don't render.
export default defineConfig({
    plugins: [react()],
    test: {
        environment: "jsdom",
        // A real origin so jsdom enables `localStorage` and `window.location`
        // has a usable `.origin` (the client's `withEnv` builds URLs from it).
        environmentOptions: { jsdom: { url: "http://localhost/" } },
        globals: true,
        setupFiles: ["./vitest.setup.ts"],
        include: ["__tests/**/*.{test,spec}.{ts,tsx}"],
    },
});
