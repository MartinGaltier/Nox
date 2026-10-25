// Adapted from vorssaint-utils (GPL-3.0-or-later), Sources/Vorssaint/Services/Notch/NotchAccessoryService.swift, commit e916961.

//! Accessories (E1): connected Bluetooth/HID devices with their batteries
//! (AirPods left/right/case), connection and low-battery alerts. In-process
//! reads every few seconds; no external process.

use crate::sys::peripherals::{self, BtDevice, HidBattery};
use nox_core::model::accessories::{
    AccessoriesState, Accessory, AccessoryCommand, AccessoryKind, AccessorySettings, BatteryPart,
};
use nox_core::permissions::PermissionState;
use nox_core::{
    Alert, AlertKind, AlertPriority, CancelToken, Hub, Module, ModuleContext, ModuleError,
    ModuleId, Reading, Settings, Unavailable, Worker,
};
use std::collections::HashSet;
use std::sync::{Arc, Mutex};
use std::time::Duration;

const POLL: Duration = Duration::from_secs(5);
/// A low-battery alert re-arms once the level is this much above threshold.
const RECOVERY_MARGIN: u8 = 5;

pub struct AccessoriesModule {
    worker: Option<Worker>,
    settings: Arc<Mutex<AccessorySettings>>,
    /// Pending Bluetooth access request (user action), dropped on stop.
    bluetooth_request: Option<peripherals::BluetoothRequest>,
}

impl Default for AccessoriesModule {
    fn default() -> Self {
        Self {
            worker: None,
            settings: Arc::new(Mutex::new(AccessorySettings::default())),
            bluetooth_request: None,
        }
    }
}

impl AccessoriesModule {
    /// `AccessoryCommand` handling (`Command::Accessories`). Never blocks: the prompt is asynchronous and
    /// the polling loop picks the answer up on its next pass.
    pub fn handle(&mut self, c: AccessoryCommand) {
        match c {
            AccessoryCommand::RequestBluetooth => {
                if peripherals::bluetooth_permission() == PermissionState::NotDetermined {
                    self.bluetooth_request = Some(peripherals::request_bluetooth());
                } else if peripherals::bluetooth_permission() == PermissionState::Denied {
                    crate::sys::workspace::open_url(
                        nox_core::permissions::Permission::Bluetooth.settings_url(),
                    );
                }
                if let Some(w) = &self.worker {
                    w.token().nudge();
                }
            }
        }
    }
}

impl Module for AccessoriesModule {
    fn id(&self) -> ModuleId {
        ModuleId::Accessories
    }

    fn start(&mut self, ctx: ModuleContext) -> Result<(), ModuleError> {
        self.stop();
        *self.settings.lock().unwrap_or_else(|e| e.into_inner()) = ctx.settings.accessories.clone();
        let hub = ctx.hub.clone();
        hub.update(|s| s.accessories = Some(AccessoriesState::default()));
        let settings = self.settings.clone();
        self.worker = Some(Worker::spawn("accessories", move |token| {
            run(hub, settings, token)
        }));
        Ok(())
    }

    fn stop(&mut self) {
        if let Some(mut w) = self.worker.take() {
            w.stop();
        }
        self.bluetooth_request = None;
    }

    fn command(&mut self, cmd: nox_core::Command) {
        if let nox_core::Command::Accessories(c) = cmd {
            self.handle(c);
        }
    }

    fn settings_changed(&mut self, settings: &Arc<Settings>) {
        *self.settings.lock().unwrap_or_else(|e| e.into_inner()) = settings.accessories.clone();
        if let Some(w) = &self.worker {
            w.token().nudge();
        }
    }
}

/// IOBluetooth may be touched only when this holds (see `run`).
pub fn uses_bluetooth(perm: PermissionState) -> bool {
    perm == PermissionState::Granted
}

pub fn kind_from_name(name: &str) -> Option<AccessoryKind> {
    let n = name.to_lowercase();
    let has = |words: &[&str]| words.iter().any(|w| n.contains(w));
    Some(if has(&["trackpad"]) {
        AccessoryKind::Trackpad
    } else if has(&[
        "airpods",
        "beats",
        "headphone",
        "headset",
        "buds",
        "casque",
        "écouteur",
    ]) {
        AccessoryKind::Headphones
    } else if has(&["mouse", "souris"]) {
        AccessoryKind::Mouse
    } else if has(&["keyboard", "clavier"]) {
        AccessoryKind::Keyboard
    } else if has(&[
        "controller",
        "manette",
        "dualsense",
        "dualshock",
        "xbox",
        "joy-con",
    ]) {
        AccessoryKind::GameController
    } else if has(&["iphone"]) {
        AccessoryKind::Phone
    } else {
        return None;
    })
}

