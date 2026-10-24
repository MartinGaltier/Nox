//! Volume space. Which mounts are shown:
//! * local file systems only (`MNT_LOCAL`): network shares can hang;
//! * browsable only (no `MNT_DONTBROWSE`): drops the APFS system volumes
//!   (`/System/Volumes/{VM,Preboot,Update,Data,…}`), Recovery, devfs;
//! * the root `/` is always kept: on macOS it is the sealed read-only system
//!   snapshot, but its APFS container is the one holding the user's data, so
//!   its free space is the startup disk's free space (what `df -h /` prints).

use nox_core::model::metrics::DiskInfo;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Mount {
    pub mount: String,
    pub from: String,
    pub fs_type: String,
    pub flags: u32,
    pub block_size: u64,
    pub blocks: u64,
    pub blocks_available: u64,
}

pub fn shown(m: &Mount) -> bool {
    if m.mount == "/" {
        return true;
    }
    let flags = m.flags as i32;
    flags & libc::MNT_LOCAL != 0
        && flags & libc::MNT_DONTBROWSE == 0
        && m.blocks > 0
        && m.fs_type != "devfs"
        && m.fs_type != "autofs"
}

/// `available` = blocks available to unprivileged users (`f_bavail`, as `df`).
pub fn info(m: &Mount, name: Option<String>) -> DiskInfo {
    let name = name.unwrap_or_else(|| {
        if m.mount == "/" {
            "Disque de démarrage".into()
        } else {
            m.mount.rsplit('/').next().unwrap_or(&m.mount).to_owned()
        }
    });
    DiskInfo {
        mount: m.mount.clone(),
        name,
        total: m.blocks.saturating_mul(m.block_size),
        available: m.blocks_available.saturating_mul(m.block_size),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mnt(mount: &str, fs: &str, flags: i32) -> Mount {
        Mount {
            mount: mount.into(),
            from: "/dev/disk3s1s1".into(),
            fs_type: fs.into(),
            flags: flags as u32,
            block_size: 4096,
            blocks: 100,
            blocks_available: 25,
        }
    }

    #[test]
    fn filters_like_finder() {
        let local = libc::MNT_LOCAL;
        let nob = libc::MNT_DONTBROWSE;
        assert!(shown(&mnt("/", "apfs", local | libc::MNT_RDONLY | nob)));
        assert!(!shown(&mnt("/System/Volumes/Data", "apfs", local | nob)));
        assert!(!shown(&mnt("/dev", "devfs", local | nob)));
        assert!(shown(&mnt("/Volumes/Clé", "msdos", local)));
        assert!(!shown(&mnt("/Volumes/nas", "smbfs", 0)));
    }

    #[test]
    fn sizes_and_names() {
        let d = info(&mnt("/Volumes/Clé", "exfat", 0), None);
        assert_eq!((d.total, d.available), (409_600, 102_400));
        assert_eq!(d.name, "Clé");
        assert_eq!(
            info(&mnt("/", "apfs", 0), Some("Macintosh HD".into())).name,
            "Macintosh HD"
        );
    }
}
