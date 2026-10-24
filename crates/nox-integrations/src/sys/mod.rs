//! All `unsafe` / Objective-C interop of the crate lives under `sys`.
//! Public functions here are safe; each unsafe block carries a SAFETY note.

pub mod applescript;
pub mod avcapture;
pub mod eventkit;
pub mod http;
pub mod workspace;
