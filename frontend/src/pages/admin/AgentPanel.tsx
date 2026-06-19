// Copyright 2026 Appbird LLC. Licensed under "GNU Affero General Public License v3.0"

// `/admin/agent` — Agent configuration screen (LLM provider settings).
// API keys are write-only: the GET returns `*_api_key_set: bool`, never plaintext.

import { useEffect, useState } from "react";
import { createPortal } from "react-dom";
import { CheckCircle2, Info, Sparkles, XCircle, Loader2, X } from "lucide-react";
import {
    getLlmSettings,
    updateLlmSettings,
    testLlmSettings,
    type LlmSettings,
    type LlmTestResult,
} from "../../api/agent";
import { Card, ErrorBanner, Toast } from "../../components/admin";
import { AGENT_ENABLED_EVENT } from "../../state/useAgentEnabled";

const PROVIDER_LABEL: Record<LlmSettings["provider"], string> = {
    openai: "OpenAI-compatible",
    anthropic: "Anthropic",
    bedrock: "AWS Bedrock (Converse)",
};

const PROVIDER_BLURB: Record<LlmSettings["provider"], string> = {
    openai: "Works with OpenAI itself plus any compatible server — llama.cpp, vLLM, LM Studio, OpenRouter, Together.",
    anthropic:
        "Anthropic Messages API. Streaming, tool use, and (where enabled by your model) extended thinking are all wired up.",
    bedrock:
        "AWS Bedrock Converse streaming. Falls back to the standard AWS credential chain when no admin credentials are configured below.",
};

// Provider choices, in display order, for the segmented control.
const PROVIDERS: { value: LlmSettings["provider"]; label: string }[] = [
    { value: "openai", label: "OpenAI-compatible" },
    { value: "bedrock", label: "AWS Bedrock" },
    { value: "anthropic", label: "Anthropic" },
];

type BedrockSecretField =
    | "bedrock_access_key_id"
    | "bedrock_secret_access_key"
    | "bedrock_session_token"
    | "bedrock_bearer_token";

