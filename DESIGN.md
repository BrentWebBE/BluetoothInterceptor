# Design & research notes

This document records the research behind `blegatt-mitm` and the reasoning for
rebuilding the original `btintercept` prototype. It is written for the
educational value of the project: the *why*, not just the *what*.

Research was gathered on 2026-09-05 via a multi-source, fact-checked pass (24
sources fetched, 106 claims extracted, 25 adversarially verified). Time-sensitive
facts (crate and compiler versions) were current then and will drift — re-check
before pinning.

---

## 1. Why the rewrite changed radios

The original `btintercept` targeted **Bluetooth Classic (BR/EDR) A2DP audio**. It
force-disconnected a target, spoofed the adapter's MAC, cloned the name / Class
-of-Device / SDP records, and relayed raw L2CAP payloads. Two problems made
"works flawlessly" unreachable for that design:

1. **The payloads were never plaintext.** The relay forwarded L2CAP data
   untouched and the crypto module was an explicit no-op stub. Classic BT audio
   is encrypted at the baseband, so a raw relay logs ciphertext, not audio.
2. **It depended on deprecated tooling.** `hcitool`, `sdptool`, `hciconfig` and
   `bdaddr` are deprecated and removed from current BlueZ, so the tool would
   degrade over time regardless of Rust version.

BLE GATT interception is the far more instructive and reliable subject for an
educational MITM, because unencrypted GATT traffic is genuinely observable and
the attack surface (services, characteristics, notifications) is legible.

---

## 2. The clone-and-relay technique

BLE MITM tools such as **GATTacker** (Jasek, Black Hat USA 2016) and
**BtleJuice** work by *cloning* the target peripheral rather than sniffing the
radio:

