fn main() {
    // The Tauri build step (config validation, icon embedding, capability
    // codegen) only matters for the GUI shell; the headless build must not
    // depend on the tauri.conf.json-driven pipeline.
    if std::env::var_os("CARGO_FEATURE_GUI").is_some() {
        tauri_build::build();
    }
}
