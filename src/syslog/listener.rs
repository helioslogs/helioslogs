// Copyright 2026 Appbird LLC. Licensed under "GNU Affero General Public License v3.0"

//! UDP + TCP syslog receive loops. Each framed message is parsed, routed, and
//! submitted to the block writer. Routing config is read live from a `watch`
//! receiver so admin rule edits apply without reconnecting senders.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use chrono::Utc;
use tokio::io::AsyncReadExt;
use tokio::net::{TcpListener, TcpStream, UdpSocket};
use tokio::sync::watch;
use tracing::{debug, warn};

use crate::catalog::PartitionKey;
use crate::engine::block::{submit, SubmitResult};
use crate::indexer::ingest;
use crate::indexer::parse::syslog::parse_line;

use super::route::{SyslogFields, SyslogRouter};

const UDP_BUF: usize = 64 * 1024; // max UDP datagram
const MAX_LINE: usize = 1024 * 1024; // cap a single TCP-framed message
const READ_CHUNK: usize = 8 * 1024;
const ERR_BACKOFF: Duration = Duration::from_millis(100);

/// Parse one raw syslog line, route it, and hand the row to the block writer.
fn handle_line(line: &str, source_ip: &str, router: &SyslogRouter) {
    let line = line.trim();
    if line.is_empty() {
        return;
    }
    let v = parse_line(line);
    let fields = SyslogFields::from_parsed(&v, source_ip);
    let (env, index_tmpl) = router.route_for(&fields);
    // The routed index may be an `app-{{field}}` template — resolve per event.
    let index = ingest::resolve_index_template(index_tmpl, &v);
    let day = ingest::event_day(&v).unwrap_or_else(|| Utc::now().date_naive());
    let Ok(row) = ingest::json_to_row(&v, Some(source_ip)) else {
        return;
    };
    let key = PartitionKey::new(env, index, day);
    match submit(key, row) {
        SubmitResult::Accepted => {}
        // Dropping under backpressure beats stalling the recv loop (which would lose
        // far more, especially over lossy UDP).
        SubmitResult::Full => debug!(%source_ip, "syslog: ingest queue full, dropping message"),
        SubmitResult::NoWriter => warn!("syslog: block ingest writer not running"),
    }
}

/// UDP receive loop. One datagram may pack several newline-separated lines.
pub async fn run_udp(socket: UdpSocket, rx: watch::Receiver<Arc<SyslogRouter>>) {
    let mut buf = vec![0u8; UDP_BUF];
    loop {
        match socket.recv_from(&mut buf).await {
            Ok((n, peer)) => {
                let router = rx.borrow().clone();
                let ip = peer.ip().to_string();
                let text = String::from_utf8_lossy(&buf[..n]);
                for line in text.split('\n') {
                    handle_line(line, &ip, &router);
                }
            }
            Err(e) => {
                warn!("syslog: udp recv error: {e}");
                tokio::time::sleep(ERR_BACKOFF).await;
            }
        }
    }
}

/// TCP accept loop — one task per connection.
pub async fn run_tcp(listener: TcpListener, rx: watch::Receiver<Arc<SyslogRouter>>) {
    loop {
        match listener.accept().await {
            Ok((stream, peer)) => {
                tokio::spawn(handle_conn(stream, peer, rx.clone()));
            }
            Err(e) => {
                warn!("syslog: tcp accept error: {e}");
                tokio::time::sleep(ERR_BACKOFF).await;
            }
        }
    }
}

async fn handle_conn(stream: TcpStream, peer: SocketAddr, rx: watch::Receiver<Arc<SyslogRouter>>) {
    let ip = peer.ip().to_string();
    let mut framer = FrameReader::new(stream);
    loop {
        match framer.next_frame().await {
            Ok(Some(line)) => {
                let router = rx.borrow().clone();
                handle_line(&line, &ip, &router);
            }
            Ok(None) => break, // clean EOF
            Err(e) => {
                debug!(%ip, "syslog: tcp read error: {e}");
                break;
            }
        }
    }
}

/// RFC 6587 framer over a TCP stream: octet-counting when a message begins with a
/// digit (`MSGLEN SP MSG`), otherwise newline-delimited. Buffer growth is bounded
/// by `MAX_LINE`; a malformed frame resynchronises to the next newline.
struct FrameReader {
    stream: TcpStream,
    frames: FrameBuf,
}

impl FrameReader {
    fn new(stream: TcpStream) -> Self {
        Self {
            stream,
            frames: FrameBuf::default(),
        }
    }

    async fn next_frame(&mut self) -> std::io::Result<Option<String>> {
        loop {
            if let Some(frame) = self.frames.try_extract() {
                return Ok(Some(frame));
            }
            if self.frames.eof {
                return Ok(None);
            }
            // Read another chunk, compacting consumed bytes first.
            self.frames.compact();
            let mut tmp = [0u8; READ_CHUNK];
            let n = self.stream.read(&mut tmp).await?;
            if n == 0 {
                return Ok(self.frames.finish());
            }
            self.frames.push(&tmp[..n]);
        }
    }
}

/// The pure (stream-free) framing buffer — split out so the RFC 6587 logic is
/// unit-testable without a socket.
#[derive(Default)]
struct FrameBuf {
    buf: Vec<u8>,
    pos: usize,
    eof: bool,
}

