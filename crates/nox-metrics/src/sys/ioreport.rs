//! libIOReport (private, user-accessible, no root): the "Energy Model" group
//! publishes per-block energy counters on Apple silicon.

use super::cf::{CFTypeRef, Cf};
use std::ffi::c_void;

#[link(name = "IOReport")]
unsafe extern "C" {
    fn IOReportCopyChannelsInGroup(
        group: CFTypeRef,
        subgroup: CFTypeRef,
        a: u64,
        b: u64,
        c: u64,
    ) -> CFTypeRef;
    fn IOReportCreateSubscription(
        a: *const c_void,
        desired: CFTypeRef,
        subscribed: *mut CFTypeRef,
        channel_id: u64,
        b: CFTypeRef,
    ) -> CFTypeRef;
    fn IOReportCreateSamples(sub: CFTypeRef, channels: CFTypeRef, a: CFTypeRef) -> CFTypeRef;
    fn IOReportCreateSamplesDelta(prev: CFTypeRef, cur: CFTypeRef, a: CFTypeRef) -> CFTypeRef;
    fn IOReportChannelGetGroup(item: CFTypeRef) -> CFTypeRef;
    fn IOReportChannelGetChannelName(item: CFTypeRef) -> CFTypeRef;
    fn IOReportChannelGetUnitLabel(item: CFTypeRef) -> CFTypeRef;
    fn IOReportSimpleGetIntegerValue(item: CFTypeRef, idx: i32) -> i64;
}

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    fn CFDictionaryCreateMutableCopy(alloc: CFTypeRef, cap: isize, d: CFTypeRef) -> CFTypeRef;
}

/// One energy counter delta between two samples.
#[derive(Debug, Clone, PartialEq)]
pub struct EnergyDelta {
    pub group: String,
    pub channel: String,
    pub unit: String,
    pub value: i64,
}

/// A live subscription to one IOReport group. Dropping it releases every CF
/// object (no thread or callback is registered).
pub struct Subscription {
    sub: Cf,
    channels: Cf,
    prev: Option<Cf>,
}

impl Subscription {
    pub fn new(group: &str) -> Result<Self, String> {
        let g = Cf::string(group);
        // SAFETY: documented libIOReport call pattern (as used by macmon /
        // asitop): copy the group's channel dictionary, make it mutable,
        // subscribe. All returned objects follow the Create rule.
        unsafe {
            let chans = Cf::from_owned(IOReportCopyChannelsInGroup(
                g.as_ptr(),
                std::ptr::null(),
                0,
                0,
                0,
            ))
            .ok_or_else(|| format!("IOReport group {group:?} not found"))?;
            let channels = Cf::from_owned(CFDictionaryCreateMutableCopy(
                std::ptr::null(),
                0,
                chans.as_ptr(),
            ))
            .ok_or("CFDictionaryCreateMutableCopy failed")?;
            let mut subscribed: CFTypeRef = std::ptr::null();
            let sub = Cf::from_owned(IOReportCreateSubscription(
                std::ptr::null(),
                channels.as_ptr(),
                &mut subscribed,
                0,
                std::ptr::null(),
            ))
            .ok_or("IOReportCreateSubscription failed")?;
            // `subscribed` (+1, ours) is the channel set actually granted;
            // samples must be requested against it.
            drop(channels);
            let channels =
                Cf::from_owned(subscribed).ok_or("IOReport subscription returned no channels")?;
            Ok(Self {
                sub,
                channels,
                prev: None,
            })
        }
    }

    /// Takes a sample; returns counter deltas since the previous call (empty
    /// on the first call, which only sets the baseline).
    pub fn sample(&mut self) -> Result<Vec<EnergyDelta>, String> {
        // SAFETY: sub/channels are the live objects created in `new`.
        let cur = unsafe {
            Cf::from_owned(IOReportCreateSamples(
                self.sub.as_ptr(),
                self.channels.as_ptr(),
                std::ptr::null(),
            ))
        }
        .ok_or("IOReportCreateSamples failed")?;
        let Some(prev) = self.prev.replace(cur.clone()) else {
            return Ok(vec![]);
        };
        // SAFETY: both are sample dictionaries from IOReportCreateSamples.
        let delta = unsafe {
            Cf::from_owned(IOReportCreateSamplesDelta(
                prev.as_ptr(),
                cur.as_ptr(),
                std::ptr::null(),
            ))
        }
        .ok_or("IOReportCreateSamplesDelta failed")?;
        let items = delta
            .dict_get("IOReportChannels")
            .map(|a| a.array_items())
            .unwrap_or_default();
        let read = |p: CFTypeRef| {
            // SAFETY: Get rule string (may be null) borrowed from `item`.
            unsafe { Cf::from_borrowed(p) }
                .and_then(|s| s.to_string_lossy())
                .unwrap_or_default()
        };
        Ok(items
            .iter()
            .map(|item| {
                let p = item.as_ptr();
                // SAFETY: `item` is a channel dictionary of the delta sample.
                unsafe {
                    EnergyDelta {
                        group: read(IOReportChannelGetGroup(p)),
                        channel: read(IOReportChannelGetChannelName(p)),
                        unit: read(IOReportChannelGetUnitLabel(p)),
                        value: IOReportSimpleGetIntegerValue(p, 0),
                    }
                }
            })
            .collect())
    }
}
