// Copyright 2026 Appbird LLC. Licensed under "GNU Affero General Public License v3.0"

// Tracks whether LLM/agent functionality is enabled system-wide. Backs the
// disabled placeholders in the Investigate panel and the AI monitor option.
// Refetches when an admin toggles it (via the `helios-agent-enabled` event).

import { useEffect, useState } from "react";
import { getAgentStatus } from "../api/agent";

// Dispatch this after an admin toggles the switch so open views update without reload.
export const AGENT_ENABLED_EVENT = "helios-agent-enabled";

export function useAgentEnabled(): { enabled: boolean | undefined; loading: boolean } {
    // `undefined` = still loading; treat as enabled until known to avoid a flash.
    const [enabled, setEnabled] = useState<boolean | undefined>(undefined);

    useEffect(() => {
        let alive = true;
        const load = () => {
            getAgentStatus()
                .then((s) => alive && setEnabled(s.enabled))
                .catch(() => alive && setEnabled(true)); // fail open on transient errors
        };
        load();
        window.addEventListener(AGENT_ENABLED_EVENT, load);
        return () => {
            alive = false;
            window.removeEventListener(AGENT_ENABLED_EVENT, load);
        };
    }, []);

    return { enabled, loading: enabled === undefined };
}
