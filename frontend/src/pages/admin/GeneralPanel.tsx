// Copyright 2026 Appbird LLC. Licensed under "GNU Affero General Public License v3.0"

// `/admin/general` — instance-wide appearance defaults (theme + light/dark,
// each overridable per-account) plus read-only server configuration: storage
// locations and the `HELIOS_*` env knobs the instance booted with.

import { useEffect, useState } from "react";
import { Lock, Settings as SettingsIcon } from "lucide-react";
import {
    getRuntimeConfig,
    getSettings,
    getTunables,
    updateSettings,
    updateTunable,
} from "../../api/client";
import type { RuntimeConfigEntry, Settings, Tunable } from "../../api/types";
import { PALETTES, useTheme } from "../../state/theme";
import { Card, ErrorBanner } from "../../components/admin";

export function GeneralPanel() {
    const [config, setConfig] = useState<RuntimeConfigEntry[] | null>(null);
    const [tunables, setTunables] = useState<Tunable[] | null>(null);
    const [settings, setSettings] = useState<Settings | null>(null);
    const [error, setError] = useState<string | null>(null);

    useEffect(() => {
        let cancelled = false;
        void (async () => {
            try {
                const [c, t, s] = await Promise.all([
                    getRuntimeConfig(),
                    getTunables(),
                    getSettings(),
                ]);
                if (cancelled) return;
                setConfig(c);
                setTunables(t);
                setSettings(s);
            } catch (e) {
                if (!cancelled) setError(e instanceof Error ? e.message : String(e));
            }
        })();
        return () => {
            cancelled = true;
        };
    }, []);

    return (
        <div>
            <Card title="General settings">
                <div className="p-6 space-y-6 max-w-3xl">
                    <GeneralHelpFrame />

                    <ErrorBanner error={error} />

                    <Subheader title="Appearance defaults" />
                    <ThemeDefaults settings={settings} onSaved={setSettings} />

                    <Subheader title="Server configuration" />
                    <ServerConfiguration
                        config={config}
                        tunables={tunables}
                        settings={settings}
                        onTunablesSaved={setTunables}
                        onSettingsSaved={setSettings}
                    />
                </div>
            </Card>
        </div>
    );
}

// Instance-wide defaults for new sessions; each user can override both on
// their /account page (their pick wins until they switch back to "default").
function ThemeDefaults({
    settings,
    onSaved,
}: {
    settings: Settings | null;
    onSaved: (s: Settings) => void;
}) {
    const { updateDefaults } = useTheme();
    const [appearance, setAppearance] = useState<"light" | "dark">("dark");
    const [palette, setPalette] = useState("slate");
    const [busy, setBusy] = useState(false);
    const [saved, setSaved] = useState(false);
    const [error, setError] = useState<string | null>(null);

    useEffect(() => {
        if (!settings) return;
        setAppearance(settings.theme_default_appearance);
        setPalette(settings.theme_default_palette);
    }, [settings]);

    if (!settings) return <div className="text-stone-700 dark:text-stone-300">loading…</div>;

    const dirty =
        appearance !== settings.theme_default_appearance ||
        palette !== settings.theme_default_palette;

    async function save() {
        setBusy(true);
        setError(null);
        setSaved(false);
        try {
            const next = await updateSettings({
                theme_default_appearance: appearance,
                theme_default_palette: palette,
            });
            onSaved(next);
            // Restyle this session right away if it follows the defaults.
            updateDefaults({
                appearance: next.theme_default_appearance,
                palette: next.theme_default_palette,
            });
            setSaved(true);
        } catch (e) {
            setError(e instanceof Error ? e.message : String(e));
        } finally {
            setBusy(false);
        }
    }

    const FIELD =
        "w-full px-2.5 py-1.5 bg-stone-50 dark:bg-stone-950 border border-stone-200 dark:border-stone-700 rounded-md focus:outline-none focus:border-orange-500";

    return (
        <div className="space-y-3">
            <div className="grid grid-cols-2 gap-4">
                <div>
                    <label
                        htmlFor="theme-default-appearance"
                        className="block font-medium text-stone-700 dark:text-stone-300 mb-1"
                    >
                        Default appearance
                    </label>
                    <select
                        id="theme-default-appearance"
                        value={appearance}
                        onChange={(e) => setAppearance(e.target.value as "light" | "dark")}
                        className={FIELD}
                    >
                        <option value="light">Light</option>
                        <option value="dark">Dark</option>
                    </select>
                </div>
                <div>
                    <label
                        htmlFor="theme-default-palette"
                        className="block font-medium text-stone-700 dark:text-stone-300 mb-1"
                    >
                        Default theme
                    </label>
                    <select
                        id="theme-default-palette"
                        value={palette}
                        onChange={(e) => setPalette(e.target.value)}
                        className={FIELD}
                    >
                        {PALETTES.map((p) => (
                            <option key={p.id} value={p.id}>
                                {p.label} — {p.blurb}
                            </option>
                        ))}
                    </select>
                </div>
            </div>
            <p className="text-stone-500 dark:text-stone-400">
                Applies to the login screen and to every account that hasn't picked its own
                appearance or theme. Users override both on their Account page.
            </p>
            {error && (
                <div className="px-3 py-2 rounded-md bg-red-50 text-red-800 border border-red-200 dark:bg-red-950 dark:text-red-200 dark:border-red-900">
                    {error}
                </div>
            )}
            <div className="flex items-center gap-3">
                <button
                    type="button"
                    onClick={() => void save()}
                    disabled={busy || !dirty}
                    className="px-3 py-1.5 font-medium text-white bg-orange-600 hover:bg-orange-500 rounded-md transition disabled:opacity-50"
                >
                    {busy ? "Saving…" : "Save defaults"}
                </button>
                {saved && !dirty && (
                    <span className="text-emerald-700 dark:text-emerald-300">Saved.</span>
                )}
            </div>
        </div>
    );
}

