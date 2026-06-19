// Copyright 2026 Appbird LLC. Licensed under "GNU Affero General Public License v3.0"

// `/admin/saml` — configure a single trusted SAML IdP for SSO. Identity is match-only
// (no auto-provisioning); the pinned signing cert is the security anchor.

import { useCallback, useEffect, useState } from "react";
import { ShieldCheck, Download } from "lucide-react";
import { getSamlConfig, updateSamlConfig } from "../../api/client";
import type { SamlConfig, SamlConfigPatch } from "../../api/types";
import { Card, HelpFrame, ErrorBanner, Toast } from "../../components/admin";

export function SamlPanel() {
    const [cfg, setCfg] = useState<SamlConfig | null>(null);
    const [busy, setBusy] = useState(false);
    const [error, setError] = useState<string | null>(null);
    const [toast, setToast] = useState<string | null>(null);

    // Editable form fields (cert is write-only — entered to replace, never shown).
    const [idpEntityId, setIdpEntityId] = useState("");
    const [idpSsoUrl, setIdpSsoUrl] = useState("");
    const [spEntityId, setSpEntityId] = useState("");
    const [acsUrl, setAcsUrl] = useState("");
    const [emailAttr, setEmailAttr] = useState("");
    const [buttonLabel, setButtonLabel] = useState("");
    const [newCert, setNewCert] = useState("");

    const load = useCallback(async () => {
        try {
            const c = await getSamlConfig();
            setCfg(c);
            setIdpEntityId(c.idp_entity_id);
            setIdpSsoUrl(c.idp_sso_url);
            setSpEntityId(c.sp_entity_id);
            setAcsUrl(c.acs_url);
            setEmailAttr(c.email_attr ?? "");
            setButtonLabel(c.button_label);
            setError(null);
        } catch (e) {
            setError(e instanceof Error ? e.message : String(e));
        }
    }, []);

    useEffect(() => {
        void load();
    }, [load]);

    const flash = (m: string) => {
        setToast(m);
        setTimeout(() => setToast(null), 2500);
    };

    async function save(patch: SamlConfigPatch, msg: string) {
        setBusy(true);
        setError(null);
        try {
            const c = await updateSamlConfig(patch);
            setCfg(c);
            if (patch.idp_cert !== undefined) setNewCert("");
            flash(msg);
        } catch (e) {
            setError(e instanceof Error ? e.message : String(e));
        } finally {
            setBusy(false);
        }
    }

    const saveDetails = () =>
        save(
            {
                idp_entity_id: idpEntityId,
                idp_sso_url: idpSsoUrl,
                sp_entity_id: spEntityId,
                acs_url: acsUrl,
                email_attr: emailAttr,
                button_label: buttonLabel,
                ...(newCert.trim() ? { idp_cert: newCert.trim() } : {}),
            },
            "Saved",
        );

    if (!cfg) {
        return (
            <div className="p-6">
                <ErrorBanner error={error} />
                {!error && <div className="text-stone-500">Loading…</div>}
            </div>
        );
    }

    const canEnable = cfg.cert_set && spEntityId.trim() !== "" && acsUrl.trim() !== "";

    return (
        <>
            <Card title="SAML single sign-on">
                <div className="p-6 space-y-5">
                    <HelpFrame icon={<ShieldCheck className="w-5 h-5" />}>
                        <p>
                            Configure one trusted identity provider (ADFS, Entra, Okta, Keycloak…).
                            A signed assertion logs in a user that <strong>already exists</strong>{" "}
                            in HeliosLogs, matched by email then userid — there is no
                            auto-provisioning.
                        </p>
                        <p>
                            Register HeliosLogs with your IdP using the SP metadata below, then
                            paste the IdP's <strong>signing certificate</strong> here to pin it.
                            Assertions must be signed by its key.
                        </p>
                    </HelpFrame>

                    {/* Enable toggle */}
                    <div className="flex items-center gap-3">
                        <span className="font-semibold text-stone-800 dark:text-stone-100 min-w-[160px]">
                            Enabled
                        </span>
                        <Toggle
                            checked={cfg.enabled}
                            busy={busy || (!cfg.enabled && !canEnable)}
                            onChange={() =>
                                save(
                                    { enabled: !cfg.enabled },
                                    cfg.enabled ? "Disabled" : "Enabled",
                                )
                            }
                            labelOn="SSO is on — the “Sign in with SSO” button appears on the login page"
                            labelOff={
                                canEnable
                                    ? "SSO is off"
                                    : "Set the pinned certificate, SP entity ID and ACS URL before enabling"
                            }
                        />
                    </div>

                    {/* SSO-only enforcement */}
                    <div className="flex items-center gap-3">
                        <span className="font-semibold text-stone-800 dark:text-stone-100 min-w-[160px]">
                            Allow local logins
                        </span>
                        <Toggle
                            checked={!cfg.local_login_disabled}
                            busy={busy || !cfg.enabled}
                            onChange={() =>
                                save(
                                    { local_login_disabled: !cfg.local_login_disabled },
                                    cfg.local_login_disabled
                                        ? "Local logins enabled"
                                        : "Local logins disabled",
                                )
                            }
                            labelOn="Password login is allowed for everyone"
                            labelOff="Password login is disabled — only admins can sign in with a password (break-glass at /login?local=1)"
                        />
                    </div>

                    {/* IdP details */}
                    <div className="space-y-3">
                        <h3 className="font-semibold text-stone-700 dark:text-stone-200">
                            Identity provider
                        </h3>
                        <Field
                            label="IdP entity ID"
                            value={idpEntityId}
                            onChange={setIdpEntityId}
                            placeholder="https://idp.example.com/adfs/services/trust"
                        />
                        <Field
                            label="IdP SSO URL (HTTP-Redirect)"
                            value={idpSsoUrl}
                            onChange={setIdpSsoUrl}
                            placeholder="https://idp.example.com/adfs/ls/"
                        />
                        <div>
                            <label className="block font-medium text-stone-700 dark:text-stone-300 mb-1">
                                Signing certificate (PEM){cfg.cert_set && " — replace"}
                            </label>
                            {cfg.cert_set && (
                                <div className="mb-2 text-stone-500">
                                    Pinned · SHA-256{" "}
                                    <code className="text-stone-600 dark:text-stone-300 break-all">
                                        {cfg.cert_fingerprint}
                                    </code>
                                </div>
                            )}
                            <textarea
                                value={newCert}
                                onChange={(e) => setNewCert(e.target.value)}
                                placeholder={
                                    "-----BEGIN CERTIFICATE-----\n…\n-----END CERTIFICATE-----"
                                }
                                rows={4}
                                className="w-full px-2.5 py-1.5 font-mono text-xs bg-stone-50 dark:bg-stone-950 border border-stone-200 dark:border-stone-700 rounded-md focus:outline-none focus:border-orange-500"
                            />
                        </div>
                    </div>

                    {/* SP details */}
                    <div className="space-y-3">
                        <h3 className="font-semibold text-stone-700 dark:text-stone-200">
                            This service provider
                        </h3>
                        <Field
                            label="SP entity ID (expected audience)"
                            value={spEntityId}
                            onChange={setSpEntityId}
                            placeholder="https://helios.example.com/saml/metadata"
                        />
                        <Field
                            label="ACS URL (where the IdP posts the response)"
                            value={acsUrl}
                            onChange={setAcsUrl}
                            placeholder="https://helios.example.com/api/auth/saml/acs"
                        />
                        <Field
                            label="Match attribute (optional — defaults to NameID)"
                            value={emailAttr}
                            onChange={setEmailAttr}
                            placeholder="e.g. mail / upn"
                        />
                        <Field
                            label="Login button label"
                            value={buttonLabel}
                            onChange={setButtonLabel}
                            placeholder="Sign in with SSO"
                        />
                        <a
                            href="/api/auth/saml/metadata"
                            target="_blank"
                            rel="noreferrer"
                            className="inline-flex items-center gap-1.5 text-orange-700 dark:text-orange-400 hover:underline"
                        >
                            <Download className="w-4 h-4" /> Download SP metadata
                        </a>
                    </div>

                    <div className="flex items-center gap-3">
                        <button
                            type="button"
                            onClick={saveDetails}
                            disabled={busy}
                            className="px-3 py-1.5 font-medium text-white bg-orange-600 hover:bg-orange-500 rounded-md transition disabled:opacity-50 disabled:cursor-not-allowed"
                        >
                            Save changes
                        </button>
                        <ErrorBanner error={error} />
                    </div>
                </div>
            </Card>
            <Toast message={toast} />
        </>
    );
}

