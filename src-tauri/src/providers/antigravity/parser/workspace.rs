use serde_json::Value;
use std::collections::HashMap;
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::Path;

pub(crate) fn load_history_workspaces() -> HashMap<String, String> {
    let mut map = HashMap::new();
    let Some(home) = dirs::home_dir() else {
        return map;
    };
    let history_path = home
        .join(".gemini")
        .join("antigravity-cli")
        .join("history.jsonl");

    if let Ok(file) = File::open(&history_path) {
        let reader = BufReader::new(file);
        for line in reader.lines() {
            let line_str = match line {
                Ok(line_str) => line_str,
                Err(error) => {
                    log::warn!(
                        "failed to read line from Antigravity history '{}': {error}",
                        history_path.display()
                    );
                    continue;
                }
            };
            if let Ok(val) = serde_json::from_str::<Value>(&line_str)
                && let (Some(cid), Some(ws)) = (
                    val.get("conversationId").and_then(|v| v.as_str()),
                    val.get("workspace").and_then(|v| v.as_str()),
                )
            {
                map.insert(cid.to_string(), ws.to_string());
            }
        }
    }
    map
}

pub(super) fn extract_absolute_paths_from_value(val: &Value, paths: &mut Vec<String>) {
    match val {
        Value::String(s) => {
            let trimmed = s.trim_matches('"').trim_matches('\'');
            if !trimmed.is_empty() && Path::new(trimmed).is_absolute() {
                paths.push(trimmed.to_string());
            }
        }
        Value::Array(arr) => {
            for item in arr {
                extract_absolute_paths_from_value(item, paths);
            }
        }
        Value::Object(obj) => {
            for (_, item) in obj {
                extract_absolute_paths_from_value(item, paths);
            }
        }
        _ => {}
    }
}

pub(crate) fn find_workspace_by_display_content(first_user_msg: &str) -> Option<String> {
    let home = dirs::home_dir()?;
    let history_path = home
        .join(".gemini")
        .join("antigravity-cli")
        .join("history.jsonl");

    if let Ok(file) = File::open(&history_path) {
        let reader = BufReader::new(file);
        for line in reader.lines() {
            let line_str = match line {
                Ok(line_str) => line_str,
                Err(error) => {
                    log::warn!(
                        "failed to read line from Antigravity history '{}': {error}",
                        history_path.display()
                    );
                    continue;
                }
            };
            if let Ok(val) = serde_json::from_str::<Value>(&line_str)
                && let (Some(display), Some(ws)) = (
                    val.get("display").and_then(|v| v.as_str()),
                    val.get("workspace").and_then(|v| v.as_str()),
                )
                && display.trim() == first_user_msg.trim()
            {
                return Some(ws.to_string());
            }
        }
    }
    None
}
