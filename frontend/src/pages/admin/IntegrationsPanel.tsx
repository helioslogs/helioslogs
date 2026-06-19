// Copyright 2026 Appbird LLC. Licensed under "GNU Affero General Public License v3.0"

// `/admin/integrations` — outbound integrations: alert webhook delivery
// (generic JSON / Slack-compatible), with a send-test affordance.

import { useEffect, useState } from "react";
import { Send, Webhook } from "lucide-react";
import { getSettings, testAlertWebhook, updateSettings } from "../../api/client";
import type { Settings } from "../../api/types";
import { Card, ErrorBanner, HelpFrame, Toast } from "../../components/admin";

export function IntegrationsPanel() {
    return (
        <div>
            <Card title="Integrations">
                <div className="p-6 space-y-6 max-w-3xl">
                    <HelpFrame icon={<Webhook className="w-4 h-4" />}>
                        <p>
                            Deliver monitor alerts to external systems over a webhook. Generic posts
                            the full alert as JSON (
                            <code className="font-mono">{"{event, alert}"}</code>); the Slack format
                            posts a <code className="font-mono">{"{text}"}</code> message that also
                            works with Mattermost and Discord's{" "}
                            <code className="font-mono">/slack</code> endpoints. Individual monitors
                            can override this target in their edit dialog.
                        </p>
                    </HelpFrame>

                    <Subheader title="Alert webhook" />
                    <AlertingSection />
                </div>
            </Card>
        </div>
    );
}

// Global alert webhook delivery: enabled toggle, target URL (write-only),
// payload format, send-test. Per-monitor overrides live in the monitor dialog.
function AlertingSection() {
    const [settings, setSettings] = useState<Settings | null>(null);
    const [url, setUrl] = useState("");
    const [urlDirty, setUrlDirty] = useState(false);
    const [format, setFormat] = useState<"generic" | "slack">("generic");
    const [busy, setBusy] = useState(false);
    const [error, setError] = useState<string | null>(null);
    const [toast, setToast] = useState<string | null>(null);

    useEffect(() => {
        getSettings()
            .then((s) => {
                setSettings(s);
                setFormat(s.alert_webhook_format);
            })
            .catch((e) => setError(e instanceof Error ? e.message : String(e)));
    }, []);

    const flash = (msg: string) => {
        setToast(msg);
        setTimeout(() => setToast(null), 3000);
    };

    async function save() {
        setBusy(true);
        setError(null);
        try {
            const next = await updateSettings({
                alert_webhook_enabled: settings?.alert_webhook_enabled ?? false,
                alert_webhook_format: format,
                ...(urlDirty ? { alert_webhook_url: url.trim() } : {}),
            });
            setSettings(next);
            setUrl("");
            setUrlDirty(false);
            flash("alerting settings saved");
        } catch (e) {
            setError(e instanceof Error ? e.message : String(e));
        } finally {
            setBusy(false);
        }
    }

    async function toggleEnabled() {
        if (!settings) return;
        setBusy(true);
        setError(null);
        try {
            const next = await updateSettings({
                alert_webhook_enabled: !settings.alert_webhook_enabled,
            });
            setSettings(next);
        } catch (e) {
            setError(e instanceof Error ? e.message : String(e));
        } finally {
            setBusy(false);
        }
    }

    async function sendTest() {
        setBusy(true);
        setError(null);
        try {
            const r = await testAlertWebhook(
                urlDirty && url.trim() ? { url: url.trim(), format } : { format },
            );
            if (r.ok) {
                flash(`test delivered (HTTP ${r.status})`);
            } else {
                setError(`test failed: ${r.error ?? `HTTP ${r.status}`}`);
            }
        } catch (e) {
            setError(e instanceof Error ? e.message : String(e));
        } finally {
            setBusy(false);
        }
    }

    if (!settings) {
        return error ? (
            <ErrorBanner error={error} />
        ) : (
            <div className="text-stone-700 dark:text-stone-300">loading…</div>
        );
    }

    return (
        <div className="space-y-4">
            <ErrorBanner error={error} />
            <Toast message={toast} />
            <label className="flex items-center gap-2.5 cursor-pointer select-none">
                <input
                    type="checkbox"
                    checked={settings.alert_webhook_enabled}
                    disabled={busy}
                    onChange={() => void toggleEnabled()}
                    className="w-4 h-4 accent-orange-600"
                />
                <span className="text-stone-700 dark:text-stone-200">
                    Deliver alerts to a webhook when monitors fire
                </span>
            </label>
            <div className="grid gap-3 sm:grid-cols-[1fr_auto]">
                <input
                    type="url"
                    value={url}
                    onChange={(e) => {
                        setUrl(e.target.value);
                        setUrlDirty(true);
                    }}
                    placeholder={
                        settings.alert_webhook_url_set
                            ? "•••••• (saved — type to replace)"
                            : "https://hooks.slack.com/services/…"
                    }
                    className="px-3 py-2 rounded-md border border-stone-300 dark:border-stone-700 bg-white dark:bg-stone-900 text-stone-900 dark:text-stone-100 placeholder:text-stone-400"
                />
                <select
                    value={format}
                    onChange={(e) => setFormat(e.target.value as "generic" | "slack")}
                    className="px-3 py-2 rounded-md border border-stone-300 dark:border-stone-700 bg-white dark:bg-stone-900 text-stone-900 dark:text-stone-100"
                >
                    <option value="generic">Generic JSON</option>
                    <option value="slack">Slack-compatible</option>
                </select>
            </div>
            <p className="text-stone-500 dark:text-stone-400 leading-relaxed">
                The saved URL is never shown again — it may embed a secret. Delivery is best-effort
                with one retry; failures are logged to{" "}
                <code className="font-mono">_helioslogs</code>.
            </p>
            <div className="flex items-center gap-2">
                <button
                    type="button"
                    onClick={() => void save()}
                    disabled={busy}
                    className="px-3 py-1.5 font-medium rounded-md bg-orange-600 hover:bg-orange-500 text-white disabled:opacity-50 disabled:cursor-not-allowed transition"
                >
                    Save
                </button>
                <button
                    type="button"
                    onClick={() => void sendTest()}
                    disabled={busy}
                    className="inline-flex items-center gap-1.5 px-3 py-1.5 rounded-md border border-stone-200 dark:border-stone-700 text-stone-700 dark:text-stone-300 hover:bg-stone-50 dark:hover:bg-stone-800 disabled:opacity-50 transition"
                >
                    <Send className="w-3.5 h-3.5" aria-hidden="true" /> Send test
                </button>
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