// Switch styled to match the MCP server page.
function Toggle({
    checked,
    busy,
    onChange,
    labelOn,
    labelOff,
}: {
    checked: boolean;
    busy: boolean;
    onChange: () => void;
    labelOn: string;
    labelOff: string;
}) {
    return (
        <div className="flex items-center gap-3">
            <button
                type="button"
                role="switch"
                aria-checked={checked}
                onClick={onChange}
                disabled={busy}
                className={`relative inline-flex h-6 w-11 items-center rounded-full transition disabled:opacity-50 ${
                    checked ? "bg-orange-600" : "bg-stone-300 dark:bg-stone-700"
                }`}
            >
                <span
                    className={`inline-block h-5 w-5 transform rounded-full bg-white transition ${
                        checked ? "translate-x-5" : "translate-x-0.5"
                    }`}
                />
            </button>
            <span className="text-stone-700 dark:text-stone-300">
                {checked ? labelOn : labelOff}
            </span>
        </div>
    );
}

function Field({
    label,
    value,
    onChange,
    placeholder,
}: {
    label: string;
    value: string;
    onChange: (v: string) => void;
    placeholder?: string;
}) {
    return (
        <div>
            <label className="block font-medium text-stone-700 dark:text-stone-300 mb-1">
                {label}
            </label>
            <input
                type="text"
                value={value}
                onChange={(e) => onChange(e.target.value)}
                placeholder={placeholder}
                className="w-full px-2.5 py-1.5 bg-stone-50 dark:bg-stone-950 border border-stone-200 dark:border-stone-700 rounded-md focus:outline-none focus:border-orange-500"
            />
        </div>
    );
}