/// Bluetooth Class of Device (major / minor as IOBluetooth reports them).
pub fn kind_from_class(major: u32, minor: u32) -> AccessoryKind {
    match major {
        0x04 => AccessoryKind::Headphones,
        0x02 => AccessoryKind::Phone,
        0x05 => match (minor & 0x30, minor & 0x0f) {
            (_, 0x01 | 0x02) => AccessoryKind::GameController,
            (0x20, _) => AccessoryKind::Mouse,
            (0x10 | 0x30, _) => AccessoryKind::Keyboard,
            _ => AccessoryKind::Other,
        },
        _ => AccessoryKind::Other,
    }
}

pub fn kind_from_usage(page: Option<i64>, usage: Option<i64>) -> AccessoryKind {
    match (page, usage) {
        (Some(1), Some(6)) => AccessoryKind::Keyboard,
        (Some(1), Some(2)) => AccessoryKind::Mouse,
        (Some(1), Some(4 | 5)) => AccessoryKind::GameController,
        (Some(13), Some(5)) => AccessoryKind::Trackpad,
        _ => AccessoryKind::Other,
    }
}

fn norm_addr(a: &str) -> String {
    a.to_ascii_lowercase().replace([':', '-'], "")
}

/// Connected devices: Bluetooth first (connection truth, AirPods parts),
/// completed by HID batteries; HID-only devices (USB/BLE) are added.
pub fn merge(bt: &[BtDevice], hid: &[HidBattery]) -> Vec<Accessory> {
    let mut used = HashSet::new();
    let mut out: Vec<Accessory> = vec![];
    for d in bt.iter().filter(|d| d.connected) {
        let hid_match = hid.iter().position(|h| {
            h.address
                .as_deref()
                .is_some_and(|a| norm_addr(a) == norm_addr(&d.address))
                || h.name.eq_ignore_ascii_case(&d.name)
        });
        let mut parts: Vec<BatteryPart> = d
            .battery
            .iter()
            .map(|(part, pct)| BatteryPart {
                part: part.clone(),
                percent: *pct,
                charging: None,
            })
            .collect();
        if parts.is_empty()
            && let Some(i) = hid_match
        {
            parts.push(BatteryPart {
                part: String::new(),
                percent: hid[i].percent,
                charging: None,
            });
        }
        if let Some(i) = hid_match {
            used.insert(i);
        }
        out.push(Accessory {
            id: norm_addr(&d.address),
            name: d.name.clone(),
            kind: kind_from_name(&d.name)
                .unwrap_or_else(|| kind_from_class(d.major_class, d.minor_class)),
            connected: true,
            battery: if parts.is_empty() {
                Reading::Unavailable(Unavailable::NotSupported)
            } else {
                Reading::Measured(parts)
            },
        });
    }
    for (i, h) in hid.iter().enumerate() {
        if used.contains(&i) {
            continue;
        }
        out.push(Accessory {
            id: h
                .address
                .as_deref()
                .map(norm_addr)
                .unwrap_or_else(|| h.id.clone()),
            name: h.name.clone(),
            kind: kind_from_name(&h.name).unwrap_or_else(|| kind_from_usage(h.usage_page, h.usage)),
            connected: true,
            battery: Reading::Measured(vec![BatteryPart {
                part: String::new(),
                percent: h.percent,
                charging: None,
            }]),
        });
    }
    out.sort_by_key(|a| a.name.to_lowercase());
    out
}

fn min_percent(a: &Accessory) -> Option<u8> {
    a.battery.value()?.iter().map(|p| p.percent).min()
}

pub fn part_label(part: &str) -> &'static str {
    match part {
        "Left" => "G",
        "Right" => "D",
        "Case" => "Boîtier",
        _ => "",
    }
}

fn battery_text(a: &Accessory) -> Option<String> {
    let parts = a.battery.value()?;
    Some(
        parts
            .iter()
            .map(|p| match part_label(&p.part) {
                "" => format!("{} %", p.percent),
                l => format!("{l} {} %", p.percent),
            })
            .collect::<Vec<_>>()
            .join(" · "),
    )
}