// Functional-category order for the merged server-configuration view. Categories
// not listed fall to the end (in first-seen order).
const CATEGORY_ORDER = [
    "Storage location",
    "Storage engine",
    "Retention",
    "Query",
    "Authentication",
    "Control plane",
    "Security",
];

// Single server-configuration view: editable tunables (env > control setting >
// default) and read-only startup config share one box per functional category —
// editable rows first, then read-only. The global retention default folds into
// the "Retention" group next to the sweep interval.
function ServerConfiguration({
    config,
    tunables,
    settings,
    onTunablesSaved,
    onSettingsSaved,
}: {
    config: RuntimeConfigEntry[] | null;
    tunables: Tunable[] | null;
    settings: Settings | null;
    onTunablesSaved: (t: Tunable[]) => void;
    onSettingsSaved: (s: Settings) => void;
}) {
    if (!config || !tunables) {
        return <div className="text-stone-700 dark:text-stone-300">loading…</div>;
    }

    const rank = (cat: string) => {
        const i = CATEGORY_ORDER.indexOf(cat);
        return i === -1 ? CATEGORY_ORDER.length : i;
    };
    const categories = [
        ...new Set([...tunables.map((t) => t.category), ...config.map((c) => c.category)]),
    ].sort((a, b) => rank(a) - rank(b));

    return (
        <div className="space-y-5">
            {categories.map((cat) => (
                <div key={cat} className="space-y-2">
                    <div className="font-semibold text-stone-700 dark:text-stone-300">{cat}</div>
                    <div className="rounded-md border border-stone-200 dark:border-stone-700 divide-y divide-stone-100 dark:divide-stone-800">
                        {tunables
                            .filter((t) => t.category === cat)
                            .map((t) => (
                                <TunableRow key={t.id} tunable={t} onSaved={onTunablesSaved} />
                            ))}
                        {cat === "Retention" && settings && (
                            <RetentionDefaultRow settings={settings} onSaved={onSettingsSaved} />
                        )}
                        {config
                            .filter((c) => c.category === cat)
                            .map((c) => (
                                <ConfigRow key={c.name} entry={c} />
                            ))}
                    </div>
                </div>
            ))}
        </div>
    );
}

const NUM_FIELD =
    "w-28 px-2.5 py-1.5 bg-stone-50 dark:bg-stone-950 border border-stone-200 dark:border-stone-700 rounded-md font-mono tabular-nums focus:outline-none focus:border-orange-500 disabled:opacity-60";

