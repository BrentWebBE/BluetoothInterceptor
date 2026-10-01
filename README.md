# btintercept

An educational **Bluetooth Low Energy (BLE) GATT man-in-the-middle proxy**,
written in modern async Rust on top of [`bluer`](https://docs.rs/bluer) (the
official BlueZ D-Bus bindings).

It connects to a real BLE peripheral as a *central*, mirrors that device's GATT
service table and advertisement onto a second adapter acting as a *peripheral*,
and relays every read, write and notification between a victim central (e.g. a
phone) and the real device — logging the plaintext as it passes.

See [`DESIGN.md`](DESIGN.md) for the cited research and the rationale behind the
design (including why this replaced an earlier Classic-Bluetooth A2DP prototype,
which now lives only in this repo's git history).

> ## ⚠️ Authorised, educational use only
> Only run this against devices **you own** or have **explicit written
> permission** to test. Intercepting other people's Bluetooth traffic without
> consent is illegal in most jurisdictions.

---

## What it can and cannot do

This is a **clone-and-relay** attack, the same technique as the classic
[GATTacker](https://github.com/securing/gattacker) / BtleJuice tools. It works
**only against BLE devices that do not use link-layer pairing/encryption**. If
the connection uses LE Secure Connections with bonding, the payloads are
ciphertext and this technique cannot read them — that is the Bluetooth security
model working as designed, not a limitation of this tool. What remains
observable on secured devices is pre-encryption metadata (advertisements).

---

## Requirements

- Linux with **BlueZ 5.x** running (`bluetoothd`).
- **Two Bluetooth adapters strongly recommended**: one to hold the central
  connection to the real device, one to advertise the clone. A GATT MITM has to
  be central and peripheral at the same time, which one controller often will
  not do reliably.
- Rust (edition 2024, so **1.85+**; developed against current stable).
- `libdbus-1-dev` (build dependency of `bluer`).
- Root / `sudo`, or `CAP_NET_ADMIN`, for adapter control.

```bash
# Debian/Ubuntu
sudo apt install bluez libdbus-1-dev build-essential

# Arch
sudo pacman -S bluez bluez-utils dbus
```

If you hit `Failed to create entry in database` when serving the clone, enable
BlueZ experimental GATT features: set `Experimental = true` in
`/etc/bluetooth/main.conf` (or run `bluetoothd --experimental`) and retry.

---

## Build & run

```bash
cargo build --release

# Interactive: scan, then pick a device to clone.
sudo ./target/release/blegatt-mitm \
    --target-adapter hci0 --clone-adapter hci1

# Direct: name the target and adapters explicitly.
sudo ./target/release/blegatt-mitm \
    -t AA:BB:CC:DD:EE:FF --target-adapter hci0 --clone-adapter hci1
```

### Options

| Flag | Default | Description |
|------|---------|-------------|
| `-t, --target <ADDR>` | — | Target peripheral MAC. Omit to scan and pick interactively. |
| `--target-adapter <HCI>` | `hci0` | Adapter that connects to the real device (central). |
| `--clone-adapter <HCI>` | = target | Adapter that advertises the clone (peripheral). |
| `-P, --port <PORT>` | `8888` | TCP port for the JSON event stream. |
| `--bind <ADDR>` | `0.0.0.0` | Bind address for the event stream. |
| `--no-stream` | — | Disable the TCP event stream. |
| `--scan-secs <SECS>` | `8` | Discovery duration in interactive mode. |
| `--clone-descriptors` | off | Also mirror characteristic descriptors (read forwarding). |
| `--log <FILTER>` | `info` | Log filter, e.g. `debug` or `blegatt_mitm=debug`. Overridden by `RUST_LOG`. |

---

## Live event stream

Every observed operation is fanned out to the console (as a hex dump) and, unless
`--no-stream` is set, to a TCP server as **newline-delimited JSON**:

```bash
nc <host> 8888 | jq .
```

Each line looks like:

```json
{"ts_ms":1788600000000,"op":"write","direction":"central_to_device",
 "service":"0000180f-...","characteristic":"00002a19-...","name":"Battery Level",
 "len":1,"hex":"64"}
```

The `client/` React Native app is an optional monitor for this stream.

---

## How it works

1. **Connect** to the real peripheral as a central (`--target-adapter`) and wait
   for its GATT table to resolve.
2. **Enumerate** every service, characteristic, its flags, and (optionally)
   descriptors. Reserved GAP/GATT services (0x1800/0x1801) are skipped because
   BlueZ provides them itself.
3. **Clone** that table into a local `bluer` GATT application and advertise a
   copy of the device's identity (name, service UUIDs, manufacturer/service
   data, appearance) on `--clone-adapter`.
4. **Relay**: each local characteristic's read/write callback forwards to the
   real characteristic; when the victim subscribes to notifications, the tool
   lazily subscribes upstream and pumps values through.
5. **Log** every message to the console and the JSON stream.

See [`DESIGN.md`](DESIGN.md) for the architecture, the security model, and the
research behind these choices.

## Project layout

```
Cargo.toml           crate manifest
rust-toolchain.toml  pinned toolchain channel
src/
  main.rs            CLI wiring, adapter setup, lifecycle
  cli.rs             clap argument definitions
  scan.rs            BLE discovery + interactive picker
  proxy.rs           connect → clone GATT table → advertise → relay
  event.rs           event model, hex dump, console logger, TCP JSON stream
  names.rs           friendly names for well-known GATT UUIDs
  error.rs           typed domain errors
DESIGN.md            cited research + design rationale
client/              React Native app (optional TCP monitor)
```

## License

MIT.
