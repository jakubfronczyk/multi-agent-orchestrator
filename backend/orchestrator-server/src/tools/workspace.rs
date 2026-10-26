//! The workspace sandbox and the trust row recorded for each file in it.
//! Error messages name the path or the argument, never file contents (§6 rule 4).

use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use chrono::Utc;
use orchestrator_core::ids::{RunId, TeamId};
use orchestrator_core::policy::Trust;
use orchestrator_core::tools::ToolError;
use serde::Serialize;
use sqlx::SqlitePool;
use tokio::io::AsyncWriteExt;
use tokio::sync::RwLock;
use ts_rs::TS;
use uuid::Uuid;

use super::sandbox::{relative, resolve};
use super::{is_trusted, storage_failed};
use crate::sqlite::teams::{self, FileAccess};
use crate::sqlite::text::{escape_like, from_text, timestamp, to_text};

/// The largest file the file tools read or write, and so the largest upload.
pub const FILE_BYTES: usize = 256 * 1024;
/// The extensions a written or uploaded file may have.
pub const TEXT_EXTENSIONS: [&str; 4] = ["md", "csv", "json", "txt"];
/// The directory uploads go to, inside a team's folder.
pub const UPLOADS_DIR: &str = "uploads";
/// Holds the reference folder every team reads and no agent writes.
pub const SHARED_DIR: &str = "shared";
/// Holds one folder per team, named by its id.
pub const TEAMS_DIR: &str = "teams";
/// What an agent calls the shared reference folder.
const DATA_DIR: &str = "data";
const UPLOAD_NAME_CHARS: usize = 100;
/// How many numbered names an upload tries before it is refused as taken.
const UPLOAD_NUMBERS: u32 = 99;
const LIST_LIMIT: usize = 500;

