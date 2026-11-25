//! Attachments: files encrypted on the sender's device, stored here as opaque bytes. The key travels
//! inside the (end-to-end encrypted) message, so the host can't read them. Only members of the file's
//! server can download it; files go when their channel or server is deleted.

use crate::api::require_member;
use crate::auth::Auth;
use crate::util::{new_id, now, ApiError, ApiResult};
use crate::Shared;
use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use rusqlite::{params, OptionalExtension};
use serde_json::{json, Value};
use std::time::Duration;

fn valid_id(id: &str) -> bool {
    id.len() == 24 && id.bytes().all(|b| b.is_ascii_hexdigit())
}

/// `POST /v1/channels/{channel}/blobs` with the encrypted file as the body.
pub async fn upload(State(app): State<Shared>, me: Auth, Path(channel): Path<String>, body: Bytes) -> ApiResult<Json<Value>> {
    let space: Option<String> = app.db.with(|c| c.query_row("SELECT space FROM channels WHERE id = ?1", [&channel], |r| r.get(0)).optional())?;
    let space = space.ok_or_else(|| ApiError::not_found("No such channel"))?;
    require_member(&app, &space, &me).map_err(|_| ApiError::not_found("No such channel"))?;
    if body.is_empty() || body.len() as u64 > app.cfg.max_file {
        return Err(ApiError::bad(format!("Files can be at most {} MB", app.cfg.max_file / 1024 / 1024)));
    }
    if !app.limits.allow(&format!("blob:{}", me.device), 30, Duration::from_secs(60)) {
        return Err(ApiError(StatusCode::TOO_MANY_REQUESTS, "Slow down: at most 30 files a minute".into()));
    }
    let used: i64 = app.db.with(|c| c.query_row("SELECT COALESCE(SUM(size), 0) FROM blobs WHERE space = ?1", [&space], |r| r.get(0)))?;
    if used as u64 + body.len() as u64 > app.cfg.space_quota {
        return Err(ApiError::bad("This server is out of space for files; ask its owner to delete old channels"));
    }
    let id = new_id();
    std::fs::create_dir_all(&app.cfg.blob_dir).map_err(|e| ApiError(StatusCode::INTERNAL_SERVER_ERROR, format!("storage: {e}")))?;
    let tmp = app.cfg.blob_dir.join(format!("{id}.part"));
    std::fs::write(&tmp, &body).and_then(|_| std::fs::rename(&tmp, app.cfg.blob_dir.join(&id))).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        ApiError(StatusCode::INTERNAL_SERVER_ERROR, format!("storage: {e}"))
    })?;
    app.db.with(|c| {
        c.execute(
            "INSERT INTO blobs (id, space, channel, uploader, size, created) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![id, space, channel, me.account, body.len() as i64, now()],
        )
    })?;
    Ok(Json(json!({ "id": id, "size": body.len() })))
}

/// `GET /v1/blobs/{id}`: the encrypted bytes, for members of the file's server. Served so that no
/// browser would ever run or display it as anything.
pub async fn download(State(app): State<Shared>, me: Auth, Path(id): Path<String>) -> ApiResult<Response> {
    if !valid_id(&id) {
        return Err(ApiError::not_found("No such file"));
    }
    let space: Option<String> = app.db.with(|c| c.query_row("SELECT space FROM blobs WHERE id = ?1", [&id], |r| r.get(0)).optional())?;
    let space = space.ok_or_else(|| ApiError::not_found("No such file"))?;
    require_member(&app, &space, &me).map_err(|_| ApiError::not_found("No such file"))?;
    let bytes = std::fs::read(app.cfg.blob_dir.join(&id)).map_err(|_| ApiError::not_found("No such file"))?;
    let mut h = HeaderMap::new();
    h.insert(header::CONTENT_TYPE, HeaderValue::from_static("application/octet-stream"));
    h.insert(header::CONTENT_DISPOSITION, HeaderValue::from_static("attachment"));
    h.insert("x-content-type-options", HeaderValue::from_static("nosniff"));
    h.insert(header::CONTENT_SECURITY_POLICY, HeaderValue::from_static("default-src 'none'; sandbox"));
    Ok((h, bytes).into_response())
}

/// Delete stored files that no longer belong to a channel (after channel or server deletes, at start).
pub fn sweep(app: &Shared) {
    let keep: std::collections::HashSet<String> = app
        .db
        .with(|c| c.prepare("SELECT id FROM blobs")?.query_map([], |r| r.get(0))?.collect())
        .unwrap_or_default();
    for e in std::fs::read_dir(&app.cfg.blob_dir).into_iter().flatten().flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        if !keep.contains(&name) {
            let _ = std::fs::remove_file(e.path());
        }
    }
}
