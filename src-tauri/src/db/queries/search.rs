use rusqlite::Connection;

use crate::models::{SearchFilters, SearchResult, SessionMeta};

use super::super::row_mapper::row_to_session_meta;

const LIKE_SNIPPET_CONTEXT_CHARS: usize = 80;

const LIKE_SNIPPET_MAX_CHARS: usize = 200;

pub(super) fn list_sessions_from_query<P>(
    conn: &Connection,
    sql: &str,
    params: P,
) -> Result<Vec<SessionMeta>, rusqlite::Error>
where
    P: rusqlite::Params,
{
    let mut stmt = conn.prepare(sql)?;
    let rows = stmt.query_map(params, row_to_session_meta)?;

    let mut sessions = Vec::new();
    for row in rows {
        sessions.push(row?);
    }
    Ok(sessions)
}

pub(super) fn search_with_fts(
    conn: &Connection,
    filters: &SearchFilters,
    query: &str,
) -> Result<Vec<SearchResult>, rusqlite::Error> {
    let mut sql = String::from(
        "SELECT s.id, s.provider, s.title, s.project_path, s.project_name,
                s.created_at, s.updated_at, s.message_count, s.file_size_bytes, s.source_path, s.is_sidechain,
                s.variant_name, s.model, s.cc_version, s.git_branch, s.parent_id,
                    input_tokens,
                    output_tokens,
                    cache_read_tokens,
                    cache_write_tokens,
                snippet(sessions_fts, -1, '<mark>', '</mark>', '...', 64) AS snip
         FROM sessions_fts
         JOIN sessions s ON s.rowid = sessions_fts.rowid
         WHERE sessions_fts MATCH ?",
    );
    let mut param_values: Vec<Box<dyn rusqlite::types::ToSql>> = vec![Box::new(query.to_string())];
    append_search_filters(&mut sql, &mut param_values, filters);
    sql.push_str(" ORDER BY bm25(sessions_fts, 10.0, 1.0, 5.0) LIMIT 100");
    query_search_results(conn, &sql, &param_values)
}

pub(super) fn search_with_like(
    conn: &Connection,
    filters: &SearchFilters,
) -> Result<Vec<SearchResult>, rusqlite::Error> {
    let raw = filters.query.trim().to_string();
    // Treat the whole query as one literal substring — "search exactly what you
    // typed" — instead of splitting into AND-ed tokens. So "347 测试" matches a
    // contiguous "347 测试", not any row that merely contains "347" and "测试"
    // separately. FTS serves >= 3-char queries; this LIKE path serves the
    // short/CJK ones (and any FTS failure) with the same literal semantics.
    let tokens: Vec<String> = if raw.is_empty() {
        Vec::new()
    } else {
        vec![raw.clone()]
    };

    let mut sql = String::from(
        "SELECT s.id, s.provider, s.title, s.project_path, s.project_name,
                s.created_at, s.updated_at, s.message_count, s.file_size_bytes, s.source_path, s.is_sidechain,
                s.variant_name, s.model, s.cc_version, s.git_branch, s.parent_id,
                    input_tokens,
                    output_tokens,
                    cache_read_tokens,
                    cache_write_tokens,
                CASE
                    WHEN ?1 <> '' THEN substr(s.content_text, 1, 200)
                    ELSE ''
                END AS snip,
                s.title AS like_title,
                s.content_text AS like_content_text,
                s.project_name AS like_project_name
         FROM sessions s
         WHERE 1=1",
    );
    let mut param_values: Vec<Box<dyn rusqlite::types::ToSql>> = vec![Box::new(raw.clone())];
    let mut token_indices: Vec<usize> = Vec::new();

    for token in &tokens {
        let idx = param_values.len() + 1;
        token_indices.push(idx);
        sql.push_str(&format!(
            " AND (
                s.title LIKE '%' || ?{idx} || '%'
                OR s.content_text LIKE '%' || ?{idx} || '%'
                OR s.project_name LIKE '%' || ?{idx} || '%'
            )"
        ));
        param_values.push(Box::new(token.clone()));
    }

    let next_index = param_values.len() + 1;
    append_search_filters_numbered(&mut sql, &mut param_values, filters, next_index);
    // Rank by approximate relevance, not pure recency. The LIKE fallback serves
    // every short/CJK query (e.g. 2-char Chinese terms), and ordering it purely
    // by created_at meant the strongest textual match could be pushed past the
    // 100-row cap and dropped. Title matches now outrank project matches, which
    // outrank content-only matches, with recency breaking ties.
    sql.push_str(&like_relevance_order_by(&token_indices));
    sql.push_str(" LIMIT 100");
    query_like_search_results(conn, &sql, &param_values, &tokens)
}

