//! IOPM power assertions (idle-sleep prevention only — they do NOT keep a
//! MacBook awake when its lid closes without an external display).

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AssertionKind {
    /// `PreventUserIdleSystemSleep`: the Mac stays awake, the display may sleep.
    System,
    /// `PreventUserIdleDisplaySleep`: the display stays on too.
    Display,
}

impl AssertionKind {
    pub fn iokit_type(self) -> &'static str {
        match self {
            Self::System => "PreventUserIdleSystemSleep",
            Self::Display => "PreventUserIdleDisplaySleep",
        }
    }
    /// Shown by `pmset -g assertions` (always starts with "Nox").
    pub fn name(self) -> &'static str {
        match self {
            Self::System => "Nox: keep Mac awake",
            Self::Display => "Nox: keep display awake",
        }
    }
}

pub type AssertionId = u32;

pub trait Assertions: Send {
    fn create(&mut self, kind: AssertionKind) -> Result<AssertionId, String>;
    fn release(&mut self, id: AssertionId) -> Result<(), String>;
}

/// The real IOKit implementation.
#[derive(Default)]
pub struct IoPmAssertions;

impl Assertions for IoPmAssertions {
    fn create(&mut self, kind: AssertionKind) -> Result<AssertionId, String> {
        crate::sys::create_assertion(kind.iokit_type(), kind.name())
    }
    fn release(&mut self, id: AssertionId) -> Result<(), String> {
        crate::sys::release_assertion(id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    fn assertions_report() -> String {
        let out = Command::new("/usr/bin/pmset")
            .args(["-g", "assertions"])
            .output()
            .expect("pmset -g assertions");
        String::from_utf8_lossy(&out.stdout).into_owned()
    }

    fn own_lines(report: &str) -> Vec<String> {
        let me = format!("pid {}(", std::process::id());
        report
            .lines()
            .filter(|l| l.contains(&me))
            .map(str::to_owned)
            .collect()
    }

    /// Real assertions on this Mac (no root needed). Creates each kind, checks
    /// `pmset -g assertions` lists it under this pid with the Nox name,
    /// releases, checks it is gone.
    #[test]
    #[ignore = "touches the real power management state; run manually: cargo test -p nox-power -- --ignored --nocapture"]
    fn real_assertions_appear_and_disappear() {
        let mut a = IoPmAssertions;
        for kind in [AssertionKind::System, AssertionKind::Display] {
            let id = a.create(kind).expect("create assertion");
            let report = assertions_report();
            let lines = own_lines(&report);
            eprintln!("after create {kind:?}: {lines:#?}");
            assert!(
                lines
                    .iter()
                    .any(|l| l.contains(kind.iokit_type()) && l.contains(kind.name())),
                "assertion {kind:?} missing from pmset -g assertions"
            );
            a.release(id).expect("release assertion");
            let lines = own_lines(&assertions_report());
            eprintln!("after release {kind:?}: {lines:#?}");
            assert!(
                !lines.iter().any(|l| l.contains(kind.name())),
                "assertion {kind:?} still listed after release"
            );
        }
    }
}
