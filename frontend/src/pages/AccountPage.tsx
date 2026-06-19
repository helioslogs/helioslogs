// Copyright 2026 Appbird LLC. Licensed under "GNU Affero General Public License v3.0"

// Self-service account page (`/account`). Shows the current user's details
// and offers a single change-password form. Intentionally minimal — there
// is no email flow and no profile editing in this scope.

import { useMemo, useState, type FormEvent } from "react";
import { Moon, MonitorCog, ShieldCheck, Sun } from "lucide-react";
import { changePassword, updateAccountPreferences } from "../api/client";
import { formatTzLabel, getAvailableTimezones, setStoredTimezone } from "../lib/timezone";
import { useAuth } from "../state/useAuth";
import { PALETTES, useTheme, type PaletteId } from "../state/theme";
import { useTimezone } from "../state/timezone";

export function AccountPage() {
    const { user } = useAuth();
    const tz = useTimezone();
    const { themePref, palettePref, defaults, setTheme, setPalette } = useTheme();
    const tzList = useMemo(getAvailableTimezones, []);
    const [current, setCurrent] = useState("");
    const [next, setNext] = useState("");
    const [confirm, setConfirm] = useState("");
    const [busy, setBusy] = useState(false);
    const [error, setError] = useState<string | null>(null);
    const [done, setDone] = useState(false);

    if (!user) return null;

    async function submit(e: FormEvent) {
        e.preventDefault();
        setError(null);
        setDone(false);
        if (next.length < 8) {
            setError("New password must be at least 8 characters.");
            return;
        }
        if (next !== confirm) {
            setError("New password and confirmation do not match.");
            return;
        }
        setBusy(true);
        try {
            await changePassword({ current_password: current, new_password: next });
            setCurrent("");
            setNext("");
            setConfirm("");
            setDone(true);
        } catch (err) {
            setError(err instanceof Error ? err.message : String(err));
        } finally {
            setBusy(false);
        }
    }

    const FIELD =
        "w-full px-2.5 py-1.5 bg-stone-50 dark:bg-stone-950 border border-stone-200 dark:border-stone-700 rounded-md focus:outline-none focus:border-orange-500";
    const LABEL = "block font-medium text-stone-700 dark:text-stone-300 mb-1";

    return (
        <div className="max-w-2xl mx-auto px-4 py-6 space-y-4">
            <header>
                <h1 className="font-semibold text-stone-900 dark:text-stone-100">Account</h1>
                <p className="text-stone-500 dark:text-stone-400">
                    Your account details and password.
                </p>
            </header>

            <section className="bg-white dark:bg-stone-900 border border-stone-200 dark:border-stone-800 rounded-lg p-4">
                <dl className="grid grid-cols-3 gap-y-2">
                    <dt className="text-stone-500 dark:text-stone-400">Userid</dt>
                    <dd className="col-span-2 font-mono">{user.userid}</dd>
                    <dt className="text-stone-500 dark:text-stone-400">Email</dt>
                    <dd className="col-span-2">{user.email}</dd>
                    <dt className="text-stone-500 dark:text-stone-400">Display name</dt>
                    <dd className="col-span-2">{user.display_name}</dd>
                    <dt className="text-stone-500 dark:text-stone-400">Role</dt>
                    <dd className="col-span-2">
                        {user.is_admin ? (
                            <span className="inline-flex items-center gap-1 px-1.5 py-0.5 rounded bg-orange-50 text-orange-800 border border-orange-200 dark:bg-orange-950 dark:text-orange-200 dark:border-orange-900">
                                <ShieldCheck className="w-3 h-3" />
                                admin
                            </span>
                        ) : (
                            <span className="text-stone-500 dark:text-stone-400">user</span>
                        )}
                    </dd>
                </dl>
            </section>

            <section className="bg-white dark:bg-stone-900 border border-stone-200 dark:border-stone-800 rounded-lg p-4 space-y-4">
                <div>
                    <h2 className="font-semibold text-stone-900 dark:text-stone-100">
                        Display preferences
                    </h2>
                    <p className="text-stone-500 dark:text-stone-400">
                        Saved to your account, so they follow you across browsers and devices.
                    </p>
                </div>

                <div>
                    <label className={LABEL}>Timezone</label>
                    <select
                        value={tz}
                        onChange={(e) => {
                            setStoredTimezone(e.target.value);
                            void updateAccountPreferences({ timezone: e.target.value }).catch(
                                () => {},
                            );
                        }}
                        className={FIELD}
                    >
                        {tzList.map((t) => (
                            <option key={t} value={t}>
                                {formatTzLabel(t)}
                            </option>
                        ))}
                    </select>
                    <p className="text-stone-500 dark:text-stone-400 mt-1">
                        UI-only — data is stored in UTC. Affects every timestamp shown.
                    </p>
                </div>

                <div>
                    <label className={LABEL}>Appearance</label>
                    <div className="inline-flex rounded-md overflow-hidden border border-stone-200 dark:border-stone-700">
                        {(
                            [
                                {
                                    value: null,
                                    label: `Default (${defaults.theme})`,
                                    Icon: MonitorCog,
                                },
                                { value: "light", label: "Light", Icon: Sun },
                                { value: "dark", label: "Dark", Icon: Moon },
                            ] as const
                        ).map(({ value, label, Icon }, i) => (
                            <button
                                key={label}
                                type="button"
                                onClick={() => setTheme(value)}
                                className={`flex items-center gap-1.5 px-3 py-1.5 ${
                                    i > 0 ? "border-l border-stone-200 dark:border-stone-700 " : ""
                                }${
                                    themePref === value
                                        ? "bg-orange-600 text-white"
                                        : "bg-white dark:bg-stone-900 text-stone-700 dark:text-stone-300 hover:bg-stone-50 dark:hover:bg-stone-800"
                                }`}
                            >
                                <Icon className="w-3.5 h-3.5" /> {label}
                            </button>
                        ))}
                    </div>
                </div>

                <div>
                    <label className={LABEL}>Theme</label>
                    <select
                        value={palettePref ?? ""}
                        onChange={(e) =>
                            setPalette(e.target.value === "" ? null : (e.target.value as PaletteId))
                        }
                        className={FIELD}
                    >
                        <option value="">
                            Instance default (
                            {PALETTES.find((p) => p.id === defaults.palette)?.label ??
                                defaults.palette}
                            )
                        </option>
                        {PALETTES.map((p) => (
                            <option key={p.id} value={p.id}>
                                {p.label} — {p.blurb}
                            </option>
                        ))}
                    </select>
                    <p className="text-stone-500 dark:text-stone-400 mt-1">
                        Color palette for the whole app, in both light and dark.
                    </p>
                </div>
            </section>

            <form
                onSubmit={submit}
                className="bg-white dark:bg-stone-900 border border-stone-200 dark:border-stone-800 rounded-lg p-4 space-y-3"
            >
                <h2 className="font-semibold text-stone-900 dark:text-stone-100">
                    Change password
                </h2>
                <div>
                    <label className={LABEL}>Current password</label>
                    <input
                        type="password"
                        autoComplete="current-password"
                        value={current}
                        onChange={(e) => setCurrent(e.target.value)}
                        required
                        className={FIELD}
                    />
                </div>
                <div>
                    <label className={LABEL}>New password</label>
                    <input
                        type="password"
                        autoComplete="new-password"
                        value={next}
                        onChange={(e) => setNext(e.target.value)}
                        required
                        minLength={8}
                        className={FIELD}
                    />
                </div>
                <div>
                    <label className={LABEL}>Confirm new password</label>
                    <input
                        type="password"
                        autoComplete="new-password"
                        value={confirm}
                        onChange={(e) => setConfirm(e.target.value)}
                        required
                        minLength={8}
                        className={FIELD}
                    />
                </div>

                {error && (
                    <div className="px-3 py-2 rounded-md bg-red-50 text-red-800 border border-red-200 dark:bg-red-950 dark:text-red-200 dark:border-red-900">
                        {error}
                    </div>
                )}
                {done && (
                    <div className="px-3 py-2 rounded-md bg-emerald-50 text-emerald-800 border border-emerald-200 dark:bg-emerald-950 dark:text-emerald-200 dark:border-emerald-900">
                        Password updated.
                    </div>
                )}

                <button
                    type="submit"
                    disabled={busy || !current || !next || !confirm}
                    className="px-3 py-1.5 font-medium text-white bg-orange-600 hover:bg-orange-500 rounded-md transition disabled:opacity-50"
                >
                    {busy ? "Updating…" : "Update password"}
                </button>
            </form>
        </div>
    );
}
