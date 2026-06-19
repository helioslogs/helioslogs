// Copyright 2026 Appbird LLC. Licensed under "GNU Affero General Public License v3.0"

//! In-process synthetic log generator for the first-run "Load sample data" button.
//! Produces a few thousand realistic events across several indexes with timestamps
//! spread over the last few hours, so a fresh instance has something to search
//! immediately. Self-contained (no external file, no `rand` dep) so it ships in the
//! binary and always emits current timestamps.

use chrono::{Duration, Utc};
use serde_json::{json, Value};

/// How many hours back to spread the sample events over (matches the default search window).
const SPAN_HOURS: i64 = 6;
/// Events generated per source; total ≈ `PER_SOURCE * <source count>`.
const PER_SOURCE: usize = 600;

/// Tiny xorshift64* PRNG — deterministic given a seed, no crate dependency. Plenty
/// random for spreading sample data; not for anything security-sensitive.
struct Rng(u64);

impl Rng {
    fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545F4914F6CDD1D)
    }
    /// Uniform in `[0, n)`.
    fn below(&mut self, n: usize) -> usize {
        (self.next_u64() % n as u64) as usize
    }
    /// Pick one element.
    fn pick<'a, T>(&mut self, xs: &'a [T]) -> &'a T {
        &xs[self.below(xs.len())]
    }
}

