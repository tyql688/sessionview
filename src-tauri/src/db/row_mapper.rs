use crate::models::{Provider, SessionMeta};

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
