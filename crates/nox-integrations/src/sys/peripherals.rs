// Adapted from vorssaint-utils (GPL-3.0-or-later), Sources/Vorssaint/Services/Metrics/PeripheralBatterySupport.swift, commit e916961.

//! Accessory batteries and connections, read in-process (no external
//! process): IORegistry HID services exposing `BatteryPercent` (Magic
//! Keyboard/Mouse/Trackpad, many BLE HID devices) and IOBluetooth paired
//! devices (connection state; AirPods left/right/case through the
//! `batteryPercent*` selectors, `unsigned char`, 0 = unknown).

use nox_core::permissions::PermissionState;
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, Sel};
use objc2::{msg_send, sel};
use objc2_core_bluetooth::{CBManager, CBManagerAuthorization};
use objc2_foundation::{NSDictionary, NSNumber, NSString};
use objc2_io_bluetooth::IOBluetoothDevice;
use std::ffi::{CStr, c_char, c_void};

#[derive(Debug, Clone, PartialEq)]
pub struct HidBattery {
    pub id: String,
    pub name: String,
    pub percent: u8,
    pub usage_page: Option<i64>,
    pub usage: Option<i64>,
    pub address: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct BtDevice {
    pub address: String,
    pub name: String,
    pub connected: bool,
    pub major_class: u32,
    pub minor_class: u32,
    /// (part, percent) with part "Left" / "Right" / "Case" / "".
    pub battery: Vec<(String, u8)>,
}

#[link(name = "IOKit", kind = "framework")]
unsafe extern "C" {
    fn IOServiceMatching(name: *const c_char) -> *mut c_void;
    fn IOServiceGetMatchingServices(
        main_port: u32,
        matching: *mut c_void,
        existing: *mut u32,
    ) -> i32;
    fn IOIteratorNext(iterator: u32) -> u32;
    fn IORegistryEntryCreateCFProperties(
        entry: u32,
        properties: *mut *mut c_void,
        allocator: *const c_void,
        options: u32,
    ) -> i32;
    fn IOObjectRelease(object: u32) -> i32;
}

fn services(class: &CStr) -> Vec<Retained<NSDictionary<NSString, AnyObject>>> {
    let mut out = vec![];
    // SAFETY: standard IOKit enumeration; the matching dictionary is
    // consumed by IOServiceGetMatchingServices; every object is released.
    unsafe {
        let matching = IOServiceMatching(class.as_ptr());
        if matching.is_null() {
            return out;
        }
        let mut it = 0u32;
        if IOServiceGetMatchingServices(0, matching, &mut it) != 0 {
            return out;
        }
        loop {
            let entry = IOIteratorNext(it);
            if entry == 0 {
                break;
            }
            let mut props: *mut c_void = std::ptr::null_mut();
            if IORegistryEntryCreateCFProperties(entry, &mut props, std::ptr::null(), 0) == 0
                && !props.is_null()
                && let Some(d) = Retained::from_raw(props as *mut NSDictionary<NSString, AnyObject>)
            {
                out.push(d);
            }
            IOObjectRelease(entry);
            if out.len() > 256 {
                break;
            }
        }
        IOObjectRelease(it);
    }
    out
}

fn get_str(d: &NSDictionary<NSString, AnyObject>, k: &str) -> Option<String> {
    let v = d.objectForKey(&NSString::from_str(k))?;
    let s = v.downcast_ref::<NSString>()?.to_string();
    let s = s.trim().to_owned();
    (!s.is_empty()).then_some(s)
}

fn get_i64(d: &NSDictionary<NSString, AnyObject>, k: &str) -> Option<i64> {
    let v = d.objectForKey(&NSString::from_str(k))?;
    Some(v.downcast_ref::<NSNumber>()?.longLongValue())
}

/// HID services with a battery (built-in devices excluded).
pub fn hid_batteries() -> Vec<HidBattery> {
    let mut out: Vec<HidBattery> = vec![];
    for class in [c"AppleDeviceManagementHIDEventService", c"IOHIDDevice"] {
        for d in services(class) {
            let Some(pct) = get_i64(&d, "BatteryPercent").filter(|p| (0..=100).contains(p)) else {
                continue;
            };
            if get_i64(&d, "Built-In").is_some_and(|b| b != 0) {
                continue;
            }
            let Some(name) = get_str(&d, "Product") else {
                continue;
            };
            let address = get_str(&d, "DeviceAddress");
            let id = address
                .clone()
                .or_else(|| get_str(&d, "SerialNumber"))
                .unwrap_or_else(|| name.clone());
            if out.iter().any(|h| h.id == id) {
                continue;
            }
            out.push(HidBattery {
                id,
                name,
                percent: pct as u8,
                usage_page: get_i64(&d, "PrimaryUsagePage"),
                usage: get_i64(&d, "PrimaryUsage"),
                address,
            });
        }
    }
    out
}

/// Bluetooth permission (never prompts).
pub fn bluetooth_permission() -> PermissionState {
    // SAFETY: class property read, no side effects.
    match unsafe { CBManager::authorization_class() } {
        CBManagerAuthorization::AllowedAlways => PermissionState::Granted,
        CBManagerAuthorization::Denied => PermissionState::Denied,
        CBManagerAuthorization::Restricted => PermissionState::Restricted,
        _ => PermissionState::NotDetermined,
    }
}

fn battery_byte(dev: &IOBluetoothDevice, sel: Sel) -> Option<u8> {
    // SAFETY: `respondsToSelector:` guards the private getter, whose type
    // encoding on macOS 27 is `C16@0:8` (returns unsigned char).
    unsafe {
        let responds: bool = msg_send![dev, respondsToSelector: sel];
        if !responds {
            return None;
        }
        let v: u8 = match sel.name().to_bytes() {
            b"batteryPercentLeft" => msg_send![dev, batteryPercentLeft],
            b"batteryPercentRight" => msg_send![dev, batteryPercentRight],
            b"batteryPercentCase" => msg_send![dev, batteryPercentCase],
            b"batteryPercentSingle" => msg_send![dev, batteryPercentSingle],
            _ => return None,
        };
        (1..=100).contains(&v).then_some(v)
    }
}

/// Paired Bluetooth devices. Call ONLY when `bluetooth_permission()` is
/// `Granted`, and NEVER on the main thread: without the permission the
/// CoreBluetooth coordinator behind `pairedDevices` waits on a semaphore
/// indefinitely (measured on macOS 27: app frozen at launch).
pub fn bluetooth_devices() -> Vec<BtDevice> {
    // SAFETY: IOBluetooth class method; elements are IOBluetoothDevice.
    let Some(arr) = (unsafe { IOBluetoothDevice::pairedDevices() }) else {
        return vec![];
    };
    let mut out = vec![];
    for obj in arr.iter().take(64) {
        let Some(dev) = obj.downcast_ref::<IOBluetoothDevice>() else {
            continue;
        };
        // SAFETY: plain property getters on a live IOBluetoothDevice.
        let (address, name, connected, major, minor) = unsafe {
            (
                dev.addressString().map(|s| s.to_string()),
                dev.name().to_string(),
                dev.isConnected(),
                dev.deviceClassMajor(),
                dev.deviceClassMinor(),
            )
        };
        let Some(address) = address else { continue };
        let mut battery = vec![];
        for (part, s) in [
            ("Left", sel!(batteryPercentLeft)),
            ("Right", sel!(batteryPercentRight)),
            ("Case", sel!(batteryPercentCase)),
        ] {
            if let Some(v) = battery_byte(dev, s) {
                battery.push((part.to_owned(), v));
            }
        }
        if battery.is_empty()
            && let Some(v) = battery_byte(dev, sel!(batteryPercentSingle))
        {
            battery.push((String::new(), v));
        }
        out.push(BtDevice {
            address,
            name: name.trim().to_owned(),
            connected,
            major_class: major,
            minor_class: minor,
            battery,
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore = "real machine: IORegistry only (no Bluetooth prompt)"]
    fn live_hid_batteries() {
        let h = hid_batteries();
        eprintln!(
            "{} HID device(s) with a battery; bluetooth permission {:?}",
            h.len(),
            bluetooth_permission()
        );
    }
}
