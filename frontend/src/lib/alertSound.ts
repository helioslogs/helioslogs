// Copyright 2026 Appbird LLC. Licensed under "GNU Affero General Public License v3.0"

// A short two-tone chime for new alert toasts, synthesized via Web Audio (no asset
// to ship). The AudioContext is created lazily and resumed on first user gesture.

let ctx: AudioContext | null = null;

function ensureContext(): AudioContext | null {
    if (typeof window === "undefined") return null;
    const Ctor =
        window.AudioContext ??
        (window as unknown as { webkitAudioContext?: typeof AudioContext }).webkitAudioContext;
    if (!Ctor) return null;
    if (!ctx) ctx = new Ctor();
    return ctx;
}

// Unlock the context on the first user gesture so later alert beeps aren't
// blocked by autoplay policy. Idempotent; the listeners remove themselves.
export function primeAlertSound(): void {
    const unlock = () => {
        ensureContext()
            ?.resume()
            .catch(() => {});
        window.removeEventListener("pointerdown", unlock);
        window.removeEventListener("keydown", unlock);
    };
    window.addEventListener("pointerdown", unlock, { once: true });
    window.addEventListener("keydown", unlock, { once: true });
}

function tone(c: AudioContext, freq: number, start: number, dur: number) {
    const osc = c.createOscillator();
    const gain = c.createGain();
    osc.type = "sine";
    osc.frequency.value = freq;
    // Short attack/release envelope so the tone doesn't click on/off.
    gain.gain.setValueAtTime(0, start);
    gain.gain.linearRampToValueAtTime(0.18, start + 0.015);
    gain.gain.linearRampToValueAtTime(0, start + dur);
    osc.connect(gain).connect(c.destination);
    osc.start(start);
    osc.stop(start + dur);
}

export function playAlertSound(): void {
    const c = ensureContext();
    if (!c) return;
    if (c.state === "suspended") {
        c.resume().catch(() => {});
        if (c.state === "suspended") return;
    }
    const now = c.currentTime;
    tone(c, 880, now, 0.12);
    tone(c, 1320, now + 0.13, 0.16);
}
