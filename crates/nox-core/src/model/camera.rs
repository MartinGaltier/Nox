use crate::permissions::PermissionState;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CameraState {
    pub access: PermissionState,
    pub running: bool,
    pub devices: Vec<String>,
    pub selected: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum CameraCommand {
    RequestAccess,
    Start,
    Stop,
    Select(String),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct CameraSettings {
    pub mirror: bool,
}

impl Default for CameraSettings {
    fn default() -> Self {
        Self { mirror: true }
    }
}