/// Why the person's own file change was refused.
#[derive(Debug, thiserror::Error)]
pub enum PersonFileError {
    #[error(transparent)]
    Tool(#[from] ToolError),
    #[error("no file there")]
    NotFound,
    #[error("the file changed since you opened it")]
    Stale,
    #[error("a file is already there")]
    Exists,
    #[error("{0}")]
    Conflict(&'static str),
}

/// Why an upload was not stored.
#[derive(Debug, thiserror::Error)]
pub enum UploadError {
    #[error("every numbered name for this upload is taken")]
    Exists,
    #[error(transparent)]
    Tool(#[from] ToolError),
}

/// A browser's file name as a safe one: its last path part, ASCII letters, digits, `.`, `_` and
/// `-` kept, every other run of characters one `-`, at most 100 characters with the extension
/// kept. Saved names stay ASCII because trust rows compare names ignoring ASCII case only.
pub fn upload_name(raw: &str) -> String {
    let base = raw.rsplit(['/', '\\']).next().unwrap_or_default();
    let mut clean = String::new();
    for c in base.chars() {
        if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') {
            clean.push(c);
        } else if !clean.ends_with('-') {
            clean.push('-');
        }
    }
    let clean = clean.replace("-.", ".");
    let clean = clean.trim_start_matches(['.', '-']).trim_end_matches('-');
    let (stem, extension) = match clean.rsplit_once('.') {
        Some((stem, extension)) if !stem.is_empty() => (stem, Some(extension)),
        _ => (clean, None),
    };
    let stem = if stem.is_empty() { "upload" } else { stem };
    let room = UPLOAD_NAME_CHARS - extension.map_or(0, |e| e.len().min(20) + 1);
    let stem: String = stem.chars().take(room).collect();
    match extension {
        Some(extension) if !extension.is_empty() => {
            format!("{stem}.{}", &extension[..extension.len().min(20)])
        }
        _ => stem,
    }
}

/// `report.csv`, then `report-2.csv`, `report-3.csv`, ...
fn numbered(name: &str, n: u32) -> String {
    if n == 1 {
        return name.to_string();
    }
    match name.rsplit_once('.') {
        Some((stem, extension)) => format!("{stem}-{n}.{extension}"),
        None => format!("{name}-{n}"),
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

/// The task that last wrote a file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Writer {
    pub root_run_id: RunId,
    pub team_id: TeamId,
}

/// One directory entry; `path` is relative to the workspace root.
#[derive(Debug, Clone)]
pub struct Entry {
    pub name: String,
    pub path: String,
    pub kind: EntryKind,
    pub bytes: u64,
    pub trust: Trust,
    /// The task that last wrote the file; `None` for seeded files, uploads and directories.
    pub written_by: Option<Writer>,
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

    /// Whether a file is at `path`; a path outside the sandbox has none.
    pub async fn exists(&self, path: &str) -> bool {
        match resolve(&self.root, path).await {
            Ok(target) => tokio::fs::try_exists(&target).await.unwrap_or(false),
            Err(_) => false,
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
            let trust = self
                .entry_trust(&entry_path, kind == EntryKind::Dir)
                .await?;
            let written_by = if kind == EntryKind::File {
                self.written_by(&entry_path).await?
            } else {
                None
            };
            listed.push(Entry {
                name,
                path: entry_path,
                kind,
                bytes,
                trust,
                written_by,
            });
        }
        Ok(listed)
    }

    /// A file's text, its trust and the root run of the task that last wrote it, read together
    /// under the shared lock. The text is `None` for a file the agents cannot read: binary, or
    /// over 256 KB.
    pub async fn read(
        &self,
        path: &str,
    ) -> Result<(Option<String>, Trust, Option<Writer>), ToolError> {
        let _shared = self.lock.read().await;
        let bytes = self.read_bytes(path).await?;
        let text = (bytes.len() <= FILE_BYTES)
            .then(|| String::from_utf8(bytes).ok())
            .flatten();
        Ok((
            text,
            self.trust_of(path).await?,
            self.written_by(path).await?,
        ))
    }

    /// A file's text and trust, read together under the shared lock.
    pub(super) async fn read_with_trust(&self, path: &str) -> Result<(String, Trust), ToolError> {
        let _shared = self.lock.read().await;
        let text = self.read_text(path).await?;
        Ok((text, self.trust_of(path).await?))
    }

    /// Saves an upload in `team`'s folder under its cleaned name and returns its path as the
    /// team's agents name it, `uploads/<name>`. A taken name gets the next free number; an
    /// existing upload is never replaced. The row is recorded untrusted, with no writing run,
    /// before any byte is written.
    pub async fn store_upload(
        &self,
        team: TeamId,
        name: &str,
        bytes: &[u8],
    ) -> Result<String, UploadError> {
        let name = upload_name(name);
        let _exclusive = self.lock.write().await;
        for n in 1..=UPLOAD_NUMBERS {
            let shown = format!("{UPLOADS_DIR}/{}", numbered(&name, n));
            let path = format!("{}/{shown}", team_dir(team)?);
            let target = self.prepare(&path).await?;
            if tokio::fs::try_exists(&target).await.unwrap_or(true)
                || self.case_twin(&path).await?
                || self.trust_row(&path).await?
            {
                continue;
            }
            self.record_trust(&path, Trust::Untrusted, None).await?;
            write_atomic(&target, bytes)
                .await
                .map_err(|_| cannot_write(&path))?;
            return Ok(shown);
        }
        Err(UploadError::Exists)
    }

    /// Writes or replaces a file at `path` of `team`'s view for `run`, and returns the path as
    /// shown and whether it replaced one. Reach is checked under the exclusive lock, which grant
    /// changes and team deletes also take, so a revoke cannot land between check and write. The
    /// row is marked untrusted before any byte changes and set to `trust` only after the rename,
    /// so a failure at any step leaves "untrusted".
    pub(super) async fn write_for(
        &self,
        team: TeamId,
        path: &str,
        text: &str,
        run: RunId,
        trust: Trust,
    ) -> Result<(String, bool), ToolError> {
        let run = to_text(&run).map_err(storage_failed)?;
        let _exclusive = self.lock.write().await;
        let at = self.locate(team, path, Need::Write).await?;
        if self.case_twin(&at.physical).await? {
            return Err(ToolError::Failed(format!(
                "{} differs only in letter case from a file that exists; use the name the file list shows",
                at.shown
            )));
        }
        let path = at.physical.as_str();
        let target = self.prepare(path).await?;
        let replaced = tokio::fs::try_exists(&target).await.unwrap_or(false);
        self.record_trust(path, Trust::Untrusted, Some(&run))
            .await?;
        write_atomic(&target, text.as_bytes())
            .await
            .map_err(|_| cannot_write(path))?;
        self.record_trust(path, trust, Some(&run)).await?;
        Ok((at.shown, replaced))
    }

    /// When the file's row last changed and whether the person wrote it; `None` with no row.
    pub async fn stamp(&self, path: &str) -> Result<Option<(String, bool)>, ToolError> {
        let row = sqlx::query!(
            "SELECT updated_at, written_by_user FROM workspace_files WHERE path = ?",
            path
        )
        .fetch_optional(&self.pool)
        .await
        .map_err(storage_failed)?;
        Ok(row.map(|row| (row.updated_at, row.written_by_user != 0)))
    }

    /// The person's own save into `team`'s own folder, with the rules an agent's save has. A new
    /// file is trusted, the person's words; a replaced one keeps its label, since the editor
    /// starts from the old text. `expected` is the `updated_at` the person saw: `None` creates
    /// only, and a file changed since is `Stale`. Returns the path as shown and whether it was new.
    pub async fn person_write(
        &self,
        team: TeamId,
        path: &str,
        text: &str,
        expected: Option<&str>,
    ) -> Result<(String, bool), PersonFileError> {
        let _exclusive = self.lock.write().await;
        let at = self.person_place(team, path).await?;
        let stamp = self.stamp(&at.physical).await?;
        let exists = self.kind_of(&at.physical).await?.is_some();
        let trust = match (exists || stamp.is_some(), expected, &stamp) {
            (false, None, _) => Trust::Trusted,
            (false, Some(_), _) => return Err(PersonFileError::Stale),
            (true, None, _) => return Err(PersonFileError::Exists),
            (true, Some(seen), Some((updated, _))) if seen == updated => {
                self.trust_of(&at.physical).await?
            }
            (true, Some(_), _) => return Err(PersonFileError::Stale),
        };
        let target = self.prepare(&at.physical).await?;
        self.record_person(&at.physical, Trust::Untrusted).await?;
        if write_atomic(&target, text.as_bytes()).await.is_err() {
            // A new file that was never written leaves no record behind, so the path stays free;
            // a replaced one keeps its record, untrusted, as a failed write always does.
            if !exists {
                sqlx::query!("DELETE FROM workspace_files WHERE path = ?", at.physical)
                    .execute(&self.pool)
                    .await
                    .map_err(storage_failed)?;
            }
            return Err(cannot_write(&at.physical).into());
        }
        self.record_person(&at.physical, trust).await?;
        Ok((at.shown, !exists))
    }

    /// Removes a file of `team`'s own folder and its row; `expected` as for `person_write`.
    pub async fn person_delete(
        &self,
        team: TeamId,
        path: &str,
        expected: &str,
    ) -> Result<(), PersonFileError> {
        let _exclusive = self.lock.write().await;
        let at = self.person_place(team, path).await?;
        self.check_stamp(&at.physical, expected).await?;
        let target = resolve(&self.root, &at.physical).await?;
        tokio::fs::remove_file(&target)
            .await
            .map_err(|_| cannot_write(&at.physical))?;
        sqlx::query!("DELETE FROM workspace_files WHERE path = ?", at.physical)
            .execute(&self.pool)
            .await
            .map_err(storage_failed)?;
        Ok(())
    }

    /// The person vouches for a file of `team`'s own folder, an upload included: its row becomes
    /// trusted. A task that already read it stays tainted; nothing clears taint.
    pub async fn person_trust(
        &self,
        team: TeamId,
        path: &str,
        expected: &str,
    ) -> Result<(), PersonFileError> {
        let _exclusive = self.lock.write().await;
        let at = self.person_place(team, path).await?;
        self.check_stamp(&at.physical, expected).await?;
        if self.trust_of(&at.physical).await? == Trust::Trusted {
            return Err(PersonFileError::Conflict("the file is already trusted"));
        }
        let trusted = to_text(&Trust::Trusted).map_err(storage_failed)?;
        let now = timestamp(Utc::now());
        sqlx::query!(
            "UPDATE workspace_files SET trust = ?, updated_at = ? WHERE path = ?",
            trusted,
            now,
            at.physical
        )
        .execute(&self.pool)
        .await
        .map_err(storage_failed)?;
        Ok(())
    }

    /// Whether the person may change `path` from `team`: its own folder, not `data/`.
    pub async fn person_editable(&self, team: TeamId, path: &str) -> bool {
        self.person_place(team, path).await.is_ok()
    }

    /// A path of the team's own folder: never `data/` and never another team's folder, which the
    /// person edits from that team.
    async fn person_place(&self, team: TeamId, path: &str) -> Result<Located, PersonFileError> {
        let clean = display(&relative(path)?);
        if clean
            .to_ascii_lowercase()
            .starts_with(&format!("{TEAMS_DIR}/"))
        {
            return Err(PersonFileError::Tool(ToolError::InvalidArgs(
                "path: edit another team's files from that team".to_string(),
            )));
        }
        if !clean.is_ascii() {
            return Err(PersonFileError::Tool(ToolError::InvalidArgs(
                "path: use ASCII letters, digits and punctuation".to_string(),
            )));
        }
        // Here a refused path is the person's request, not a failure of the server.
        let at = self
            .locate(team, &clean, Need::Write)
            .await
            .map_err(|error| match error {
                ToolError::Failed(message) => ToolError::InvalidArgs(format!("path: {message}")),
                other => other,
            })?;
        if self.case_twin(&at.physical).await? {
            return Err(PersonFileError::Tool(ToolError::InvalidArgs(format!(
                "path: {} differs only in letter case from a file that exists",
                at.shown
            ))));
        }
        Ok(at)
    }

    async fn check_stamp(&self, physical: &str, expected: &str) -> Result<(), PersonFileError> {
        match self.stamp(physical).await? {
            None => Err(PersonFileError::NotFound),
            Some((updated, _)) if updated == expected => Ok(()),
            Some(_) => Err(PersonFileError::Stale),
        }
    }

    /// A row written by the person: no run, marked as theirs.
    async fn record_person(&self, path: &str, trust: Trust) -> Result<(), ToolError> {
        let trust = to_text(&trust).map_err(storage_failed)?;
        let now = timestamp(Utc::now());
        sqlx::query!(
            "INSERT INTO workspace_files (path, trust, written_by_run, written_by_user, updated_at) VALUES (?, ?, NULL, 1, ?)
             ON CONFLICT (path) DO UPDATE SET trust = excluded.trust, written_by_run = NULL,
                                              written_by_user = 1, updated_at = excluded.updated_at",
            path,
            trust,
            now
        )
        .execute(&self.pool)
        .await
        .map_err(storage_failed)?;
        Ok(())
    }

    /// Holds every file operation off while a caller changes what a team may reach.
    pub async fn exclusive(&self) -> tokio::sync::OwnedRwLockWriteGuard<()> {
        Arc::clone(&self.lock).write_owned().await
    }

    /// Whether `path` has a trust row, in any letter case.
    async fn trust_row(&self, path: &str) -> Result<bool, ToolError> {
        let rows = sqlx::query_scalar!("SELECT COUNT(*) FROM workspace_files WHERE path = ?", path)
            .fetch_one(&self.pool)
            .await
            .map_err(storage_failed)?;
        Ok(rows > 0)
    }

    /// Whether a recorded file's path equals `path` ignoring letter case but not exactly. Such
    /// a path is the same file on a case-insensitive disk, so writing it is refused.
    async fn case_twin(&self, path: &str) -> Result<bool, ToolError> {
        let twins = sqlx::query_scalar!(
            "SELECT COUNT(*) FROM workspace_files WHERE path = ?1 AND path COLLATE BINARY <> ?1",
            path
        )
        .fetch_one(&self.pool)
        .await
        .map_err(storage_failed)?;
        Ok(twins > 0)
    }

    /// Resolves the target and creates its directory; directories this call created are
    /// checked again before anything is written in them.
    async fn prepare(&self, path: &str) -> Result<PathBuf, ToolError> {
        let target = resolve(&self.root, path).await?;
        let parent = target.parent().ok_or_else(|| cannot_write(path))?;
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|_| cannot_write(path))?;
        resolve(&self.root, path).await?;
        Ok(target)
    }

    /// A directory has no row of its own: it is untrusted when any recorded file under it is,
    /// or when none under it is recorded.
    async fn entry_trust(&self, path: &str, is_dir: bool) -> Result<Trust, ToolError> {
        if !is_dir {
            return self.trust_of(path).await;
        }
        let pattern = format!("{}/%", escape_like(path));
        let trusts = sqlx::query_scalar!(
            "SELECT trust FROM workspace_files WHERE path LIKE ? ESCAPE '\\'",
            pattern
        )
        .fetch_all(&self.pool)
        .await
        .map_err(storage_failed)?;
        let trusted = !trusts.is_empty() && trusts.iter().all(|trust| is_trusted(Some(trust)));
        Ok(trust_from(trusted))
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
            "INSERT INTO workspace_files (path, trust, written_by_run, written_by_user, updated_at) VALUES (?, ?, ?, 0, ?)
             ON CONFLICT (path) DO UPDATE SET trust = excluded.trust, written_by_run = excluded.written_by_run,
                                              written_by_user = 0, updated_at = excluded.updated_at",
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

    /// `None` for a file no run wrote, such as a seeded one.
    async fn written_by(&self, path: &str) -> Result<Option<Writer>, ToolError> {
        let row = sqlx::query!(
            "SELECT r.root_run_id, r.team_id FROM workspace_files f JOIN runs r ON r.id = f.written_by_run
             WHERE f.path = ?",
            path
        )
        .fetch_optional(&self.pool)
        .await
        .map_err(storage_failed)?;
        row.map(|row| {
            Ok(Writer {
                root_run_id: from_text(&row.root_run_id).map_err(storage_failed)?,
                team_id: from_text(&row.team_id).map_err(storage_failed)?,
            })
        })
        .transpose()
    }

    /// A file with no row, or a row whose value does not decode, is untrusted.
    async fn trust_of(&self, path: &str) -> Result<Trust, ToolError> {
        let trust = sqlx::query_scalar!("SELECT trust FROM workspace_files WHERE path = ?", path)
            .fetch_optional(&self.pool)
            .await
            .map_err(storage_failed)?;
        Ok(trust_from(is_trusted(trust.as_deref())))
    }

    /// Reads a text file inside the sandbox, at most 256 KB.
    async fn read_bytes(&self, path: &str) -> Result<Vec<u8>, ToolError> {
        let full = resolve(&self.root, path).await?;
        match tokio::fs::read(&full).await {
            Ok(bytes) => Ok(bytes),
            Err(error) if error.kind() == ErrorKind::NotFound => {
                Err(ToolError::Failed(format!("no file at {path}")))
            }
            Err(_) => Err(ToolError::Failed(format!("{path} cannot be read"))),
        }
    }

    async fn read_text(&self, path: &str) -> Result<String, ToolError> {
        let bytes = self.read_bytes(path).await?;
        if bytes.len() > FILE_BYTES {
            return Err(ToolError::Failed(format!("{path} is larger than 256 KB")));
        }
        String::from_utf8(bytes).map_err(|_| {
            ToolError::Failed(format!(
                "{path} is a binary file the agents cannot read; ask the person for a CSV or text version"
            ))
        })
    }
}

/// What a call needs from a path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Need {
    Read,
    Write,
}

/// A path as one team's agents name it, and where it is on disk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Located {
    /// Relative to the workspace root: `shared/data/...` or `teams/<team id>/...`.
    pub physical: String,
    /// As the agent named it, normalised; results, cards and taint sources use it.
    pub shown: String,
}

/// Where a path of a team's view points before any I/O beyond the grant lookup.
enum Place {
    /// `.`: the own folder's entries plus `data` and `teams`.
    Root,
    /// `teams`: the folders shared with this team.
    Teams,
    File(Located),
}

impl Workspace {
    /// Maps a path as `team`'s agents name it: `data/...` is the shared reference folder, read
    /// only; `teams/<slug>/...` is that team's folder, its own or one granted to it; anything
    /// else is in its own folder. Grants are read on every call, so a revoke applies at once.
    pub async fn locate(&self, team: TeamId, path: &str, need: Need) -> Result<Located, ToolError> {
        match self.place(team, path, need).await? {
            Place::File(located) => Ok(located),
            Place::Root | Place::Teams => Err(ToolError::Failed(format!(
                "{} is a directory",
                display(&relative(path)?)
            ))),
        }
    }

