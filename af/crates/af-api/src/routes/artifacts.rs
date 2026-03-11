use axum::body::Body;
use axum::extract::{Multipart, Path, State};
use axum::http::header;
use axum::response::IntoResponse;
use axum::Json;
use af_auth::Action;
use sha2::{Digest, Sha256};
use std::io::Write;
use std::sync::Arc;
use tokio::io::AsyncWriteExt;
use uuid::Uuid;

use crate::auth::{require_project_access, AuthenticatedUser};
use crate::dto::{ArtifactResponse, UpdateArtifactDescriptionRequest, UploadArtifactResponse};
use crate::error::ApiError;
use crate::AppState;

pub async fn upload(
    State(state): State<Arc<AppState>>,
    AuthenticatedUser(identity): AuthenticatedUser,
    Path(project_id): Path<Uuid>,
    mut multipart: Multipart,
) -> Result<Json<UploadArtifactResponse>, ApiError> {
    let user = AuthenticatedUser(identity);

    // Auth check in short-lived scoped tx (entity lookup inside tx to prevent enumeration)
    {
        let mut tx = if user.is_admin() {
            state.pool.begin().await?
        } else {
            let uid = user
                .user_id()
                .ok_or_else(|| ApiError::Forbidden("no user_id on identity".into()))?;
            af_db::scoped::begin_scoped(&state.pool, uid).await?
        };
        af_db::projects::get_project(&mut *tx, project_id)
            .await?
            .ok_or_else(|| ApiError::Forbidden("access denied".into()))?;
        require_project_access(&mut *tx, &user, project_id, Action::Write).await?;
        tx.commit().await?;
    }

    let mut field = multipart
        .next_field()
        .await
        .map_err(|e| ApiError::BadRequest(format!("multipart error: {e}")))?
        .ok_or_else(|| ApiError::BadRequest("no file field in multipart body".into()))?;

    let filename = field
        .file_name()
        .unwrap_or("upload")
        .to_string();

    // Stream multipart chunks to a temp file on disk, computing SHA-256
    // incrementally.  Memory usage: O(chunk_size) instead of O(file_size).
    let max = state.upload_max_bytes;
    let storage_root = &state.core_config.storage_root;

    let (mut tmp_file, tmp_path) =
        af_storage::blob_store::create_upload_temp_file(storage_root)
            .await
            .map_err(|e| ApiError::Internal(format!("failed to create temp file: {e}")))?;

    let stream_result: Result<(String, usize), ApiError> = async {
        let mut hasher = Sha256::new();
        let mut total: usize = 0;

        while let Some(chunk) = field
            .chunk()
            .await
            .map_err(|e| ApiError::BadRequest(format!("failed to read file data: {e}")))?
        {
            let new_total = total.checked_add(chunk.len()).ok_or_else(|| {
                ApiError::PayloadTooLarge(format!("file size exceeds max {max}"))
            })?;
            if new_total > max {
                return Err(ApiError::PayloadTooLarge(format!(
                    "file size exceeds max {max}"
                )));
            }
            hasher.update(&chunk);
            tmp_file.write_all(&chunk).await.map_err(|e| {
                ApiError::Internal(format!("failed to write temp file: {e}"))
            })?;
            total = new_total;
        }

        tmp_file.flush().await.map_err(|e| {
            ApiError::Internal(format!("failed to flush temp file: {e}"))
        })?;

        let sha256 = hex::encode(hasher.finalize());
        Ok((sha256, total))
    }
    .await;

    let (sha256, total_bytes) = match stream_result {
        Ok(v) => v,
        Err(e) => {
            let _ = tokio::fs::remove_file(&tmp_path).await;
            return Err(e);
        }
    };
    // Drop the file handle before rename
    drop(tmp_file);

    let delta_bytes = total_bytes as i64;

    // Check per-user upload size and storage quota
    if let Some(uid) = user.user_id() {
        // Check per-file upload limit from quota
        if let Ok(Some(quota)) = af_db::user_quotas::get_quota(&state.pool, uid).await {
            if delta_bytes > quota.max_upload_bytes {
                let _ = tokio::fs::remove_file(&tmp_path).await;
                return Err(ApiError::PayloadTooLarge(format!(
                    "file size {} exceeds per-file upload limit {}",
                    total_bytes, quota.max_upload_bytes
                )));
            }
        }

        // Atomic quota reservation — check + increment in a single UPDATE
        if !af_db::user_quotas::reserve_storage_atomic(&state.pool, uid, delta_bytes)
            .await
            .unwrap_or(true)
        {
            let _ = tokio::fs::remove_file(&tmp_path).await;
            return Err(ApiError::PayloadTooLarge("storage quota exceeded".into()));
        }
    }

    let blob_result = af_storage::blob_store::store_blob_from_file(
        &state.pool,
        storage_root,
        &tmp_path,
        &sha256,
        delta_bytes,
    )
    .await;

    // On blob write failure, release the reserved storage and clean up temp file
    let (_sha256_out, _storage_path) = match blob_result {
        Ok(v) => v,
        Err(e) => {
            let _ = tokio::fs::remove_file(&tmp_path).await;
            if let Some(uid) = user.user_id() {
                let _ = af_db::user_quotas::release_storage(&state.pool, uid, delta_bytes).await;
            }
            return Err(ApiError::Internal(e.to_string()));
        }
    };

    // Create artifact in scoped tx (RLS enforced for tenant-scoped table).
    // On failure, release the reserved quota so it isn't leaked.
    let artifact_result = if let Some(uid) = user.user_id() {
        let mut tx = af_db::scoped::begin_scoped(&state.pool, uid).await?;
        let res = af_db::artifacts::create_artifact(
            &mut *tx,
            project_id,
            &sha256,
            &filename,
            None,
            None,
        )
        .await;
        match res {
            Ok(a) => {
                tx.commit().await?;
                Ok(a)
            }
            Err(e) => {
                let _ = tx.rollback().await;
                Err(e)
            }
        }
    } else {
        af_db::artifacts::create_artifact(
            &state.pool,
            project_id,
            &sha256,
            &filename,
            None,
            None,
        )
        .await
    };

    let artifact = match artifact_result {
        Ok(a) => a,
        Err(e) => {
            if let Some(uid) = user.user_id() {
                let _ =
                    af_db::user_quotas::release_storage(&state.pool, uid, delta_bytes).await;
            }
            return Err(e.into());
        }
    };

    // Fire artifact_uploaded hooks (non-blocking)
    let project_name = af_db::projects::get_project(&state.pool, project_id)
        .await
        .ok()
        .flatten()
        .map(|p| p.name)
        .unwrap_or_default();
    crate::hooks::fire_artifact_hooks(
        &state,
        project_id,
        &project_name,
        artifact.id,
        &artifact.filename,
        &artifact.sha256,
    )
    .await;

    Ok(Json(UploadArtifactResponse {
        id: artifact.id,
        sha256: artifact.sha256,
        filename: artifact.filename,
        created_at: artifact.created_at,
    }))
}

