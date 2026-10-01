//! Command-line interface (clap derive).

use clap::Parser;

/// Educational BLE GATT man-in-the-middle proxy.
///
/// The tool connects to a real BLE peripheral as a *central*, mirrors its GATT
/// service table and advertisement onto a *second* adapter acting as a
/// *peripheral*, and relays every read / write / notification between the victim
/// central (e.g. a phone) and the real device while logging the plaintext.
///
/// FOR AUTHORISED, EDUCATIONAL USE ONLY. Only run against devices you own or
/// have explicit written permission to test.
#[derive(Debug, Parser)]
#[command(name = "blegatt-mitm", version, about, long_about = None)]
pub struct Cli {
    /// Target peripheral address, e.g. `AA:BB:CC:DD:EE:FF`.
    ///
    /// If omitted, the tool scans and lets you pick interactively.
    #[arg(short = 't', long = "target", value_name = "ADDR")]
    pub target: Option<String>,

    /// Adapter used to connect to the REAL device (central role).
    #[arg(long = "target-adapter", value_name = "HCI", default_value = "hci0")]
    pub target_adapter: String,

    /// Adapter used to ADVERTISE the clone the victim connects to (peripheral role).
    ///
    /// A GATT MITM needs to be central and peripheral at the same time, which in
    /// practice means two adapters. Defaults to the same adapter as
    /// `--target-adapter`, but that only works on controllers that allow the
    /// central+peripheral combination; two dongles are strongly recommended.
    #[arg(long = "clone-adapter", value_name = "HCI")]
    pub clone_adapter: Option<String>,

    /// TCP port for the JSON event stream (newline-delimited JSON).
    #[arg(short = 'P', long = "port", value_name = "PORT", default_value_t = 8888)]
    pub port: u16,

    /// Bind address for the TCP event stream.
    #[arg(long = "bind", value_name = "ADDR", default_value = "0.0.0.0")]
    pub bind: String,

    /// Disable the TCP event stream entirely.
    #[arg(long = "no-stream")]
    pub no_stream: bool,

    /// Seconds to scan when discovering devices in interactive mode.
    #[arg(long = "scan-secs", value_name = "SECS", default_value_t = 8)]
    pub scan_secs: u64,

    /// Also forward writes/reads to descriptors (best-effort). Off by default.
    #[arg(long = "clone-descriptors")]
    pub clone_descriptors: bool,

    /// Log level filter (e.g. `info`, `debug`, `blegatt_mitm=debug`).
    ///
    /// Overridden by the `RUST_LOG` environment variable if set.
    #[arg(long = "log", value_name = "FILTER", default_value = "info")]
    pub log: String,
}

impl Cli {
    /// The adapter to advertise the clone on, defaulting to the target adapter.
    pub fn clone_adapter(&self) -> &str {
        self.clone_adapter.as_deref().unwrap_or(&self.target_adapter)
    }
}
