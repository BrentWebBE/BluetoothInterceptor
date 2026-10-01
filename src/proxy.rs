//! The GATT proxy: connect to the real peripheral as a central, mirror its
//! service table onto the clone adapter as a peripheral, and wire every local
//! characteristic to forward to the real one while logging the plaintext.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use bluer::adv::{Advertisement, AdvertisementHandle};
use bluer::gatt::local::{
    Application, ApplicationHandle, Characteristic, CharacteristicNotify, CharacteristicNotifyMethod,
    CharacteristicNotifier, CharacteristicRead, CharacteristicWrite, CharacteristicWriteMethod,
    Descriptor, DescriptorRead, ReqError, Service,
};
use bluer::gatt::remote::Characteristic as RemoteChar;
use bluer::{Adapter, Address, Device, Uuid};
use futures::{FutureExt, StreamExt};

use crate::event::{Direction, EventBus, Op, RelayEvent};
use crate::names;

/// A running proxy. Dropping it stops advertising and unregisters the clone.
pub struct Proxy {
    // Kept alive so the real connection, the served application, and the
    // advertisement all persist for the lifetime of the proxy.
    _device: Device,
    _app: ApplicationHandle,
    _adv: AdvertisementHandle,
}

/// Connect to the target peripheral and wait for its GATT table to resolve.
pub async fn connect_target(adapter: &Adapter, addr: Address) -> Result<Device> {
    let device = adapter.device(addr)?;

    if !device.is_connected().await? {
        tracing::info!("connecting to target {addr}…");
        device
            .connect()
            .await
            .with_context(|| format!("connect to {addr}"))?;
    }

    // GATT services resolve asynchronously after connection.
    for _ in 0..50 {
        if device.is_services_resolved().await.unwrap_or(false) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }

    Ok(device)
}

/// Build the clone application from `device`'s GATT table and serve + advertise
/// it on `clone_adapter`.
pub async fn build_clone(
    device: &Device,
    clone_adapter: &Adapter,
    bus: EventBus,
    clone_descriptors: bool,
) -> Result<Proxy> {
    let mut services = Vec::new();

    for rsvc in device.services().await.context("read target services")? {
        let su = rsvc.uuid().await?;

        // BlueZ reserves the GAP (0x1800) and GATT (0x1801) services and serves
        // them itself; trying to register our own copies makes the whole
        // application registration fail with "Failed to create entry in
        // database". Skip them — the local stack provides them automatically.
        if matches!(names::sig_id(&su), Some(0x1800 | 0x1801)) {
            bus.emit(RelayEvent::info(format!(
                "skipping reserved service {} (provided by BlueZ)",
                names::describe(&su)
            )));
            continue;
        }

        let primary = rsvc.primary().await.unwrap_or(true);
        bus.emit(RelayEvent::info(format!("cloning service {}", names::describe(&su))));

        let mut chars = Vec::new();
        for rchar in rsvc.characteristics().await? {
            let cu = rchar.uuid().await?;
            let flags = rchar.flags().await?;
            let remote = Arc::new(rchar);
            let mut c = Characteristic {
                uuid: cu,
                ..Default::default()
            };

            // ── Read: central reads → forward to real device ──────────────
            if flags.read {
                let remote = Arc::clone(&remote);
                let bus = bus.clone();
                c.read = Some(CharacteristicRead {
                    read: true,
                    fun: Box::new(move |_req| {
                        let remote = Arc::clone(&remote);
                        let bus = bus.clone();
                        async move {
                            match remote.read().await {
                                Ok(val) => {
                                    bus.emit(RelayEvent::data(
                                        Op::Read,
                                        Direction::DeviceToCentral,
                                        &su,
                                        &cu,
                                        &val,
                                    ));
                                    Ok(val)
                                }
                                Err(e) => {
                                    bus.emit(RelayEvent::info(format!("read {cu} failed: {e}")));
                                    Err(ReqError::Failed)
                                }
                            }
                        }
                        .boxed()
                    }),
                    ..Default::default()
                });
            }

            // ── Write: central writes → forward to real device ────────────
            if flags.write || flags.write_without_response {
                let remote = Arc::clone(&remote);
                let bus = bus.clone();
                c.write = Some(CharacteristicWrite {
                    write: flags.write,
                    write_without_response: flags.write_without_response,
                    method: CharacteristicWriteMethod::Fun(Box::new(move |new_value, _req| {
                        let remote = Arc::clone(&remote);
                        let bus = bus.clone();
                        async move {
                            bus.emit(RelayEvent::data(
                                Op::Write,
                                Direction::CentralToDevice,
                                &su,
                                &cu,
                                &new_value,
                            ));
                            remote.write(&new_value).await.map_err(|_| ReqError::Failed)?;
                            Ok(())
                        }
                        .boxed()
                    })),
                    ..Default::default()
                });
            }

            // ── Notify/Indicate: subscribe upstream lazily when the central does
            if flags.notify || flags.indicate {
                let remote = Arc::clone(&remote);
                let bus = bus.clone();
                c.notify = Some(CharacteristicNotify {
                    notify: flags.notify,
                    indicate: flags.indicate,
                    method: CharacteristicNotifyMethod::Fun(Box::new(move |notifier| {
                        let remote = Arc::clone(&remote);
                        let bus = bus.clone();
                        async move {
                            tokio::spawn(pump_notify(remote, notifier, bus, su, cu));
                        }
                        .boxed()
                    })),
                    ..Default::default()
                });
            }

            // ── Descriptors (optional, read forwarding only) ──────────────
            if clone_descriptors {
                for rdesc in remote.descriptors().await.unwrap_or_default() {
                    let du = rdesc.uuid().await?;
                    let rdesc = Arc::new(rdesc);
                    let bus = bus.clone();
                    c.descriptors.push(Descriptor {
                        uuid: du,
                        read: Some(DescriptorRead {
                            read: true,
                            fun: Box::new(move |_req| {
                                let rdesc = Arc::clone(&rdesc);
                                let bus = bus.clone();
                                async move {
                                    match rdesc.read().await {
                                        Ok(val) => {
                                            bus.emit(RelayEvent::data(
                                                Op::Descriptor,
                                                Direction::DeviceToCentral,
                                                &su,
                                                &du,
                                                &val,
                                            ));
                                            Ok(val)
                                        }
                                        Err(_) => Err(ReqError::Failed),
                                    }
                                }
                                .boxed()
                            }),
                            ..Default::default()
                        }),
                        ..Default::default()
                    });
                }
            }

            bus.emit(RelayEvent::info(format!(
                "  ├─ characteristic {}  (read:{} write:{} notify:{})",
                names::describe(&cu),
                flags.read,
                flags.write || flags.write_without_response,
                flags.notify || flags.indicate,
            )));
            chars.push(c);
        }

        services.push(Service {
            uuid: su,
            primary,
            characteristics: chars,
            ..Default::default()
        });
    }

    let app = Application {
        services,
        ..Default::default()
    };
    let app_handle = clone_adapter
        .serve_gatt_application(app)
        .await
        .context("serve cloned GATT application")?;

    // Advertise a clone of the target's advertisement.
    let adv = build_advertisement(device).await?;
    let adv_handle = match clone_adapter.advertise(adv.clone()).await {
        Ok(h) => h,
        Err(e) => {
            tracing::warn!("full advertisement rejected ({e}); retrying with name only");
            let fallback = Advertisement {
                advertisement_type: bluer::adv::Type::Peripheral,
                local_name: adv.local_name.clone(),
                discoverable: Some(true),
                ..Default::default()
            };
            clone_adapter
                .advertise(fallback)
                .await
                .context("advertise clone")?
        }
    };

    Ok(Proxy {
        _device: device.clone(),
        _app: app_handle,
        _adv: adv_handle,
    })
}

