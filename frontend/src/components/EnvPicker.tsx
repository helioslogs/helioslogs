// Copyright 2026 Appbird LLC. Licensed under "GNU Affero General Public License v3.0"

// Top-nav env picker. Reads the env list from /api/envs; the active env is a
// localStorage preference (`helios.env`) sent as `?env=` and applied via full reload on switch.

import { useEffect, useRef, useState } from "react";
import { ChevronDown, Globe } from "lucide-react";

import { getEnv, listEnvs, setEnv, type EnvRow } from "../api/client";
import { useAuth } from "../state/useAuth";

export function EnvPicker() {
    const { user } = useAuth();
    const [open, setOpen] = useState(false);
    const [envs, setEnvs] = useState<EnvRow[]>([]);
    const wrapRef = useRef<HTMLDivElement>(null);

    useEffect(() => {
        let cancelled = false;
        void (async () => {
            try {
                const list = await listEnvs(true);
                if (!cancelled) setEnvs(list);
            } catch {
                if (!cancelled) setEnvs([]);
            }
        })();
        return () => {
            cancelled = true;
        };
    }, []);

    // Click-outside to close.
    useEffect(() => {
        if (!open) return;
        const onClick = (e: MouseEvent) => {
            if (wrapRef.current && !wrapRef.current.contains(e.target as Node)) {
                setOpen(false);
            }
        };
        window.addEventListener("mousedown", onClick);
        return () => window.removeEventListener("mousedown", onClick);
    }, [open]);

    if (!user) return null;
    const active = getEnv();

    const handlePick = (name: string) => {
        if (name === active) {
            setOpen(false);
            return;
        }
        // Persist + sync `?env=` so the post-reload startup check sees the new env,
        // then hard-reload so every list/hook re-fetches.
        setEnv(name);
        const url = new URL(window.location.href);
        url.searchParams.set("env", name);
        window.history.replaceState(window.history.state, "", url);
        window.location.reload();
    };

    return (
        <div ref={wrapRef} className="relative">
            <button
                type="button"
                onClick={() => setOpen((v) => !v)}
                className="flex items-center gap-1.5 px-2.5 py-1 mr-2 rounded-md text-stone-300 hover:bg-stone-800 hover:text-white transition text-sm"
                title="Switch environment"
            >
                <Globe className="w-3.5 h-3.5" />
                <span className="font-medium">{active}</span>
                <ChevronDown className="w-3 h-3 opacity-70" />
            </button>
            {open && (
                <div className="absolute right-0 mt-1 w-44 max-h-64 overflow-y-auto bg-stone-900 border border-stone-700 rounded-md shadow-lg py-1 z-50">
                    {envs.length === 0 && (
                        <div className="px-3 py-1.5 text-xs text-stone-500">(no envs)</div>
                    )}
                    {envs.map((e) => (
                        <button
                            key={e.name}
                            type="button"
                            onClick={() => void handlePick(e.name)}
                            className={`w-full text-left px-3 py-1.5 text-sm transition flex items-center justify-between ${
                                e.name === active
                                    ? "text-white bg-stone-800"
                                    : "text-stone-300 hover:bg-stone-800 hover:text-white"
                            }`}
                        >
                            <span>{e.name}</span>
                            {e.name === active && (
                                <span className="text-xs text-orange-400 font-medium">active</span>
                            )}
                        </button>
                    ))}
                </div>
            )}
        </div>
    );
}
