// Copyright 2026 Appbird LLC. Licensed under "GNU Affero General Public License v3.0"

import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

export default defineConfig({
    plugins: [react()],
    // react-draggable (via react-grid-layout) reads `process.env.DRAGGABLE_DEBUG`
    // in its drag-start path. Vite doesn't define `process` in the browser, so
    // without this the dashboard grid throws "process is not defined" on drag.
    // Replacing the exact expression avoids referencing `process` at all.
    define: {
        "process.env.DRAGGABLE_DEBUG": "false",
    },
    server: {
        port: 5173,
        proxy: {
            "/api": "http://127.0.0.1:7300",
        },
    },
});