    /// Whether `team`'s agents may use `path` for `need`; access only, so a missing file or a
    /// directory named as a file is left for the call itself to report.
    pub async fn reach(&self, team: TeamId, path: &str, need: Need) -> Result<(), ToolError> {
        self.place(team, path, need).await.map(drop)
    }

    /// The path as `team`'s agents name it, normalised, and what is there in their view.
    pub async fn kind_for(
        &self,
        team: TeamId,
        path: &str,
    ) -> Result<(String, Option<EntryKind>), ToolError> {
        let shown = display(&relative(path)?);
        match self.place(team, path, Need::Read).await? {
            Place::Root | Place::Teams => Ok((shown, Some(EntryKind::Dir))),
            Place::File(at) => Ok((at.shown, self.kind_of(&at.physical).await?)),
        }
    }

    /// `path` of `team`'s view as a directory listing; entry paths are in the same view.
    pub async fn list_for(&self, team: TeamId, path: &str) -> Result<Vec<Entry>, ToolError> {
        match self.place(team, path, Need::Read).await? {
            Place::Root => {
                let own = team_dir(team)?;
                let mut entries = if self.kind_of(&own).await?.is_some() {
                    self.list(&own).await?
                } else {
                    Vec::new()
                };
                let base = format!("{own}/");
                for entry in &mut entries {
                    entry.path = entry
                        .path
                        .strip_prefix(&base)
                        .unwrap_or(&entry.name)
                        .to_string();
                }
                let data = format!("{SHARED_DIR}/{DATA_DIR}");
                if self.kind_of(&data).await?.is_some() {
                    entries.push(self.folder(DATA_DIR, DATA_DIR, &[data]).await?);
                }
                let granted = self.granted(team).await?;
                if !granted.is_empty() {
                    let dirs: Vec<String> = granted
                        .iter()
                        .map(|(_, id, _)| team_dir(*id))
                        .collect::<Result<_, _>>()?;
                    entries.push(self.folder(TEAMS_DIR, TEAMS_DIR, &dirs).await?);
                }
                Ok(entries)
            }
            Place::Teams => {
                let mut entries = Vec::new();
                for (slug, id, _) in self.granted(team).await? {
                    let shown = format!("{TEAMS_DIR}/{slug}");
                    entries.push(self.folder(&slug, &shown, &[team_dir(id)?]).await?);
                }
                Ok(entries)
            }
            Place::File(located) => {
                let mut entries = self.list(&located.physical).await?;
                let (physical, shown) = (
                    format!("{}/", located.physical),
                    format!("{}/", located.shown),
                );
                for entry in &mut entries {
                    if let Some(rest) = entry.path.strip_prefix(&physical) {
                        entry.path = format!("{shown}{rest}");
                    }
                }
                Ok(entries)
            }
        }
    }