/// Generate the sample event set as `(index, event)` pairs ready for ingestion.
/// Seeded from the wall clock so repeated clicks add fresh, slightly different data.
pub fn generate() -> Vec<(&'static str, Value)> {
    let seed = Utc::now().timestamp_nanos_opt().unwrap_or(0) as u64 | 1;
    let mut rng = Rng(seed);
    let now = Utc::now();
    let span_secs = SPAN_HOURS * 3600;

    let mut out: Vec<(&'static str, Value)> = Vec::with_capacity(PER_SOURCE * 5);
    // Random timestamp within the span, RFC3339 — biased toward "now" so recent
    // buckets are denser, like real traffic.
    let mut ts = |rng: &mut Rng| {
        let r = rng.below(1000) as i64;
        // Square the [0,1) draw so most events land in the last hour or two.
        let back = (r * r) / 1000 * span_secs / 1000;
        (now - Duration::seconds(back)).to_rfc3339()
    };

    gen_apache(&mut out, &mut rng, &mut ts);
    gen_python(&mut out, &mut rng, &mut ts);
    gen_postgres(&mut out, &mut rng, &mut ts);
    gen_auth(&mut out, &mut rng, &mut ts);
    gen_k8s(&mut out, &mut rng, &mut ts);
    out
}

fn gen_apache(
    out: &mut Vec<(&'static str, Value)>,
    rng: &mut Rng,
    ts: &mut impl FnMut(&mut Rng) -> String,
) {
    const PATHS: &[&str] = &[
        "/",
        "/login",
        "/api/orders",
        "/api/products",
        "/static/app.js",
        "/checkout",
        "/api/cart",
        "/health",
        "/api/search",
        "/favicon.ico",
    ];
    const METHODS: &[&str] = &["GET", "GET", "GET", "POST", "POST", "PUT", "DELETE"];
    // Weighted toward 200s, with a realistic tail of errors.
    const STATUSES: &[u16] = &[200, 200, 200, 200, 200, 301, 304, 400, 401, 404, 500, 503];
    for _ in 0..PER_SOURCE {
        let status = *rng.pick(STATUSES);
        let method = *rng.pick(METHODS);
        let path = *rng.pick(PATHS);
        let ip = format!("10.0.{}.{}", rng.below(255), rng.below(255));
        let bytes = rng.below(8000) + 120;
        out.push((
            "apache_access",
            json!({
                "timestamp": ts(rng),
                "source": "apache",
                "client_ip": ip,
                "method": method,
                "path": path,
                "status": status,
                "bytes": bytes,
                "message": format!("{method} {path} {status}"),
            }),
        ));
    }
}

fn gen_python(
    out: &mut Vec<(&'static str, Value)>,
    rng: &mut Rng,
    ts: &mut impl FnMut(&mut Rng) -> String,
) {
    const LOGGERS: &[&str] = &[
        "api.orders",
        "api.auth",
        "worker.email",
        "db.pool",
        "cache.redis",
    ];
    const LEVELS: &[&str] = &["INFO", "INFO", "INFO", "INFO", "DEBUG", "WARN", "ERROR"];
    const MSGS: &[&str] = &[
        "request completed",
        "order placed",
        "cache miss",
        "retrying upstream call",
        "connection pool exhausted",
        "unhandled exception in handler",
        "payment authorized",
        "slow query detected",
    ];
    for _ in 0..PER_SOURCE {
        let level = *rng.pick(LEVELS);
        let logger = *rng.pick(LOGGERS);
        let msg = *rng.pick(MSGS);
        out.push((
            "python_app",
            json!({
                "timestamp": ts(rng),
                "source": "python_app",
                "level": level,
                "logger": logger,
                "message": msg,
                "latency_ms": rng.below(1500),
            }),
        ));
    }
}

fn gen_postgres(
    out: &mut Vec<(&'static str, Value)>,
    rng: &mut Rng,
    ts: &mut impl FnMut(&mut Rng) -> String,
) {
    const LEVELS: &[&str] = &["LOG", "LOG", "LOG", "WARNING", "ERROR", "FATAL"];
    const STMTS: &[&str] = &[
        "SELECT * FROM orders WHERE user_id = $1",
        "UPDATE inventory SET qty = qty - $1 WHERE sku = $2",
        "INSERT INTO events (kind, payload) VALUES ($1, $2)",
        "SELECT count(*) FROM sessions",
        "DELETE FROM carts WHERE updated_at < $1",
    ];
    for _ in 0..PER_SOURCE {
        let level = *rng.pick(LEVELS);
        let dur = rng.below(4000);
        let stmt = *rng.pick(STMTS);
        out.push((
            "postgres_logs",
            json!({
                "timestamp": ts(rng),
                "source": "postgres",
                "level": level,
                "duration_ms": dur,
                "statement": stmt,
                "message": format!("duration: {dur} ms  statement: {stmt}"),
            }),
        ));
    }
}

fn gen_auth(
    out: &mut Vec<(&'static str, Value)>,
    rng: &mut Rng,
    ts: &mut impl FnMut(&mut Rng) -> String,
) {
    const USERS: &[&str] = &[
        "alice",
        "bob",
        "carol",
        "dave",
        "erin",
        "admin",
        "svc-deploy",
    ];
    const EVENTS: &[&str] = &[
        "login",
        "login",
        "login",
        "logout",
        "mfa_challenge",
        "password_change",
    ];
    const OUTCOMES: &[&str] = &["success", "success", "success", "success", "failure"];
    for _ in 0..PER_SOURCE {
        let user = *rng.pick(USERS);
        let event = *rng.pick(EVENTS);
        let outcome = *rng.pick(OUTCOMES);
        let ip = format!("203.0.113.{}", rng.below(255));
        out.push((
            "auth_audit",
            json!({
                "timestamp": ts(rng),
                "source": "auth",
                "event": event,
                "user": user,
                "outcome": outcome,
                "source_ip": ip,
                "message": format!("{event} {outcome} for {user}"),
            }),
        ));
    }
}

fn gen_k8s(
    out: &mut Vec<(&'static str, Value)>,
    rng: &mut Rng,
    ts: &mut impl FnMut(&mut Rng) -> String,
) {
    const NS: &[&str] = &["default", "payments", "checkout", "ingest", "kube-system"];
    const REASONS: &[&str] = &[
        "Scheduled",
        "Pulled",
        "Started",
        "BackOff",
        "Unhealthy",
        "Killing",
        "OOMKilling",
        "FailedMount",
    ];
    const KINDS: &[&str] = &["Normal", "Normal", "Normal", "Warning"];
    for _ in 0..PER_SOURCE {
        let ns = *rng.pick(NS);
        let reason = *rng.pick(REASONS);
        let kind = *rng.pick(KINDS);
        let pod = format!(
            "{}-{}",
            rng.pick(&["api", "worker", "web", "cron"]),
            rng.below(9999)
        );
        out.push((
            "k8s_events",
            json!({
                "timestamp": ts(rng),
                "source": "k8s",
                "namespace": ns,
                "pod": pod,
                "reason": reason,
                "type": kind,
                "message": format!("{reason} pod/{pod} in {ns}"),
            }),
        ));
    }
}
