// Copyright 2026 Appbird LLC. Licensed under "GNU Affero General Public License v3.0"

import { describe, it, expect } from "vitest";
import {
    notifySavedChanged,
    onSavedChanged,
    notifyMonitorsChanged,
    onMonitorsChanged,
    notifyAlertsChanged,
    onAlertsChanged,
    notifySourcesChanged,
    onSourcesChanged,
} from "../../src/api/events";

// The cross-component bus: each `notifyX` dispatches a window event that
// `onX` subscribers receive, and the returned disposer unsubscribes.
const pairs = [
    { name: "saved", notify: notifySavedChanged, on: onSavedChanged },
    { name: "monitors", notify: notifyMonitorsChanged, on: onMonitorsChanged },
    { name: "alerts", notify: notifyAlertsChanged, on: onAlertsChanged },
    { name: "sources", notify: notifySourcesChanged, on: onSourcesChanged },
];

describe("change-event bus", () => {
    for (const { name, notify, on } of pairs) {
        it(`${name}: notify reaches subscribers and unsubscribe stops them`, () => {
            let count = 0;
            const off = on(() => {
                count++;
            });
            notify();
            notify();
            off();
            notify();
            expect(count).toBe(2);
        });
    }

    it("channels are independent (saved notify doesn't trigger alerts)", () => {
        let alerts = 0;
        const off = onAlertsChanged(() => {
            alerts++;
        });
        notifySavedChanged();
        off();
        expect(alerts).toBe(0);
    });
});
