//! Nox shared contracts.
//!
//! Architecture (see docs/ARCHITECTURE.md):
//! producers (`nox-metrics`, `nox-power`, `nox-integrations`, `nox-activities`)
//! implement [`module::Module`] and publish into the [`hub::Hub`]; the UI
//! (`nox-ui`) reads [`hub::IslandState`] on the main thread and sends
//! [`command::Command`]s back. [`manager::ModuleManager`] starts/stops modules
//! from [`settings::Settings`].

pub mod alert;
pub mod cancel;
pub mod command;
pub mod hub;
pub mod main_thread;
pub mod manager;
pub mod model;
pub mod module;
pub mod paths;
pub mod permissions;
pub mod reading;
pub mod settings;

pub use alert::{Alert, AlertKind, AlertPriority};
pub use cancel::{CancelToken, Worker};
pub use command::Command;
pub use hub::{Hub, IslandState};
pub use module::{Module, ModuleContext, ModuleError, ModuleId};
pub use reading::{Reading, Unavailable};
pub use settings::{Section, Settings, SettingsStore};
