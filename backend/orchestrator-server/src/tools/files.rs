//! File tools over the workspace sandbox: list, read and write; `csv` holds the CSV query.
//! Error messages name the path or the argument, never file contents (§6 rule 4).

use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use chrono::Utc;
use orchestrator_core::ids::RunId;
use orchestrator_core::policy::{TaintSource, Trust};
use orchestrator_core::tools::{
    Action, Target, Tool, ToolCtx, ToolError, ToolName, ToolOutput, ToolSpec, parse_args,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::SqlitePool;
use tokio::io::AsyncWriteExt;
use tokio::sync::RwLock;
use ts_rs::TS;
use uuid::Uuid;

use super::sandbox::{relative, resolve};
use super::{escape_like, is_trusted, spec, storage_failed};
use crate::sqlite::text::{from_text, timestamp, to_text};

const TIMEOUT: Duration = Duration::from_secs(5);
const FILE_BYTES: usize = 256 * 1024;
const LIST_LIMIT: usize = 500;
const WRITABLE: [&str; 4] = ["md", "csv", "json", "txt"];
/// The one directory uploads go to.
pub const UPLOADS_DIR: &str = "uploads";
const UPLOAD_NAME_CHARS: usize = 100;

/// Why an upload was not stored.
#[derive(Debug, thiserror::Error)]
pub enum UploadError {
    #[error("a file with this name was already uploaded")]
    Exists,
    #[error(transparent)]
    Tool(#[from] ToolError),
}

/// A bare file name of ASCII letters, digits, `.`, `_` and `-`, at most 100 characters, not
/// starting with a dot; so it names one file directly inside `uploads/`.
pub fn check_upload_name(name: &str) -> Result<(), ToolError> {
    let valid = !name.is_empty()
        && name.len() <= UPLOAD_NAME_CHARS
        && !name.starts_with('.')
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'));
    if valid {
        Ok(())
    } else {
        Err(ToolError::InvalidArgs(
            "file: the name must be up to 100 letters, digits, '.', '_' or '-', not starting with '.'"
                .to_string(),
        ))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum EntryKind {
    File,
    Dir,
    /// Listed but never followed.
    Symlink,
}

/// One directory entry; `path` is relative to the workspace root.
#[derive(Debug, Clone)]
pub struct Entry {
    pub name: String,
    pub path: String,
    pub kind: EntryKind,
    pub bytes: u64,
    pub untrusted: bool,
    /// The root run of the task that last wrote the file; `None` for seeded files, uploads and
    /// directories.
    pub written_by_root_run_id: Option<RunId>,
}

/// The sandbox root and the database that records each file's trust. One lock pairs them:
/// writes hold it exclusively, reads share it, so a read never sees new bytes with old trust.
#[derive(Clone)]
pub struct Workspace {
    root: PathBuf,
    pool: SqlitePool,
    lock: Arc<RwLock<()>>,
}

impl Workspace {
    pub fn new(root: PathBuf, pool: SqlitePool) -> Self {
        Self {
            root,
            pool,
            lock: Arc::new(RwLock::new(())),
        }
    }

    /// What is at `path`, or `None` when nothing is. A symlink anywhere on the path is rejected.
    pub async fn kind_of(&self, path: &str) -> Result<Option<EntryKind>, ToolError> {
        let full = resolve(&self.root, path).await?;
        match tokio::fs::symlink_metadata(&full).await {
            Ok(meta) if meta.is_dir() => Ok(Some(EntryKind::Dir)),
            Ok(_) => Ok(Some(EntryKind::File)),
            Err(error) if error.kind() == ErrorKind::NotFound => Ok(None),
            Err(_) => Err(ToolError::Failed(format!("{path} cannot be read"))),
        }
    }

    /// A directory's entries with their trust, at most 500, read under the shared lock.
    pub async fn list(&self, path: &str) -> Result<Vec<Entry>, ToolError> {
        let base = display(&relative(path)?);
        let _shared = self.lock.read().await;
        let dir = resolve(&self.root, path).await?;
        let mut entries = tokio::fs::read_dir(&dir)
            .await
            .map_err(|_| ToolError::Failed(format!("{path} is not a readable directory")))?;
        let mut listed = Vec::new();
        while let Some(entry) = entries
            .next_entry()
            .await
            .map_err(|_| ToolError::Failed(format!("{path} cannot be listed")))?
        {
            if listed.len() == LIST_LIMIT {
                break;
            }
            let name = entry.file_name().to_string_lossy().into_owned();
            // A write aborted mid-way can leave its temp file; it is never a workspace file.
            if name.starts_with('.') && name.ends_with(".tmp") {
                continue;
            }
            let Ok(kind) = entry.file_type().await else {
                continue;
            };
            let bytes = entry.metadata().await.map(|m| m.len()).unwrap_or(0);
            let kind = if kind.is_symlink() {
                EntryKind::Symlink
            } else if kind.is_dir() {
                EntryKind::Dir
            } else {
                EntryKind::File
            };
            let entry_path = if base == "." {
                name.clone()
            } else {
                format!("{base}/{name}")
            };
            let untrusted = self
                .entry_untrusted(&entry_path, kind == EntryKind::Dir)
                .await?;
            let written_by_root_run_id = if kind == EntryKind::File {
                self.written_by_root(&entry_path).await?
            } else {
                None
            };
            listed.push(Entry {
                name,
                path: entry_path,
                kind,
                bytes,
                untrusted,
                written_by_root_run_id,
            });
        }
        Ok(listed)
    }

    /// A file's text, its trust and the root run of the task that last wrote it, read together
    /// under the shared lock; a file with no row counts as untrusted.
    pub async fn read(&self, path: &str) -> Result<(String, Trust, Option<RunId>), ToolError> {
        let _shared = self.lock.read().await;
        let text = self.read_text(path).await?;
        let trust = if is_trusted(self.trust_of(path).await?.as_deref()) {
            Trust::Trusted
        } else {
            Trust::Untrusted
        };
        Ok((text, trust, self.written_by_root(path).await?))
    }

    /// A file's text and stored trust, read together under the shared lock.
    pub(super) async fn read_with_trust(
        &self,
        path: &str,
    ) -> Result<(String, Option<String>), ToolError> {
        let _shared = self.lock.read().await;
        let text = self.read_text(path).await?;
        Ok((text, self.trust_of(path).await?))
    }

    /// A file is untrusted when its row says so or it has none. A directory has no row of its
    /// own: it is untrusted when any recorded file under it is, or when none under it is recorded.
    async fn entry_untrusted(&self, path: &str, is_dir: bool) -> Result<bool, ToolError> {
        if !is_dir {
            return Ok(!is_trusted(self.trust_of(path).await?.as_deref()));
        }
        let pattern = format!("{}/%", escape_like(path));
        let trusts = sqlx::query_scalar!(
            "SELECT trust FROM workspace_files WHERE path LIKE ? ESCAPE '\\'",
            pattern
        )
        .fetch_all(&self.pool)
        .await
        .map_err(storage_failed)?;
        Ok(trusts.is_empty() || trusts.iter().any(|trust| !is_trusted(Some(trust))))
    }

    /// `run` is `None` for a file no run wrote, such as an upload.
    async fn record_trust(
        &self,
        path: &str,
        trust: Trust,
        run: Option<&str>,
    ) -> Result<(), ToolError> {
        let trust = to_text(&trust).map_err(storage_failed)?;
        let now = timestamp(Utc::now());
        sqlx::query!(
            "INSERT INTO workspace_files (path, trust, written_by_run, updated_at) VALUES (?, ?, ?, ?)
             ON CONFLICT (path) DO UPDATE SET trust = excluded.trust, written_by_run = excluded.written_by_run,
                                              updated_at = excluded.updated_at",
            path,
            trust,
            run,
            now
        )
        .execute(&self.pool)
        .await
        .map_err(storage_failed)?;
        Ok(())
    }

    /// Saves an upload as `uploads/<name>` and returns that path; an existing upload is never
    /// replaced. Uploads come from outside, so the row is untrusted with no writing run, and
    /// it is recorded under the exclusive lock before any byte is written.
    pub async fn store_upload(&self, name: &str, text: &str) -> Result<String, UploadError> {
        check_upload_name(name)?;
        let path = format!("{UPLOADS_DIR}/{name}");
        let failed = || ToolError::Failed(format!("{path} cannot be written"));
        let _exclusive = self.lock.write().await;

        let target = resolve(&self.root, &path).await?;
        let parent = target.parent().ok_or_else(failed)?.to_path_buf();
        tokio::fs::create_dir_all(&parent)
            .await
            .map_err(|_| failed())?;
        // The directory may have been created just now; it is checked again before the write.
        resolve(&self.root, &path).await?;
        if tokio::fs::try_exists(&target).await.unwrap_or(true) {
            return Err(UploadError::Exists);
        }

        self.record_trust(&path, Trust::Untrusted, None).await?;
        let temp = parent.join(format!(".{name}.{}.tmp", Uuid::now_v7()));
        let written = async {
            let mut file = tokio::fs::File::create_new(&temp).await?;
            file.write_all(text.as_bytes()).await?;
            file.sync_all().await?;
            tokio::fs::rename(&temp, &target).await
        }
        .await;
        if written.is_err() {
            let _ = tokio::fs::remove_file(&temp).await;
            return Err(failed().into());
        }
        Ok(path)
    }

    /// `None` for a file no run wrote, such as a seeded one.
    async fn written_by_root(&self, path: &str) -> Result<Option<RunId>, ToolError> {
        let root = sqlx::query_scalar!(
            "SELECT r.root_run_id FROM workspace_files f JOIN runs r ON r.id = f.written_by_run
             WHERE f.path = ?",
            path
        )
        .fetch_optional(&self.pool)
        .await
        .map_err(storage_failed)?;
        root.map(|root| from_text(&root).map_err(storage_failed))
            .transpose()
    }

    /// The trust stored for a file, or `None` when nothing recorded it.
    async fn trust_of(&self, path: &str) -> Result<Option<String>, ToolError> {
        sqlx::query_scalar!("SELECT trust FROM workspace_files WHERE path = ?", path)
            .fetch_optional(&self.pool)
            .await
            .map_err(storage_failed)
    }

    /// Reads a text file inside the sandbox, at most 256 KB.
    async fn read_text(&self, path: &str) -> Result<String, ToolError> {
        let full = resolve(&self.root, path).await?;
        let bytes = match tokio::fs::read(&full).await {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == ErrorKind::NotFound => {
                return Err(ToolError::Failed(format!("no file at {path}")));
            }
            Err(_) => return Err(ToolError::Failed(format!("{path} cannot be read"))),
        };
        if bytes.len() > FILE_BYTES {
            return Err(ToolError::Failed(format!("{path} is larger than 256 KB")));
        }
        String::from_utf8(bytes).map_err(|_| ToolError::Failed(format!("{path} is not UTF-8 text")))
    }
}

/// The path as stored and shown: relative, `/`-separated, no `.` parts.
pub(super) fn display(path: &Path) -> String {
    path.components()
        .map(|part| part.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

pub(super) fn file_action(verb: &str, path: &str) -> Result<Action, ToolError> {
    Ok(Action {
        verb: verb.to_string(),
        target: Target::File {
            path: display(&relative(path)?),
        },
    })
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ListArgs {
    /// A directory inside the workspace; the workspace root when absent.
    #[serde(default)]
    path: Option<String>,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ReadArgs {
    /// A file inside the workspace, such as `data/access-policy.md`.
    path: String,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct WriteArgs {
    /// Where to save, inside the workspace; must end in .md, .csv, .json or .txt.
    path: String,
    /// The whole file, at most 256 KB. An existing file is replaced.
    content: String,
}

pub struct FileList {
    workspace: Workspace,
    spec: ToolSpec,
}

pub struct FileRead {
    workspace: Workspace,
    spec: ToolSpec,
}

pub struct FileWrite {
    workspace: Workspace,
    spec: ToolSpec,
}

impl FileList {
    pub fn new(workspace: Workspace) -> Self {
        let purpose =
            "List the files in a workspace directory. Use it to find a path before reading.";
        Self {
            workspace,
            spec: spec::<ListArgs>(ToolName::FileList, purpose, TIMEOUT),
        }
    }
}

impl FileRead {
    pub fn new(workspace: Workspace) -> Self {
        let purpose =
            "Read a text file from the workspace. For CSV questions use csv_query instead.";
        Self {
            workspace,
            spec: spec::<ReadArgs>(ToolName::FileRead, purpose, TIMEOUT),
        }
    }
}

impl FileWrite {
    pub fn new(workspace: Workspace) -> Self {
        let purpose = "Save a .md, .csv, .json or .txt file in the workspace, replacing any file at that path.";
        Self {
            workspace,
            spec: spec::<WriteArgs>(ToolName::FileWrite, purpose, TIMEOUT),
        }
    }
}

#[async_trait]
impl Tool for FileList {
    fn spec(&self) -> &ToolSpec {
        &self.spec
    }

    fn check(&self, args: &Value) -> Result<Action, ToolError> {
        let args: ListArgs = parse_args(args)?;
        file_action("list", args.path.as_deref().unwrap_or("."))
    }

    /// Symlinks are listed as such and never followed.
    /// Entry names are stored text too, so each untrusted entry is a taint source.
    async fn run(&self, _ctx: &ToolCtx, args: Value) -> Result<ToolOutput, ToolError> {
        let args: ListArgs = parse_args(&args)?;
        let path = args.path.unwrap_or_else(|| ".".to_string());
        let entries = self.workspace.list(&path).await?;
        let taint = entries
            .iter()
            .filter(|entry| entry.untrusted)
            .map(|entry| TaintSource::File {
                path: entry.path.clone(),
            })
            .collect();
        let listed: Vec<Value> = entries
            .into_iter()
            .map(|entry| json!({ "name": entry.name, "kind": entry.kind, "bytes": entry.bytes }))
            .collect();
        Ok(ToolOutput {
            content: json!({ "path": path, "entries": listed }),
            taint,
        })
    }
}

#[async_trait]
impl Tool for FileRead {
    fn spec(&self) -> &ToolSpec {
        &self.spec
    }

    fn check(&self, args: &Value) -> Result<Action, ToolError> {
        let args: ReadArgs = parse_args(args)?;
        file_action("read", &args.path)
    }

    /// A file with no trust row counts as untrusted; the seed records rows for its files.
    async fn run(&self, _ctx: &ToolCtx, args: Value) -> Result<ToolOutput, ToolError> {
        let args: ReadArgs = parse_args(&args)?;
        let path = display(&relative(&args.path)?);
        let (text, trust) = self.workspace.read_with_trust(&path).await?;
        let taint = file_taint(&path, trust.as_deref());
        Ok(ToolOutput {
            content: json!({ "path": path, "content": text, "trust": trust.unwrap_or_else(|| "untrusted".to_string()) }),
            taint,
        })
    }
}

#[async_trait]
impl Tool for FileWrite {
    fn spec(&self) -> &ToolSpec {
        &self.spec
    }

    fn check(&self, args: &Value) -> Result<Action, ToolError> {
        let args: WriteArgs = parse_args(args)?;
        let extension = Path::new(&args.path)
            .extension()
            .and_then(|e| e.to_str())
            .map(str::to_ascii_lowercase);
        if !extension.is_some_and(|e| WRITABLE.contains(&e.as_str())) {
            return Err(ToolError::InvalidArgs(
                "path: must end in .md, .csv, .json or .txt".to_string(),
            ));
        }
        if args.content.len() > FILE_BYTES {
            return Err(ToolError::InvalidArgs(
                "content: at most 256 KB".to_string(),
            ));
        }
        file_action("write", &args.path)
    }

    /// Under the exclusive lock, the row is marked untrusted before any byte changes and set to
    /// the writer's trust only after the rename, so a failure at any step leaves "untrusted".
    /// The temp file sits next to its target: `rename` is atomic only within one filesystem.
    async fn run(&self, ctx: &ToolCtx, args: Value) -> Result<ToolOutput, ToolError> {
        let args: WriteArgs = parse_args(&args)?;
        let path = display(&relative(&args.path)?);
        let run = to_text(&ctx.run_id).map_err(storage_failed)?;
        let failed = || ToolError::Failed(format!("{path} cannot be written"));
        let _exclusive = self.workspace.lock.write().await;

        let target = resolve(&self.workspace.root, &path).await?;
        let parent = target.parent().ok_or_else(failed)?.to_path_buf();
        tokio::fs::create_dir_all(&parent)
            .await
            .map_err(|_| failed())?;
        // Directories this call created are checked again before anything is written in them.
        resolve(&self.workspace.root, &path).await?;
        let replaced = tokio::fs::try_exists(&target).await.unwrap_or(false);

        self.workspace
            .record_trust(&path, Trust::Untrusted, Some(&run))
            .await?;
        let name = target
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let temp = parent.join(format!(".{name}.{}.tmp", Uuid::now_v7()));
        let written = async {
            let mut file = tokio::fs::File::create(&temp).await?;
            file.write_all(args.content.as_bytes()).await?;
            file.sync_all().await?;
            tokio::fs::rename(&temp, &target).await
        }
        .await;
        if written.is_err() {
            let _ = tokio::fs::remove_file(&temp).await;
            return Err(failed());
        }
        self.workspace
            .record_trust(&path, ctx.trust, Some(&run))
            .await?;

        let trust = to_text(&ctx.trust).map_err(storage_failed)?;
        Ok(ToolOutput {
            content: json!({ "path": path, "bytes": args.content.len(), "replaced": replaced, "trust": trust }),
            taint: Vec::new(),
        })
    }

    /// The card shows the whole new file and says when it replaces one.
    async fn preview(&self, args: &Value) -> String {
        let Ok(args) = parse_args::<WriteArgs>(args) else {
            return serde_json::to_string_pretty(args).unwrap_or_default();
        };
        let replaces = match resolve(&self.workspace.root, &args.path).await {
            Ok(target) => tokio::fs::try_exists(&target).await.unwrap_or(false),
            Err(_) => false,
        };
        let note = if replaces {
            "\n\n(replaces an existing file)"
        } else {
            ""
        };
        format!("{}\n\n{}{note}", args.path, args.content)
    }
}

/// A file read back from storage taints the reader unless its row says trusted.
pub(super) fn file_taint(path: &str, trust: Option<&str>) -> Vec<TaintSource> {
    if is_trusted(trust) {
        Vec::new()
    } else {
        vec![TaintSource::File {
            path: path.to_string(),
        }]
    }
}