/// Build an `ORDER BY` clause approximating relevance for the LIKE fallback:
/// title matches (tier 0) before project-name matches (tier 1) before
/// content-only matches (tier 2), with `created_at DESC` breaking ties. Falls
/// back to pure recency for filter-only queries that carry no tokens.
fn like_relevance_order_by(token_indices: &[usize]) -> String {
    if token_indices.is_empty() {
        return " ORDER BY s.created_at DESC".to_string();
    }
    let title_match = column_matches_any("s.title", token_indices);
    let project_match = column_matches_any("s.project_name", token_indices);
    format!(
        " ORDER BY CASE WHEN {title_match} THEN 0 WHEN {project_match} THEN 1 ELSE 2 END, s.created_at DESC"
    )
}

/// `col LIKE '%t1%' OR col LIKE '%t2%' ...` over the given bound parameter
/// indices. Reuses the same `?N` placeholders already bound for the WHERE
/// clause (SQLite allows a numbered parameter to appear more than once).
fn column_matches_any(column: &str, token_indices: &[usize]) -> String {
    token_indices
        .iter()
        .map(|idx| format!("{column} LIKE '%' || ?{idx} || '%'"))
        .collect::<Vec<_>>()
        .join(" OR ")
}

fn append_search_filters(
    sql: &mut String,
    param_values: &mut Vec<Box<dyn rusqlite::types::ToSql>>,
    filters: &SearchFilters,
) {
    if let Some(ref provider) = filters.provider {
        sql.push_str(" AND s.provider = ?");
        param_values.push(Box::new(provider.clone()));
    }
    if let Some(ref project) = filters.project {
        sql.push_str(" AND s.project_name LIKE '%' || ? || '%'");
        param_values.push(Box::new(project.clone()));
    }
    if let Some(after) = filters.after {
        sql.push_str(" AND s.created_at > ?");
        param_values.push(Box::new(after));
    }
    if let Some(before) = filters.before {
        sql.push_str(" AND s.created_at < ?");
        param_values.push(Box::new(before));
    }
}

fn append_search_filters_numbered(
    sql: &mut String,
    param_values: &mut Vec<Box<dyn rusqlite::types::ToSql>>,
    filters: &SearchFilters,
    mut next_index: usize,
) {
    if let Some(ref provider) = filters.provider {
        sql.push_str(&format!(" AND s.provider = ?{next_index}"));
        param_values.push(Box::new(provider.clone()));
        next_index += 1;
    }
    if let Some(ref project) = filters.project {
        sql.push_str(&format!(
            " AND s.project_name LIKE '%' || ?{next_index} || '%'"
        ));
        param_values.push(Box::new(project.clone()));
        next_index += 1;
    }
    if let Some(after) = filters.after {
        sql.push_str(&format!(" AND s.created_at > ?{next_index}"));
        param_values.push(Box::new(after));
        next_index += 1;
    }
    if let Some(before) = filters.before {
        sql.push_str(&format!(" AND s.created_at < ?{next_index}"));
        param_values.push(Box::new(before));
    }
}

fn query_search_results(
    conn: &Connection,
    sql: &str,
    param_values: &[Box<dyn rusqlite::types::ToSql>],
) -> Result<Vec<SearchResult>, rusqlite::Error> {
    let mut stmt = conn.prepare(sql)?;
    let params_refs: Vec<&dyn rusqlite::types::ToSql> = param_values
        .iter()
        .map(std::convert::AsRef::as_ref)
        .collect();
    let rows = stmt.query_map(params_refs.as_slice(), |row| {
        Ok(SearchResult {
            session: row_to_session_meta(row)?,
            snippet: row.get(20)?,
        })
    })?;

    let mut results = Vec::new();
    for row in rows {
        results.push(row?);
    }
    Ok(results)
}

fn query_like_search_results(
    conn: &Connection,
    sql: &str,
    param_values: &[Box<dyn rusqlite::types::ToSql>],
    tokens: &[String],
) -> Result<Vec<SearchResult>, rusqlite::Error> {
    let mut stmt = conn.prepare(sql)?;
    let params_refs: Vec<&dyn rusqlite::types::ToSql> = param_values
        .iter()
        .map(std::convert::AsRef::as_ref)
        .collect();
    let rows = stmt.query_map(params_refs.as_slice(), |row| {
        let fallback_snippet: String = row.get(20)?;
        let title: String = row.get(21)?;
        let content_text: String = row.get(22)?;
        let project_name: String = row.get(23)?;
        let snippet = build_like_snippet(&title, &content_text, &project_name, tokens)
            .unwrap_or(fallback_snippet);

        Ok(SearchResult {
            session: row_to_session_meta(row)?,
            snippet,
        })
    })?;

    let mut results = Vec::new();
    for row in rows {
        results.push(row?);
    }
    Ok(results)
}

