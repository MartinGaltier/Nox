use crate::reading::Reading;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AccessoryKind {
    Headphones,
    Keyboard,
    Mouse,
    Trackpad,
    GameController,
    Phone,
    Other,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BatteryPart {
    /// "Left", "Right", "Case" or "" for single-battery devices.
    pub part: String,
    pub percent: u8,
    pub charging: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Accessory {
    pub id: String,
    pub name: String,
    pub kind: AccessoryKind,
    pub connected: bool,
    pub battery: Reading<Vec<BatteryPart>>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AccessoriesState {
    pub devices: Vec<Accessory>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AccessorySettings {
    pub alert_on_connect: bool,
    /// Low-battery alert threshold in percent (0 = off).
    pub low_battery_percent: u8,
}

impl Default for AccessorySettings {
    fn default() -> Self {
        Self {
            alert_on_connect: true,
            low_battery_percent: 20,
        }
    }
}