// One tunable: editable number bound to the configured value (empty = default).
// Locked read-only when an env var pins it; flagged when a change needs a restart.
function TunableRow({ tunable, onSaved }: { tunable: Tunable; onSaved: (t: Tunable[]) => void }) {
    const t = tunable;
    const locked = t.env_override !== null;
    const [value, setValue] = useState(t.configured != null ? String(t.configured) : "");
    const [busy, setBusy] = useState(false);
    const [error, setError] = useState<string | null>(null);

    useEffect(() => {
        setValue(t.configured != null ? String(t.configured) : "");
    }, [t.configured]);

    const parsed = value.trim() === "" ? null : parseInt(value.trim(), 10);
    const invalid = parsed !== null && (!Number.isFinite(parsed) || parsed < 0);
    const dirty = !invalid && parsed !== (t.configured ?? null);

    async function save(next: number | null) {
        setBusy(true);
        setError(null);
        try {
            onSaved(await updateTunable(t.id, next));
        } catch (e) {
            setError(e instanceof Error ? e.message : String(e));
        } finally {
            setBusy(false);
        }
    }

    return (
        <div className="px-3 py-2.5 flex items-start justify-between gap-4">
            <div className="min-w-0 space-y-0.5">
                <div className="flex items-center gap-2 flex-wrap">
                    <span className="font-medium text-stone-900 dark:text-stone-100">
                        {t.label}
                    </span>
                    <code className="font-mono font-semibold text-stone-600 dark:text-stone-300 break-all">
                        {t.env}
                    </code>
                    {!t.live && (
                        <span className="px-1.5 py-0.5 rounded text-orange-700 bg-orange-100 dark:text-orange-300 dark:bg-orange-950/50">
                            restart to apply
                        </span>
                    )}
                </div>
                <p className="text-stone-700 dark:text-stone-300 leading-relaxed">
                    {t.description}
                </p>
                {error && <p className="text-red-700 dark:text-red-300">{error}</p>}
            </div>
            <div className="flex-shrink-0 flex flex-col items-end gap-1 w-44">
                {locked ? (
                    <div className="flex items-center gap-1.5 text-stone-600 dark:text-stone-300">
                        <Lock className="w-3.5 h-3.5" />
                        <span className="font-mono tabular-nums">
                            {t.env_override} {t.unit}
                        </span>
                    </div>
                ) : (
                    <div className="flex items-center gap-1.5">
                        <input
                            type="number"
                            min={0}
                            value={value}
                            disabled={busy}
                            placeholder={String(t.default)}
                            onChange={(e) => setValue(e.target.value)}
                            className={NUM_FIELD}
                        />
                        <button
                            type="button"
                            onClick={() => void save(parsed)}
                            disabled={busy || !dirty}
                            className="px-2.5 py-1.5 font-medium text-white bg-orange-600 hover:bg-orange-500 rounded-md transition disabled:opacity-40"
                        >
                            Save
                        </button>
                    </div>
                )}
                <div className="text-stone-500 dark:text-stone-400 text-right">
                    {locked ? (
                        <>set via env</>
                    ) : (
                        <>
                            now {t.effective} {t.unit}
                            {t.configured == null && " · default"}
                        </>
                    )}
                </div>
            </div>
        </div>
    );
}

