//! EventKit wrapper. `Store` is used from the calendar worker thread only.

use block2::RcBlock;
use crossbeam_channel::Sender;
use nox_core::permissions::PermissionState;
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, Bool, NSObjectProtocol, ProtocolObject};
use objc2_app_kit::NSColorSpace;
use objc2_event_kit::{
    EKAuthorizationStatus, EKCalendar, EKEntityType, EKEvent, EKEventStore,
    EKEventStoreChangedNotification, EKReminder,
};
use objc2_foundation::{
    NSArray, NSCalendar, NSDate, NSError, NSNotification, NSNotificationCenter, NSString,
};
use std::ptr::NonNull;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Entity {
    Events,
    Reminders,
}

impl Entity {
    fn raw(self) -> EKEntityType {
        match self {
            Entity::Events => EKEntityType::Event,
            Entity::Reminders => EKEntityType::Reminder,
        }
    }
}

pub fn status(entity: Entity) -> PermissionState {
    // SAFETY: class method without side effects (never prompts).
    let s = unsafe { EKEventStore::authorizationStatusForEntityType(entity.raw()) };
    match s {
        EKAuthorizationStatus::FullAccess => PermissionState::Granted,
        EKAuthorizationStatus::Denied => PermissionState::Denied,
        // Write-only access cannot read events: treat as denied for display.
        EKAuthorizationStatus::WriteOnly => PermissionState::Denied,
        EKAuthorizationStatus::Restricted => PermissionState::Restricted,
        _ => PermissionState::NotDetermined,
    }
}

#[derive(Debug, Clone)]
pub struct RawCalendar {
    pub id: String,
    pub title: String,
    pub color: [f32; 3],
    pub source: String,
}

#[derive(Debug, Clone)]
pub struct RawItem {
    pub id: String,
    pub title: String,
    pub start: f64,
    pub end: f64,
    pub all_day: bool,
    pub calendar_id: String,
    pub calendar_title: String,
    pub color: [f32; 3],
    pub location: Option<String>,
    pub notes: Option<String>,
    pub url: Option<String>,
    pub cancelled: bool,
}

fn color_of(cal: &EKCalendar) -> [f32; 3] {
    // SAFETY: plain property getter.
    let c = unsafe { cal.color() };
    match c.colorUsingColorSpace(&NSColorSpace::sRGBColorSpace()) {
        Some(c) => [
            c.redComponent() as f32,
            c.greenComponent() as f32,
            c.blueComponent() as f32,
        ],
        None => [0.5, 0.5, 0.5],
    }
}

fn calendar_info(cal: &EKCalendar) -> RawCalendar {
    // SAFETY: plain property getters on a live EKCalendar.
    unsafe {
        RawCalendar {
            id: cal.calendarIdentifier().to_string(),
            title: cal.title().to_string(),
            color: color_of(cal),
            source: cal
                .source()
                .map(|s| s.title().to_string())
                .unwrap_or_default(),
        }
    }
}

fn opt(s: Option<Retained<NSString>>) -> Option<String> {
    s.map(|s| s.to_string()).filter(|s| !s.trim().is_empty())
}

pub struct Store {
    store: Retained<EKEventStore>,
}

impl Default for Store {
    fn default() -> Self {
        Self::new()
    }
}

impl Store {
    pub fn new() -> Self {
        // SAFETY: `new` creates an event store; access is governed by TCC.
        let store = unsafe { EKEventStore::new() };
        Self { store }
    }

    /// Shows the system prompt if access is not determined yet; the answer
    /// arrives on `tx` (from an EventKit queue).
    pub fn request_access(&self, entity: Entity, tx: Sender<(Entity, bool)>) {
        let block = RcBlock::new(move |granted: Bool, _err: *mut NSError| {
            let _ = tx.send((entity, granted.as_bool()));
        });
        // SAFETY: the completion block is heap-allocated and copied by EventKit.
        unsafe {
            match entity {
                Entity::Events => self
                    .store
                    .requestFullAccessToEventsWithCompletion(RcBlock::as_ptr(&block)),
                Entity::Reminders => self
                    .store
                    .requestFullAccessToRemindersWithCompletion(RcBlock::as_ptr(&block)),
            }
        }
    }

    pub fn calendars(&self, entity: Entity) -> Vec<RawCalendar> {
        // SAFETY: plain query.
        let cals = unsafe { self.store.calendarsForEntityType(entity.raw()) };
        cals.iter().map(|c| calendar_info(&c)).collect()
    }

    fn calendars_except(
        &self,
        entity: Entity,
        hidden: &dyn Fn(&RawCalendar) -> bool,
    ) -> Option<Retained<NSArray<EKCalendar>>> {
        // SAFETY: plain query.
        let cals = unsafe { self.store.calendarsForEntityType(entity.raw()) };
        let kept: Vec<Retained<EKCalendar>> =
            cals.iter().filter(|c| !hidden(&calendar_info(c))).collect();
        (!kept.is_empty()).then(|| NSArray::from_retained_slice(&kept))
    }

