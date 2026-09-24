use crate::models::{Provider, SessionMeta};

/// The columns `row_to_session_meta` reads, in position order. Select them
/// from `sessions s`; extra columns may follow from index 20.
pub(crate) const SESSION_META_COLUMNS: &str =
    "s.id, s.provider, s.title, s.project_path, s.project_name,
     s.created_at, s.updated_at, s.message_count, s.file_size_bytes, s.source_path,
     s.is_sidechain, s.variant_name, s.model, s.cc_version, s.git_branch, s.parent_id,
     s.input_tokens, s.output_tokens, s.cache_read_tokens, s.cache_write_tokens";

pub(crate) fn row_to_session_meta(row: &rusqlite::Row) -> rusqlite::Result<SessionMeta> {
    let provider = row.get::<_, String>(1)?;
    Ok(SessionMeta {
        id: row.get(0)?,
        provider: str_to_provider(&provider)?,
        title: row.get(2)?,
        project_path: row.get(3)?,
        project_name: row.get(4)?,
        created_at: row.get(5)?,
        updated_at: row.get(6)?,
        message_count: row.get(7)?,
        file_size_bytes: row.get(8)?,
        source_path: row.get(9)?,
        is_sidechain: row.get::<_, i64>(10)? != 0,
        variant_name: row.get(11)?,
        model: row.get(12)?,
        cc_version: row.get(13)?,
        git_branch: row.get(14)?,
        parent_id: row.get(15)?,
        input_tokens: row.get(16)?,
        output_tokens: row.get(17)?,
        cache_read_tokens: row.get(18)?,
        cache_write_tokens: row.get(19)?,
    })
}

fn str_to_provider(s: &str) -> rusqlite::Result<Provider> {
    Provider::parse(s).ok_or_else(|| {
        rusqlite::Error::FromSqlConversionFailure(
            1,
            rusqlite::types::Type::Text,
            format!("unknown provider: '{s}'").into(),
        )
    })
}
