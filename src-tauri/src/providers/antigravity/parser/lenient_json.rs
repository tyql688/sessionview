use serde_json::Value;

/// Split free-form text into the top-level `{...}` JSON object substrings it
/// contains. Honors string literals and `\"` escapes so braces inside strings
/// don't break the bracket count. Unterminated objects (truncated logs) are
/// silently dropped; callers should expect at most one warning per call from
/// the surrounding parser.
pub(super) fn extract_top_level_json_objects(input: &str) -> Vec<String> {
    let mut out = Vec::new();
    let bytes = input.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] != b'{' {
            i += 1;
            continue;
        }
        let start = i;
        let mut depth = 0i32;
        let mut in_string = false;
        let mut escape = false;
        while i < bytes.len() {
            let b = bytes[i];
            if in_string {
                if escape {
                    escape = false;
                } else if b == b'\\' {
                    escape = true;
                } else if b == b'"' {
                    in_string = false;
                }
            } else {
                match b {
                    b'"' => in_string = true,
                    b'{' => depth += 1,
                    b'}' => {
                        depth -= 1;
                        if depth == 0 {
                            i += 1;
                            if let Ok(slice) = std::str::from_utf8(&bytes[start..i]) {
                                out.push(slice.to_string());
                            }
                            break;
                        }
                    }
                    _ => {}
                }
            }
            i += 1;
        }
        if depth != 0 {
            // Unterminated object — skip the rest of the input.
            break;
        }
    }
    out
}

/// Extract per-subagent Prompt strings from antigravity's `invoke_subagent`
/// tool arguments. Returns one entry per subagent in declaration order; the
/// caller zips this with the conversationIds emitted by the matching
/// `INVOKE_SUBAGENT` result, so positional alignment matters.
///
/// Antigravity ships the prompts as `tc.args["Subagents"]` — a JSON-encoded
/// string holding an array of `{Prompt, TypeName, …}` objects. The encoding
/// is *almost* JSON but riddled with invalid escapes (literal `` \` ``,
/// unescaped control chars, etc.) so `serde_json` refuses to parse it. We
/// fall through to a lenient substring scan: find each `"Prompt"` key, read
/// the string value that follows (honoring `\"` escapes), and un-escape the
/// common sequences. Anything we can't decode gets returned as a best-effort
/// substring — better than a missing label.
pub(super) fn invoke_subagent_prompts(subagents_value: Option<&Value>) -> Vec<String> {
    let Some(value) = subagents_value else {
        return Vec::new();
    };
    match value {
        Value::Array(arr) => arr
            .iter()
            .map(|sub| {
                sub.get("Prompt")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string()
            })
            .collect(),
        Value::String(raw) => extract_prompts_lenient(raw),
        _ => Vec::new(),
    }
}

/// Scan a (likely malformed) JSON-encoded subagents string for `"Prompt"`
/// values without going through `serde_json`. We treat the input as raw
/// bytes, track string literals by their unescaped boundary `"`, and
/// un-escape the common JSON escape sequences in the extracted value.
fn extract_prompts_lenient(raw: &str) -> Vec<String> {
    const KEY: &str = "\"Prompt\"";
    let mut out = Vec::new();
    let bytes = raw.as_bytes();
    let mut cursor = 0usize;
    while let Some(rel) = raw[cursor..].find(KEY) {
        let key_end = cursor + rel + KEY.len();
        // Skip whitespace + the `:` separator.
        let mut i = key_end;
        while i < bytes.len() && (bytes[i] as char).is_whitespace() {
            i += 1;
        }
        if i >= bytes.len() || bytes[i] != b':' {
            cursor = key_end;
            continue;
        }
        i += 1;
        while i < bytes.len() && (bytes[i] as char).is_whitespace() {
            i += 1;
        }
        if i >= bytes.len() || bytes[i] != b'"' {
            cursor = key_end;
            continue;
        }
        let value_start = i + 1;
        // Walk the string body until an unescaped `"`. Track an explicit
        // escape flag so a trailing `\` before the closing quote can't make
        // us overshoot into the next field (which used to drop a prompt).
        let mut j = value_start;
        let mut escape = false;
        while j < bytes.len() {
            let b = bytes[j];
            if escape {
                escape = false;
                j += 1;
                continue;
            }
            match b {
                b'\\' => {
                    escape = true;
                    j += 1;
                }
                b'"' => break,
                _ => j += 1,
            }
        }
        if j >= bytes.len() {
            // Truncated value (no closing quote) — record what we have and
            // stop scanning so we don't false-positive on later fields.
            out.push(unescape_json_literals(&raw[value_start..]));
            break;
        }
        out.push(unescape_json_literals(&raw[value_start..j]));
        cursor = j + 1;
    }
    out
}

