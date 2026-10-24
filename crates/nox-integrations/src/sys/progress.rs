//! Download progress published by browsers: `NSProgress` file subscribers
//! (real totals), the `com.apple.DownloadFileFinished` distributed
//! notification (final path), and Safari's `.download/Info.plist`.

use block2::{DynBlock, RcBlock};
use crossbeam_channel::Sender;
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, NSObjectProtocol, ProtocolObject};
use objc2_foundation::{
    NSDictionary, NSDistributedNotificationCenter, NSNotification, NSNumber, NSProgress, NSString,
    NSURL,
};
use std::ptr::NonNull;
use std::sync::{Arc, Mutex};

#[derive(Debug, Clone, PartialEq)]
pub struct Published {
    pub path: String,
    pub completed: i64,
    pub total: i64,
    pub indeterminate: bool,
    pub finished: bool,
    pub cancelled: bool,
}

struct SendProgress(Retained<NSProgress>);
// SAFETY: NSProgress is documented as thread-safe for reading its properties.
unsafe impl Send for SendProgress {}

struct SendObj(Retained<AnyObject>);

struct SendObserver(Retained<ProtocolObject<dyn NSObjectProtocol>>);
// SAFETY: opaque observer token, only handed back to removeObserver.
unsafe impl Send for SendObserver {}
// SAFETY: opaque subscriber/observer tokens, only passed back to Foundation
// (removeSubscriber / removeObserver), which are thread-safe.
unsafe impl Send for SendObj {}

/// Subscriptions for a set of folders. Created and removed on the main thread.
pub struct Subscriptions {
    progresses: Arc<Mutex<Vec<SendProgress>>>,
    subscribers: Vec<SendObj>,
    finished_observer: Option<SendObserver>,
}

/// Must run on the main thread (its run loop delivers both sources).
pub fn subscribe(folders: &[String], finished_tx: Sender<String>) -> Subscriptions {
    let progresses: Arc<Mutex<Vec<SendProgress>>> = Default::default();
    let subscribers = folders
        .iter()
        .map(|f| {
            let list = progresses.clone();
            let handler = RcBlock::new(move |p: NonNull<NSProgress>| -> *mut DynBlock<dyn Fn()> {
                // SAFETY: Foundation passes a live NSProgress; retaining it
                // keeps it readable after the handler returns.
                if let Some(p) = unsafe { Retained::retain(p.as_ptr()) } {
                    let mut l = list.lock().unwrap_or_else(|e| e.into_inner());
                    l.retain(|x| !x.0.isFinished() && !x.0.isCancelled());
                    if l.len() < 64 {
                        l.push(SendProgress(p));
                    }
                }
                // No unpublishing handler: finished/cancelled progresses are
                // dropped by polling instead.
                std::ptr::null_mut()
            });
            let url = NSURL::fileURLWithPath(&NSString::from_str(f));
            // SAFETY: the handler is a heap block copied by Foundation.
            let token = unsafe {
                NSProgress::addSubscriberForFileURL_withPublishingHandler(
                    &url,
                    RcBlock::as_ptr(&handler),
                )
            };
            SendObj(token)
        })
        .collect();
    let block = RcBlock::new(move |n: NonNull<NSNotification>| {
        // SAFETY: valid notification for the duration of the call.
        let n = unsafe { n.as_ref() };
        if let Some(path) = n
            .object()
            .and_then(|o| o.downcast_ref::<NSString>().map(|s| s.to_string()))
        {
            let _ = finished_tx.send(path);
        }
    });
    let center = NSDistributedNotificationCenter::defaultCenter();
    // SAFETY: block captures only a Sender; no operation queue.
    let observer: Retained<ProtocolObject<dyn NSObjectProtocol>> = unsafe {
        center.addObserverForName_object_queue_usingBlock(
            Some(&NSString::from_str("com.apple.DownloadFileFinished")),
            None,
            None,
            &block,
        )
    };
    Subscriptions {
        progresses,
        subscribers,
        finished_observer: Some(SendObserver(observer)),
    }
}

impl Subscriptions {
    pub fn snapshot(&self) -> Vec<Published> {
        let mut l = self.progresses.lock().unwrap_or_else(|e| e.into_inner());
        l.retain(|p| !p.0.isCancelled());
        l.iter()
            .filter_map(|p| {
                let p = &p.0;
                let path = p.fileURL()?.path()?.to_string();
                Some(Published {
                    path,
                    completed: p.completedUnitCount(),
                    total: p.totalUnitCount(),
                    indeterminate: p.isIndeterminate(),
                    finished: p.isFinished(),
                    cancelled: p.isCancelled(),
                })
            })
            .collect()
    }

    /// Removes subscribers and the observer (thread-safe Foundation calls).
    pub fn remove(&mut self) {
        for s in self.subscribers.drain(..) {
            // SAFETY: token returned by addSubscriberForFileURL.
            unsafe { NSProgress::removeSubscriber(&s.0) };
        }
        if let Some(o) = self.finished_observer.take() {
            // SAFETY: token returned by addObserverForName on this center.
            unsafe {
                NSDistributedNotificationCenter::defaultCenter().removeObserver(o.0.as_ref())
            };
        }
        self.progresses
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clear();
    }
}

impl Drop for Subscriptions {
    fn drop(&mut self) {
        self.remove();
    }
}

/// Safari keeps `DownloadEntryProgressBytesSoFar` / `…TotalToLoad` in the
/// bundle's Info.plist. Returns (bytes, total).
pub fn safari_progress(bundle: &str) -> Option<(u64, Option<u64>)> {
    let path = NSString::from_str(&format!("{bundle}/Info.plist"));
    // SAFETY: reads a property list file; nil on any error.
    #[allow(deprecated)]
    let d: Retained<NSDictionary<NSString, AnyObject>> =
        unsafe { NSDictionary::dictionaryWithContentsOfFile(&path) }?;
    let num = |k: &str| {
        d.objectForKey(&NSString::from_str(k))
            .and_then(|v| v.downcast_ref::<NSNumber>().map(|n| n.longLongValue()))
    };
    let bytes = num("DownloadEntryProgressBytesSoFar")?;
    let total = num("DownloadEntryProgressTotalToLoad").filter(|t| *t > 0);
    Some((bytes.max(0) as u64, total.map(|t| t as u64)))
}
