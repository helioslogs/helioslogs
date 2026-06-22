// Copyright 2026 Appbird LLC. Licensed under "GNU Affero General Public License v3.0"

// Transient toast shown when a write API is rejected by read-only demo mode.
// Driven by the `helios-demo-blocked` window event (dispatched from apiFetch).

import { useEffect, useState } from "react";
import { Lock } from "lucide-react";
import { onDemoBlocked } from "../api/events";

export function DemoBlockedToast() {
    const [msg, setMsg] = useState<string | null>(null);

    useEffect(() => {
        let timer: ReturnType<typeof setTimeout> | undefined;
        const off = onDemoBlocked((m) => {
            setMsg(m);
            if (timer) clearTimeout(timer);
            timer = setTimeout(() => setMsg(null), 4000);
        });
        return () => {
            off();
            if (timer) clearTimeout(timer);
        };
    }, []);

    if (!msg) return null;
    return (
        <div className="fixed bottom-4 left-1/2 -translate-x-1/2 z-[60] flex items-center gap-2 px-4 py-2.5 rounded-lg shadow-lg bg-amber-600 text-white max-w-md">
            <Lock className="w-4 h-4 flex-shrink-0" />
            <span className="text-sm">{msg}</span>
        </div>
    );
}
