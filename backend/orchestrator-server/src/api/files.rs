//! `GET /api/files?team_id=&path=`: read-only view of the workspace as that team's agents see it,
//! through the same mapper, sandbox and lock as the file tools; no trust record means untrusted.
//! `POST /api/files?team_id=`: one uploaded file, any name and kind, stored untrusted in the team's
//! `uploads/`; spreadsheets, Word documents and PDFs are stored as text the agents can read.

use axum::Json;
use axum::extract::multipart::MultipartError;
use axum::extract::{Multipart, Query, State};
use axum::http::StatusCode;
use orchestrator_core::ids::{RunId, TeamId};
use orchestrator_core::policy::Trust;
use orchestrator_core::tools::ToolError;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use super::error::ApiError;
use super::error::ErrorCode;
use super::{AppState, query};
use crate::sqlite::teams;
use crate::tools::convert::{ConvertError, convert, is_text};
use crate::tools::workspace::{EntryKind, FILE_BYTES, Need, UploadError, upload_name};

const UPLOAD_FIELD: &str = "file";
/// The largest file an upload takes as sent; what is stored as text stays within `FILE_BYTES`.
pub const UPLOAD_BYTES: usize = 10 * 1024 * 1024;
const CONVERT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

#[derive(Debug, Deserialize)]
pub struct FileQuery {
    team_id: Option<String>,
    path: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct UploadQuery {
    team_id: Option<String>,
}

#[derive(Debug, Serialize, TS)]
#[ts(export)]
pub struct FileEntryView {
    pub name: String,
    pub path: String,
    pub kind: EntryKind,
    pub bytes: u64,
    pub trust: Trust,
    /// The root run of the task that last wrote the file; null for seeded files, uploads and
    /// directories.
    pub written_by_root_run_id: Option<RunId>,
    /// That task's team; null exactly when `written_by_root_run_id` is.
    pub written_by_team_id: Option<TeamId>,
}

#[derive(Debug, Serialize, TS)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[ts(export)]
pub enum FileView {
    Dir {
        path: String,
        entries: Vec<FileEntryView>,
    },
    File {
        path: String,
        content: String,
        trust: Trust,
        /// The root run of the task that last wrote the file; null for seeded files and uploads.
        written_by_root_run_id: Option<RunId>,
        /// That task's team; null exactly when `written_by_root_run_id` is.
        written_by_team_id: Option<TeamId>,
    },
}

/// The one user may view any team's files, a deleted team's included. 404 for an unknown team or
/// a path with nothing there; 422 on `path` for a bad path or a folder not shared with the team.
pub async fn view(
    State(state): State<AppState>,
    Query(query): Query<FileQuery>,
) -> Result<Json<FileView>, ApiError> {
    let team = required_team(query.team_id)?;
    teams::get(&state.pool, team).await?;
    let path = query.path.unwrap_or_else(|| ".".to_string());
    let workspace = &state.workspace;
    let (shown, kind) = workspace.kind_for(team, &path).await.map_err(path_error)?;
    let view = match kind {
        None => return Err(ApiError::not_found()),
        Some(EntryKind::Dir) => {
            let entries = workspace.list_for(team, &path).await.map_err(path_error)?;
            FileView::Dir {
                path: shown,
                entries: entries
                    .into_iter()
                    .map(|entry| FileEntryView {
                        name: entry.name,
                        path: entry.path,
                        kind: entry.kind,
                        bytes: entry.bytes,
                        trust: entry.trust,
                        written_by_root_run_id: entry.written_by.map(|w| w.root_run_id),
                        written_by_team_id: entry.written_by.map(|w| w.team_id),
                    })
                    .collect(),
            }
        }
        Some(EntryKind::File | EntryKind::Symlink) => {
            let at = workspace
                .locate(team, &path, Need::Read)
                .await
                .map_err(path_error)?;
            let (content, trust, written_by) =
                workspace.read(&at.physical).await.map_err(path_error)?;
            FileView::File {
                path: at.shown,
                content,
                trust,
                written_by_root_run_id: written_by.map(|w| w.root_run_id),
                written_by_team_id: written_by.map(|w| w.team_id),
            }
        }
    };
    Ok(Json(view))
}

#[derive(Debug, Serialize, TS)]
#[ts(export)]
pub struct UploadResponse {
    /// `uploads/<name>`, as the team's agents name it.
    pub path: String,
    pub bytes: u64,
    /// Always untrusted: an upload is outside content.
    pub trust: Trust,
    /// False for a binary file kept as sent, which the agents cannot read.
    pub readable: bool,
}

/// What one upload stored: one file, or one per sheet of a workbook.
#[derive(Debug, Serialize, TS)]
#[ts(export)]
pub struct UploadResult {
    pub files: Vec<UploadResponse>,
    /// The name as sent, when the file was converted to text (a workbook, a document, a PDF).
    pub converted_from: Option<String>,
}

/// 201 with what was stored. A spreadsheet becomes one CSV per sheet, a Word document Markdown,
/// a PDF its text; other text is kept as it is and any other file as sent, under the cleaned name
/// or the next free number. 422 for a missing field, no `team_id` or a file that cannot be read,
/// 404 for an unknown or deleted team, 409 when every numbered name is taken, 413 over 10 MB or
/// for converted text over 256 KB, 500 when the disk fails.
pub async fn upload(
    State(state): State<AppState>,
    Query(query): Query<UploadQuery>,
    mut multipart: Multipart,
) -> Result<(StatusCode, Json<UploadResult>), ApiError> {
    let team = required_team(query.team_id)?;
    if teams::get(&state.pool, team).await?.deleted_at.is_some() {
        return Err(ApiError::not_found());
    }
    let mut field = multipart
        .next_field()
        .await
        .map_err(multipart_error)?
        .filter(|field| field.name() == Some(UPLOAD_FIELD))
        .ok_or_else(|| ApiError::invalid(UPLOAD_FIELD, "send one field named file"))?;
    // Any name is accepted; the workspace cleans it and numbers a taken one.
    let name = upload_name(field.file_name().unwrap_or_default());

    let mut bytes = Vec::new();
    while let Some(chunk) = field.chunk().await.map_err(multipart_error)? {
        if bytes.len() + chunk.len() > UPLOAD_BYTES {
            return Err(too_large("the file is larger than 10 MB"));
        }
        bytes.extend_from_slice(&chunk);
    }
    drop(field);
    if multipart
        .next_field()
        .await
        .map_err(multipart_error)?
        .is_some()
    {
        return Err(ApiError::invalid(UPLOAD_FIELD, "send one field named file"));
    }

    // Parsing a workbook or a PDF is CPU work on untrusted bytes: off the async threads, bounded.
    let converting = name.clone();
    let outputs = tokio::time::timeout(
        CONVERT_TIMEOUT,
        tokio::task::spawn_blocking(move || convert(&converting, bytes)),
    )
    .await
    .map_err(|_| ApiError::invalid(UPLOAD_FIELD, "the file took too long to read"))?
    .map_err(|_| ApiError::invalid(UPLOAD_FIELD, "the file could not be read"))?
    .map_err(|error| match error {
        ConvertError::TooBig(_) => too_large(&error.to_string()),
        other => ApiError::invalid(UPLOAD_FIELD, &other.to_string()),
    })?;
    let converted = outputs.len() != 1 || outputs[0].name != name;
    for output in &outputs {
        if is_text(&output.bytes) && output.bytes.len() > FILE_BYTES {
            return Err(too_large(&format!(
                "{} is larger than 256 KB as text",
                output.name
            )));
        }
    }

    let mut files = Vec::with_capacity(outputs.len());
    for output in outputs {
        let path = state
            .workspace
            .store_upload(team, &output.name, &output.bytes)
            .await
            .map_err(|error| match error {
                UploadError::Exists => {
                    ApiError::conflict("too many uploads share this name; rename the file")
                }
                UploadError::Tool(ToolError::Failed(message)) => {
                    tracing::error!(%message, "upload could not be written");
                    ApiError::internal()
                }
                UploadError::Tool(error) => tool_error(UPLOAD_FIELD, error),
            })?;
        files.push(UploadResponse {
            path,
            bytes: output.bytes.len() as u64,
            trust: Trust::Untrusted,
            readable: is_text(&output.bytes),
        });
    }
    let converted_from = converted.then_some(name);
    Ok((
        StatusCode::CREATED,
        Json(UploadResult {
            files,
            converted_from,
        }),
    ))
}

fn required_team(raw: Option<String>) -> Result<TeamId, ApiError> {
    query::id("team_id", raw)?.ok_or_else(|| ApiError::invalid("team_id", "name a team"))
}

fn too_large(message: &str) -> ApiError {
    ApiError::new(
        StatusCode::PAYLOAD_TOO_LARGE,
        ErrorCode::InvalidInput,
        message.to_string(),
        Some(UPLOAD_FIELD.to_string()),
    )
}

/// A body over the server's limit is 413 like an oversized file; any other parse failure is 422.
fn multipart_error(error: MultipartError) -> ApiError {
    if error.status() == StatusCode::PAYLOAD_TOO_LARGE {
        too_large("the file is larger than 10 MB")
    } else {
        ApiError::invalid(UPLOAD_FIELD, "not a valid multipart upload")
    }
}

fn path_error(error: ToolError) -> ApiError {
    tool_error("path", error)
}

/// Tool messages name the path or the limit, never file contents, so they are safe to return.
/// An infrastructure failure is the server's fault, so it is a 500, never a field error.
fn tool_error(field: &str, error: ToolError) -> ApiError {
    match error {
        ToolError::InvalidArgs(message) | ToolError::Failed(message) => {
            let message = message
                .strip_prefix(&format!("{field}: "))
                .or_else(|| message.strip_prefix("path: "))
                .unwrap_or(&message);
            ApiError::invalid(field, message)
        }
        ToolError::Internal(_) => ApiError::internal(),
    }
}

#[cfg(test)]
mod tests {
    use axum::http::StatusCode;

    use super::*;

    #[test]
    fn only_request_errors_are_path_errors() {
        let invalid = path_error(ToolError::InvalidArgs(
            "path: .. is not allowed".to_string(),
        ));
        assert_eq!(invalid.status, StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(invalid.detail.message, ".. is not allowed");
        assert_eq!(invalid.detail.field.as_deref(), Some("path"));
        let failed = path_error(ToolError::Failed("a.md is larger than 256 KB".to_string()));
        assert_eq!(failed.status, StatusCode::UNPROCESSABLE_ENTITY);
        let internal = path_error(ToolError::Internal("storage failed".to_string()));
        assert_eq!(internal.status, StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(internal.detail.field, None);
    }
}
