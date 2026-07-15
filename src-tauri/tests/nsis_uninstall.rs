use std::{fs, path::Path};

#[test]
fn nsis_hook_deletes_sessionview_data_only_when_requested() {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let config_path = manifest_dir.join("tauri.conf.json");
    let config: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&config_path).expect("read tauri.conf.json"))
            .expect("parse tauri.conf.json");

    let hook_relative = config
        .pointer("/bundle/windows/nsis/installerHooks")
        .and_then(serde_json::Value::as_str)
        .expect("NSIS installer hook must be configured");
    let hook = fs::read_to_string(manifest_dir.join(hook_relative)).expect("read NSIS hook");
    let commands: Vec<&str> = hook
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with(';'))
        .collect();

    assert_eq!(
        commands,
        [
            "!macro NSIS_HOOK_POSTUNINSTALL",
            "${If} $DeleteAppDataCheckboxState = 1",
            "${AndIf} $UpdateMode <> 1",
            "RMDir /r \"$LOCALAPPDATA\\sessionview\"",
            "${EndIf}",
            "!macroend",
        ]
    );
}