export function AgentPanel() {
    const [cfg, setCfg] = useState<LlmSettings | null>(null);
    const [busy, setBusy] = useState(false);
    const [error, setError] = useState<string | null>(null);
    const [toast, setToast] = useState<string | null>(null);
    const [testing, setTesting] = useState(false);
    const [testResult, setTestResult] = useState<LlmTestResult | null>(null);
    const [enabled, setEnabled] = useState(true);

    // Local form state — staged until "Save". Mirrors `cfg` after a load;
    // API-key fields are separate because they're write-only.
    const [provider, setProvider] = useState<LlmSettings["provider"]>("openai");
    // Model is kept per-provider so switching providers doesn't lose the entry.
    const [openaiModel, setOpenaiModel] = useState("");
    const [anthropicModel, setAnthropicModel] = useState("");
    const [bedrockModel, setBedrockModel] = useState("");
    const [openaiEndpoint, setOpenaiEndpoint] = useState("");
    const [openaiApiKey, setOpenaiApiKey] = useState("");
    const [anthropicEndpoint, setAnthropicEndpoint] = useState("");
    const [anthropicApiKey, setAnthropicApiKey] = useState("");
    const [bedrockRegion, setBedrockRegion] = useState("");
    const [bedrockAuthMode, setBedrockAuthMode] =
        useState<LlmSettings["bedrock_auth_mode"]>("default_chain");
    const [bedrockAccessKeyId, setBedrockAccessKeyId] = useState("");
    const [bedrockSecretAccessKey, setBedrockSecretAccessKey] = useState("");
    const [bedrockSessionToken, setBedrockSessionToken] = useState("");
    const [bedrockBearerToken, setBedrockBearerToken] = useState("");

    useEffect(() => {
        let alive = true;
        getLlmSettings()
            .then((c) => {
                if (!alive) return;
                setCfg(c);
                setEnabled(c.enabled);
                setProvider(c.provider);
                setOpenaiModel(c.openai_model);
                setAnthropicModel(c.anthropic_model);
                setBedrockModel(c.bedrock_model);
                setOpenaiEndpoint(c.openai_endpoint);
                setAnthropicEndpoint(c.anthropic_endpoint);
                setBedrockRegion(c.bedrock_region);
                setBedrockAuthMode(c.bedrock_auth_mode);
            })
            .catch((e) => alive && setError(String(e)));
        return () => {
            alive = false;
        };
    }, []);

    // Patch reflecting the staged form. API-key fields are included only when
    // the user typed one; empty = "keep stored key", not "clear".
    function buildPatch(): Parameters<typeof updateLlmSettings>[0] {
        const patch: Parameters<typeof updateLlmSettings>[0] = {
            provider,
            openai_model: openaiModel.trim(),
            anthropic_model: anthropicModel.trim(),
            bedrock_model: bedrockModel.trim(),
            openai_endpoint: openaiEndpoint.trim(),
            anthropic_endpoint: anthropicEndpoint.trim(),
            bedrock_region: bedrockRegion.trim(),
            bedrock_auth_mode: bedrockAuthMode,
        };
        if (openaiApiKey) patch.openai_api_key = openaiApiKey;
        if (anthropicApiKey) patch.anthropic_api_key = anthropicApiKey;
        if (bedrockAccessKeyId) patch.bedrock_access_key_id = bedrockAccessKeyId;
        if (bedrockSecretAccessKey) patch.bedrock_secret_access_key = bedrockSecretAccessKey;
        if (bedrockSessionToken) patch.bedrock_session_token = bedrockSessionToken;
        if (bedrockBearerToken) patch.bedrock_bearer_token = bedrockBearerToken;
        return patch;
    }

    async function save() {
        setBusy(true);
        setError(null);
        try {
            const updated = await updateLlmSettings(buildPatch());
            setCfg(updated);
            setOpenaiApiKey("");
            setAnthropicApiKey("");
            setBedrockAccessKeyId("");
            setBedrockSecretAccessKey("");
            setBedrockSessionToken("");
            setBedrockBearerToken("");
            setToast("Saved");
            setTimeout(() => setToast(null), 2000);
        } catch (e) {
            setError(e instanceof Error ? e.message : String(e));
        } finally {
            setBusy(false);
        }
    }

    async function clearKey(field: "openai_api_key" | "anthropic_api_key" | BedrockSecretField) {
        setBusy(true);
        setError(null);
        try {
            const updated = await updateLlmSettings({ [field]: "" } as Parameters<
                typeof updateLlmSettings
            >[0]);
            setCfg(updated);
            setToast("Key cleared");
            setTimeout(() => setToast(null), 2000);
        } catch (e) {
            setError(e instanceof Error ? e.message : String(e));
        } finally {
            setBusy(false);
        }
    }

    async function toggleEnabled() {
        const next = !enabled;
        setEnabled(next);
        setBusy(true);
        setError(null);
        try {
            const updated = await updateLlmSettings({ enabled: next });
            setCfg(updated);
            window.dispatchEvent(new Event(AGENT_ENABLED_EVENT));
            setToast(next ? "Agent enabled" : "Agent disabled");
            setTimeout(() => setToast(null), 2000);
        } catch (e) {
            setEnabled(!next); // revert on failure
            setError(e instanceof Error ? e.message : String(e));
        } finally {
            setBusy(false);
        }
    }

    async function runTest() {
        setTesting(true);
        setTestResult(null);
        try {
            setTestResult(await testLlmSettings(buildPatch()));
        } catch (e) {
            setTestResult({ ok: false, error: e instanceof Error ? e.message : String(e) });
        } finally {
            setTesting(false);
        }
    }

    if (!cfg) {
        return <div className="p-6 text-stone-700 dark:text-stone-300">Loading…</div>;
    }

    // The Model input edits whichever provider is currently selected.
    const model =
        provider === "openai"
            ? openaiModel
            : provider === "anthropic"
              ? anthropicModel
              : bedrockModel;
    const setModel =
        provider === "openai"
            ? setOpenaiModel
            : provider === "anthropic"
              ? setAnthropicModel
              : setBedrockModel;

    return (
        <div>
            <Card title="LLM Provider Configuration">
                <div className="p-6 space-y-6 max-w-3xl">
                    <HelpFrame />

                    <ErrorBanner error={error} />

                    <FormRow
                        label="Enabled"
                        hint="Master switch for all AI features. When off, the Investigate panel and AI monitors are disabled across the system."
                    >
                        <EnabledToggle checked={enabled} busy={busy} onChange={toggleEnabled} />
                    </FormRow>

                    <FormRow label="Provider" hint={PROVIDER_BLURB[provider]}>
                        <div
                            role="radiogroup"
                            aria-label="LLM provider"
                            className="inline-flex rounded-md border border-stone-200 dark:border-stone-700 overflow-hidden"
                        >
                            {PROVIDERS.map((p, i) => {
                                const active = provider === p.value;
                                return (
                                    <button
                                        key={p.value}
                                        type="button"
                                        role="radio"
                                        aria-checked={active}
                                        onClick={() => setProvider(p.value)}
                                        className={`px-3.5 py-1.5 text-sm font-medium transition-colors ${
                                            i > 0
                                                ? "border-l border-stone-200 dark:border-stone-700"
                                                : ""
                                        } ${
                                            active
                                                ? "bg-orange-500 text-white"
                                                : "bg-white dark:bg-stone-950 text-stone-700 dark:text-stone-300 hover:bg-stone-50 dark:hover:bg-stone-800"
                                        }`}
                                    >
                                        {p.label}
                                    </button>
                                );
                            })}
                        </div>
                    </FormRow>

                    <FormRow
                        label="Model"
                        hint={`Model identifier the chosen provider understands. ${
                            provider === "openai"
                                ? 'For local servers, "local" is conventional.'
                                : provider === "anthropic"
                                  ? "Example: claude-sonnet-4-6, claude-opus-4-8."
                                  : "A Bedrock model ID, inference-profile ID, or ARN. Example: anthropic.claude-sonnet-4-6 (region-prefixed, e.g. us.anthropic.claude-sonnet-4-6, when an inference profile is required)."
                        }`}
                    >
                        <input
                            value={model}
                            onChange={(e) => setModel(e.target.value)}
                            placeholder={
                                provider === "openai"
                                    ? "local"
                                    : provider === "anthropic"
                                      ? "claude-sonnet-4-6"
                                      : "anthropic.claude-sonnet-4-6"
                            }
                            className="w-full px-2.5 py-1.5 bg-white dark:bg-stone-950 border border-stone-200 dark:border-stone-700 rounded-md focus:outline-none focus:border-orange-500 focus:ring-1 focus:ring-orange-500 text-stone-900 dark:text-stone-100 font-mono"
                        />
                    </FormRow>

                    <Subheader title={`${PROVIDER_LABEL[provider]} connection`} />

                    {provider === "openai" && (
                        <>
                            <FormRow
                                label="Endpoint"
                                hint={
                                    <>
                                        Base URL up to and including{" "}
                                        <code className="font-mono">/v1</code>. HeliosLogs appends{" "}
                                        <code className="font-mono">/chat/completions</code>.
                                    </>
                                }
                            >
                                <input
                                    value={openaiEndpoint}
                                    onChange={(e) => setOpenaiEndpoint(e.target.value)}
                                    placeholder="https://api.openai.com/v1"
                                    className="w-full px-2.5 py-1.5 bg-white dark:bg-stone-950 border border-stone-200 dark:border-stone-700 rounded-md focus:outline-none focus:border-orange-500 focus:ring-1 focus:ring-orange-500 text-stone-900 dark:text-stone-100 font-mono"
                                />
                            </FormRow>
                            <ApiKeyField
                                label="API key"
                                value={openaiApiKey}
                                isSet={cfg.openai_api_key_set}
                                onChange={setOpenaiApiKey}
                                onClear={() => clearKey("openai_api_key")}
                                hint="Leave blank to keep the existing key. Required only when the endpoint enforces auth — local servers usually accept anything."
                            />
                        </>
                    )}

                    {provider === "anthropic" && (
                        <>
                            <FormRow label="Endpoint" hint={<>Anthropic Messages API base URL.</>}>
                                <input
                                    value={anthropicEndpoint}
                                    onChange={(e) => setAnthropicEndpoint(e.target.value)}
                                    placeholder="https://api.anthropic.com/v1"
                                    className="w-full px-2.5 py-1.5 bg-white dark:bg-stone-950 border border-stone-200 dark:border-stone-700 rounded-md focus:outline-none focus:border-orange-500 focus:ring-1 focus:ring-orange-500 text-stone-900 dark:text-stone-100 font-mono"
                                />
                            </FormRow>
                            <ApiKeyField
                                label="API key"
                                value={anthropicApiKey}
                                isSet={cfg.anthropic_api_key_set}
                                onChange={setAnthropicApiKey}
                                onClear={() => clearKey("anthropic_api_key")}
                                hint="Leave blank to keep the existing key."
                            />
                        </>
                    )}

                    {provider === "bedrock" && (
                        <>
                            <FormRow label="Region" hint="AWS region hosting the Bedrock runtime.">
                                <input
                                    value={bedrockRegion}
                                    onChange={(e) => setBedrockRegion(e.target.value)}
                                    placeholder="us-east-1"
                                    className="w-full px-2.5 py-1.5 bg-white dark:bg-stone-950 border border-stone-200 dark:border-stone-700 rounded-md focus:outline-none focus:border-orange-500 focus:ring-1 focus:ring-orange-500 text-stone-900 dark:text-stone-100 font-mono"
                                />
                            </FormRow>
                            <FormRow
                                label="Authentication"
                                hint={
                                    <>
                                        <strong>Standard chain</strong> uses the credentials below
                                        if set, otherwise walks env vars, the shared credentials
                                        file, and IMDS — and auto-picks up{" "}
                                        <code className="font-mono">AWS_BEARER_TOKEN_BEDROCK</code>{" "}
                                        when set. <strong>Bearer token</strong> explicitly requires
                                        a bearer token (from the field below or the env var) and
                                        fails early if missing.
                                    </>
                                }
                            >
                                <select
                                    value={bedrockAuthMode}
                                    onChange={(e) =>
                                        setBedrockAuthMode(
                                            e.target.value as LlmSettings["bedrock_auth_mode"],
                                        )
                                    }
                                    className="w-full px-2.5 py-1.5 bg-white dark:bg-stone-950 border border-stone-200 dark:border-stone-700 rounded-md focus:outline-none focus:border-orange-500 focus:ring-1 focus:ring-orange-500 text-stone-900 dark:text-stone-100"
                                >
                                    <option value="default_chain">
                                        Standard AWS credential chain
                                    </option>
                                    <option value="bearer_token">Bedrock bearer token</option>
                                </select>
                            </FormRow>

                            <div className="pt-2 text-stone-700 dark:text-stone-300 leading-relaxed">
                                Any value you set below overrides the corresponding{" "}
                                <code className="font-mono">AWS_*</code> environment variable for
                                HeliosLogs. Leave blank to defer to whatever the host environment
                                provides.
                            </div>

                            <ApiKeyField
                                label="Access key ID"
                                value={bedrockAccessKeyId}
                                isSet={cfg.bedrock_access_key_id_set}
                                onChange={setBedrockAccessKeyId}
                                onClear={() => clearKey("bedrock_access_key_id")}
                                hint={
                                    <>
                                        Overrides{" "}
                                        <code className="font-mono">AWS_ACCESS_KEY_ID</code>. Used
                                        together with the secret access key below for SigV4.
                                    </>
                                }
                            />
                            <ApiKeyField
                                label="Secret access key"
                                value={bedrockSecretAccessKey}
                                isSet={cfg.bedrock_secret_access_key_set}
                                onChange={setBedrockSecretAccessKey}
                                onClear={() => clearKey("bedrock_secret_access_key")}
                                hint={
                                    <>
                                        Overrides{" "}
                                        <code className="font-mono">AWS_SECRET_ACCESS_KEY</code>.
                                    </>
                                }
                            />
                            <ApiKeyField
                                label="Session token"
                                value={bedrockSessionToken}
                                isSet={cfg.bedrock_session_token_set}
                                onChange={setBedrockSessionToken}
                                onClear={() => clearKey("bedrock_session_token")}
                                hint={
                                    <>
                                        Overrides{" "}
                                        <code className="font-mono">AWS_SESSION_TOKEN</code>. Only
                                        needed for temporary credentials (STS, SSO).
                                    </>
                                }
                            />
                            <ApiKeyField
                                label="Bearer token"
                                value={bedrockBearerToken}
                                isSet={cfg.bedrock_bearer_token_set}
                                onChange={setBedrockBearerToken}
                                onClear={() => clearKey("bedrock_bearer_token")}
                                hint={
                                    <>
                                        Overrides{" "}
                                        <code className="font-mono">AWS_BEARER_TOKEN_BEDROCK</code>.
                                        Required when the auth mode above is set to bearer token,
                                        unless the env var is already present.
                                    </>
                                }
                            />
                        </>
                    )}
                </div>

                <div className="px-6 py-4 bg-stone-50 dark:bg-stone-950/50 border-t border-stone-200 dark:border-stone-800 flex items-center gap-3">
                    <button
                        type="button"
                        onClick={save}
                        disabled={busy}
                        className="px-3 py-1.5 font-medium rounded-md bg-orange-500 hover:bg-orange-600 text-white disabled:opacity-50 disabled:cursor-not-allowed transition"
                    >
                        Save settings
                    </button>
                    <button
                        type="button"
                        onClick={runTest}
                        disabled={testing || busy}
                        className="px-3 py-1.5 font-medium rounded-md border border-stone-300 dark:border-stone-700 text-stone-700 dark:text-stone-200 hover:bg-stone-100 dark:hover:bg-stone-800 disabled:opacity-50 disabled:cursor-not-allowed transition inline-flex items-center gap-1.5"
                    >
                        {testing && <Loader2 className="w-3.5 h-3.5 animate-spin" />}
                        Test connection
                    </button>
                    <span className="text-stone-700 dark:text-stone-300 flex items-center gap-1.5">
                        <CheckCircle2 className="w-3.5 h-3.5 text-emerald-600 dark:text-emerald-400" />
                        Loaded
                    </span>
                    <span className="text-stone-500 dark:text-stone-400 ml-auto">
                        Test uses the values above — no need to save first.
                    </span>
                </div>
            </Card>

            {testResult && (
                <TestResultModal result={testResult} onClose={() => setTestResult(null)} />
            )}

            <Toast message={toast} />
        </div>
    );
}