    async fn place(&self, team: TeamId, path: &str, need: Need) -> Result<Place, ToolError> {
        let clean = relative(path)?;
        let shown = display(&clean);
        let parts: Vec<String> = clean
            .components()
            .map(|part| part.as_os_str().to_string_lossy().into_owned())
            .filter(|part| part != ".")
            .collect();
        match parts.as_slice() {
            [] => Ok(Place::Root),
            [first, ..] if first == DATA_DIR => {
                if need == Need::Write {
                    return Err(ToolError::Failed(
                        "data/ is shared reference and read-only; save in your own folder"
                            .to_string(),
                    ));
                }
                Ok(Place::File(Located {
                    physical: format!("{SHARED_DIR}/{shown}"),
                    shown,
                }))
            }
            [first] if first == TEAMS_DIR => Ok(Place::Teams),
            [first, slug, rest @ ..] if first == TEAMS_DIR => {
                let owner = self.team_by_slug(slug).await?;
                let allowed = match owner {
                    Some(owner) if owner == team => true,
                    Some(owner) => {
                        self.grant(team, owner)
                            .await?
                            .is_some_and(|access| match need {
                                Need::Read => {
                                    matches!(access, FileAccess::Read | FileAccess::ReadWrite)
                                }
                                Need::Write => access == FileAccess::ReadWrite,
                            })
                    }
                    None => false,
                };
                let Some(owner) = owner.filter(|_| allowed) else {
                    return Err(ToolError::Failed(format!(
                        "{TEAMS_DIR}/{slug} is not shared with this team{}",
                        if need == Need::Write {
                            " for writing"
                        } else {
                            ""
                        }
                    )));
                };
                let mut physical = team_dir(owner)?;
                for part in rest {
                    physical.push('/');
                    physical.push_str(part);
                }
                Ok(Place::File(Located { physical, shown }))
            }
            _ => Ok(Place::File(Located {
                physical: format!("{}/{shown}", team_dir(team)?),
                shown,
            })),
        }
    }

