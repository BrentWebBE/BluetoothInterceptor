//! BLE discovery and interactive target selection.

use std::collections::BTreeSet;
use std::time::Duration;

use anyhow::{Context, Result};
use bluer::{Adapter, Address, AdapterEvent};
use futures::StreamExt;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::time::timeout;

use crate::error::AppError;

/// A discovered peripheral summary.
struct Found {
    addr: Address,
    name: Option<String>,
    rssi: Option<i16>,
}

/// Scan for `scan_secs` seconds and return every peripheral seen.
async fn discover(adapter: &Adapter, scan_secs: u64) -> Result<Vec<Found>> {
    tracing::info!("scanning for BLE devices for {scan_secs}s on {}…", adapter.name());
    let events = adapter.discover_devices().await.context("start discovery")?;
    futures::pin_mut!(events);

    let mut addrs: BTreeSet<Address> = BTreeSet::new();
    // Drain discovery events until the timeout elapses.
    let _ = timeout(Duration::from_secs(scan_secs), async {
        while let Some(evt) = events.next().await {
            if let AdapterEvent::DeviceAdded(addr) = evt {
                addrs.insert(addr);
            }
        }
    })
    .await;

    let mut out = Vec::with_capacity(addrs.len());
    for addr in addrs {
        let Ok(dev) = adapter.device(addr) else { continue };
        let name = dev.name().await.ok().flatten();
        let rssi = dev.rssi().await.ok().flatten();
        out.push(Found { addr, name, rssi });
    }

    // Strongest signal first; named devices ahead of unnamed at equal signal.
    out.sort_by(|a, b| {
        b.rssi
            .unwrap_or(i16::MIN)
            .cmp(&a.rssi.unwrap_or(i16::MIN))
            .then(b.name.is_some().cmp(&a.name.is_some()))
    });
    Ok(out)
}

/// Scan, present a numbered list, and return the address the user picks.
pub async fn pick_target(adapter: &Adapter, scan_secs: u64) -> Result<Address> {
    let found = discover(adapter, scan_secs).await?;
    if found.is_empty() {
        return Err(AppError::NoTarget.into());
    }

    println!("\n─── Discovered BLE peripherals ───────────────────────────────");
    for (i, f) in found.iter().enumerate() {
        let name = f.name.as_deref().unwrap_or("[unknown]");
        let rssi = f
            .rssi
            .map(|r| format!("{r:>4} dBm"))
            .unwrap_or_else(|| "   ? dBm".to_string());
        println!("  {:>2}  {}  {}  {}", i + 1, f.addr, rssi, name);
    }
    println!("──────────────────────────────────────────────────────────────\n");

    let mut lines = BufReader::new(tokio::io::stdin()).lines();
    loop {
        print!("Select a device to intercept (1-{}), or q to quit: ", found.len());
        use std::io::Write;
        std::io::stdout().flush().ok();

        let Some(line) = lines.next_line().await? else {
            return Err(AppError::Cancelled.into());
        };
        let input = line.trim();
        if input.eq_ignore_ascii_case("q") {
            return Err(AppError::Cancelled.into());
        }
        match input.parse::<usize>() {
            Ok(n) if (1..=found.len()).contains(&n) => return Ok(found[n - 1].addr),
            _ => println!("Invalid — enter a number between 1 and {}.", found.len()),
        }
    }
}