pub async fn list(
    State(state): State<Arc<AppState>>,
    AuthenticatedUser(identity): AuthenticatedUser,
    Path(project_id): Path<Uuid>,
) -> Result<Json<Vec<ArtifactResponse>>, ApiError> {
    let user = AuthenticatedUser(identity);

    let mut tx = if user.is_admin() {
        state.pool.begin().await?
    } else {
        let uid = user
            .user_id()
            .ok_or_else(|| ApiError::Forbidden("no user_id on identity".into()))?;
        af_db::scoped::begin_scoped(&state.pool, uid).await?
    };

    af_db::projects::get_project(&mut *tx, project_id)
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("project {project_id} not found")))?;

    require_project_access(&mut *tx, &user, project_id, Action::Read).await?;

    let rows = af_db::artifacts::list_artifacts(&mut *tx, project_id).await?;
    tx.commit().await?;
    Ok(Json(rows.into_iter().map(|r| r.into()).collect()))
}

pub async fn download(
    State(state): State<Arc<AppState>>,
    AuthenticatedUser(identity): AuthenticatedUser,
    Path(id): Path<Uuid>,
) -> Result<impl IntoResponse, ApiError> {
    let user = AuthenticatedUser(identity);

    let mut tx = if user.is_admin() {
        state.pool.begin().await?
    } else {
        let uid = user
            .user_id()
            .ok_or_else(|| ApiError::Forbidden("no user_id on identity".into()))?;
        af_db::scoped::begin_scoped(&state.pool, uid).await?
    };

    let artifact = af_db::artifacts::get_artifact(&mut *tx, id)
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("artifact {id} not found")))?;

    require_project_access(&mut *tx, &user, artifact.project_id, Action::Read).await?;

    let blob = af_db::blobs::get_blob(&mut *tx, &artifact.sha256)
        .await?
        .ok_or_else(|| ApiError::NotFound("blob not found".into()))?;

    tx.commit().await?;

    let file = tokio::fs::File::open(&blob.storage_path)
        .await
        .map_err(|e| ApiError::Internal(format!("failed to open blob: {e}")))?;

    let stream = tokio_util::io::ReaderStream::new(file);
    let body = Body::from_stream(stream);

    let disposition = format!("attachment; filename=\"{}\"", artifact.filename);

    Ok((
        [
            (header::CONTENT_TYPE, "application/octet-stream".to_string()),
            (header::CONTENT_DISPOSITION, disposition),
        ],
        body,
    ))
}

