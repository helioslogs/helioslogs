// Copyright 2026 Appbird LLC. Licensed under "GNU Affero General Public License v3.0"

// Env-grouped env+index allowlist editor. Shared between the MCP server
// config (`/admin/mcp`) and per-user permissions (`/admin/users`).

import type { EnvIndexAllow } from "../api/types";

export interface AllowlistEditorProps {
    value: EnvIndexAllow[];
    onChange: (next: EnvIndexAllow[]) => void;
    // Env name → sorted list of index names known to exist on disk.
    // Drives the rendered sections; envs absent here aren't shown.
    catalogByEnv: Record<string, string[]>;
    disabled?: boolean;
    // Override the derived "unrestricted" state when the parent owns its own
    // toggle (UserDialog); omit to fall back to value-derived (empty ⇒ unrestricted).
    unrestricted?: boolean;
}

// True iff open-ended: empty, or any `{env:"*", indexes:["*"]}` rule.
export function allowlistUnrestricted(rules: EnvIndexAllow[]): boolean {
    if (rules.length === 0) return true;
    return rules.some((r) => r.env === "*" && r.indexes.includes("*"));
}

export function ruleForEnv(rules: EnvIndexAllow[], env: string): EnvIndexAllow | null {
    return rules.find((r) => r.env.toLowerCase() === env.toLowerCase()) ?? null;
}

function setEnvRule(rules: EnvIndexAllow[], env: string, indexes: string[]): EnvIndexAllow[] {
    const others = rules.filter((r) => r.env.toLowerCase() !== env.toLowerCase());
    if (indexes.length === 0) return others;
    return [...others, { env, indexes }];
}

export function AllowlistEditor({
    value,
    onChange,
    catalogByEnv,
    disabled,
    unrestricted: unrestrictedOverride,
}: AllowlistEditorProps) {
    const unrestricted =
        unrestrictedOverride !== undefined ? unrestrictedOverride : allowlistUnrestricted(value);
    // User envs first, `_system` last so opt-in is deliberate.
    const sortedEnvs = Object.keys(catalogByEnv).sort((a, b) => {
        const sa = a.startsWith("_") ? 1 : 0;
        const sb = b.startsWith("_") ? 1 : 0;
        if (sa !== sb) return sa - sb;
        return a.localeCompare(b);
    });

    const toggleIndex = (env: string, name: string) => {
        const current = unrestricted ? [] : value;
        const rule = ruleForEnv(current, env);
        const indexes = (rule?.indexes ?? []).filter((i) => i !== "*");
        const next = indexes.includes(name)
            ? indexes.filter((i) => i !== name)
            : [...indexes, name];
        onChange(setEnvRule(current, env, next));
    };

    const toggleEnvAll = (env: string) => {
        const current = unrestricted ? [] : value;
        const rule = ruleForEnv(current, env);
        const isAllOn = rule?.indexes.includes("*") ?? false;
        onChange(isAllOn ? setEnvRule(current, env, []) : setEnvRule(current, env, ["*"]));
    };

    if (sortedEnvs.length === 0) {
        return (
            <div className="text-stone-700 dark:text-stone-300 italic">
                No envs / indexes on disk yet. Ingest some events first, then come back to grant
                access.
            </div>
        );
    }

    return (
        <div className="space-y-3">
            {sortedEnvs.map((env) => {
                const indexes = catalogByEnv[env] ?? [];
                const rule = ruleForEnv(value, env);
                const allOn = rule?.indexes.includes("*") ?? false;
                const allowedSet = new Set(rule?.indexes ?? []);
                const effectiveAll = unrestricted || allOn;
                return (
                    <div
                        key={env}
                        className="border border-stone-200 dark:border-stone-800 rounded-md overflow-hidden"
                    >
                        <div className="flex items-center gap-3 px-3 py-2 bg-stone-50 dark:bg-stone-950/40 border-b border-stone-200 dark:border-stone-800">
                            <code className="font-mono font-semibold text-stone-900 dark:text-stone-100">
                                {env}
                            </code>
                            {env.startsWith("_") && (
                                <span className="px-1.5 py-0.5 text-stone-700 dark:text-stone-300 bg-stone-200/60 dark:bg-stone-800/60 rounded uppercase tracking-wider">
                                    system
                                </span>
                            )}
                            <div className="flex-grow" />
                            <label className="flex items-center gap-2 cursor-pointer">
                                <input
                                    type="checkbox"
                                    checked={effectiveAll}
                                    disabled={disabled}
                                    onChange={() => toggleEnvAll(env)}
                                    className="h-4 w-4 accent-orange-600"
                                />
                                <span className="font-medium text-stone-700 dark:text-stone-300">
                                    All indexes (including future)
                                </span>
                            </label>
                        </div>
                        <ul className="grid grid-cols-1 sm:grid-cols-2 gap-x-4 gap-y-1 px-3 py-2">
                            {indexes.length === 0 && (
                                <li className="px-2 py-1 text-stone-700 dark:text-stone-300 italic">
                                    no indexes in this env
                                </li>
                            )}
                            {indexes.map((name) => {
                                const isLiteral = allowedSet.has(name);
                                const isOn = effectiveAll || isLiteral;
                                return (
                                    <li
                                        key={name}
                                        className="flex items-center gap-2 px-2 py-1 hover:bg-stone-50 dark:hover:bg-stone-800/50 rounded"
                                    >
                                        <input
                                            type="checkbox"
                                            id={`alw-${env}-${name}`}
                                            checked={isOn}
                                            disabled={disabled || effectiveAll}
                                            onChange={() => toggleIndex(env, name)}
                                            className="h-4 w-4 accent-orange-600 flex-shrink-0"
                                        />
                                        <label
                                            htmlFor={`alw-${env}-${name}`}
                                            className={`flex-1 font-mono truncate ${
                                                effectiveAll
                                                    ? "cursor-default text-stone-700 dark:text-stone-300"
                                                    : "cursor-pointer text-stone-800 dark:text-stone-200"
                                            }`}
                                            title={name}
                                        >
                                            {name}
                                        </label>
                                        {effectiveAll && (
                                            <span className="text-stone-700 dark:text-stone-300 flex-shrink-0">
                                                via {unrestricted ? "*" : "all"}
                                            </span>
                                        )}
                                    </li>
                                );
                            })}
                        </ul>
                    </div>
                );
            })}
        </div>
    );
}
