//! Temporary placeholder until the real module lands. Publishes nothing.

use nox_core::{Module, ModuleContext, ModuleError, ModuleId};

pub(crate) struct Placeholder(ModuleId);

impl Placeholder {
    pub(crate) fn boxed(id: ModuleId) -> Box<dyn Module> {
        Box::new(Self(id))
    }
}

impl Module for Placeholder {
    fn id(&self) -> ModuleId {
        self.0
    }
    fn start(&mut self, _ctx: ModuleContext) -> Result<(), ModuleError> {
        Err(ModuleError::NotSupported(format!(
            "{} not implemented yet",
            self.0.key()
        )))
    }
    fn stop(&mut self) {}
}