> "The tool creates exact copy of attacked device in Bluetooth layer, and then
> tricks mobile application to interpret its broadcasts and connect to it instead
> the original device. At the same time, it keeps active connection to the
> device, and forwards to it the data exchanged with mobile application."
> — [GATTacker FAQ](https://github.com/securing/gattacker/wiki/FAQ)

The clone advertises the same identity, faster/stronger than the original, so the
victim central connects to it; the tool stays connected to the real device and
relays. `blegatt-mitm` implements exactly this in `proxy.rs`.

**Handle matching caveat.** For a victim that has *already bonded/cached* the
original device, the clone's GATT attribute **handle numbers must match the
original's**, or the OS GATT cache rejects it
([GATTacker FAQ](https://github.com/securing/gattacker/wiki/FAQ)). A fresh client
rediscovers the table, and BT 5.1+'s Database Hash lets modern clients detect
changes and re-discover. `bluer` assigns handles in registration order; for a
cached victim you may need to reproduce ordering/handles precisely.

---

## 3. The hard security limit

This class of attack **only works against devices without LE link-layer
pairing/encryption**. The GATTacker FAQ states it "works for devices which do
not implement Bluetooth LE link-layer pairing/encryption." Real-world corroboration:
BLE relay research found earlier unencrypted car models vulnerable while models
using LE link-layer pairing/encryption were not
([GATTacker FAQ](https://github.com/securing/gattacker/wiki/FAQ),
[Argenox BLE Security & Privacy 2025](https://argenox.com/blog/bluetooth-low-energy-ble-security-privacy-a-2025-guide)).

With **LE Secure Connections** (ECDH key agreement), encryption and bonding, the
GATT payloads are ciphertext and this technique cannot read them. Address
randomization / LE Privacy further complicates even identifying the target. What
remains observable is pre-encryption metadata: advertisements, and unencrypted
GATT traffic on non-secured devices. The tool documents this honestly rather than
pretending otherwise.

---

## 4. Why `bluer`, and why two adapters

A GATT MITM needs **both** BLE roles at once:

- **Central / GATT client** — discover, read, write, subscribe on the real device.
- **Peripheral / GATT server** — publish a cloned service table and advertise it.

`bluer` (v0.17.4, the official BlueZ Rust interface hosted under the BlueZ GitHub
org) is the only mature Rust option that provides both, plus L2CAP sockets
([docs.rs/bluer](https://docs.rs/bluer/latest/bluer/),
[github.com/bluez/bluer](https://github.com/bluez/bluer),
[crates.io/crates/bluer](https://crates.io/crates/bluer)). By contrast:

- **btleplug** is "meant to be host/central mode only" and points peripheral
  users elsewhere ([btleplug README](https://github.com/deviceplug/btleplug)).
- **bluest** supports "the GAP Central and GATT Client roles. Peripheral and
  Server roles are not supported" ([bluest docs](https://docs.rs/bluest)).
- **gattrs** is "Peripheral only … not central/client connections"
  ([gattrs README](https://github.com/benarmstrongg/gattrs)).

So `bluer` is the correct single foundation on Linux — which matches the choice
made for this rebuild.

**Two adapters.** `bluer`'s docs confirm both roles and advertising exist but do
*not* guarantee concurrent central+peripheral on one controller; that is a
controller-dependent capability and unreliable in practice. `blegatt-mitm`
therefore takes `--target-adapter` and `--clone-adapter` and warns when they are
the same. Full MAC-address cloning of the clone adapter (for bonded victims) is
out of scope here and would require `btmgmt`/kernel address setting; the tool
clones the *advertised* identity and GATT table, which is what makes fresh
clients reconnect.

---

## 5. Concrete `bluer` API used

From the official [`gatt_server_cb.rs`](https://github.com/bluez/bluer/blob/master/bluer/examples/gatt_server_cb.rs)
example and the [`Adapter`](https://docs.rs/bluer/latest/bluer/struct.Adapter.html)
docs:

- `Adapter::discover_devices()` → stream of `AdapterEvent::DeviceAdded(addr)` (scan).
- `Adapter::device(addr)` → `Device`; `device.connect()`, `device.services()`,
  and per-characteristic `read()` / `write()` / `notify()` (central role).
- `Adapter::serve_gatt_application(Application)` → `ApplicationHandle` (server).
- `Adapter::advertise(Advertisement)` → `AdvertisementHandle` (broadcast clone).
- Per-characteristic callbacks are the relay insertion point: `CharacteristicRead`,
  `CharacteristicWriteMethod::Fun`, `CharacteristicNotifyMethod::Fun`. Notifications
  are driven by spawning a task that calls `notifier.notify()` — which is exactly
  how `pump_notify` in `proxy.rs` forwards upstream notifications downstream.

Dropping the returned handles unregisters the server and stops advertising, so
`Proxy` simply holds them for its lifetime.

---

## 6. Toolchain & language

- **Current stable Rust** was 1.98.1 (2026-09-03) at research time
  ([release notes](https://doc.rust-lang.org/stable/releases.html)). The local
  machine had 1.96.0, which is fine.
- **Rust 2024 edition** was stabilized in **1.85.0** (2025-02-20), not a later
  release — a competing claim naming 1.88.0 was explicitly refuted during
  verification ([Rust 1.85.0 announcement](https://blog.rust-lang.org/2025/02/20/Rust-1.85.0/)).
  Edition 2024 requires compiler ≥ 1.85.
- Edition-2024 async ergonomics (`Future`/`IntoFuture` in the prelude, async
  closures) suit an async networking tool. This crate uses edition 2024 and a
  `tokio` multi-threaded runtime.

Modern-Rust choices versus the prototype:

| Prototype (`btintercept`) | This rewrite (`blegatt-mitm`) |
|---|---|
| Manual `libc` `select()` relay loop, raw fds | Async `tokio`, per-notification tasks |
| Shell-outs to `hcitool`/`sdptool`/`hciconfig`/`bdaddr` | Native BlueZ D-Bus via `bluer` |
| `static mut` global link key | No global mutable state |
| Manual `argv` parsing | `clap` derive |
| `eprintln!` macros | `tracing` + `tracing-subscriber` |
| Raw-byte TCP stream | Structured newline-delimited JSON events |
| `String` error strings | `anyhow` + a small typed `AppError` |

---

## 7. Open questions worth exploring next

Carried over from the research as genuine unknowns for anyone extending this:

1. Can `bluer` reliably run central **and** peripheral on a *single* adapter for
   a given controller, or is two-adapter always required?
2. How best to clone a specific device **address** on the clone adapter (BlueZ /
   kernel restrictions) so bonded victims accept the clone?
3. Best `tokio` architecture for relaying *many* characteristics with backpressure
   and ordering — per-characteristic tasks (current approach) vs a central relay
   actor.
4. For encrypted/bonded targets, what lower-layer (HCI/L2CAP) or purpose-built
   relay-hardware approaches exist, and are any reachable from `bluer`.

---

## Sources

Primary and high-quality secondary sources used:

- bluer — [docs.rs](https://docs.rs/bluer/latest/bluer/),
  [Adapter API](https://docs.rs/bluer/latest/bluer/struct.Adapter.html),
  [GitHub](https://github.com/bluez/bluer),
  [crates.io](https://crates.io/crates/bluer),
  [gatt_server_cb.rs example](https://github.com/bluez/bluer/blob/master/bluer/examples/gatt_server_cb.rs),
  [l2cap module](https://docs.rs/bluer/latest/bluer/l2cap/index.html)
- Alternatives — [btleplug](https://github.com/deviceplug/btleplug),
  [bluest](https://docs.rs/bluest), [gattrs](https://github.com/benarmstrongg/gattrs)
- BLE MITM prior art — [GATTacker FAQ](https://github.com/securing/gattacker/wiki/FAQ),
  [GATTacker (SecuRing intro)](https://www.securing.pl/en/gattacking-bluetooth-smart-devices-introducing-a-new-ble-proxy-tool/),
  [Black Hat USA 2016 whitepaper (Jasek)](https://blackhat.com/docs/us-16/materials/us-16-Jasek-GATTacking-Bluetooth-Smart-Devices-Introducing-a-New-BLE-Proxy-Tool-wp.pdf),
  [BtleJuice writeup](https://blog.attify.com/btlejuice-mitm-attack-smart-bulb/)
- BLE security — [Argenox BLE Security & Privacy 2025](https://argenox.com/blog/bluetooth-low-energy-ble-security-privacy-a-2025-guide),
  [Rtone deep dive into BLE security](https://medium.com/rtone-iot-security/deep-dive-into-bluetooth-le-security-d2301d640bfc),
  [Silicon Labs Bluetooth LE security](https://docs.silabs.com/bluetooth/latest/bluetooth-le-fundamentals/08-bluetooth-smart-security)
- Rust — [Rust 1.85.0 & 2024 edition announcement](https://blog.rust-lang.org/2025/02/20/Rust-1.85.0/),
  [Rust release notes](https://doc.rust-lang.org/stable/releases.html),
  [releases.rs 1.85.0](https://releases.rs/docs/1.85.0/)
