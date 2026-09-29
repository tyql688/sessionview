use serde::Deserialize;
use serde_json::Value;

pub(super) const CURRENT_SESSION_VERSION: u32 = 3;

#[derive(Debug, Deserialize)]
pub(super) struct SessionHeader {
    #[serde(rename = "type")]
    pub kind: String,
    pub version: u32,
    pub id: String,
    pub timestamp: String,
    pub cwd: String,
}

#[derive(Clone, Debug, Deserialize)]
pub(super) struct EntryBase {
    #[serde(rename = "type")]
    pub kind: String,
    pub id: String,
    #[serde(default, rename = "parentId")]
    pub parent_id: Option<String>,
    pub timestamp: String,
}

#[derive(Debug, Deserialize)]
pub(super) struct MessageEntry {
    #[serde(flatten)]
    pub base: EntryBase,
    pub message: WireMessage,
    #[serde(default)]
    pub usage: Option<Value>,
    #[serde(default)]
    pub model: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(super) struct WireMessage {
    pub role: String,
    #[serde(default)]
    pub content: Vec<Value>,
    #[serde(default)]
    pub meta: Option<Value>,
}

#[derive(Debug, Deserialize)]
pub(super) struct ModelChangeEntry {
    #[serde(flatten)]
    pub base: EntryBase,
    pub model: String,
}

#[derive(Debug, Deserialize)]
pub(super) struct SummaryEntry {
    #[serde(flatten)]
    pub base: EntryBase,
    pub summary: String,
}

#[derive(Debug, Deserialize)]
pub(super) struct CustomMessageEntry {
    #[serde(flatten)]
    pub base: EntryBase,
    #[serde(rename = "customType")]
    pub custom_type: String,
    pub content: Value,
    pub display: bool,
}

#[derive(Debug, Deserialize)]
pub(super) struct SessionInfoEntry {
    #[serde(flatten)]
    pub base: EntryBase,
    #[serde(default)]
    pub name: Option<String>,
}

#[derive(Debug)]
pub(super) enum Entry {
    Message(MessageEntry),
    ModelChange(ModelChangeEntry),
    Compaction(SummaryEntry),
    BranchSummary(SummaryEntry),
    CustomMessage(CustomMessageEntry),
    SessionInfo(SessionInfoEntry),
    Metadata(EntryBase),
}

impl Entry {
    pub fn base(&self) -> &EntryBase {
        match self {
            Self::Message(entry) => &entry.base,
            Self::ModelChange(entry) => &entry.base,
            Self::Compaction(entry) | Self::BranchSummary(entry) => &entry.base,
            Self::CustomMessage(entry) => &entry.base,
            Self::SessionInfo(entry) => &entry.base,
            Self::Metadata(base) => base,
        }
    }
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct MetaSidecar {
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub git_branch: Option<String>,
}