pub async fn download_ghidra_project(
    State(state): State<Arc<AppState>>,
    AuthenticatedUser(identity): AuthenticatedUser,
    Path(id): Path<Uuid>,
) -> Result<impl IntoResponse, ApiError> {
    let user = AuthenticatedUser(identity);

    let cache_dir = state
        .ghidra_cache_dir
        .as_ref()
        .ok_or_else(|| ApiError::NotFound("Ghidra not configured".into()))?
        .clone();

    let mut tx = if user.is_admin() {
        state.pool.begin().await?
    } else {
        let uid = user
            .user_id()
            .ok_or_else(|| ApiError::Forbidden("no user_id on identity".into()))?;
        af_db::scoped::begin_scoped(&state.pool, uid).await?
    };

    let artifact = af_db::artifacts::get_artifact(&mut *tx, id)
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("artifact {id} not found")))?;

    require_project_access(&mut *tx, &user, artifact.project_id, Action::Read).await?;
    tx.commit().await?;

    let project_dir = cache_dir
        .join(artifact.project_id.to_string())
        .join(&artifact.sha256);

    if !project_dir.join("analysis.gpr").exists() {
        return Err(ApiError::NotFound(
            "Run ghidra.analyze first to create the analysis project".into(),
        ));
    }

    // Zip the Ghidra project directory in a blocking task
    let sha256_prefix = artifact.sha256.chars().take(8).collect::<String>();
    let zip_bytes = tokio::task::spawn_blocking(move || -> Result<Vec<u8>, String> {
        let mut buf = Vec::new();
        {
            let mut zip = zip::ZipWriter::new(std::io::Cursor::new(&mut buf));
            let options = zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Deflated);

            // Walk the project directory and add all files
            fn walk_dir(
                zip: &mut zip::ZipWriter<std::io::Cursor<&mut Vec<u8>>>,
                options: zip::write::SimpleFileOptions,
                base: &std::path::Path,
                current: &std::path::Path,
            ) -> Result<(), String> {
                let entries = std::fs::read_dir(current)
                    .map_err(|e| format!("failed to read dir {}: {e}", current.display()))?;
                for entry in entries {
                    let entry = entry.map_err(|e| format!("dir entry error: {e}"))?;
                    let path = entry.path();
                    let relative = path
                        .strip_prefix(base)
                        .map_err(|e| format!("strip prefix: {e}"))?;
                    let name = relative.to_string_lossy();

                    if path.is_dir() {
                        zip.add_directory(format!("{name}/"), options)
                            .map_err(|e| format!("zip add dir: {e}"))?;
                        walk_dir(zip, options, base, &path)?;
                    } else {
                        zip.start_file(name.to_string(), options)
                            .map_err(|e| format!("zip start file: {e}"))?;
                        let data = std::fs::read(&path)
                            .map_err(|e| format!("read {}: {e}", path.display()))?;
                        zip.write_all(&data)
                            .map_err(|e| format!("zip write: {e}"))?;
                    }
                }
                Ok(())
            }

            walk_dir(&mut zip, options, &project_dir, &project_dir)?;
            zip.finish().map_err(|e| format!("zip finish: {e}"))?;
        }
        Ok(buf)
    })
    .await
    .map_err(|e| ApiError::Internal(format!("zip task panicked: {e}")))?
    .map_err(ApiError::Internal)?;

    let body = Body::from(zip_bytes);
    let disposition = format!("attachment; filename=\"ghidra-{sha256_prefix}.zip\"");

    Ok((
        [
            (header::CONTENT_TYPE, "application/zip".to_string()),
            (header::CONTENT_DISPOSITION, disposition),
        ],
        body,
    ))
}

