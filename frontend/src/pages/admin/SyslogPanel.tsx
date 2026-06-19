// Copyright 2026 Appbird LLC. Licensed under "GNU Affero General Public License v3.0"

// `/admin/syslog` — configure the raw syslog network listener (UDP + TCP, RFC
// 5424/3164). Messages route to a default env/index unless a rule matches first.

import { useCallback, useEffect, useState } from "react";
import { Network, Plus, Trash2 } from "lucide-react";
import { getSyslogConfig, listEnvs, updateSyslogConfig } from "../../api/client";
import type { SyslogConfig, SyslogConfigPatch, SyslogRoute } from "../../api/types";
import { Card, HelpFrame, ErrorBanner, Toast } from "../../components/admin";

export function SyslogPanel() {
    const [cfg, setCfg] = useState<SyslogConfig | null>(null);
    const [busy, setBusy] = useState(false);
    const [error, setError] = useState<string | null>(null);
    const [toast, setToast] = useState<string | null>(null);

    // Editable form state.
    const [bind, setBind] = useState("");
    const [udpPort, setUdpPort] = useState("");
    const [tcpPort, setTcpPort] = useState("");
    const [defaultEnv, setDefaultEnv] = useState("");
    const [defaultIndex, setDefaultIndex] = useState("");
    const [routes, setRoutes] = useState<SyslogRoute[]>([]);
    const [envs, setEnvs] = useState<string[]>([]);

    const load = useCallback(async () => {
        try {
            const c = await getSyslogConfig();
            setCfg(c);
            setBind(c.bind);
            setUdpPort(String(c.udp_port));
            setTcpPort(String(c.tcp_port));
            setDefaultEnv(c.default_env);
            setDefaultIndex(c.default_index);
            setRoutes(c.routes);
            setError(null);
        } catch (e) {
            setError(e instanceof Error ? e.message : String(e));
        }
        // Populate the env dropdowns; non-fatal if it fails (selects still show
        // the configured value).
        try {
            setEnvs((await listEnvs(true)).map((e) => e.name));
        } catch {
            /* keep whatever we have */
        }
    }, []);

    useEffect(() => {
        void load();
    }, [load]);

    const flash = (m: string) => {
        setToast(m);
        setTimeout(() => setToast(null), 2500);
    };

    const save = useCallback(async (patch: SyslogConfigPatch, msg: string) => {
        setBusy(true);
        setError(null);
        try {
            const c = await updateSyslogConfig(patch);
            setCfg(c);
            setRoutes(c.routes);
            flash(msg);
        } catch (e) {
            setError(e instanceof Error ? e.message : String(e));
        } finally {
            setBusy(false);
        }
    }, []);

    const saveDetails = () =>
        save(
            {
                bind,
                udp_port: Number(udpPort) || 0,
                tcp_port: Number(tcpPort) || 0,
                default_env: defaultEnv,
                default_index: defaultIndex,
                routes,
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

    const updateRoute = (i: number, patch: Partial<SyslogRoute>) =>
        setRoutes((rs) => rs.map((r, j) => (j === i ? { ...r, ...patch } : r)));
    const addRoute = () =>
        setRoutes((rs) => [
            ...rs,
            {
                field: cfg.route_fields[0] ?? "appname",
                op: cfg.route_ops[0] ?? "equals",
                value: "",
            },
        ]);
    const removeRoute = (i: number) => setRoutes((rs) => rs.filter((_, j) => j !== i));

    return (
        <>
            <Card title="Raw syslog ingestion">
                <div className="p-6 space-y-5">
                    <HelpFrame icon={<Network className="w-5 h-5" />}>
                        <p>
                            Listen for syslog directly over the network (UDP and TCP, RFC 5424 and
                            RFC 3164) — no JSON wrapping or log shipper required. Point your
                            devices, routers, or <code>rsyslog</code>/<code>syslog-ng</code>{" "}
                            forwarders at this host and port.
                        </p>
                        <p>
                            <strong>There is no authentication on the syslog port.</strong> Anything
                            that can reach it can write to the configured env/index — bind it to a
                            trusted interface and firewall it accordingly. The sender's IP is
                            recorded as the per-event <code>source</code>.
                        </p>
                    </HelpFrame>

                    {/* Enable toggle */}
                    <div className="flex items-center gap-3">
                        <span className="font-semibold text-stone-800 dark:text-stone-100 min-w-[160px]">
                            Enabled
                        </span>
                        <Toggle
                            checked={cfg.enabled}
                            busy={busy}
                            onChange={() =>
                                save(
                                    { enabled: !cfg.enabled },
                                    cfg.enabled ? "Disabled" : "Enabled",
                                )
                            }
                            labelOn="Listening for syslog on the configured ports"
                            labelOff="Syslog listener is off"
                        />
                    </div>

                    {/* Listener binding */}
                    <div className="space-y-3">
                        <h3 className="font-semibold text-stone-700 dark:text-stone-200">
                            Listener
                        </h3>
                        <Field
                            label="Bind address"
                            value={bind}
                            onChange={setBind}
                            placeholder="0.0.0.0"
                        />
                        <div className="flex gap-4">
                            <Field
                                label="UDP port (0 = off)"
                                value={udpPort}
                                onChange={setUdpPort}
                                placeholder="5514"
                            />
                            <Field
                                label="TCP port (0 = off)"
                                value={tcpPort}
                                onChange={setTcpPort}
                                placeholder="5514"
                            />
                        </div>
                        <p className="text-stone-500 text-sm">
                            Ports below 1024 (e.g. the standard 514) usually need elevated
                            privileges; 5514 binds without root. Changing the bind address or ports
                            rebinds the sockets within a few seconds.
                        </p>
                        {cfg.port_override != null && (
                            <p className="text-sm text-amber-700 dark:text-amber-400">
                                This instance was started with{" "}
                                <code>--syslog-port {cfg.port_override}</code> — it listens on port{" "}
                                {cfg.port_override} (UDP + TCP) and ignores the ports above.
                            </p>
                        )}
                    </div>

                    {/* Default target */}
                    <div className="space-y-3">
                        <h3 className="font-semibold text-stone-700 dark:text-stone-200">
                            Default target
                        </h3>
                        <div className="flex gap-4">
                            <div className="flex-1">
                                <label className="block font-medium text-stone-700 dark:text-stone-300 mb-1">
                                    Environment
                                </label>
                                <EnvSelect
                                    value={defaultEnv}
                                    envs={envs}
                                    onChange={setDefaultEnv}
                                />
                            </div>
                            <Field
                                label="Index"
                                value={defaultIndex}
                                onChange={setDefaultIndex}
                                placeholder="syslog"
                            />
                        </div>
                    </div>

                    {/* Routing rules */}
                    <div className="space-y-3">
                        <h3 className="font-semibold text-stone-700 dark:text-stone-200">
                            Routing rules (optional)
                        </h3>
                        <p className="text-stone-500 text-sm">
                            Messages are matched top-to-bottom; the first matching rule sets the
                            env/index (blank fields fall back to the default target). With no rules,
                            everything lands in the default target.
                        </p>
                        {routes.map((r, i) => (
                            <RouteRow
                                key={i}
                                route={r}
                                fields={cfg.route_fields}
                                ops={cfg.route_ops}
                                envs={envs}
                                onChange={(patch) => updateRoute(i, patch)}
                                onRemove={() => removeRoute(i)}
                            />
                        ))}
                        <button
                            type="button"
                            onClick={addRoute}
                            className="inline-flex items-center gap-1.5 text-orange-700 dark:text-orange-400 hover:underline"
                        >
                            <Plus className="w-4 h-4" /> Add rule
                        </button>
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

function RouteRow({
    route,
    fields,
    ops,
    envs,
    onChange,
    onRemove,
}: {
    route: SyslogRoute;
    fields: string[];
    ops: string[];
    envs: string[];
    onChange: (patch: Partial<SyslogRoute>) => void;
    onRemove: () => void;
}) {
    const selectCls =
        "px-2 py-1.5 bg-stone-50 dark:bg-stone-950 border border-stone-200 dark:border-stone-700 rounded-md focus:outline-none focus:border-orange-500";
    const inputCls = `${selectCls} flex-1 min-w-0`;
    return (
        <div className="flex flex-wrap items-center gap-2">
            <select
                value={route.field}
                onChange={(e) => onChange({ field: e.target.value })}
                className={selectCls}
            >
                {fields.map((f) => (
                    <option key={f} value={f}>
                        {f}
                    </option>
                ))}
            </select>
            <select
                value={route.op}
                onChange={(e) => onChange({ op: e.target.value })}
                className={selectCls}
            >
                {ops.map((o) => (
                    <option key={o} value={o}>
                        {o}
                    </option>
                ))}
            </select>
            <input
                type="text"
                value={route.value}
                onChange={(e) => onChange({ value: e.target.value })}
                placeholder="value"
                className={inputCls}
            />
            <span className="text-stone-400">→</span>
            <EnvSelect
                value={route.env ?? ""}
                envs={envs}
                onChange={(v) => onChange({ env: v })}
                allowDefault
                className={`${selectCls} flex-1 min-w-0`}
            />
            <input
                type="text"
                value={route.index ?? ""}
                onChange={(e) => onChange({ index: e.target.value })}
                placeholder="index (default)"
                className={inputCls}
            />
            <button
                type="button"
                onClick={onRemove}
                className="p-1.5 text-stone-500 hover:text-red-600 transition"
                aria-label="Remove rule"
            >
                <Trash2 className="w-4 h-4" />
            </button>
        </div>
    );
}

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

function EnvSelect({
    value,
    envs,
    onChange,
    allowDefault,
    className,
}: {
    value: string;
    envs: string[];
    onChange: (v: string) => void;
    allowDefault?: boolean;
    className?: string;
}) {
    const cls =
        className ??
        "w-full px-2.5 py-1.5 bg-stone-50 dark:bg-stone-950 border border-stone-200 dark:border-stone-700 rounded-md focus:outline-none focus:border-orange-500";
    // Keep the configured value selectable even if it isn't a registered env.
    const opts = value && !envs.includes(value) ? [value, ...envs] : envs;
    return (
        <select className={cls} value={value} onChange={(e) => onChange(e.target.value)}>
            {allowDefault && <option value="">(default)</option>}
            {opts.map((n) => (
                <option key={n} value={n}>
                    {n}
                </option>
            ))}
        </select>
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
        <div className="flex-1">
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