/// Connection and low-battery episodes. The first update is a silent
/// baseline (devices already connected or already low do not alert).
#[derive(Default)]
pub struct Monitor {
    baseline: bool,
    connected: HashSet<String>,
    low: HashSet<String>,
}

impl Monitor {
    pub fn update(&mut self, devices: &[Accessory], s: &AccessorySettings) -> Vec<Alert> {
        let mut alerts = vec![];
        let now: HashSet<String> = devices.iter().map(|d| d.id.clone()).collect();
        for d in devices {
            let low = s.low_battery_percent > 0
                && min_percent(d).is_some_and(|p| p <= s.low_battery_percent);
            let recovered = min_percent(d)
                .is_some_and(|p| p > s.low_battery_percent.saturating_add(RECOVERY_MARGIN));
            if self.baseline && !self.connected.contains(&d.id) && s.alert_on_connect {
                let mut a = Alert::new(
                    AlertKind::AccessoryConnected,
                    d.name.clone(),
                    symbol(d.kind),
                )
                .subtitle(match battery_text(d) {
                    Some(b) => format!("Connecté · {b}"),
                    None => "Connecté".into(),
                })
                .priority(AlertPriority::Low);
                a.progress = min_percent(d).map(|p| f64::from(p) / 100.0);
                alerts.push(a);
            }
            if low && !self.low.contains(&d.id) {
                if self.baseline {
                    let mut a = Alert::new(
                        AlertKind::AccessoryLowBattery,
                        d.name.clone(),
                        symbol(d.kind),
                    )
                    .subtitle(format!(
                        "Batterie faible · {}",
                        battery_text(d).unwrap_or_default()
                    ))
                    .priority(AlertPriority::Normal)
                    .ttl(6.0);
                    a.progress = min_percent(d).map(|p| f64::from(p) / 100.0);
                    alerts.push(a);
                }
                self.low.insert(d.id.clone());
            } else if recovered {
                self.low.remove(&d.id);
            }
        }
        self.low.retain(|id| now.contains(id));
        self.connected = now;
        self.baseline = true;
        alerts
    }
}

pub fn symbol(k: AccessoryKind) -> &'static str {
    match k {
        AccessoryKind::Headphones => "airpods",
        AccessoryKind::Keyboard => "keyboard",
        AccessoryKind::Mouse => "magicmouse",
        AccessoryKind::Trackpad => "rectangle.and.hand.point.up.left",
        AccessoryKind::GameController => "gamecontroller",
        AccessoryKind::Phone => "iphone",
        AccessoryKind::Other => "dot.radiowaves.left.and.right",
    }
}