// One labelled message in the test conversation. Assistant turns get an
// orange accent so the model's reply stands out from what HeliosLogs sent.
function MessageBubble({ role, content }: { role: string; content: string }) {
    const isAssistant = role === "assistant";
    return (
        <div>
            <div
                className={`mb-1 text-xs font-semibold uppercase tracking-wider ${
                    isAssistant
                        ? "text-orange-600 dark:text-orange-400"
                        : "text-stone-500 dark:text-stone-400"
                }`}
            >
                {role}
            </div>
            <div
                className={`p-2.5 rounded-md border font-mono whitespace-pre-wrap break-words ${
                    isAssistant
                        ? "bg-orange-50/60 dark:bg-orange-950/20 border-orange-200 dark:border-orange-900/40"
                        : "bg-stone-50 dark:bg-stone-950 border-stone-200 dark:border-stone-800"
                }`}
            >
                {content}
            </div>
        </div>
    );
}

// Modal showing the outcome of a "Test connection" run: the full exchange
// (what HeliosLogs sent + the model's reply), or the error. Closes on Esc /
// backdrop click / button.
function TestResultModal({ result, onClose }: { result: LlmTestResult; onClose: () => void }) {
    useEffect(() => {
        const onKey = (e: KeyboardEvent) => {
            if (e.key === "Escape") onClose();
        };
        document.addEventListener("keydown", onKey);
        return () => document.removeEventListener("keydown", onKey);
    }, [onClose]);

    return createPortal(
        <div
            className="fixed inset-0 z-50 flex items-center justify-center bg-black/40"
            onMouseDown={(e) => {
                if (e.target === e.currentTarget) onClose();
            }}
        >
            <div className="w-full max-w-lg mx-4 bg-white dark:bg-stone-900 border border-stone-200 dark:border-stone-700 rounded-xl shadow-xl overflow-hidden flex flex-col max-h-[85vh]">
                <header className="px-4 py-3 flex items-center justify-between border-b border-stone-200 dark:border-stone-800">
                    <h2 className="font-semibold flex items-center gap-2 text-stone-900 dark:text-stone-100">
                        {result.ok ? (
                            <CheckCircle2 className="w-4 h-4 text-emerald-600 dark:text-emerald-400" />
                        ) : (
                            <XCircle className="w-4 h-4 text-red-600 dark:text-red-400" />
                        )}
                        {result.ok ? "Connection OK" : "Connection failed"}
                    </h2>
                    <button
                        type="button"
                        onClick={onClose}
                        className="p-1 rounded text-stone-400 hover:text-stone-700 dark:hover:text-stone-200 hover:bg-stone-100 dark:hover:bg-stone-800"
                        aria-label="close"
                    >
                        <X className="w-4 h-4" />
                    </button>
                </header>

                <div className="px-4 py-4 space-y-3 text-stone-800 dark:text-stone-100 overflow-y-auto">
                    {(result.provider || result.model) && (
                        <div className="text-stone-600 dark:text-stone-300">
                            Provider <span className="font-mono">{result.provider}</span> · model{" "}
                            <span className="font-mono">{result.model}</span>
                        </div>
                    )}

                    <div className="text-stone-500 dark:text-stone-400 uppercase tracking-wider">
                        Conversation
                    </div>

                    {result.request?.map((m, i) => (
                        <MessageBubble key={i} role={m.role} content={m.content} />
                    ))}

                    {result.ok ? (
                        <MessageBubble role="assistant" content={result.reply ?? "(empty reply)"} />
                    ) : (
                        <div>
                            <div className="mb-1 text-red-600 dark:text-red-400 uppercase tracking-wider">
                                Error
                            </div>
                            <div className="p-2.5 rounded-md bg-red-50 dark:bg-red-950/30 border border-red-200 dark:border-red-900/50 font-mono break-words text-red-800 dark:text-red-200">
                                {result.error ?? "Unknown error"}
                            </div>
                        </div>
                    )}
                </div>

                <footer className="px-4 py-3 flex justify-end border-t border-stone-200 dark:border-stone-800">
                    <button
                        type="button"
                        onClick={onClose}
                        className="px-3 py-1.5 font-medium rounded-md bg-stone-900 hover:bg-stone-800 dark:bg-stone-800 dark:hover:bg-stone-700 text-white transition"
                    >
                        Close
                    </button>
                </footer>
            </div>
        </div>,
        document.body,
    );
}