/// Forward notifications from the real characteristic to the subscribed central.
async fn pump_notify(
    remote: Arc<RemoteChar>,
    mut notifier: CharacteristicNotifier,
    bus: EventBus,
    su: Uuid,
    cu: Uuid,
) {
    let stream = match remote.notify().await {
        Ok(s) => s,
        Err(e) => {
            bus.emit(RelayEvent::info(format!("subscribe {cu} failed: {e}")));
            return;
        }
    };
    futures::pin_mut!(stream);

    bus.emit(RelayEvent::info(format!(
        "central subscribed to {}",
        names::describe(&cu)
    )));

    while let Some(val) = stream.next().await {
        if notifier.is_stopped() {
            break;
        }
        bus.emit(RelayEvent::data(
            Op::Notify,
            Direction::DeviceToCentral,
            &su,
            &cu,
            &val,
        ));
        if notifier.notify(val).await.is_err() {
            break;
        }
    }
    bus.emit(RelayEvent::info(format!("notify relay for {cu} ended")));
}

/// Clone the target's advertised identity (name, service UUIDs, manufacturer /
/// service data, appearance) into a peripheral advertisement.
async fn build_advertisement(device: &Device) -> Result<Advertisement> {
    let local_name = device.name().await.ok().flatten();
    let service_uuids: BTreeSet<Uuid> = device
        .uuids()
        .await
        .ok()
        .flatten()
        .unwrap_or_default()
        .into_iter()
        .collect();
    let manufacturer_data: BTreeMap<u16, Vec<u8>> = device
        .manufacturer_data()
        .await
        .ok()
        .flatten()
        .unwrap_or_default()
        .into_iter()
        .collect();
    let service_data: BTreeMap<Uuid, Vec<u8>> = device
        .service_data()
        .await
        .ok()
        .flatten()
        .unwrap_or_default()
        .into_iter()
        .collect();
    let appearance = device.appearance().await.ok().flatten();

    tracing::info!(
        "cloning advertisement: name={:?} services={} mfd={} appearance={:?}",
        local_name,
        service_uuids.len(),
        manufacturer_data.len(),
        appearance,
    );

    Ok(Advertisement {
        advertisement_type: bluer::adv::Type::Peripheral,
        local_name,
        service_uuids,
        manufacturer_data,
        service_data,
        appearance,
        discoverable: Some(true),
        ..Default::default()
    })
}
