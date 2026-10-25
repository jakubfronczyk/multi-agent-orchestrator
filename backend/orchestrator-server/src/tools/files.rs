//! File tools over the workspace sandbox: list, read, write and a small CSV query.
//! Error messages name the path or the argument, never file contents (§6 rule 4).

use std::collections::BTreeMap;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use chrono::Utc;
use orchestrator_core::policy::{Trust, untrusted};
use orchestrator_core::tools::{
    Action, Target, Tool, ToolCtx, ToolError, ToolName, ToolOutput, ToolSpec,
};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};
use sqlx::SqlitePool;
use tokio::io::AsyncWriteExt;
use tokio::sync::RwLock;
use uuid::Uuid;

use super::sandbox::{relative, resolve};
use super::{as_stored, is_trusted, parse, spec, storage_failed};
use crate::sqlite::text::{timestamp, to_text};

const TIMEOUT: Duration = Duration::from_secs(5);
const FILE_BYTES: usize = 256 * 1024;
const LIST_LIMIT: usize = 500;
const ROW_LIMIT: usize = 200;
const WRITABLE: [&str; 4] = ["md", "csv", "json", "txt"];

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

    /// A file's text and stored trust, read together under the shared lock.
    async fn read_with_trust(&self, path: &str) -> Result<(String, Option<String>), ToolError> {
        let _shared = self.lock.read().await;
        let text = self.read_text(path).await?;
        Ok((text, self.trust_of(path).await?))
    }

    async fn record_trust(&self, path: &str, trust: Trust, run: &str) -> Result<(), ToolError> {
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
fn display(path: &Path) -> String {
    path.components()
        .map(|part| part.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

fn file_action(verb: &str, path: &str) -> Result<Action, ToolError> {
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

#[derive(Clone, Copy, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
enum FilterOp {
    Eq,
    Ne,
    Contains,
    Gt,
    Lt,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct Filter {
    /// A column name from the header row.
    column: String,
    /// `eq`, `ne`, `contains`, `gt` or `lt`; `gt` and `lt` compare numbers when both sides are numbers.
    op: FilterOp,
    value: String,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct CsvArgs {
    /// A .csv file inside the workspace.
    path: String,
    /// Rows must match every filter.
    #[serde(default)]
    filter: Vec<Filter>,
    /// Columns to return; all columns when absent.
    #[serde(default)]
    select: Vec<String>,
    /// Count matching rows per value of this column.
    #[serde(default)]
    group_by: Option<String>,
    /// Return only the number of matching rows.
    #[serde(default)]
    count: bool,
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

pub struct CsvQuery {
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

impl CsvQuery {
    pub fn new(workspace: Workspace) -> Self {
        let purpose = "Filter, select, count or group the rows of a CSV file in the workspace. No expressions.";
        Self {
            workspace,
            spec: spec::<CsvArgs>(ToolName::CsvQuery, purpose, TIMEOUT),
        }
    }
}

#[async_trait]
impl Tool for FileList {
    fn spec(&self) -> &ToolSpec {
        &self.spec
    }

    fn check(&self, args: &Value) -> Result<Action, ToolError> {
        let args: ListArgs = parse(args)?;
        file_action("list", args.path.as_deref().unwrap_or("."))
    }

    /// Symlinks are listed as such and never followed.
    async fn run(&self, _ctx: &ToolCtx, args: Value) -> Result<ToolOutput, ToolError> {
        let args: ListArgs = parse(&args)?;
        let path = args.path.unwrap_or_else(|| ".".to_string());
        let dir = resolve(&self.workspace.root, &path).await?;
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
            let Ok(kind) = entry.file_type().await else {
                continue;
            };
            let size = entry.metadata().await.map(|m| m.len()).unwrap_or(0);
            let kind = if kind.is_symlink() {
                "symlink"
            } else if kind.is_dir() {
                "dir"
            } else {
                "file"
            };
            listed.push(
                json!({ "name": entry.file_name().to_string_lossy(), "kind": kind, "bytes": size }),
            );
        }
        Ok(ToolOutput {
            content: json!({ "path": path, "entries": listed }),
            taint: Vec::new(),
        })
    }
}

#[async_trait]
impl Tool for FileRead {
    fn spec(&self) -> &ToolSpec {
        &self.spec
    }

    fn check(&self, args: &Value) -> Result<Action, ToolError> {
        let args: ReadArgs = parse(args)?;
        file_action("read", &args.path)
    }

    /// A file with no trust row counts as untrusted; the seed records rows for its files.
    async fn run(&self, _ctx: &ToolCtx, args: Value) -> Result<ToolOutput, ToolError> {
        let args: ReadArgs = parse(&args)?;
        let path = display(&relative(&args.path)?);
        let (text, trust) = self.workspace.read_with_trust(&path).await?;
        let content = as_stored(&format!("file:{path}"), &text, trust.as_deref());
        Ok(ToolOutput {
            content: json!({ "path": path, "content": content, "trust": trust.unwrap_or_else(|| "untrusted".to_string()) }),
            taint: Vec::new(),
        })
    }
}

#[async_trait]
impl Tool for FileWrite {
    fn spec(&self) -> &ToolSpec {
        &self.spec
    }

    fn check(&self, args: &Value) -> Result<Action, ToolError> {
        let args: WriteArgs = parse(args)?;
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
        let args: WriteArgs = parse(&args)?;
        let path = display(&relative(&args.path)?);
        let run = to_text(&ctx.run_id).map_err(storage_failed)?;
        let failed = || ToolError::Failed(format!("{path} cannot be written"));
        let _exclusive = self.workspace.lock.write().await;

        let target = resolve(&self.workspace.root, &path).await?;
        let parent = target.parent().ok_or_else(failed)?.to_path_buf();
        tokio::fs::create_dir_all(&parent)
            .await
            .map_err(|_| failed())?;
        // Directories created just now are checked again before anything is written in them.
        resolve(&self.workspace.root, &path).await?;
        let replaced = tokio::fs::try_exists(&target).await.unwrap_or(false);

        self.workspace
            .record_trust(&path, Trust::Untrusted, &run)
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
        self.workspace.record_trust(&path, ctx.trust, &run).await?;

        let trust = to_text(&ctx.trust).map_err(storage_failed)?;
        Ok(ToolOutput {
            content: json!({ "path": path, "bytes": args.content.len(), "replaced": replaced, "trust": trust }),
            taint: Vec::new(),
        })
    }

    /// The card shows the whole new file and says when it replaces one.
    async fn preview(&self, args: &Value) -> String {
        let Ok(args) = parse::<WriteArgs>(args) else {
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

#[async_trait]
impl Tool for CsvQuery {
    fn spec(&self) -> &ToolSpec {
        &self.spec
    }

    fn check(&self, args: &Value) -> Result<Action, ToolError> {
        let args: CsvArgs = parse(args)?;
        if !args.path.to_ascii_lowercase().ends_with(".csv") {
            return Err(ToolError::InvalidArgs(
                "path: must be a .csv file".to_string(),
            ));
        }
        file_action("query", &args.path)
    }

    async fn run(&self, _ctx: &ToolCtx, args: Value) -> Result<ToolOutput, ToolError> {
        let args: CsvArgs = parse(&args)?;
        let path = display(&relative(&args.path)?);
        let (text, trust) = self.workspace.read_with_trust(&path).await?;
        let not_csv = || ToolError::Failed(format!("{path} is not valid CSV"));
        let mut reader = csv::Reader::from_reader(text.as_bytes());
        let headers: Vec<String> = reader
            .headers()
            .map_err(|_| not_csv())?
            .iter()
            .map(str::to_string)
            .collect();
        let column = |name: &str| {
            headers
                .iter()
                .position(|h| h == name)
                .ok_or_else(|| ToolError::InvalidArgs(format!("unknown column: {name}")))
        };

        let filters = args
            .filter
            .iter()
            .map(|f| Ok((column(&f.column)?, f.op, f.value.as_str())))
            .collect::<Result<Vec<_>, ToolError>>()?;
        let mut matching = Vec::new();
        for record in reader.records() {
            let record = record.map_err(|_| not_csv())?;
            let keep = filters
                .iter()
                .all(|(index, op, value)| matches(record.get(*index).unwrap_or(""), *op, value));
            if keep {
                matching.push(record);
            }
        }

        let result = if let Some(group) = &args.group_by {
            let index = column(group)?;
            let mut counts: BTreeMap<String, usize> = BTreeMap::new();
            for record in &matching {
                *counts
                    .entry(record.get(index).unwrap_or("").to_string())
                    .or_insert(0) += 1;
            }
            json!({ "group_by": group, "counts": counts })
        } else if args.count {
            json!({ "count": matching.len() })
        } else {
            let selected = if args.select.is_empty() {
                (0..headers.len()).collect()
            } else {
                args.select
                    .iter()
                    .map(|name| column(name))
                    .collect::<Result<Vec<_>, ToolError>>()?
            };
            let rows: Vec<Vec<&str>> = matching
                .iter()
                .take(ROW_LIMIT)
                .map(|record| {
                    selected
                        .iter()
                        .map(|i| record.get(*i).unwrap_or(""))
                        .collect()
                })
                .collect();
            let columns: Vec<&str> = selected.iter().map(|i| headers[*i].as_str()).collect();
            json!({ "columns": columns, "rows": rows, "total": matching.len() })
        };

        let content = if is_trusted(trust.as_deref()) {
            result
        } else {
            json!({ "trust": "untrusted", "result": untrusted(&format!("file:{path}"), &result.to_string()) })
        };
        Ok(ToolOutput {
            content,
            taint: Vec::new(),
        })
    }
}

fn matches(cell: &str, op: FilterOp, value: &str) -> bool {
    let numbers = cell
        .trim()
        .parse::<f64>()
        .ok()
        .zip(value.trim().parse::<f64>().ok());
    match op {
        FilterOp::Eq => cell == value,
        FilterOp::Ne => cell != value,
        FilterOp::Contains => cell.to_lowercase().contains(&value.to_lowercase()),
        FilterOp::Gt => numbers.map_or(cell > value, |(c, v)| c > v),
        FilterOp::Lt => numbers.map_or(cell < value, |(c, v)| c < v),
    }
}
