//! `GET /api/files?path=`: read-only view of the workspace through the same sandbox and lock as
//! the file tools; a file's trust follows the tools' rule: no record means untrusted.
//! `POST /api/files`: one uploaded text file, stored untrusted under `uploads/`.

use axum::Json;
use axum::extract::multipart::MultipartError;
use axum::extract::{Multipart, Query, State};
use axum::http::StatusCode;
use orchestrator_core::ids::RunId;
use orchestrator_core::policy::Trust;
use orchestrator_core::tools::ToolError;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use super::AppState;
use super::error::ApiError;
use super::error::ErrorCode;
use crate::tools::files::{EntryKind, FILE_BYTES, UploadError};

/// The text formats an upload may have, by extension.
const UPLOAD_EXTENSIONS: [&str; 4] = ["csv", "md", "txt", "json"];
const UPLOAD_FIELD: &str = "file";

#[derive(Debug, Deserialize)]
pub struct FileQuery {
    path: Option<String>,
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
    },
}

pub async fn view(
    State(state): State<AppState>,
    Query(query): Query<FileQuery>,
) -> Result<Json<FileView>, ApiError> {
    let path = query.path.unwrap_or_else(|| ".".to_string());
    let workspace = &state.workspace;
    let view = match workspace.kind_of(&path).await.map_err(tool_error)? {
        None => return Err(ApiError::not_found()),
        Some(EntryKind::Dir) => {
            let entries = workspace.list(&path).await.map_err(tool_error)?;
            FileView::Dir {
                path,
                entries: entries
                    .into_iter()
                    .map(|entry| FileEntryView {
                        name: entry.name,
                        path: entry.path,
                        kind: entry.kind,
                        bytes: entry.bytes,
                        trust: if entry.untrusted {
                            Trust::Untrusted
                        } else {
                            Trust::Trusted
                        },
                        written_by_root_run_id: entry.written_by_root_run_id,
                    })
                    .collect(),
            }
        }
        Some(EntryKind::File | EntryKind::Symlink) => {
            let (content, trust, written_by_root_run_id) =
                workspace.read(&path).await.map_err(tool_error)?;
            FileView::File {
                path,
                content,
                trust,
                written_by_root_run_id,
            }
        }
    };
    Ok(Json(view))
}

#[derive(Debug, Serialize, TS)]
#[ts(export)]
pub struct UploadResponse {
    /// `uploads/<name>`, relative to the workspace root.
    pub path: String,
    pub bytes: u64,
    /// Always untrusted: an upload is outside content.
    pub trust: Trust,
}

/// 201 with the stored path. 422 for a missing field or a bad name, 409 when the name is taken,
/// 413 over 256 KB, 415 for anything but UTF-8 .csv, .md, .txt or .json.
pub async fn upload(
    State(state): State<AppState>,
    mut multipart: Multipart,
) -> Result<(StatusCode, Json<UploadResponse>), ApiError> {
    let mut field = multipart
        .next_field()
        .await
        .map_err(multipart_error)?
        .filter(|field| field.name() == Some(UPLOAD_FIELD))
        .ok_or_else(|| ApiError::invalid(UPLOAD_FIELD, "send one field named file"))?;
    let name = field.file_name().unwrap_or_default().to_string();
    crate::tools::files::check_upload_name(&name).map_err(tool_error_on_file)?;
    let extension = name
        .rsplit_once('.')
        .map(|(_, extension)| extension.to_ascii_lowercase());
    if !extension.is_some_and(|e| UPLOAD_EXTENSIONS.contains(&e.as_str())) {
        return Err(unsupported());
    }

    let mut bytes = Vec::new();
    while let Some(chunk) = field.chunk().await.map_err(multipart_error)? {
        if bytes.len() + chunk.len() > FILE_BYTES {
            return Err(too_large());
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
    let text = String::from_utf8(bytes).map_err(|_| unsupported())?;

    let path = state
        .workspace
        .store_upload(&name, &text)
        .await
        .map_err(|error| match error {
            UploadError::Exists => ApiError::conflict("a file with this name was already uploaded"),
            UploadError::Tool(error) => tool_error_on_file(error),
        })?;
    let response = UploadResponse {
        path,
        bytes: text.len() as u64,
        trust: Trust::Untrusted,
    };
    Ok((StatusCode::CREATED, Json(response)))
}

fn too_large() -> ApiError {
    ApiError::new(
        StatusCode::PAYLOAD_TOO_LARGE,
        ErrorCode::InvalidInput,
        "the file is larger than 256 KB".to_string(),
        Some(UPLOAD_FIELD.to_string()),
    )
}

fn unsupported() -> ApiError {
    ApiError::new(
        StatusCode::UNSUPPORTED_MEDIA_TYPE,
        ErrorCode::InvalidInput,
        "only UTF-8 text files ending in .csv, .md, .txt or .json".to_string(),
        Some(UPLOAD_FIELD.to_string()),
    )
}

/// A body over the server's limit is 413 like an oversized file; any other parse failure is 422.
fn multipart_error(error: MultipartError) -> ApiError {
    if error.status() == StatusCode::PAYLOAD_TOO_LARGE {
        too_large()
    } else {
        ApiError::invalid(UPLOAD_FIELD, "not a valid multipart upload")
    }
}

/// Upload path errors name the `file` field; infrastructure failures stay 500.
fn tool_error_on_file(error: ToolError) -> ApiError {
    match error {
        ToolError::InvalidArgs(message) | ToolError::Failed(message) => {
            let message = message
                .strip_prefix("file: ")
                .or_else(|| message.strip_prefix("path: "))
                .unwrap_or(&message);
            ApiError::invalid(UPLOAD_FIELD, message)
        }
        ToolError::Internal(_) => ApiError::internal(),
    }
}

/// Tool messages name the path or the limit, never file contents, so they are safe to return.
/// An infrastructure failure is the server's fault, so it is a 500, never a path error.
fn tool_error(error: ToolError) -> ApiError {
    match error {
        ToolError::InvalidArgs(message) | ToolError::Failed(message) => {
            ApiError::invalid("path", message.strip_prefix("path: ").unwrap_or(&message))
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
        let invalid = tool_error(ToolError::InvalidArgs(
            "path: .. is not allowed".to_string(),
        ));
        assert_eq!(invalid.status, StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(invalid.detail.message, ".. is not allowed");
        assert_eq!(invalid.detail.field.as_deref(), Some("path"));
        let failed = tool_error(ToolError::Failed("a.md is larger than 256 KB".to_string()));
        assert_eq!(failed.status, StatusCode::UNPROCESSABLE_ENTITY);
        let internal = tool_error(ToolError::Internal("storage failed".to_string()));
        assert_eq!(internal.status, StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(internal.detail.field, None);
    }
}