// Master on/off switch for agent functionality (mirrors the MCP panel toggle).
function EnabledToggle({
    checked,
    busy,
    onChange,
}: {
    checked: boolean;
    busy: boolean;
    onChange: () => void;
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
                {checked ? "AI features are on" : "AI features are off"}
            </span>
        </div>
    );
}

// Inline help banner at the top of the panel: what the LLM provider setting
// controls and how it's stored.
function HelpFrame() {
    return (
        <div className="flex gap-3 p-4 rounded-lg bg-orange-50/60 dark:bg-orange-950/20 border border-orange-200/70 dark:border-orange-900/40">
            <div className="flex-shrink-0 mt-0.5">
                <Sparkles className="w-4 h-4 text-orange-600 dark:text-orange-400" />
            </div>
            <div className="space-y-1.5 text-stone-700 dark:text-stone-200 leading-relaxed">
                <p>
                    Choose the LLM provider and model that power HeliosLogs's AI features. Pick a
                    provider, enter the model and connection details, then use{" "}
                    <strong>Test connection</strong> to confirm HeliosLogs can reach it.
                </p>
                <p className="text-stone-700 dark:text-stone-300">
                    The setting is global — all users share one provider. API keys are write-only:
                    once saved, this screen only shows whether a key is set, never the value.
                </p>
            </div>
        </div>
    );
}

