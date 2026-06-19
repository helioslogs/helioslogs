// Copyright 2026 Appbird LLC. Licensed under "GNU Affero General Public License v3.0"

// `stone` (surfaces/text) and `orange` (accent) resolve to the RGB-triplet
// CSS variables defined per theme in src/index.css, so the palette switches
// at runtime via `data-theme` on <html> without touching any component.
const varScale = (name) =>
    Object.fromEntries(
        [50, 100, 200, 300, 400, 500, 600, 700, 800, 900, 950].map((n) => [
            n,
            `rgb(var(--hl-${name}-${n}) / <alpha-value>)`,
        ]),
    );

/** @type {import('tailwindcss').Config} */
export default {
    content: ["./index.html", "./src/**/*.{ts,tsx}"],
    darkMode: "class",
    theme: {
        extend: {
            // One unified font family across the whole app. Both `font-sans` and
            // `font-mono` resolve to the same system UI stack (SF Pro on macOS,
            // Segoe UI on Windows) so JSON content, sidebar values, counts, and
            // labels all render in the same typeface. The shared stack means
            // existing components that pick `font-mono` get the system font too
            // without any per-file edits.
            fontFamily: {
                sans: ["sans-serif"],
                mono: ["sans-serif"],
            },
            colors: {
                stone: varScale("stone"),
                orange: varScale("accent"),
            },
        },
    },
    plugins: [],
};
