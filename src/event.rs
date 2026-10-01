//! Relay events: the plaintext GATT traffic we observe, plus fan-out to the
//! console log and to any connected TCP stream clients.
//!
//! One [`tokio::sync::broadcast`] channel carries every [`RelayEvent`]. The
//! console logger and each TCP client are independent subscribers, so nothing
//! blocks the relay hot path.

use std::time::{SystemTime, UNIX_EPOCH};

use bluer::Uuid;
use serde::Serialize;
use tokio::io::AsyncWriteExt;
use tokio::net::TcpListener;
use tokio::sync::broadcast;

use crate::names;

/// Direction of a relayed message.
#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Direction {
    /// Victim central (phone) → real peripheral.
    CentralToDevice,
    /// Real peripheral → victim central (phone).
    DeviceToCentral,
    /// Tool-generated status/meta line (no payload direction).
    Meta,
}

impl Direction {
    fn arrow(self) -> &'static str {
        match self {
            Direction::CentralToDevice => "central → device",
            Direction::DeviceToCentral => "device → central",
            Direction::Meta => "meta",
        }
    }
}

/// The kind of GATT operation observed.
#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Op {
    Read,
    Write,
    Notify,
    Descriptor,
    Info,
}

/// A single observed event, serialisable to JSON for the TCP stream.
#[derive(Debug, Clone, Serialize)]
pub struct RelayEvent {
    /// Milliseconds since the Unix epoch.
    pub ts_ms: u128,
    pub op: Op,
    pub direction: Direction,
    /// Service UUID, when applicable.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub service: Option<String>,
    /// Characteristic (or descriptor) UUID, when applicable.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub characteristic: Option<String>,
    /// Friendly name if the UUID is well known.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Human-readable note (used for `Op::Info`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    /// Payload length in bytes.
    pub len: usize,
    /// Payload as lowercase hex.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub hex: String,
}

fn now_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0)
}

impl RelayEvent {
    /// Build a data-carrying event for a characteristic operation.
    pub fn data(op: Op, direction: Direction, service: &Uuid, ch: &Uuid, data: &[u8]) -> Self {
        RelayEvent {
            ts_ms: now_ms(),
            op,
            direction,
            service: Some(service.to_string()),
            characteristic: Some(ch.to_string()),
            name: names::label(ch).map(str::to_owned),
            note: None,
            len: data.len(),
            hex: to_hex(data),
        }
    }

    /// Build an informational meta event.
    pub fn info(note: impl Into<String>) -> Self {
        RelayEvent {
            ts_ms: now_ms(),
            op: Op::Info,
            direction: Direction::Meta,
            service: None,
            characteristic: None,
            name: None,
            note: Some(note.into()),
            len: 0,
            hex: String::new(),
        }
    }
}

/// Cheap clonable handle used to publish events from the relay closures.
#[derive(Clone)]
pub struct EventBus {
    tx: broadcast::Sender<RelayEvent>,
}

impl EventBus {
    pub fn new(capacity: usize) -> Self {
        let (tx, _rx) = broadcast::channel(capacity);
        EventBus { tx }
    }

    pub fn subscribe(&self) -> broadcast::Receiver<RelayEvent> {
        self.tx.subscribe()
    }

    /// Publish an event. Errors only when there are no subscribers, which is fine.
    pub fn emit(&self, ev: RelayEvent) {
        let _ = self.tx.send(ev);
    }
}

/// Lowercase hex string, no separators (compact, for JSON).
fn to_hex(data: &[u8]) -> String {
    let mut s = String::with_capacity(data.len() * 2);
    for b in data {
        s.push(char::from_digit((b >> 4) as u32, 16).unwrap());
        s.push(char::from_digit((b & 0xf) as u32, 16).unwrap());
    }
    s
}

/// Canonical `hexdump -C` style rendering for the console.
pub fn hexdump(data: &[u8]) -> String {
    if data.is_empty() {
        return "  (empty)".to_string();
    }
    let mut out = String::new();
    for (i, chunk) in data.chunks(16).enumerate() {
        out.push_str(&format!("  {:04x}  ", i * 16));
        for (j, b) in chunk.iter().enumerate() {
            out.push_str(&format!("{b:02x} "));
            if j == 7 {
                out.push(' ');
            }
        }
        // pad short final line so the ASCII column lines up.
        for j in chunk.len()..16 {
            out.push_str("   ");
            if j == 7 {
                out.push(' ');
            }
        }
        out.push_str(" |");
        for b in chunk {
            let c = *b;
            out.push(if (0x20..0x7f).contains(&c) { c as char } else { '.' });
        }
        out.push_str("|\n");
    }
    out.pop(); // trailing newline
    out
}

/// Console logger: subscribe to the bus and pretty-print each event.
pub fn spawn_console_logger(bus: &EventBus) {
    let mut rx = bus.subscribe();
    tokio::spawn(async move {
        loop {
            match rx.recv().await {
                Ok(ev) => print_event(&ev),
                Err(broadcast::error::RecvError::Lagged(n)) => {
                    tracing::warn!("console logger lagged, dropped {n} events");
                }
                Err(broadcast::error::RecvError::Closed) => break,
            }
        }
    });
}

fn print_event(ev: &RelayEvent) {
    match ev.op {
        Op::Info => tracing::info!("{}", ev.note.as_deref().unwrap_or("")),
        _ => {
            let label = ev
                .name
                .clone()
                .or_else(|| ev.characteristic.clone())
                .unwrap_or_default();
            let bytes = hex_to_bytes(&ev.hex);
            tracing::info!(
                "{:<16} {:>16} {} ({} bytes)\n{}",
                op_tag(ev.op),
                ev.direction.arrow(),
                label,
                ev.len,
                hexdump(&bytes),
            );
        }
    }
}

fn op_tag(op: Op) -> &'static str {
    match op {
        Op::Read => "READ",
        Op::Write => "WRITE",
        Op::Notify => "NOTIFY",
        Op::Descriptor => "DESCRIPTOR",
        Op::Info => "INFO",
    }
}

fn hex_to_bytes(s: &str) -> Vec<u8> {
    (0..s.len() / 2)
        .filter_map(|i| u8::from_str_radix(&s[i * 2..i * 2 + 2], 16).ok())
        .collect()
}

/// Start a TCP server that streams newline-delimited JSON events to each client.
///
/// Runs until the listener is dropped / task is aborted. Every accepted client
/// gets its own broadcast subscription; a slow client that lags is disconnected
/// rather than being allowed to stall the relay.
pub async fn serve_stream(bind: &str, port: u16, bus: EventBus) -> anyhow::Result<()> {
    let listener = TcpListener::bind((bind, port)).await?;
    tracing::info!("event stream listening on {bind}:{port} (newline-delimited JSON)");

    loop {
        let (mut sock, peer) = match listener.accept().await {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!("stream accept failed: {e}");
                continue;
            }
        };
        tracing::info!("stream client connected: {peer}");
        let mut rx = bus.subscribe();
        tokio::spawn(async move {
            loop {
                match rx.recv().await {
                    Ok(ev) => {
                        let Ok(mut line) = serde_json::to_vec(&ev) else { continue };
                        line.push(b'\n');
                        if sock.write_all(&line).await.is_err() {
                            break;
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }
            tracing::info!("stream client disconnected: {peer}");
        });
    }
}
