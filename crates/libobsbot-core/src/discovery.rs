// SPDX-License-Identifier: GPL-3.0-only
//! Device enumeration and hot-plug.

use std::sync::Arc;

use crate::status::{Event, EventReceiver, EventSender};
use crate::types::ProductType;
use crate::{Device, Result};

/// Description of a connected OBSBOT camera that has not been opened yet.
#[derive(Debug, Clone)]
pub struct DeviceInfo {
    /// USB vendor id.
    pub vendor_id: u16,
    /// USB product id.
    pub product_id: u16,
    /// Camera model.
    pub product_type: ProductType,
    /// USB product string, when available.
    pub product_name: String,
    /// USB manufacturer string, when available.
    pub manufacturer: String,
    /// Device serial number, when the OS exposes it. Empty when unknown
    /// (the Meet 2 sets `iSerial = 0` in its USB descriptor, so no serial
    /// is available before opening; the camera reports one at runtime).
    pub serial: String,

    #[cfg(target_os = "linux")]
    pub(crate) busnum: u8,
    #[cfg(target_os = "linux")]
    pub(crate) devnum: u8,

    /// `IOKit` registry id of the matched USB device on macOS - used
    /// to re-open the same device even if enumeration order changed.
    #[cfg(target_os = "macos")]
    pub(crate) registry_id: u64,
}

impl DeviceInfo {
    /// ID for selecting this physical USB camera across process invocations.
    /// It remains valid while connected; reconnecting can change the ID.
    #[must_use]
    pub fn id(&self) -> String {
        #[cfg(target_os = "macos")]
        {
            format!("usb-{:x}", self.registry_id)
        }
        #[cfg(target_os = "linux")]
        {
            format!("usb-{}-{}", self.busnum, self.devnum)
        }
        #[cfg(not(any(target_os = "linux", target_os = "macos")))]
        {
            format!(
                "usb-{:04x}-{:04x}-{}",
                self.vendor_id, self.product_id, self.serial
            )
        }
    }
}

/// Owns the hot-plug watcher and the registry of connected cameras.
///
/// Construct with [`Devices::new`]. The watcher thread is spawned on
/// `Devices::new` and stops when the struct is dropped.
pub struct Devices {
    events_tx: EventSender,
    events_rx: EventReceiver,
    /// Set by `Drop` to signal the watcher thread to exit.
    stop: Arc<std::sync::atomic::AtomicBool>,
    watcher: Option<std::thread::JoinHandle<()>>,
}

impl Devices {
    /// Start the hot-plug watcher and return a handle.
    pub fn new() -> Result<Self> {
        let (tx, rx) = crossbeam_channel::unbounded::<Event>();
        let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let watcher = spawn_hotplug_thread(tx.clone(), stop.clone());
        Ok(Self {
            events_tx: tx,
            events_rx: rx,
            stop,
            watcher: Some(watcher),
        })
    }

    /// Snapshot of currently-connected OBSBOT cameras.
    #[must_use]
    pub fn list(&self) -> Vec<DeviceInfo> {
        enumerate()
    }

    /// Select a connected camera by [`DeviceInfo::id`], or select the only
    /// connected supported camera when `id` is `None`.
    pub fn select(&self, id: Option<&str>) -> Result<DeviceInfo> {
        select_device(&self.list(), id).cloned()
    }

    /// Find a connected camera by serial number.
    #[must_use]
    pub fn by_serial(&self, sn: &str) -> Option<DeviceInfo> {
        self.list().into_iter().find(|d| d.serial == sn)
    }

    /// Open a connected camera for control.
    pub fn open(&self, info: &DeviceInfo) -> Result<Device> {
        let transport: Arc<dyn crate::transport::Transport> = open_transport(info)?;
        let mac = match info.product_type {
            ProductType::Meet2 => crate::device::learn_mac(transport.as_ref())?,
            // Tiny 3 frames do not contain the Meet 2 MAC address.
            ProductType::Tiny3 => [0; 6],
        };
        Ok(Device::new(
            info.clone(),
            transport,
            Some(self.events_tx.clone()),
            mac,
        ))
    }

    /// Subscribe to device add/remove events. Each call returns a clone
    /// of the receiver; events are broadcast to every clone.
    #[must_use]
    pub fn events(&self) -> EventReceiver {
        self.events_rx.clone()
    }
}

impl Drop for Devices {
    fn drop(&mut self) {
        self.stop.store(true, std::sync::atomic::Ordering::Relaxed);
        if let Some(handle) = self.watcher.take() {
            // The thread checks `stop` between sleeps; give it one
            // poll interval to wake and exit cleanly.
            let _ = handle.join();
        }
    }
}

