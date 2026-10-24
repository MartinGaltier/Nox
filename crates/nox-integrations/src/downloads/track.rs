// Adapted from vorssaint-utils (GPL-3.0-or-later), Sources/Vorssaint/Services/Notch/NotchDownloadSupport.swift, commit e916961.

//! Pure download tracking: partial-file naming, folder scans and the
//! in-progress → finished/cancelled state machine. No Objective-C here.

use nox_core::model::downloads::{DownloadItem, DownloadStatus};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Browser partial-file suffixes and who writes them.
const PARTIAL: &[(&str, &str)] = &[
    ("crdownload", "Chrome"),
    ("download", "Safari"),
    ("part", "Firefox"),
    ("opdownload", "Opera"),
    ("partial", "Edge"),
];

/// `foo.zip.crdownload` → (`foo.zip`, "Chrome").
pub fn classify(name: &str) -> Option<(String, &'static str)> {
    let (stem, ext) = name.rsplit_once('.')?;
    if stem.is_empty() {
        return None;
    }
    let ext = ext.to_ascii_lowercase();
    let (_, source) = PARTIAL.iter().find(|(e, _)| *e == ext)?;
    // Chrome names fresh downloads "Unconfirmed 123.crdownload": no final name yet.
    Some((stem.to_owned(), source))
}

/// One partial file seen during a scan.
#[derive(Debug, Clone, PartialEq)]
pub struct Partial {
    pub partial_path: PathBuf,
    pub final_path: PathBuf,
    pub source: &'static str,
    pub bytes: u64,
    /// Expected total when a source reports it (Safari Info.plist, NSProgress).
    pub total: Option<u64>,
}

/// Size of a file, or of the data file inside a Safari `.download` bundle.
pub fn partial_size(path: &Path) -> u64 {
    let Ok(meta) = std::fs::symlink_metadata(path) else {
        return 0;
    };
    if meta.is_file() {
        return meta.len();
    }
    if meta.is_dir() {
        return std::fs::read_dir(path)
            .into_iter()
            .flatten()
            .flatten()
            .filter(|e| e.file_name() != "Info.plist")
            .filter_map(|e| e.metadata().ok())
            .filter(|m| m.is_file())
            .map(|m| m.len())
            .max()
            .unwrap_or(0);
    }
    0
}

/// Direct children of `folder` that look like partial downloads.
pub fn scan(folder: &Path) -> Vec<Partial> {
    let Ok(rd) = std::fs::read_dir(folder) else {
        return vec![];
    };
    rd.flatten()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            let (final_name, source) = classify(&name)?;
            let p = e.path();
            Some(Partial {
                bytes: partial_size(&p),
                final_path: folder.join(final_name),
                partial_path: p,
                source,
                total: None,
            })
        })
        .take(64)
        .collect()
}

#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    Finished(DownloadItem),
}

struct Live {
    item: DownloadItem,
    last_bytes: u64,
    last_at: f64,
}

/// Keeps in-progress and recently finished downloads.
#[derive(Default)]
pub struct Tracker {
    live: HashMap<String, Live>,
    done: Vec<DownloadItem>,
}

/// Exponential smoothing of the byte rate.
const RATE_ALPHA: f64 = 0.4;

impl Tracker {
    /// Applies one scan. `final_exists(path)` says whether a completed file
    /// is present (non-empty). Returns finish events (for alerts).
    pub fn update(
        &mut self,
        partials: Vec<Partial>,
        now: f64,
        final_exists: &dyn Fn(&Path) -> bool,
    ) -> Vec<Event> {
        let mut seen = std::collections::HashSet::new();
        for p in partials {
            let id = p.final_path.to_string_lossy().into_owned();
            seen.insert(id.clone());
            let progress = p
                .total
                .filter(|t| *t > 0)
                .map(|t| (p.bytes as f64 / t as f64).clamp(0.0, 1.0));
            match self.live.get_mut(&id) {
                Some(l) => {
                    let dt = now - l.last_at;
                    if dt > 0.2 {
                        let inst = p.bytes.saturating_sub(l.last_bytes) as f64 / dt;
                        l.item.rate = Some(match l.item.rate {
                            Some(r) => r + RATE_ALPHA * (inst - r),
                            None => inst,
                        });
                        l.last_bytes = p.bytes;
                        l.last_at = now;
                    }
                    l.item.bytes = p.bytes;
                    l.item.total = p.total.or(l.item.total);
                    l.item.progress = progress.or(l.item.progress);
                    l.item.updated_at = now;
                }
                None => {
                    self.done.retain(|d| d.id != id);
                    self.live.insert(
                        id.clone(),
                        Live {
                            item: DownloadItem {
                                id: id.clone(),
                                file_name: p
                                    .final_path
                                    .file_name()
                                    .map(|n| n.to_string_lossy().into_owned())
                                    .unwrap_or_default(),
                                path: id,
                                progress,
                                bytes: p.bytes,
                                total: p.total,
                                rate: None,
                                status: DownloadStatus::InProgress,
                                source: p.source.to_owned(),
                                updated_at: now,
                            },
                            last_bytes: p.bytes,
                            last_at: now,
                        },
                    );
                }
            }
        }
        let gone: Vec<String> = self
            .live
            .keys()
            .filter(|k| !seen.contains(*k))
            .cloned()
            .collect();
        let mut events = vec![];
        for id in gone {
            let Some(l) = self.live.remove(&id) else {
                continue;
            };
            let mut item = l.item;
            item.rate = None;
            item.updated_at = now;
            if final_exists(Path::new(&item.path)) {
                item.status = DownloadStatus::Finished;
                item.progress = Some(1.0);
                if let Ok(m) = std::fs::metadata(&item.path) {
                    item.bytes = m.len();
                }
                events.push(Event::Finished(item.clone()));
            } else {
                item.status = DownloadStatus::Cancelled;
            }
            self.done.push(item);
        }
        events
    }

