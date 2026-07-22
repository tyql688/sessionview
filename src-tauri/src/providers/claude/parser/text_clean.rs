//! Small string-cleaning helpers for Claude system/local-command lines:
//! ANSI stripping, tag extraction, and command input/output extraction.

use crate::models::MessageKind;

pub(super) struct LocalCommandText {
    pub(super) kind: MessageKind,
    pub(super) content: String,
}

pub(super) fn format_local_command_text(raw: &str) -> Option<LocalCommandText> {
    let trimmed = raw.trim_start();
    if !trimmed.starts_with("<command-name>")
        && !trimmed.starts_with("<command-message>")
        && !trimmed.starts_with("<local-command-stdout>")
        && !trimmed.starts_with("<local-command-stderr>")
    {
        return None;
    }

    if let Some(command) = extract_tag_text(raw, "command-name").filter(|s| !s.is_empty()) {
        let args = extract_tag_text(raw, "command-args").unwrap_or_default();
        let detail = [command, args]
            .into_iter()
            .filter(|part| !part.is_empty())
            .collect::<Vec<_>>()
            .join(" ");
        return Some(LocalCommandText {
            kind: MessageKind::CommandInput,
            content: detail,
        });
    }

    if let Some(command) = extract_tag_text(raw, "command-message").filter(|s| !s.is_empty()) {
        return Some(LocalCommandText {
            kind: MessageKind::CommandInput,
            content: command,
        });
    }

    let stdout = extract_tag_text(raw, "local-command-stdout")
        .or_else(|| extract_tag_text(raw, "local-command-stderr"))
        .map(|value| clean_system_text(&value))
        .filter(|s| !s.is_empty())?;
    Some(LocalCommandText {
        kind: MessageKind::CommandOutput,
        content: stdout,
    })
}

fn extract_tag_text(raw: &str, tag: &str) -> Option<String> {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let start = raw.find(&open)? + open.len();
    let end = raw[start..].find(&close)? + start;
    Some(clean_system_text(&raw[start..end]))
}

/// Harness-injected teammate mail: a user-role line wrapping one or more
/// `<teammate-message teammate_id=".." summary="..">payload</teammate-message>`
/// or `<agent-message from="..">payload</agent-message>` blocks in boilerplate
/// ("Another Claude session sent a message: ... that's permission
/// laundering."). Returns one `[agent_mail]` system line per block; the
/// boilerplate is model-facing plumbing, not conversation content. A prefix
/// match without parseable blocks keeps the raw text as a single mail.
pub(super) fn extract_teammate_mail(raw: &str) -> Option<Vec<String>> {
    if !raw
        .trim_start()
        .starts_with("Another Claude session sent a message")
    {
        return None;
    }

    const TAGS: [&str; 2] = ["teammate-message", "agent-message"];
    let mut mails = Vec::new();
    let mut rest = raw;
    while let Some((start, tag)) = TAGS
        .iter()
        .filter_map(|tag| rest.find(&format!("<{tag}")).map(|at| (at, *tag)))
        .min()
    {
        let block = &rest[start..];
        let Some((open_end, body_end)) = block
            .find('>')
            .zip(block.find(&format!("</{tag}>")))
            .filter(|(open, close)| open < close)
        else {
            break;
        };
        let attrs = &block[..open_end];
        let sender =
            extract_attr_value(attrs, "teammate_id").or_else(|| extract_attr_value(attrs, "from"));
        let summary = extract_attr_value(attrs, "summary");
        let body = block[open_end + 1..body_end].trim();
        if !body.is_empty() {
            let header = match (sender, summary) {
                (Some(sender), Some(summary)) => format!("[agent_mail] {sender}: {summary}"),
                (Some(sender), None) => format!("[agent_mail] {sender}"),
                (None, Some(summary)) => format!("[agent_mail] {summary}"),
                (None, None) => "[agent_mail]".to_string(),
            };
            mails.push(format!("{header}\n{body}"));
        }
        rest = &block[body_end..];
    }

    if mails.is_empty() {
        mails.push(format!("[agent_mail]\n{}", raw.trim()));
    }
    Some(mails)
}

fn extract_attr_value(tag: &str, attr: &str) -> Option<String> {
    let marker = format!("{attr}=\"");
    let start = tag.find(&marker)? + marker.len();
    let end = tag[start..].find('"')? + start;
    Some(tag[start..end].to_string()).filter(|s| !s.is_empty())
}

pub(super) fn clean_system_text(raw: &str) -> String {
    strip_ansi_codes(raw)
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

fn strip_ansi_codes(raw: &str) -> String {
    let mut cleaned = String::new();
    let mut chars = raw.chars().peekable();

    while let Some(ch) = chars.next() {
        if ch == '\u{1b}' && chars.peek() == Some(&'[') {
            chars.next();
            for code in chars.by_ref() {
                if ('@'..='~').contains(&code) {
                    break;
                }
            }
            continue;
        }
        cleaned.push(ch);
    }

    cleaned
}