/// DELETE /api/v1/artifacts/:id
pub async fn delete(
    State(state): State<Arc<AppState>>,
    AuthenticatedUser(identity): AuthenticatedUser,
    Path(id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let user = AuthenticatedUser(identity);

    let mut tx = if user.is_admin() {
        state.pool.begin().await?
    } else {
        let uid = user
            .user_id()
            .ok_or_else(|| ApiError::Forbidden("no user_id on identity".into()))?;
        af_db::scoped::begin_scoped(&state.pool, uid).await?
    };

    let artifact = af_db::artifacts::get_artifact(&mut *tx, id)
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("artifact {id} not found")))?;

    require_project_access(&mut *tx, &user, artifact.project_id, Action::Write).await?;

    af_db::artifacts::delete_artifact(&mut *tx, id).await?;

    // Audit
    let detail = serde_json::json!({
        "artifact_id": id.to_string(),
        "filename": artifact.filename,
        "project_id": artifact.project_id.to_string(),
    });
    let actor_uid = user.user_id();
    let pool = state.pool.clone();
    tokio::spawn(async move {
        let _ = af_db::audit_log::insert(&pool, "artifact_deleted", None, actor_uid, Some(&detail)).await;
    });

    tx.commit().await?;
    Ok(Json(serde_json::json!({"deleted": true})))
}

/// DELETE /api/v1/projects/:id/artifacts/generated
pub async fn delete_generated(
    State(state): State<Arc<AppState>>,
    AuthenticatedUser(identity): AuthenticatedUser,
    Path(project_id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let user = AuthenticatedUser(identity);

    let mut tx = if user.is_admin() {
        state.pool.begin().await?
    } else {
        let uid = user
            .user_id()
            .ok_or_else(|| ApiError::Forbidden("no user_id on identity".into()))?;
        af_db::scoped::begin_scoped(&state.pool, uid).await?
    };

    af_db::projects::get_project(&mut *tx, project_id)
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("project {project_id} not found")))?;

    require_project_access(&mut *tx, &user, project_id, Action::Write).await?;

    let count = af_db::artifacts::delete_generated_artifacts(&mut *tx, project_id).await?;

    let detail = serde_json::json!({
        "project_id": project_id.to_string(),
        "deleted_count": count,
    });
    let actor_uid = user.user_id();
    let pool = state.pool.clone();
    tokio::spawn(async move {
        let _ = af_db::audit_log::insert(&pool, "generated_artifacts_deleted", None, actor_uid, Some(&detail)).await;
    });

    tx.commit().await?;
    Ok(Json(serde_json::json!({"deleted": count})))
}

pub async fn update_description(
    State(state): State<Arc<AppState>>,
    AuthenticatedUser(identity): AuthenticatedUser,
    Path(id): Path<Uuid>,
    Json(body): Json<UpdateArtifactDescriptionRequest>,
) -> Result<Json<ArtifactResponse>, ApiError> {
    let user = AuthenticatedUser(identity);

    // Validate description length
    if body.description.is_empty() || body.description.len() > 1000 {
        return Err(ApiError::BadRequest(
            "description must be 1-1000 characters".into(),
        ));
    }

    let mut tx = if user.is_admin() {
        state.pool.begin().await?
    } else {
        let uid = user
            .user_id()
            .ok_or_else(|| ApiError::Forbidden("no user_id on identity".into()))?;
        af_db::scoped::begin_scoped(&state.pool, uid).await?
    };

    // Look up artifact and check project access
    let artifact = af_db::artifacts::get_artifact(&mut *tx, id)
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("artifact {id} not found")))?;

    require_project_access(&mut *tx, &user, artifact.project_id, Action::Write).await?;

    let updated = af_db::artifacts::update_artifact_description(&mut *tx, id, &body.description)
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("artifact {id} not found")))?;

    tx.commit().await?;
    Ok(Json(updated.into()))
}
