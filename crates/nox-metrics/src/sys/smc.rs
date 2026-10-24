// Adapted from vorssaint-utils (GPL-3.0-or-later), Sources/Vorssaint/Services/SystemMonitor/SMCClient.swift, commit e916961.
//! Read-only AppleSMC user client. Nox never writes SMC keys.

use super::iokit::{Connection, IoObject};

const HANDLE_YPC_EVENT: u32 = 2;
const CMD_READ_KEY: u8 = 5;
const CMD_KEY_FROM_INDEX: u8 = 8;
const CMD_KEY_INFO: u8 = 9;

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct Vers {
    major: u8,
    minor: u8,
    build: u8,
    reserved: u8,
    release: u16,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct PLimit {
    version: u16,
    length: u16,
    cpu: u32,
    gpu: u32,
    mem: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct KeyInfoRaw {
    data_size: u32,
    data_type: u32,
    attributes: u8,
}

/// Wire format of the AppleSMC user client (fixed 80-byte layout).
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct Param {
    key: u32,
    vers: Vers,
    plimit: PLimit,
    info: KeyInfoRaw,
    result: u8,
    status: u8,
    data8: u8,
    data32: u32,
    bytes: [u8; 32],
}

const _: () = assert!(std::mem::size_of::<Param>() == 80);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeyInfo {
    pub size: u32,
    /// Four-character type code, e.g. `flt `, `sp78`, `ui8 `.
    pub kind: [u8; 4],
}

pub struct Smc {
    conn: Connection,
}

impl Smc {
    pub fn open() -> Result<Self, String> {
        let svc = IoObject::matching("AppleSMC").ok_or("AppleSMC service not found")?;
        let conn = svc
            .open(0)
            .map_err(|kr| format!("IOServiceOpen(AppleSMC) = {kr:#x}"))?;
        Ok(Self { conn })
    }

    fn call(&self, p: &Param) -> Option<Param> {
        // SAFETY: Param is the POD struct the AppleSMC user client expects for
        // selector 2 (kSMCHandleYPCEvent); size asserted above.
        let out = unsafe { self.conn.call_struct(HANDLE_YPC_EVENT, p) }.ok()?;
        (out.result == 0).then_some(out)
    }

    pub fn key_info(&self, key: u32) -> Option<KeyInfo> {
        let p = Param {
            key,
            data8: CMD_KEY_INFO,
            ..Default::default()
        };
        let out = self.call(&p)?;
        Some(KeyInfo {
            size: out.info.data_size,
            kind: out.info.data_type.to_be_bytes(),
        })
    }

    pub fn read(&self, key: u32, info: KeyInfo) -> Option<Vec<u8>> {
        if info.size == 0 || info.size > 32 {
            return None;
        }
        let mut p = Param {
            key,
            data8: CMD_READ_KEY,
            ..Default::default()
        };
        p.info.data_size = info.size;
        let out = self.call(&p)?;
        Some(out.bytes[..info.size as usize].to_vec())
    }

    pub fn key_count(&self) -> u32 {
        let key = u32::from_be_bytes(*b"#KEY");
        let info = KeyInfo {
            size: 4,
            kind: *b"ui32",
        };
        self.read(key, info)
            .and_then(|b| {
                b.get(..4)
                    .map(|b| u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
            })
            .unwrap_or(0)
    }

    pub fn key_at(&self, index: u32) -> Option<u32> {
        let p = Param {
            data8: CMD_KEY_FROM_INDEX,
            data32: index,
            ..Default::default()
        };
        self.call(&p).map(|o| o.key)
    }
}