    /// A folder entry named by the server; untrusted when anything recorded under any of
    /// `physical` is, or nothing is.
    async fn folder(
        &self,
        name: &str,
        shown: &str,
        physical: &[String],
    ) -> Result<Entry, ToolError> {
        let mut trust = Trust::Trusted;
        for dir in physical {
            if self.entry_trust(dir, true).await? == Trust::Untrusted {
                trust = Trust::Untrusted;
            }
        }
        Ok(Entry {
            name: name.to_string(),
            path: shown.to_string(),
            kind: EntryKind::Dir,
            bytes: 0,
            trust,
            written_by: None,
        })
    }

    async fn team_by_slug(&self, slug: &str) -> Result<Option<TeamId>, ToolError> {
        let mut conn = self.pool.acquire().await.map_err(storage_failed)?;
        teams::by_slug(&mut conn, slug)
            .await
            .map_err(storage_failed)
    }

    async fn grant(&self, team: TeamId, of: TeamId) -> Result<Option<FileAccess>, ToolError> {
        let (team, of) = (
            to_text(&team).map_err(storage_failed)?,
            to_text(&of).map_err(storage_failed)?,
        );
        let access = sqlx::query_scalar!(
            "SELECT g.access FROM team_file_grants g JOIN teams t ON t.id = g.of_team_id
             WHERE g.team_id = ? AND g.of_team_id = ? AND t.deleted_at IS NULL",
            team,
            of
        )
        .fetch_optional(&self.pool)
        .await
        .map_err(storage_failed)?;
        access
            .map(|a| from_text(&a).map_err(storage_failed))
            .transpose()
    }