impl FrameBuf {
    fn push(&mut self, bytes: &[u8]) {
        self.buf.extend_from_slice(bytes);
    }

    fn compact(&mut self) {
        if self.pos > 0 {
            self.buf.drain(..self.pos);
            self.pos = 0;
        }
    }

    /// Mark EOF and return any trailing partial as a best-effort final frame.
    fn finish(&mut self) -> Option<String> {
        self.eof = true;
        if self.pos < self.buf.len() {
            let rest = String::from_utf8_lossy(&self.buf[self.pos..]).into_owned();
            self.pos = self.buf.len();
            Some(rest)
        } else {
            None
        }
    }

    /// Try to pull one complete frame from the buffer. `None` means "need more bytes".
    fn try_extract(&mut self) -> Option<String> {
        let avail = self.buf.len() - self.pos;
        if avail == 0 {
            return None;
        }
        if self.buf[self.pos].is_ascii_digit() {
            self.try_octet_counted(avail)
        } else {
            self.try_newline(avail)
        }
    }

    fn try_octet_counted(&mut self, avail: usize) -> Option<String> {
        let space = self.buf[self.pos..].iter().position(|&b| b == b' ');
        let Some(sp) = space else {
            // No space yet; if the length token is already implausibly long, resync.
            return (avail > 12).then(|| self.drop_to_newline());
        };
        if sp > 12 {
            return Some(self.drop_to_newline());
        }
        let len = std::str::from_utf8(&self.buf[self.pos..self.pos + sp])
            .ok()
            .and_then(|s| s.trim().parse::<usize>().ok());
        match len {
            Some(len) if (1..=MAX_LINE).contains(&len) => {
                let start = self.pos + sp + 1;
                if self.buf.len() >= start + len {
                    let msg = String::from_utf8_lossy(&self.buf[start..start + len]).into_owned();
                    self.pos = start + len;
                    Some(msg)
                } else {
                    None // need more bytes for the full message
                }
            }
            _ => Some(self.drop_to_newline()),
        }
    }

    fn try_newline(&mut self, avail: usize) -> Option<String> {
        match self.buf[self.pos..].iter().position(|&b| b == b'\n') {
            Some(nl) => {
                let end = self.pos + nl;
                let trim = if end > self.pos && self.buf[end - 1] == b'\r' {
                    end - 1
                } else {
                    end
                };
                let line = String::from_utf8_lossy(&self.buf[self.pos..trim]).into_owned();
                self.pos = end + 1;
                Some(line)
            }
            // Over-long line with no newline in sight: drop it to bound memory.
            None => (avail > MAX_LINE).then(|| self.drop_to_newline()),
        }
    }

    /// Discard bytes up to and including the next newline (or all buffered if none).
    /// Returns an empty string so `handle_line` skips it; the loop continues.
    fn drop_to_newline(&mut self) -> String {
        match self.buf[self.pos..].iter().position(|&b| b == b'\n') {
            Some(nl) => self.pos += nl + 1,
            None => self.pos = self.buf.len(),
        }
        String::new()
    }
}

#[cfg(test)]
mod tests {
    use super::FrameBuf;

    /// Pull every currently-extractable frame, ignoring the empty resync sentinels.
    fn drain(fb: &mut FrameBuf) -> Vec<String> {
        let mut out = Vec::new();
        while let Some(f) = fb.try_extract() {
            if !f.is_empty() {
                out.push(f);
            }
        }
        out
    }

    #[test]
    fn newline_framing_splits_lines() {
        let mut fb = FrameBuf::default();
        fb.push(b"<34>line one\n<13>line two\n");
        assert_eq!(drain(&mut fb), vec!["<34>line one", "<13>line two"]);
    }

    #[test]
    fn newline_strips_cr_and_waits_for_partial() {
        let mut fb = FrameBuf::default();
        fb.push(b"<34>complete\r\n<34>partial");
        assert_eq!(drain(&mut fb), vec!["<34>complete"]);
        fb.push(b" now\n");
        assert_eq!(drain(&mut fb), vec!["<34>partial now"]);
    }

    #[test]
    fn octet_counted_framing() {
        let mut fb = FrameBuf::default();
        fb.push(b"11 hello world"); // 11 = len("hello world")
        assert_eq!(drain(&mut fb), vec!["hello world"]);
    }

    #[test]
    fn octet_counted_waits_for_full_message() {
        let mut fb = FrameBuf::default();
        fb.push(b"11 hello");
        assert!(drain(&mut fb).is_empty());
        fb.push(b" world");
        assert_eq!(drain(&mut fb), vec!["hello world"]);
    }

    #[test]
    fn back_to_back_octet_counted_frames() {
        let mut fb = FrameBuf::default();
        fb.push(b"5 aaaaa3 bbb");
        assert_eq!(drain(&mut fb), vec!["aaaaa", "bbb"]);
    }

    #[test]
    fn finish_flushes_trailing_partial() {
        let mut fb = FrameBuf::default();
        fb.push(b"<34>no newline at eof");
        assert!(drain(&mut fb).is_empty());
        assert_eq!(fb.finish().as_deref(), Some("<34>no newline at eof"));
    }

    #[test]
    fn malformed_octet_count_resyncs_to_newline() {
        let mut fb = FrameBuf::default();
        fb.push(b"99x garbage\n<34>good\n");
        assert_eq!(drain(&mut fb), vec!["<34>good"]);
    }
}