    /// A browser announced a finished file we may not have seen as partial.
    pub fn announce_finished(&mut self, path: &Path, now: f64) -> Option<Event> {
        let id = path.to_string_lossy().into_owned();
        if self
            .done
            .iter()
            .any(|d| d.id == id && d.status == DownloadStatus::Finished)
        {
            return None;
        }
        if self.live.contains_key(&id) {
            // The next scan will see the partial disappear.
            return None;
        }
        let meta = std::fs::metadata(path).ok()?;
        let item = DownloadItem {
            id: id.clone(),
            file_name: path.file_name()?.to_string_lossy().into_owned(),
            path: id,
            progress: Some(1.0),
            bytes: meta.len(),
            total: Some(meta.len()),
            rate: None,
            status: DownloadStatus::Finished,
            source: "Navigateur".into(),
            updated_at: now,
        };
        self.done.push(item.clone());
        Some(Event::Finished(item))
    }

    pub fn dismiss(&mut self, id: &str) {
        self.done.retain(|d| d.id != id);
        self.live.remove(id);
    }

    /// Drops finished/cancelled entries older than `keep_secs`.
    pub fn expire(&mut self, now: f64, keep_secs: u64) -> bool {
        let before = self.done.len();
        self.done.retain(|d| now - d.updated_at < keep_secs as f64);
        before != self.done.len()
    }

    pub fn has_live(&self) -> bool {
        !self.live.is_empty()
    }

    /// In-progress first (newest first), then finished (newest first).
    pub fn items(&self) -> Vec<DownloadItem> {
        let mut live: Vec<DownloadItem> = self.live.values().map(|l| l.item.clone()).collect();
        live.sort_by(|a, b| a.file_name.cmp(&b.file_name));
        let mut done = self.done.clone();
        done.sort_by(|a, b| b.updated_at.total_cmp(&a.updated_at));
        live.extend(done);
        live
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_partial_names() {
        assert_eq!(
            classify("a.zip.crdownload"),
            Some(("a.zip".into(), "Chrome"))
        );
        assert_eq!(
            classify("Film.mov.download"),
            Some(("Film.mov".into(), "Safari"))
        );
        assert_eq!(
            classify("x.tar.gz.part"),
            Some(("x.tar.gz".into(), "Firefox"))
        );
        assert_eq!(
            classify("Unconfirmed 12.CRDOWNLOAD"),
            Some(("Unconfirmed 12".into(), "Chrome"))
        );
        assert_eq!(classify("photo.jpg"), None);
        assert_eq!(classify(".crdownload"), None);
        assert_eq!(classify("noext"), None);
    }

    fn partial(name: &str, bytes: u64, total: Option<u64>) -> Partial {
        Partial {
            partial_path: PathBuf::from(format!("/d/{name}.crdownload")),
            final_path: PathBuf::from(format!("/d/{name}")),
            source: "Chrome",
            bytes,
            total,
        }
    }

    #[test]
    fn progress_rate_and_finish() {
        let mut t = Tracker::default();
        assert!(
            t.update(vec![partial("f.bin", 0, Some(1000))], 0.0, &|_| false)
                .is_empty()
        );
        t.update(vec![partial("f.bin", 500, Some(1000))], 1.0, &|_| false);
        let it = &t.items()[0];
        assert_eq!(it.progress, Some(0.5));
        assert_eq!(it.rate, Some(500.0));
        assert_eq!(it.status, DownloadStatus::InProgress);
        let ev = t.update(vec![], 2.0, &|p| p == Path::new("/d/f.bin"));
        assert!(
            matches!(&ev[..], [Event::Finished(i)] if i.file_name == "f.bin" && i.progress == Some(1.0))
        );
        assert_eq!(t.items()[0].status, DownloadStatus::Finished);
        assert!(t.expire(700.0, 600));
        assert!(t.items().is_empty());
    }

    #[test]
    fn vanished_without_file_is_cancelled_and_unknown_total_stays_none() {
        let mut t = Tracker::default();
        t.update(vec![partial("g.iso", 10, None)], 0.0, &|_| false);
        assert_eq!(t.items()[0].progress, None, "no invented progress");
        assert!(t.update(vec![], 1.0, &|_| false).is_empty());
        assert_eq!(t.items()[0].status, DownloadStatus::Cancelled);
    }

    #[test]
    fn scans_a_synthetic_folder() {
        let dir = std::env::temp_dir().join(format!("nox-dl-test-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("Movie.mov.download")).unwrap();
        std::fs::write(dir.join("Movie.mov.download/Movie.mov"), vec![0u8; 300]).unwrap();
        std::fs::write(dir.join("Movie.mov.download/Info.plist"), b"x").unwrap();
        std::fs::write(dir.join("a.zip.crdownload"), vec![0u8; 42]).unwrap();
        std::fs::write(dir.join("done.pdf"), b"pdf").unwrap();
        let mut found = scan(&dir);
        found.sort_by(|a, b| a.final_path.cmp(&b.final_path));
        assert_eq!(found.len(), 2);
        assert_eq!((found[0].source, found[0].bytes), ("Safari", 300));
        assert_eq!((found[1].source, found[1].bytes), ("Chrome", 42));
        let _ = std::fs::remove_dir_all(dir);
    }
}