    /// Events overlapping `[start, end)` (Unix seconds) in visible calendars.
    pub fn events(
        &self,
        start: f64,
        end: f64,
        hidden: &dyn Fn(&RawCalendar) -> bool,
    ) -> Vec<RawItem> {
        let Some(cals) = self.calendars_except(Entity::Events, hidden) else {
            return vec![];
        };
        let s = NSDate::dateWithTimeIntervalSince1970(start);
        let e = NSDate::dateWithTimeIntervalSince1970(end);
        // SAFETY: predicate built by the store itself, as EventKit requires.
        let events = unsafe {
            let p = self
                .store
                .predicateForEventsWithStartDate_endDate_calendars(&s, &e, Some(&cals));
            self.store.eventsMatchingPredicate(&p)
        };
        events.iter().map(|ev| event_item(&ev)).collect()
    }

    /// Incomplete reminders due in `[start, end)`; delivered asynchronously.
    pub fn reminders(
        &self,
        start: f64,
        end: f64,
        hidden: &dyn Fn(&RawCalendar) -> bool,
        tx: Sender<Vec<RawItem>>,
    ) {
        let Some(cals) = self.calendars_except(Entity::Reminders, hidden) else {
            let _ = tx.send(vec![]);
            return;
        };
        let s = NSDate::dateWithTimeIntervalSince1970(start);
        let e = NSDate::dateWithTimeIntervalSince1970(end);
        let block = RcBlock::new(move |arr: *mut NSArray<EKReminder>| {
            // SAFETY: EventKit passes a valid (possibly null) array for the call.
            let items = unsafe { arr.as_ref() }
                .map(|a| a.iter().filter_map(|r| reminder_item(&r)).collect())
                .unwrap_or_default();
            let _ = tx.send(items);
        });
        // SAFETY: predicate built by the store; block copied by EventKit.
        unsafe {
            let p = self
                .store
                .predicateForIncompleteRemindersWithDueDateStarting_ending_calendars(
                    Some(&s),
                    Some(&e),
                    Some(&cals),
                );
            let _ = self
                .store
                .fetchRemindersMatchingPredicate_completion(&p, &block);
        }
    }
}

fn event_item(ev: &EKEvent) -> RawItem {
    // SAFETY: plain property getters on a live EKEvent.
    unsafe {
        let cal = ev.calendar();
        let info = cal.as_deref().map(calendar_info);
        RawItem {
            id: ev.calendarItemIdentifier().to_string(),
            title: ev.title().to_string(),
            start: ev.startDate().timeIntervalSince1970(),
            end: ev.endDate().timeIntervalSince1970(),
            all_day: ev.isAllDay(),
            calendar_id: info.as_ref().map(|c| c.id.clone()).unwrap_or_default(),
            calendar_title: info.as_ref().map(|c| c.title.clone()).unwrap_or_default(),
            color: info.map(|c| c.color).unwrap_or([0.5; 3]),
            location: opt(ev.location()),
            notes: opt(ev.notes()),
            url: ev
                .URL()
                .and_then(|u| u.absoluteString())
                .map(|s| s.to_string()),
            cancelled: ev.status() == objc2_event_kit::EKEventStatus::Canceled,
        }
    }
}

fn reminder_item(r: &EKReminder) -> Option<RawItem> {
    // SAFETY: plain property getters on a live EKReminder.
    unsafe {
        let comps = r.dueDateComponents()?;
        let date = NSCalendar::currentCalendar().dateFromComponents(&comps)?;
        let t = date.timeIntervalSince1970();
        // Components without an hour describe a whole day.
        let all_day = comps.hour() == objc2_foundation::NSDateComponentUndefined;
        let cal = r.calendar();
        let info = cal.as_deref().map(calendar_info);
        Some(RawItem {
            id: r.calendarItemIdentifier().to_string(),
            title: r.title().to_string(),
            start: t,
            end: if all_day { t + 86_400.0 } else { t },
            all_day,
            calendar_id: info.as_ref().map(|c| c.id.clone()).unwrap_or_default(),
            calendar_title: info.as_ref().map(|c| c.title.clone()).unwrap_or_default(),
            color: info.map(|c| c.color).unwrap_or([0.5; 3]),
            location: opt(r.location()),
            notes: opt(r.notes()),
            url: r
                .URL()
                .and_then(|u| u.absoluteString())
                .map(|s| s.to_string()),
            cancelled: false,
        })
    }
}

/// Removes the observer on drop.
pub struct ChangeObserver(Retained<ProtocolObject<dyn NSObjectProtocol>>);

impl Drop for ChangeObserver {
    fn drop(&mut self) {
        // SAFETY: the token was returned by `addObserverForName…` below.
        unsafe {
            NSNotificationCenter::defaultCenter()
                .removeObserver(&*(Retained::as_ptr(&self.0) as *const AnyObject))
        };
    }
}

/// Calls `f` whenever EventKit reports a change (any store, any thread).
pub fn observe_changes(f: impl Fn() + 'static) -> ChangeObserver {
    let block = RcBlock::new(move |_n: NonNull<NSNotification>| f());
    // SAFETY: immutable EventKit constant; no operation queue (runs on the
    // posting thread); the block only captures `'static` state.
    let token = unsafe {
        NSNotificationCenter::defaultCenter().addObserverForName_object_queue_usingBlock(
            Some(EKEventStoreChangedNotification),
            None,
            None,
            &block,
        )
    };
    ChangeObserver(token)
}
