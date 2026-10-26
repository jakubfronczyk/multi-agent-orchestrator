//! File tools over the workspace sandbox: list, read and write; `csv` holds the CSV query.
//! Error messages name the path or the argument, never file contents (§6 rule 4).

use std::path::Path;
use std::time::Duration;

use async_trait::async_trait;
use orchestrator_core::ids::TeamId;
use orchestrator_core::policy::{TaintSource, Trust};
use orchestrator_core::tools::{
    Action, Target, Tool, ToolCtx, ToolError, ToolName, ToolOutput, ToolSpec, args_preview,
    parse_args,
};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};

use super::sandbox::relative;
use super::spec;
use super::workspace::{FILE_BYTES, Need, TEXT_EXTENSIONS, Workspace, display};

const TIMEOUT: Duration = Duration::from_secs(5);

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
        let purpose = "List the files in a workspace directory. Use it to find a path before reading. \
                       The root is your team's folder, with data/ (shared reference, read only) and \
                       teams/ (folders other teams share with you).";
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
        let purpose = "Save a .md, .csv, .json or .txt file in your team's folder, replacing any file at that path. \
                       data/ is read only.";
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

    async fn reach(&self, team: TeamId, args: &Value) -> Result<(), ToolError> {
        let args: ListArgs = parse_args(args)?;
        self.workspace
            .reach(team, args.path.as_deref().unwrap_or("."), Need::Read)
            .await
    }

    /// Symlinks are listed as such and never followed.
    /// Entry names are stored text too, so each untrusted entry is a taint source.
    async fn run(&self, ctx: &ToolCtx, args: Value) -> Result<ToolOutput, ToolError> {
        let args: ListArgs = parse_args(&args)?;
        let path = display(&relative(args.path.as_deref().unwrap_or("."))?);
        let entries = self.workspace.list_for(ctx.team_id, &path).await?;
        let taint = entries
            .iter()
            .filter(|entry| entry.trust == Trust::Untrusted)
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

    async fn reach(&self, team: TeamId, args: &Value) -> Result<(), ToolError> {
        let args: ReadArgs = parse_args(args)?;
        self.workspace.reach(team, &args.path, Need::Read).await
    }

    /// A file with no trust row counts as untrusted; the seed records rows for its files.
    async fn run(&self, ctx: &ToolCtx, args: Value) -> Result<ToolOutput, ToolError> {
        let args: ReadArgs = parse_args(&args)?;
        let at = self
            .workspace
            .locate(ctx.team_id, &args.path, Need::Read)
            .await?;
        let (text, trust) = self.workspace.read_with_trust(&at.physical).await?;
        let path = at.shown;
        Ok(ToolOutput {
            content: json!({ "path": path, "content": text, "trust": trust }),
            taint: file_taint(&path, trust),
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
        // Letter case is compared for ASCII only, so a saved path stays within it.
        if !args.path.is_ascii() {
            return Err(ToolError::InvalidArgs(
                "path: use ASCII letters, digits and punctuation".to_string(),
            ));
        }
        let extension = Path::new(&args.path)
            .extension()
            .and_then(|e| e.to_str())
            .map(str::to_ascii_lowercase);
        if !extension.is_some_and(|e| TEXT_EXTENSIONS.contains(&e.as_str())) {
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

    /// The temp file sits next to its target and is renamed over it, so a reader sees the old
    /// file or the new one, never part of either.
    async fn run(&self, ctx: &ToolCtx, args: Value) -> Result<ToolOutput, ToolError> {
        let args: WriteArgs = parse_args(&args)?;
        let (path, replaced) = self
            .workspace
            .write_for(
                ctx.team_id,
                &args.path,
                &args.content,
                ctx.run_id,
                ctx.trust,
            )
            .await?;
        Ok(ToolOutput {
            content: json!({ "path": path, "bytes": args.content.len(), "replaced": replaced, "trust": ctx.trust }),
            taint: Vec::new(),
        })
    }

    /// The card shows the whole new file and says when it replaces one.
    async fn reach(&self, team: TeamId, args: &Value) -> Result<(), ToolError> {
        let args: WriteArgs = parse_args(args)?;
        self.workspace.reach(team, &args.path, Need::Write).await
    }

    async fn preview(&self, team: TeamId, args: &Value) -> String {
        let Ok(args) = parse_args::<WriteArgs>(args) else {
            return args_preview(args);
        };
        let exists = match self.workspace.locate(team, &args.path, Need::Write).await {
            Ok(at) => self.workspace.exists(&at.physical).await,
            Err(_) => false,
        };
        let note = if exists {
            "\n\n(replaces an existing file)"
        } else {
            ""
        };
        format!("{}\n\n{}{note}", args.path, args.content)
    }
}

/// A file read back from storage taints the reader unless its row says trusted.
pub(super) fn file_taint(path: &str, trust: Trust) -> Vec<TaintSource> {
    match trust {
        Trust::Trusted => Vec::new(),
        Trust::Untrusted => vec![TaintSource::File {
            path: path.to_string(),
        }],
    }
}