// Global default retention (days; empty/0 = keep forever). Lives in the Retention
// group next to the sweep interval. Per-env overrides are on Admin → Environments.
function RetentionDefaultRow({
    settings,
    onSaved,
}: {
    settings: Settings;
    onSaved: (s: Settings) => void;
}) {
    const locked = settings.retention_default_days_env_overridden;
    const [value, setValue] = useState(
        settings.retention_default_days > 0 ? String(settings.retention_default_days) : "",
    );
    const [busy, setBusy] = useState(false);
    const [error, setError] = useState<string | null>(null);

    useEffect(() => {
        setValue(
            settings.retention_default_days > 0 ? String(settings.retention_default_days) : "",
        );
    }, [settings.retention_default_days]);

    const trimmed = value.trim();
    const days = trimmed === "" ? 0 : parseInt(trimmed, 10);
    const invalid = !Number.isFinite(days) || days < 0;
    const dirty = !invalid && days !== settings.retention_default_days;

    async function save() {
        setBusy(true);
        setError(null);
        try {
            onSaved(await updateSettings({ retention_default_days: days }));
        } catch (e) {
            setError(e instanceof Error ? e.message : String(e));
        } finally {
            setBusy(false);
        }
    }

    const effective = settings.retention_default_days_effective;
    return (
        <div className="px-3 py-2.5 flex items-start justify-between gap-4">
            <div className="min-w-0 space-y-0.5">
                <div className="flex items-center gap-2 flex-wrap">
                    <span className="font-medium text-stone-900 dark:text-stone-100">
                        Default retention
                    </span>
                    <code className="font-mono text-stone-500 dark:text-stone-400 break-all">
                        HELIOS_RETENTION_DEFAULT_DAYS
                    </code>
                </div>
                <p className="text-stone-700 dark:text-stone-300 leading-relaxed">
                    Day-partitions older than this are dropped by the sweep above (empty or 0 = keep
                    forever). Per-environment overrides live on Admin → Environments and win over
                    this default.
                </p>
                {error && <p className="text-red-700 dark:text-red-300">{error}</p>}
            </div>
            <div className="flex-shrink-0 flex flex-col items-end gap-1 w-44">
                {locked ? (
                    <div className="flex items-center gap-1.5 text-stone-600 dark:text-stone-300">
                        <Lock className="w-3.5 h-3.5" />
                        <span className="font-mono tabular-nums">
                            {effective > 0 ? `${effective} days` : "∞"}
                        </span>
                    </div>
                ) : (
                    <div className="flex items-center gap-1.5">
                        <input
                            type="number"
                            min={0}
                            value={value}
                            disabled={busy}
                            placeholder="∞"
                            onChange={(e) => setValue(e.target.value)}
                            className={NUM_FIELD}
                        />
                        <button
                            type="button"
                            onClick={() => void save()}
                            disabled={busy || !dirty}
                            className="px-2.5 py-1.5 font-medium text-white bg-orange-600 hover:bg-orange-500 rounded-md transition disabled:opacity-40"
                        >
                            Save
                        </button>
                    </div>
                )}
                <div className="text-stone-500 dark:text-stone-400 text-right">
                    {locked ? (
                        <>set via env</>
                    ) : (
                        <>now {effective > 0 ? `${effective} days` : "∞"}</>
                    )}
                </div>
            </div>
        </div>
    );
}

// Inline help banner at the top of the General settings card. Matches
// the visual treatment used by the LLM provider and MCP server panels.
function GeneralHelpFrame() {
    return (
        <div className="flex gap-3 p-4 rounded-lg bg-orange-50/60 dark:bg-orange-950/20 border border-orange-200/70 dark:border-orange-900/40">
            <div className="flex-shrink-0 mt-0.5">
                <SettingsIcon className="w-4 h-4 text-orange-600 dark:text-orange-400" />
            </div>
            <div className="space-y-1.5 text-stone-700 dark:text-stone-200 leading-relaxed">
                <p>
                    Instance-wide appearance defaults (users override them on their own Account
                    page) and server configuration grouped by function. Editable values are stored
                    in the control plane and take effect on a running instance — but a matching{" "}
                    <code className="font-mono">HELIOS_*</code> environment variable always wins and
                    locks the field. Entries shown as a fixed value (no input) are read-only, set at
                    startup.
                </p>
            </div>
        </div>
    );
}

function ConfigRow({ entry }: { entry: RuntimeConfigEntry }) {
    return (
        <div className="px-3 py-2.5 flex items-start justify-between gap-4">
            <div className="min-w-0 space-y-0.5">
                <code className="font-mono font-semibold text-stone-900 dark:text-stone-100 break-all">
                    {entry.name}
                </code>
                <p className="text-stone-700 dark:text-stone-300 leading-relaxed">
                    {entry.description}
                </p>
            </div>
            <div className="flex-shrink-0 flex flex-col items-end gap-1">
                <span className="px-2 py-0.5 rounded font-mono tabular-nums bg-stone-100 dark:bg-stone-800 text-stone-800 dark:text-stone-200">
                    {entry.value}
                </span>
                <span
                    className={
                        entry.overridden
                            ? "text-orange-600 dark:text-orange-400"
                            : "text-stone-700 dark:text-stone-300"
                    }
                >
                    {entry.overridden ? "configured" : "default"}
                </span>
            </div>
        </div>
    );
}

function Subheader({ title }: { title: string }) {
    return (
        <div className="flex items-center gap-3 pt-2">
            <div className="font-semibold uppercase tracking-wider text-stone-700 dark:text-stone-300">
                {title}
            </div>
            <div className="flex-grow h-px bg-stone-200 dark:bg-stone-800" />
        </div>
    );
}
