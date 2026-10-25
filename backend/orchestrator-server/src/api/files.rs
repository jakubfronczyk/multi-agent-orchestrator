//! `GET /api/files?path=`: read-only view of the workspace through the same sandbox and lock as
//! the file tools. A file's trust follows the tools' rule: no record means untrusted.

use axum::Json;
use axum::extract::{Query, State};
use orchestrator_core::ids::RunId;
use orchestrator_core::policy::Trust;
use orchestrator_core::tools::ToolError;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use super::AppState;
use super::error::ApiError;
use crate::tools::files::EntryKind;

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
    /// The root run of the task that last wrote the file; null for seeded files and for directories.
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
        /// The root run of the task that last wrote the file; null for seeded files.
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
