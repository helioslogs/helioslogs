// Copyright 2026 Appbird LLC. Licensed under "GNU Affero General Public License v3.0"

import { useEffect, useState } from "react";
import { formatDuration } from "../lib/formatDuration";

// Wall-clock counter ticking once per second; elapsed = performance.now() - startedAt.
// setInterval (not rAF) so background tabs throttle it.
export function LiveDuration({ startedAt }: { startedAt: number }) {
    // Force-re-render once per second; the state value itself is unused.
    const [, force] = useState(0);
    useEffect(() => {
        const id = setInterval(() => force((x) => x + 1), 1000);
        return () => clearInterval(id);
    }, []);
    const elapsed = performance.now() - startedAt;
    return <span className="tabular-nums">{formatDuration(elapsed)}</span>;
}