fn run(hub: Hub, settings: Arc<Mutex<AccessorySettings>>, token: CancelToken) {
    let mut monitor = Monitor::default();
    let mut last: Option<AccessoriesState> = None;
    while !token.is_cancelled() {
        // Class-level read: never prompts, never blocks.
        let perm = peripherals::bluetooth_permission();
        // IOBluetooth only once Bluetooth is allowed, and on this worker
        // thread: without the permission `+[IOBluetoothDevice pairedDevices]`
        // blocks on a semaphore (its CoreBluetooth coordinator waits for the
        // TCC answer); on the main thread that froze the whole app at launch.
        // Until then only IORegistry batteries (no permission) are listed.
        let bt = if uses_bluetooth(perm) {
            peripherals::bluetooth_devices()
        } else {
            vec![]
        };
        let hid = peripherals::hid_batteries();
        let devices = merge(&bt, &hid);
        let s = settings.lock().unwrap_or_else(|e| e.into_inner()).clone();
        for a in monitor.update(&devices, &s) {
            hub.push_alert(a);
        }
        let state = AccessoriesState {
            bluetooth: perm,
            devices,
        };
        // Publish (and wake the UI) only when something actually changed.
        if last.as_ref() != Some(&state) {
            let publish = state.clone();
            hub.update(move |st| {
                if st.accessories.is_some() {
                    st.accessories = Some(publish);
                }
            });
            last = Some(state);
        }
        if token.sleep(POLL) {
            break;
        }
    }
    hub.update(|s| s.accessories = None);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn airpods(connected: bool, l: u8) -> BtDevice {
        BtDevice {
            address: "AA:BB:CC:DD:EE:01".into(),
            name: "Synthetic AirPods Pro".into(),
            connected,
            major_class: 0x04,
            minor_class: 0x06,
            battery: vec![
                ("Left".into(), l),
                ("Right".into(), 80),
                ("Case".into(), 55),
            ],
        }
    }

    fn mouse_hid(pct: u8) -> HidBattery {
        HidBattery {
            id: "aa-bb-cc-dd-ee-02".into(),
            name: "Synthetic Mouse".into(),
            percent: pct,
            usage_page: Some(1),
            usage: Some(2),
            address: Some("aa-bb-cc-dd-ee-02".into()),
        }
    }

    #[test]
    fn bluetooth_only_when_allowed() {
        assert!(uses_bluetooth(PermissionState::Granted));
        for p in [
            PermissionState::NotDetermined,
            PermissionState::Denied,
            PermissionState::Restricted,
            PermissionState::NotRequired,
        ] {
            assert!(!uses_bluetooth(p), "{p:?} must not reach IOBluetooth");
        }
    }

    #[test]
    fn kinds() {
        assert_eq!(
            kind_from_name("Magic Trackpad"),
            Some(AccessoryKind::Trackpad)
        );
        assert_eq!(
            kind_from_name("Clavier de Martin"),
            Some(AccessoryKind::Keyboard)
        );
        assert_eq!(kind_from_class(0x05, 0x20), AccessoryKind::Mouse);
        assert_eq!(kind_from_class(0x05, 0x02), AccessoryKind::GameController);
        assert_eq!(kind_from_usage(Some(1), Some(6)), AccessoryKind::Keyboard);
    }

    #[test]
    fn merges_bluetooth_and_hid() {
        let bt = vec![
            airpods(true, 70),
            BtDevice {
                address: "AA:BB:CC:DD:EE:02".into(),
                name: "Synthetic Mouse".into(),
                connected: true,
                major_class: 0x05,
                minor_class: 0x20,
                battery: vec![],
            },
            BtDevice {
                address: "AA:BB:CC:DD:EE:03".into(),
                name: "Old Speaker".into(),
                connected: false,
                major_class: 0x04,
                minor_class: 0,
                battery: vec![],
            },
        ];
        let devs = merge(&bt, &[mouse_hid(42)]);
        assert_eq!(devs.len(), 2, "disconnected devices are not listed");
        let pods = devs
            .iter()
            .find(|d| d.kind == AccessoryKind::Headphones)
            .unwrap();
        assert_eq!(pods.battery.value().unwrap().len(), 3);
        assert_eq!(
            battery_text(pods).unwrap(),
            "G 70 % · D 80 % · Boîtier 55 %"
        );
        let mouse = devs
            .iter()
            .find(|d| d.kind == AccessoryKind::Mouse)
            .unwrap();
        assert_eq!(
            min_percent(mouse),
            Some(42),
            "HID battery joined by address"
        );
    }

    #[test]
    fn alerts_after_baseline_once_per_episode() {
        let s = AccessorySettings::default();
        let mut m = Monitor::default();
        // Baseline: already connected and already low → silent.
        assert!(m.update(&merge(&[airpods(true, 10)], &[]), &s).is_empty());
        assert!(m.update(&merge(&[airpods(true, 9)], &[]), &s).is_empty());
        // Mouse connects at 15 % → connect + low alerts.
        let a = m.update(&merge(&[airpods(true, 9)], &[mouse_hid(15)]), &s);
        let kinds: Vec<AlertKind> = a.iter().map(|a| a.kind).collect();
        assert_eq!(
            kinds,
            vec![
                AlertKind::AccessoryConnected,
                AlertKind::AccessoryLowBattery
            ]
        );
        // AirPods recharge above 25 % then drop again → one new low alert.
        assert!(
            m.update(&merge(&[airpods(true, 60)], &[mouse_hid(15)]), &s)
                .is_empty()
        );
        let a = m.update(&merge(&[airpods(true, 18)], &[mouse_hid(15)]), &s);
        assert_eq!(a.len(), 1);
        assert_eq!(a[0].kind, AlertKind::AccessoryLowBattery);
        // Disconnect + reconnect → connect alert again.
        m.update(&merge(&[], &[mouse_hid(15)]), &s);
        let a = m.update(&merge(&[airpods(true, 60)], &[mouse_hid(15)]), &s);
        assert_eq!(a[0].kind, AlertKind::AccessoryConnected);
    }
}