/// Live enumeration of OBSBOT cameras, regardless of platform.
fn enumerate() -> Vec<DeviceInfo> {
    #[cfg(target_os = "linux")]
    let devices = linux::enumerate();
    #[cfg(target_os = "macos")]
    let devices = crate::transport::macos::enumerate();
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    let devices = Vec::new();
    deduplicate(devices)
}

fn deduplicate(mut devices: Vec<DeviceInfo>) -> Vec<DeviceInfo> {
    devices.sort_by_cached_key(DeviceInfo::id);
    devices.dedup_by_key(|d| d.id());
    devices
}

fn select_device<'a>(devices: &'a [DeviceInfo], id: Option<&str>) -> Result<&'a DeviceInfo> {
    if let Some(id) = id {
        return devices.iter().find(|d| d.id() == id).ok_or_else(|| {
            crate::Error::DeviceSelection(format!("no supported camera with device ID {id:?}"))
        });
    }
    match devices {
        [] => Err(crate::Error::DeviceSelection(
            "no supported OBSBOT camera connected".into(),
        )),
        [device] => Ok(device),
        _ => Err(crate::Error::DeviceSelection(
            "multiple supported cameras connected; select a device ID explicitly".into(),
        )),
    }
}

/// Open a Transport for the given device on the current platform.
fn open_transport(info: &DeviceInfo) -> Result<Arc<dyn crate::transport::Transport>> {
    #[cfg(target_os = "linux")]
    {
        Ok(Arc::new(crate::transport::usb::UsbTransport::open(info)?))
    }
    #[cfg(target_os = "macos")]
    {
        Ok(Arc::new(crate::transport::macos::MacosTransport::open(
            info,
        )?))
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        let _ = info;
        Err(crate::Error::Unsupported(
            "open: only Linux and macOS transports are implemented; Windows is planned",
        ))
    }
}

/// How often the hot-plug thread polls the host for device changes.
const HOTPLUG_POLL_INTERVAL: std::time::Duration = std::time::Duration::from_secs(2);

fn spawn_hotplug_thread(
    tx: crossbeam_channel::Sender<Event>,
    stop: Arc<std::sync::atomic::AtomicBool>,
) -> std::thread::JoinHandle<()> {
    std::thread::Builder::new()
        .name("libobsbot-hotplug".into())
        .spawn(move || hotplug_loop(&tx, &stop))
        .expect("spawn hotplug thread")
}

/// Diff-based hot-plug detector. Emits one `DeviceAdded` event per
/// device present at startup, then a `DeviceAdded` or `DeviceRemoved`
/// every time a poll-interval comparison sees the set change.
fn hotplug_loop(tx: &crossbeam_channel::Sender<Event>, stop: &std::sync::atomic::AtomicBool) {
    use std::collections::HashMap;
    let mut known: HashMap<String, DeviceInfo> = HashMap::new();
    while !stop.load(std::sync::atomic::Ordering::Relaxed) {
        let current: HashMap<String, DeviceInfo> =
            enumerate().into_iter().map(|d| (d.id(), d)).collect();
        // Additions.
        for (k, info) in &current {
            if !known.contains_key(k)
                && tx
                    .send(Event::DeviceAdded {
                        serial: info.serial.clone(),
                    })
                    .is_err()
            {
                return;
            }
        }
        // Removals.
        for (k, info) in &known {
            if !current.contains_key(k)
                && tx
                    .send(Event::DeviceRemoved {
                        serial: info.serial.clone(),
                    })
                    .is_err()
            {
                return;
            }
        }
        known = current;
        // Sleep in small chunks so Drop can interrupt us promptly.
        let mut slept = std::time::Duration::ZERO;
        while slept < HOTPLUG_POLL_INTERVAL {
            if stop.load(std::sync::atomic::Ordering::Relaxed) {
                return;
            }
            let chunk = std::time::Duration::from_millis(100);
            std::thread::sleep(chunk);
            slept += chunk;
        }
    }
}

#[cfg(target_os = "linux")]
mod linux {
    use super::DeviceInfo;
    use crate::types::ProductType;
    use std::collections::HashMap;