/// Cheap un-escaper for the handful of sequences we actually care about in
/// extracted prompts. Anything we don't recognise gets passed through with
/// the backslash preserved (so a literal `` \` `` round-trips faithfully).
fn unescape_json_literals(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut chars = input.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('n') => out.push('\n'),
            Some('r') => out.push('\r'),
            Some('t') => out.push('\t'),
            Some('"') => out.push('"'),
            Some('\\') => out.push('\\'),
            Some('/') => out.push('/'),
            Some(other) => {
                out.push('\\');
                out.push(other);
            }
            None => out.push('\\'),
        }
    }
    out
}

/// Maximum depth for `decode_antigravity_value`. Real antigravity payloads
/// wrap each leaf at most twice (the outer args layer plus one JSON-encoded
/// literal); anything deeper is data corruption or an adversarial payload.
const MAX_DECODE_DEPTH: usize = 6;

/// Antigravity stores every tool-call argument as a *JSON-encoded string* —
/// even booleans and numbers come in as `"true"` / `"2000"`, and strings
/// arrive double-quoted (`"\"/foo\""`). The shape is unusable downstream:
/// `input_summary` would treat the literal quotes as part of the path, and
/// the JSON we persist for the UI looks like garbage.
///
/// This walk decodes each `Value::String` once via `serde_json::from_str`
/// and substitutes the parsed value when decoding succeeds. Strings that
/// aren't valid JSON literals (rare — only happens with malformed steps)
/// fall through unchanged so we never silently lose information.
///
/// Bounded by [`MAX_DECODE_DEPTH`] so pathological deeply-nested literals
/// (`"\"\\\"\\\\\\\"…\\\"\\\\\\\"\\\"\""`) can't blow the stack.
///
/// Before each `from_str` we pre-escape literal control characters (raw
/// `\n`, `\t`, …) into their JSON escapes. Antigravity's `invoke_subagent`
/// embeds multi-line prompts as JSON-encoded array strings without escaping
/// the inner newlines, which `serde_json::from_str` rejects per RFC 8259;
/// the pre-escape lets us round-trip those payloads instead of giving up.
pub(super) fn decode_antigravity_value(value: &Value) -> Value {
    fn try_decode_string(raw: &str) -> Option<Value> {
        if let Ok(parsed) = serde_json::from_str::<Value>(raw) {
            return Some(parsed);
        }
        // Retry with control characters escaped (e.g. literal 0x0A → `\n`).
        // Only meaningful when the string looks JSON-shaped, so we cheaply
        // skip pure prose to avoid unnecessary allocation.
        let trimmed = raw.trim_start();
        let first = trimmed.chars().next()?;
        if !matches!(first, '"' | '[' | '{') {
            return None;
        }
        let escaped = escape_control_chars_for_json(raw);
        if escaped != raw
            && let Ok(parsed) = serde_json::from_str::<Value>(&escaped)
        {
            return Some(parsed);
        }
        // Last-resort lenient unwrap: agy occasionally truncates the
        // inner double-encoded payload mid-string (`...<truncated N
        // bytes>` marker, no closing `"`). serde_json refuses such
        // input, so for any value that *looks* like an outer
        // JSON-encoded string but won't round-trip, manually strip the
        // outer quotes and decode the common escape sequences. This
        // produces readable diff content for truncated Edit tool calls
        // instead of leaving literal `"` / `\n` glyphs in the UI.
        if first == '"' {
            return Some(Value::String(lenient_unwrap_json_string(raw)));
        }
        None
    }

    fn walk(value: &Value, depth: usize) -> Value {
        if depth >= MAX_DECODE_DEPTH {
            return value.clone();
        }
        match value {
            Value::String(raw) => match try_decode_string(raw) {
                Some(decoded) => walk(&decoded, depth + 1),
                None => value.clone(),
            },
            Value::Array(items) => {
                Value::Array(items.iter().map(|item| walk(item, depth + 1)).collect())
            }
            Value::Object(map) => {
                let mut next = serde_json::Map::with_capacity(map.len());
                for (key, val) in map {
                    next.insert(key.clone(), walk(val, depth + 1));
                }
                Value::Object(next)
            }
            _ => value.clone(),
        }
    }
    walk(value, 0)
}