    /// The live teams whose folders `team` may open, by slug.
    async fn granted(&self, team: TeamId) -> Result<Vec<(String, TeamId, FileAccess)>, ToolError> {
        let team = to_text(&team).map_err(storage_failed)?;
        let rows = sqlx::query!(
            "SELECT t.slug, t.id, g.access FROM team_file_grants g JOIN teams t ON t.id = g.of_team_id
             WHERE g.team_id = ? AND t.deleted_at IS NULL ORDER BY t.slug",
            team
        )
        .fetch_all(&self.pool)
        .await
        .map_err(storage_failed)?;
        rows.into_iter()
            .map(|row| {
                Ok((
                    row.slug,
                    from_text(&row.id).map_err(storage_failed)?,
                    from_text(&row.access).map_err(storage_failed)?,
                ))
            })
            .collect()
    }
}

fn team_dir(team: TeamId) -> Result<String, ToolError> {
    Ok(format!(
        "{TEAMS_DIR}/{}",
        to_text(&team).map_err(storage_failed)?
    ))
}

/// Writes a fresh temp file next to `target`, syncs it and renames it over the target; `rename`
/// is atomic only within one filesystem. The temp file is removed when any step fails.
async fn write_atomic(target: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let name = target
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let temp = target.with_file_name(format!(".{name}.{}.tmp", Uuid::now_v7()));
    let written = async {
        let mut file = tokio::fs::File::create_new(&temp).await?;
        file.write_all(bytes).await?;
        file.sync_all().await?;
        tokio::fs::rename(&temp, target).await
    }
    .await;
    if written.is_err() {
        let _ = tokio::fs::remove_file(&temp).await;
    }
    written
}

fn cannot_write(path: &str) -> ToolError {
    ToolError::Failed(format!("{path} cannot be written"))
}

fn trust_from(trusted: bool) -> Trust {
    if trusted {
        Trust::Trusted
    } else {
        Trust::Untrusted
    }
}

/// The path as stored and shown: relative, `/`-separated, no `.` parts.
pub(super) fn display(path: &Path) -> String {
    path.components()
        .map(|part| part.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

#[cfg(test)]
mod upload_tests {
    use super::*;

    #[test]
    fn any_name_becomes_a_safe_one() {
        for (raw, clean) in [
            ("report.csv", "report.csv"),
            ("Raport Q3 (final).CSV", "Raport-Q3-final.CSV"),
            ("../../etc/passwd", "passwd"),
            ("C:\\Users\\me\\notes.md", "notes.md"),
            (".env", "env"),
            ("Zespół łódź.txt", "Zesp-d.txt"),
            ("   ", "upload"),
            ("no-extension", "no-extension"),
        ] {
            assert_eq!(upload_name(raw), clean, "{raw:?}");
        }
        assert!(upload_name(&format!("{}.csv", "a".repeat(300))).len() <= UPLOAD_NAME_CHARS);
        assert_eq!(numbered("report.csv", 2), "report-2.csv");
        assert_eq!(numbered("notes", 3), "notes-3");
    }
}