    /// Walk `/sys/class/video4linux/` and return one entry per OBSBOT USB
    /// device. Multiple v4l2 nodes for the same camera are collapsed by
    /// `(busnum, devnum)`.
    pub(super) fn enumerate() -> Vec<DeviceInfo> {
        let Ok(dir) = std::fs::read_dir("/sys/class/video4linux") else {
            return Vec::new();
        };
        let mut by_dev: HashMap<(u8, u8), DeviceInfo> = HashMap::new();
        for entry in dir.flatten() {
            let device_link = entry.path().join("device");
            let Ok(iface_dir) = std::fs::canonicalize(&device_link) else {
                continue;
            };
            let Some(dev_dir) = iface_dir.parent() else {
                continue;
            };
            let (Ok(busnum), Ok(devnum), Ok(vendor_id), Ok(product_id)) = (
                read_u8(&dev_dir.join("busnum")),
                read_u8(&dev_dir.join("devnum")),
                read_u16_hex(&dev_dir.join("idVendor")),
                read_u16_hex(&dev_dir.join("idProduct")),
            ) else {
                continue;
            };
            let Some(product_type) = ProductType::from_usb_ids(vendor_id, product_id) else {
                continue;
            };
            let serial = std::fs::read_to_string(dev_dir.join("serial"))
                .unwrap_or_default()
                .trim()
                .to_owned();
            by_dev.entry((busnum, devnum)).or_insert(DeviceInfo {
                vendor_id,
                product_id,
                product_type,
                product_name: read_string(&dev_dir.join("product")),
                manufacturer: read_string(&dev_dir.join("manufacturer")),
                serial,
                busnum,
                devnum,
            });
        }
        by_dev.into_values().collect()
    }

    fn read_string(path: &std::path::Path) -> String {
        std::fs::read_to_string(path)
            .unwrap_or_default()
            .trim()
            .to_owned()
    }

    fn read_u8(path: &std::path::Path) -> crate::Result<u8> {
        let s = std::fs::read_to_string(path)
            .map_err(|e| crate::Error::Usb(format!("read {}: {e}", path.display())))?;
        s.trim()
            .parse()
            .map_err(|_| crate::Error::Usb(format!("parse u8 from {}", path.display())))
    }

    fn read_u16_hex(path: &std::path::Path) -> crate::Result<u16> {
        let s = std::fs::read_to_string(path)
            .map_err(|e| crate::Error::Usb(format!("read {}: {e}", path.display())))?;
        u16::from_str_radix(s.trim(), 16)
            .map_err(|_| crate::Error::Usb(format!("parse u16 hex from {}", path.display())))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn camera(index: u8) -> DeviceInfo {
        let mut info = crate::testing::meet2_mock_info();
        info.serial.clear();
        #[cfg(target_os = "macos")]
        {
            info.registry_id = u64::from(index);
        }
        #[cfg(target_os = "linux")]
        {
            info.devnum = index;
        }
        #[cfg(not(any(target_os = "linux", target_os = "macos")))]
        {
            info.serial = index.to_string();
        }
        info
    }

    #[test]
    fn selection_requires_one_camera_or_a_matching_id() {
        assert!(select_device(&[], None)
            .unwrap_err()
            .to_string()
            .contains("no supported"));
        let first = camera(1);
        let second = camera(2);
        assert_ne!(first.id(), second.id());
        let cameras = [first.clone()];
        assert_eq!(select_device(&cameras, None).unwrap().id(), first.id());
        let cameras = [first, second];
        assert!(select_device(&cameras, None)
            .unwrap_err()
            .to_string()
            .contains("multiple"));
        assert_eq!(
            select_device(&cameras, Some(&cameras[1].id()))
                .unwrap()
                .id(),
            cameras[1].id()
        );
        assert!(select_device(&cameras, Some("unknown"))
            .unwrap_err()
            .to_string()
            .contains("unknown"));
    }

    #[test]
    fn duplicate_interfaces_collapse_but_same_model_cameras_do_not() {
        let first = camera(1);
        let second = camera(2);
        let devices = deduplicate(vec![second.clone(), first.clone(), first.clone()]);
        assert_eq!(devices.len(), 2);
        assert!(devices.iter().any(|d| d.id() == first.id()));
        assert!(devices.iter().any(|d| d.id() == second.id()));
    }

    #[test]
    fn list_does_not_panic_without_hardware() {
        let d = Devices::new().expect("ctor");
        let _ = d.list();
    }

    #[test]
    fn by_serial_returns_none_when_empty() {
        let d = Devices::new().expect("ctor");
        assert!(d.by_serial("nonexistent").is_none());
    }

    #[test]
    fn events_channel_open() {
        let d = Devices::new().expect("ctor");
        let rx = d.events();
        // Drain whatever's already there; channel just needs to be alive.
        while rx.try_recv().is_ok() {}
        assert!(!rx.is_full(), "unbounded channel should never be full");
    }
}