fn escape_control_chars_for_json(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 8);
    for ch in s.chars() {
        match ch {
            '\x08' => out.push_str("\\b"),
            '\t' => out.push_str("\\t"),
            '\n' => out.push_str("\\n"),
            '\x0C' => out.push_str("\\f"),
            '\r' => out.push_str("\\r"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out
}

/// Lenient fallback for the truncation-marker case in
/// `try_decode_string`. Strips the outer `"..."` quotes (if present)
/// and unescapes the common JSON escapes by hand so the diff renderer
/// gets readable text instead of a literal `"`+`\n` salad.
///
/// Not a full JSON string parser — intentionally narrow. Bogus escapes
/// pass through unchanged so we never silently lose information.
fn lenient_unwrap_json_string(raw: &str) -> String {
    let mut inner = raw;
    if let Some(stripped) = inner.strip_prefix('"') {
        inner = stripped;
    }
    // Only strip the trailing quote when it actually closes the string —
    // for the truncated case it's missing and we keep all the bytes.
    if inner.ends_with('"') && !inner.ends_with("\\\"") {
        inner = &inner[..inner.len() - 1];
    }

    let mut out = String::with_capacity(inner.len());
    let mut chars = inner.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.peek() {
            Some(&'n') => {
                out.push('\n');
                chars.next();
            }
            Some(&'t') => {
                out.push('\t');
                chars.next();
            }
            Some(&'r') => {
                out.push('\r');
                chars.next();
            }
            Some(&'"') => {
                out.push('"');
                chars.next();
            }
            Some(&'\\') => {
                out.push('\\');
                chars.next();
            }
            Some(&'/') => {
                out.push('/');
                chars.next();
            }
            _ => out.push('\\'),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn decode_antigravity_value_unwraps_typical_double_encoding() {
        // The common case agy emits: every leaf is wrapped in literal `"..."`.
        // After decode, paths/numbers/bools come back as their natural types.
        let input = json!({
            "AbsolutePath": "\"/tmp/x\"",
            "StartLine": "1",
            "IsSkill": "false",
            "Nested": {
                "TargetContent": "\"old text\""
            }
        });
        let out = decode_antigravity_value(&input);
        assert_eq!(out["AbsolutePath"], json!("/tmp/x"));
        assert_eq!(out["StartLine"], json!(1));
        assert_eq!(out["IsSkill"], json!(false));
        assert_eq!(out["Nested"]["TargetContent"], json!("old text"));
    }

    #[test]
    fn decode_antigravity_value_caps_recursion_depth() {
        // Build a string that *looks* like a JSON literal at every level: each
        // outer string contains another JSON string. The naïve recursion would
        // keep peeling layers forever; the guard must stop at MAX_DECODE_DEPTH.
        //
        // We use a manually-built nesting (cheap — depth N grows linearly in
        // bytes, not exponentially like re-`to_string`-ing would).
        let leaf = "\"deep\"";
        let mut layer = leaf.to_string();
        for _ in 0..(MAX_DECODE_DEPTH + 5) {
            layer = format!("\"{}\"", layer.replace('"', "\\\""));
        }
        // The depth-limit guard returns *some* Value without recursing
        // unboundedly — the test just needs to terminate.
        let _ = decode_antigravity_value(&Value::String(layer));
    }

    #[test]
    fn decode_antigravity_value_passes_through_non_json_strings() {
        let input = json!({ "note": "this is just text, not JSON" });
        let out = decode_antigravity_value(&input);
        assert_eq!(out["note"], json!("this is just text, not JSON"));
    }

    #[test]
    fn decode_antigravity_value_lenient_unwraps_truncated_payload() {
        // Real agy bug: large `replace_file_content` payloads get
        // truncated mid-string by agy itself, leaving an opening `"`
        // and a `<truncated N bytes>` marker but no closing `"`.
        // serde_json refuses such input; the lenient fallback should
        // still hand us readable text (no leading `"`, no literal
        // `\n` glyphs) so the Edit diff renders properly.
        let truncated = "\"fn build_codex_runtime() {\\n    let x = 1;\\n    println!(\\\"hi\\\");\\n}\\n<truncated 1929 bytes>";
        let input = json!({ "ReplacementContent": truncated });
        let out = decode_antigravity_value(&input);
        let decoded = out["ReplacementContent"].as_str().expect("string");
        assert!(
            !decoded.starts_with('"'),
            "leading quote should be stripped, got: {decoded:?}"
        );
        assert!(
            decoded.contains("fn build_codex_runtime() {\n"),
            "literal `\\n` should be decoded to a real newline, got: {decoded:?}"
        );
        assert!(
            decoded.contains("println!(\"hi\");"),
            "escaped quotes should be unescaped, got: {decoded:?}"
        );
        assert!(
            decoded.contains("<truncated 1929 bytes>"),
            "truncation marker should survive so users see why content is short, got: {decoded:?}"
        );
    }

    #[test]
    fn invoke_subagent_prompts_handles_decoded_array() {
        let value = json!([
            { "Prompt": "first", "TypeName": "research" },
            { "Prompt": "second", "TypeName": "research" },
        ]);
        let prompts = invoke_subagent_prompts(Some(&value));
        assert_eq!(prompts, vec!["first".to_string(), "second".to_string()]);
    }

    #[test]
    fn invoke_subagent_prompts_handles_malformed_json_string() {
        // Real agy payload — `[{"Prompt":"...","TypeName":"..."}]` as a single
        // JSON-encoded string with embedded raw newlines and invalid escapes
        // like `\`backticks. serde_json can't parse this; the lenient scanner
        // must still recover every Prompt value in order.
        let raw = "[{\"Prompt\":\"analyze `core.py`\\nstep 1\",\"TypeName\":\"r\"},\
                   {\"Prompt\":\"second prompt\",\"TypeName\":\"r\"},\
                   {\"Prompt\":\"third\",\"TypeName\":\"r\"}]";
        let value = Value::String(raw.to_string());
        let prompts = invoke_subagent_prompts(Some(&value));
        assert_eq!(prompts.len(), 3);
        assert!(prompts[0].contains("analyze `core.py`"));
        assert_eq!(prompts[1], "second prompt");
        assert_eq!(prompts[2], "third");
    }

    #[test]
    fn invoke_subagent_prompts_does_not_overshoot_on_trailing_backslash() {
        // A Prompt value ending in `\\` followed by the closing `"` used to
        // make the naive walker skip past the closing quote and absorb the
        // next field, dropping subsequent prompts.
        let raw = r#"[{"Prompt":"ends with backslash \\","TypeName":"r"},{"Prompt":"second","TypeName":"r"}]"#;
        let value = Value::String(raw.to_string());
        let prompts = invoke_subagent_prompts(Some(&value));
        assert_eq!(prompts.len(), 2);
        assert_eq!(prompts[1], "second");
    }
}
