// Copyright 2026 Appbird LLC. Licensed under "GNU Affero General Public License v3.0"

import {
    createContext,
    useCallback,
    useContext,
    useEffect,
    useRef,
    useState,
    type ReactNode,
} from "react";
import { getSetupStatus, updateAccountPreferences } from "../api/client";
import { useAuth } from "./useAuth";

export type Theme = "light" | "dark";
export type PaletteId = "helios" | "slate" | "emerald" | "indigo" | "homebrew" | "dracula";

// Color themes the UI ships. Must match THEME_PALETTES on the backend and the
// `data-theme` variable blocks in index.css.
export const PALETTES: { id: PaletteId; label: string; blurb: string }[] = [
    { id: "helios", label: "Helios Classic", blurb: "Warm neutral grays with the orange accent" },
    { id: "slate", label: "Slate", blurb: "Cool grays with a blue accent" },
    { id: "emerald", label: "Emerald", blurb: "Pure neutral grays with a green accent" },
    { id: "indigo", label: "Indigo", blurb: "Deep blue-slate grays with an indigo accent" },
    { id: "homebrew", label: "Homebrew", blurb: "Terminal green-on-black, phosphor accent" },
    { id: "dracula", label: "Dracula", blurb: "Stark black & white with a violet accent" },
];

const THEME_KEY = "helios-theme";
const PALETTE_KEY = "helios-palette";
const BUILTIN_DEFAULTS = { theme: "dark" as Theme, palette: "slate" as PaletteId };

function asTheme(v: unknown): Theme | null {
    return v === "light" || v === "dark" ? v : null;
}

function asPalette(v: unknown): PaletteId | null {
    if (v === "github") return "slate"; // pre-rename id, may linger in stored prefs
    return PALETTES.some((p) => p.id === v) ? (v as PaletteId) : null;
}

function readStored(key: string): string | null {
    try {
        return localStorage.getItem(key);
    } catch {
        return null;
    }
}

function writeStored(key: string, v: string | null) {
    try {
        if (v === null) localStorage.removeItem(key);
        else localStorage.setItem(key, v);
    } catch {
        // localStorage unavailable
    }
}

function applyToDocument(theme: Theme, palette: PaletteId) {
    const root = document.documentElement;
    if (theme === "dark") root.classList.add("dark");
    else root.classList.remove("dark");
    root.dataset.theme = palette;
}

interface Ctx {
    /// Effective values (explicit preference, else instance default).
    theme: Theme;
    palette: PaletteId;
    /// Explicit per-user preferences; null = follow the instance default.
    themePref: Theme | null;
    palettePref: PaletteId | null;
    /// Instance defaults (admin-configured, fetched at boot).
    defaults: { theme: Theme; palette: PaletteId };
    setTheme: (t: Theme | null) => void;
    setPalette: (p: PaletteId | null) => void;
    toggleTheme: () => void;
    /// Push fresh instance defaults (e.g. right after an admin saves them) so
    /// default-following sessions restyle without a reload.
    updateDefaults: (d: { appearance?: string; palette?: string }) => void;
}

const ThemeContext = createContext<Ctx>({
    theme: BUILTIN_DEFAULTS.theme,
    palette: BUILTIN_DEFAULTS.palette,
    themePref: null,
    palettePref: null,
    defaults: BUILTIN_DEFAULTS,
    setTheme: () => {},
    setPalette: () => {},
    toggleTheme: () => {},
    updateDefaults: () => {},
});

export function ThemeProvider({ children }: { children: ReactNode }) {
    const { user } = useAuth();
    const [themePref, setThemePrefState] = useState<Theme | null>(() =>
        asTheme(readStored(THEME_KEY)),
    );
    const [palettePref, setPalettePrefState] = useState<PaletteId | null>(() =>
        asPalette(readStored(PALETTE_KEY)),
    );
    const [defaults, setDefaults] = useState(BUILTIN_DEFAULTS);
    // Adopt the account prefs once per login without clobbering a later local change.
    const hydratedFor = useRef<string | null>(null);

    const theme = themePref ?? defaults.theme;
    const palette = palettePref ?? defaults.palette;

    // Effective values follow pref/default changes from any source.
    useEffect(() => {
        applyToDocument(theme, palette);
    }, [theme, palette]);

    // Instance defaults ride on the public setup-status probe, so the login
    // page and default-following accounts render the admin-chosen look.
    useEffect(() => {
        let cancelled = false;
        void getSetupStatus().then((s) => {
            if (cancelled) return;
            setDefaults({
                theme: asTheme(s.default_appearance) ?? BUILTIN_DEFAULTS.theme,
                palette: asPalette(s.default_palette) ?? BUILTIN_DEFAULTS.palette,
            });
        });
        return () => {
            cancelled = true;
        };
    }, []);

    // Apply + cache locally without touching the account (hydration + cross-tab sync).
    const applyLocalTheme = useCallback((t: Theme | null) => {
        setThemePrefState(t);
        writeStored(THEME_KEY, t);
    }, []);
    const applyLocalPalette = useCallback((p: PaletteId | null) => {
        setPalettePrefState(p);
        writeStored(PALETTE_KEY, p);
    }, []);

    // Write through to the account (canonical, cross-device) when logged in;
    // empty string clears the server-side pref (= follow the instance default).
    const setTheme = useCallback(
        (t: Theme | null) => {
            applyLocalTheme(t);
            if (user) void updateAccountPreferences({ theme: t ?? "" }).catch(() => {});
        },
        [applyLocalTheme, user],
    );
    const setPalette = useCallback(
        (p: PaletteId | null) => {
            applyLocalPalette(p);
            if (user) void updateAccountPreferences({ palette: p ?? "" }).catch(() => {});
        },
        [applyLocalPalette, user],
    );

    const toggleTheme = useCallback(() => {
        setTheme(theme === "dark" ? "light" : "dark");
    }, [theme, setTheme]);

    const updateDefaults = useCallback((d: { appearance?: string; palette?: string }) => {
        setDefaults((prev) => ({
            theme: asTheme(d.appearance) ?? prev.theme,
            palette: asPalette(d.palette) ?? prev.palette,
        }));
    }, []);

    // Adopt the account's saved prefs on login (once per user). localStorage
    // stays the synchronous source at boot, so there's no flash. Only adopt
    // values the account actually has — a null/unknown server value must not
    // clear a local pick (a failed write-through would otherwise undo the
    // user's choice on the next load).
    useEffect(() => {
        if (!user) {
            hydratedFor.current = null;
            return;
        }
        if (hydratedFor.current === user.user_id) return;
        hydratedFor.current = user.user_id;
        const t = asTheme(user.theme);
        if (t) applyLocalTheme(t);
        const p = asPalette(user.palette);
        if (p) applyLocalPalette(p);
    }, [user, applyLocalTheme, applyLocalPalette]);

    // Cross-tab sync — if another tab changes a pref, follow it.
    useEffect(() => {
        const onStorage = (e: StorageEvent) => {
            if (e.key === THEME_KEY) setThemePrefState(asTheme(e.newValue));
            if (e.key === PALETTE_KEY) setPalettePrefState(asPalette(e.newValue));
        };
        window.addEventListener("storage", onStorage);
        return () => window.removeEventListener("storage", onStorage);
    }, []);

    return (
        <ThemeContext.Provider
            value={{
                theme,
                palette,
                themePref,
                palettePref,
                defaults,
                setTheme,
                setPalette,
                toggleTheme,
                updateDefaults,
            }}
        >
            {children}
        </ThemeContext.Provider>
    );
}

export function useTheme(): Ctx {
    return useContext(ThemeContext);
}
