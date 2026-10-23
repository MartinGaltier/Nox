//! Nox entry point (coordinator-owned wiring).

fn main() {
    let settings = nox_core::SettingsStore::default_location().load();
    println!(
        "Nox — {} sections visibles",
        settings.visible_sections().len()
    );
}