fn build_like_snippet(
    title: &str,
    content_text: &str,
    project_name: &str,
    tokens: &[String],
) -> Option<String> {
    if tokens.is_empty() {
        return Some(String::new());
    }

    for source in [title, content_text, project_name] {
        if source.trim().is_empty() {
            continue;
        }
        if let Some(match_start) = find_first_like_match(source, tokens) {
            return Some(snippet_around_match(source, match_start, tokens));
        }
    }

    None
}

fn snippet_around_match(source: &str, match_byte_start: usize, tokens: &[String]) -> String {
    let total_chars = source.chars().count();
    if total_chars <= LIKE_SNIPPET_MAX_CHARS {
        return highlight_like_tokens(source, tokens);
    }

    let match_char_start = source[..match_byte_start].chars().count();
    let mut start_char = match_char_start.saturating_sub(LIKE_SNIPPET_CONTEXT_CHARS);
    let mut end_char = (start_char + LIKE_SNIPPET_MAX_CHARS).min(total_chars);
    if end_char == total_chars {
        start_char = total_chars.saturating_sub(LIKE_SNIPPET_MAX_CHARS);
        end_char = total_chars;
    }

    let start_byte = byte_index_for_char(source, start_char);
    let end_byte = byte_index_for_char(source, end_char);
    let mut snippet = String::new();
    if start_char > 0 {
        snippet.push_str("...");
    }
    snippet.push_str(&source[start_byte..end_byte]);
    if end_char < total_chars {
        snippet.push_str("...");
    }

    highlight_like_tokens(&snippet, tokens)
}

fn byte_index_for_char(source: &str, char_index: usize) -> usize {
    if char_index == 0 {
        return 0;
    }

    source
        .char_indices()
        .nth(char_index)
        .map(|(idx, _)| idx)
        .unwrap_or(source.len())
}

fn find_first_like_match(source: &str, tokens: &[String]) -> Option<usize> {
    tokens
        .iter()
        .filter(|token| !token.is_empty())
        .filter_map(|token| find_like_match(source, token))
        .min()
}

fn find_like_match(source: &str, token: &str) -> Option<usize> {
    source.find(token).or_else(|| {
        if token.is_ascii() {
            source
                .to_ascii_lowercase()
                .find(&token.to_ascii_lowercase())
        } else {
            None
        }
    })
}

fn highlight_like_tokens(snippet: &str, tokens: &[String]) -> String {
    let mut ranges = Vec::new();
    for token in tokens {
        collect_like_match_ranges(snippet, token, &mut ranges);
    }
    ranges.sort_by(|a, b| {
        let a_len = a.1 - a.0;
        let b_len = b.1 - b.0;
        a.0.cmp(&b.0).then_with(|| b_len.cmp(&a_len))
    });

    let mut selected = Vec::new();
    let mut covered_until = 0;
    for (start, end) in ranges {
        if start >= covered_until {
            selected.push((start, end));
            covered_until = end;
        }
    }

    if selected.is_empty() {
        return snippet.to_string();
    }

    let mut highlighted = String::with_capacity(snippet.len() + selected.len() * 13);
    let mut cursor = 0;
    for (start, end) in selected {
        highlighted.push_str(&snippet[cursor..start]);
        highlighted.push_str("<mark>");
        highlighted.push_str(&snippet[start..end]);
        highlighted.push_str("</mark>");
        cursor = end;
    }
    highlighted.push_str(&snippet[cursor..]);
    highlighted
}

fn collect_like_match_ranges(snippet: &str, token: &str, ranges: &mut Vec<(usize, usize)>) {
    if token.is_empty() {
        return;
    }

    if token.is_ascii() {
        let haystack = snippet.to_ascii_lowercase();
        let needle = token.to_ascii_lowercase();
        let mut offset = 0;
        while let Some(relative_start) = haystack[offset..].find(&needle) {
            let start = offset + relative_start;
            let end = start + token.len();
            ranges.push((start, end));
            offset = end;
        }
        return;
    }

    let mut offset = 0;
    while let Some(relative_start) = snippet[offset..].find(token) {
        let start = offset + relative_start;
        let end = start + token.len();
        ranges.push((start, end));
        offset = end;
    }
}

pub(super) fn build_fts_query(raw: &str) -> Option<String> {
    // Match the WHOLE query as one literal phrase (a contiguous substring), not
    // AND-ed tokens — "search exactly what you typed". With the trigram
    // tokenizer a quoted phrase matches the contiguous text (spaces included).
    // Trigram needs >= 3 codepoints; shorter queries (including 2-char CJK)
    // return None so the caller falls back to LIKE, which applies the same
    // literal substring match.
    let trimmed = raw.trim();
    if trimmed.chars().count() < 3 {
        return None;
    }
    Some(format!("\"{}\"", trimmed.replace('"', "\"\"")))
}
