// Copyright 2026 Appbird LLC. Licensed under "GNU Affero General Public License v3.0"

//! Raw syslog network listener (UDP + TCP, RFC 5424 / RFC 3164). A supervisor task
//! polls the control plane and (re)binds sockets when the listen config changes,
//! feeding parsed messages into the existing block-engine ingest path. Routing
//! (default env/index + rules) is delivered to live listeners via a `watch` channel
//! so rule edits apply without rebinding sockets. Mirrors `crate::source`.

mod listener;
mod route;

use std::sync::Arc;
use std::time::Duration;

use anyhow::Context;
use tokio::net::{TcpListener, UdpSocket};
use tokio::sync::watch;
use tokio::task::JoinHandle;
use tracing::{info, warn};

use crate::control::settings::SyslogSettings;
use crate::control::Control;
use route::SyslogRouter;

const TICK_INTERVAL: Duration = Duration::from_secs(5);

/// The listen identity that forces a socket rebind when changed. Routing-only edits
/// (default env/index, rules) do not change this and are pushed without a rebind.
#[derive(Clone, PartialEq)]
struct BindKey {
    enabled: bool,
    bind: String,
    udp_port: u16,
    tcp_port: u16,
}

impl BindKey {
    fn of(s: &SyslogSettings) -> Self {
        Self {
            enabled: s.enabled,
            bind: s.bind.clone(),
            udp_port: s.udp_port,
            tcp_port: s.tcp_port,
        }
    }
}

/// Currently-bound listeners and the channel used to push routing updates to them.
struct Running {
    key: BindKey,
    router_tx: watch::Sender<Arc<SyslogRouter>>,
    tasks: Vec<JoinHandle<()>>,
}

impl Running {
    fn abort(self) {
        for t in self.tasks {
            t.abort();
        }
    }
}

/// Long-running supervisor. Never returns; a failing tick is logged and retried.
/// `port_override` (from `--syslog-port`) shadows the control-plane UDP + TCP ports.
pub async fn run_supervisor(control: Control, port_override: Option<u16>) {
    info!(
        tick_interval_secs = TICK_INTERVAL.as_secs(),
        port_override = port_override.map(|p| p as i64).unwrap_or(-1),
        "syslog supervisor started"
    );
    let mut interval = tokio::time::interval(TICK_INTERVAL);
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut running: Option<Running> = None;
    loop {
        interval.tick().await;
        match control.syslog_settings().await {
            Ok(mut settings) => {
                // CLI override pins both transports to one port (0 disables them).
                if let Some(p) = port_override {
                    settings.udp_port = p;
                    settings.tcp_port = p;
                }
                running = reconcile(running, settings).await;
            }
            Err(e) => warn!("syslog supervisor: load settings failed: {e:#}"),
        }
    }
}

/// Bring running listeners in line with the desired settings. Same bind identity →
/// just push fresh routing; changed identity → tear down and (re)bind.
async fn reconcile(running: Option<Running>, settings: SyslogSettings) -> Option<Running> {
    let want = BindKey::of(&settings);
    let router = Arc::new(SyslogRouter::build(&settings));

    if let Some(r) = &running {
        if r.key == want {
            let _ = r.router_tx.send(router);
            return running;
        }
    }
    if let Some(r) = running {
        info!("syslog: listen config changed, restarting listeners");
        r.abort();
    }
    if !want.enabled {
        return None;
    }
    match start_listeners(&settings, router).await {
        Ok(r) => Some(r),
        Err(e) => {
            warn!(bind = %settings.bind, "syslog: failed to start listeners: {e:#}");
            None
        }
    }
}

async fn start_listeners(
    settings: &SyslogSettings,
    router: Arc<SyslogRouter>,
) -> anyhow::Result<Running> {
    let (router_tx, router_rx) = watch::channel(router);
    let mut tasks = Vec::new();

    if settings.udp_port != 0 {
        let addr = format!("{}:{}", settings.bind, settings.udp_port);
        let socket = UdpSocket::bind(&addr)
            .await
            .with_context(|| format!("bind syslog udp {addr}"))?;
        info!(%addr, "syslog: UDP listener bound");
        tasks.push(tokio::spawn(listener::run_udp(socket, router_rx.clone())));
    }
    if settings.tcp_port != 0 {
        let addr = format!("{}:{}", settings.bind, settings.tcp_port);
        let l = TcpListener::bind(&addr)
            .await
            .with_context(|| format!("bind syslog tcp {addr}"))?;
        info!(%addr, "syslog: TCP listener bound");
        tasks.push(tokio::spawn(listener::run_tcp(l, router_rx.clone())));
    }

    Ok(Running {
        key: BindKey::of(settings),
        router_tx,
        tasks,
    })
}
