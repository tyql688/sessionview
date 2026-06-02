use crate::db::Database;
use crate::models::SessionMeta;
use crate::provider::{DeletionPlan, SessionProvider};
use crate::services::error::{ServiceError, ServiceResult};

pub(crate) struct ResolvedDeletion {
    pub meta: SessionMeta,
    pub plan: DeletionPlan,
    pub provider: Box<dyn SessionProvider>,
}

pub(crate) fn load_session_meta(db: &Database, session_id: &str) -> ServiceResult<SessionMeta> {
    let mut meta = db
        .get_session(session_id)
        .map_err(|e| ServiceError::LoadSession(session_id.to_string(), e.to_string()))?
        .ok_or_else(|| ServiceError::SessionNotFound(session_id.to_string()))?;
    crate::providers::cc_mirror::populate_variant_name(&mut meta);
    Ok(meta)
}

pub(crate) fn load_session_for_mutation(
    db: &Database,
    session_id: &str,
) -> ServiceResult<(SessionMeta, Vec<SessionMeta>)> {
    let meta = load_session_meta(db, session_id)?;
    let children = db
        .get_child_sessions(session_id)
        .map_err(|e| ServiceError::LoadChildSessions(session_id.to_string(), e.to_string()))?;
    Ok((meta, children))
}

pub(crate) fn resolve_session_deletion(
    db: &Database,
    session_id: &str,
) -> ServiceResult<ResolvedDeletion> {
    let (meta, children) = load_session_for_mutation(db, session_id)?;
    let provider = meta.provider.require_runtime()?;
    let plan = provider.deletion_plan(&meta, &children);

    Ok(ResolvedDeletion {
        meta,
        plan,
        provider,
    })
}