// Labelled horizontal rule introducing a subsection; lighter than a full Card.
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

// Form row with a left label column + right input column. Inline labels (not
// label-above) keep the eye from sweeping vertically across the wide panel.
function FormRow({
    label,
    hint,
    children,
}: {
    label: string;
    hint?: React.ReactNode;
    children: React.ReactNode;
}) {
    return (
        <div className="grid grid-cols-[10rem_1fr] gap-x-4 gap-y-1 items-start">
            <label className="pt-1.5 font-semibold text-stone-800 dark:text-stone-100">
                {label}
            </label>
            <div className="min-w-0 space-y-1">
                {children}
                {hint && (
                    <p className="text-stone-700 dark:text-stone-300 leading-relaxed">{hint}</p>
                )}
            </div>
        </div>
    );
}

function ApiKeyField({
    label,
    value,
    isSet,
    onChange,
    onClear,
    hint,
}: {
    label: string;
    value: string;
    isSet: boolean;
    onChange: (v: string) => void;
    onClear: () => void;
    hint?: React.ReactNode;
}) {
    return (
        <FormRow
            label={label}
            hint={
                <>
                    {isSet && (
                        <span className="inline-flex items-center gap-1 mr-2 px-1.5 py-0.5 rounded bg-emerald-50 dark:bg-emerald-950/40 border border-emerald-200 dark:border-emerald-900/50 text-emerald-700 dark:text-emerald-300">
                            <Info className="w-3 h-3" /> key configured
                        </span>
                    )}
                    {hint}
                </>
            }
        >
            <div className="flex items-center gap-2">
                <input
                    type="password"
                    value={value}
                    onChange={(e) => onChange(e.target.value)}
                    placeholder={isSet ? "(leave blank to keep existing key)" : "(not set)"}
                    className="flex-grow min-w-0 px-2.5 py-1.5 bg-white dark:bg-stone-950 border border-stone-200 dark:border-stone-700 rounded-md focus:outline-none focus:border-orange-500 focus:ring-1 focus:ring-orange-500 text-stone-900 dark:text-stone-100 font-mono"
                />
                {isSet && (
                    <button
                        type="button"
                        onClick={onClear}
                        className="px-2.5 py-1.5 text-stone-600 dark:text-stone-300 hover:bg-stone-100 dark:hover:bg-stone-800 rounded-md border border-stone-200 dark:border-stone-700 flex-shrink-0"
                        title="Clear stored key"
                    >
                        Clear
                    </button>
                )}
            </div>
        </FormRow>
    );
}
