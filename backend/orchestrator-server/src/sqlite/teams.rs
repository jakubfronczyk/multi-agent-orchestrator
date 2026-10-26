//! Teams: the unit that groups agents and tasks. No policy reads a team (ADR 0007).
//! A team is created with its Lead in one transaction, so every live team has one.

use chrono::{DateTime, Utc};
use orchestrator_core::agents::{Role, ValidationError};
use orchestrator_core::ids::{AgentId, TeamId};
use serde::{Deserialize, Serialize};
use sqlx::{SqliteConnection, SqlitePool};
use ts_rs::TS;

use super::agents::{self, AgentError};
use super::begin_write;
use super::text::{from_text, parse_timestamp, timestamp, to_text};
use crate::seed;

const NAME_CHARS: usize = 60;
const SLUG_CHARS: usize = 40;

#[derive(Debug, Clone, Deserialize, TS)]
#[ts(export)]
pub struct TeamInput {
    pub name: String,
}

#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
pub struct TeamView {
    pub id: TeamId,
    pub name: String,
    /// Fixed at create, unique among live teams; agents reach the team's folder as `teams/<slug>/`.
    pub slug: String,
    /// Stays set on a deleted team: it names the soft-deleted Lead, so history can show it.
    pub lead_agent_id: AgentId,
    /// Live agents, the Lead included; 0 on a deleted team.
    pub agent_count: i64,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub deleted_at: Option<DateTime<Utc>>,
}

/// How much of another team's folder a team's agents may open; `none` is stored as no row.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum FileAccess {
    None,
    Read,
    ReadWrite,
}

#[derive(Debug, Clone, Deserialize, TS)]
#[ts(export)]
pub struct FileAccessInput {
    pub access: FileAccess,
}

/// Another live team and how much of its folder this team's agents may open.
#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
pub struct FileAccessView {
    pub team_id: TeamId,
    pub name: String,
    pub slug: String,
    pub access: FileAccess,
}

#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
pub struct FileAccessList {
    /// Every other live team, oldest first.
    pub teams: Vec<FileAccessView>,
}

#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
pub struct TeamList {
    /// Oldest first.
    pub teams: Vec<TeamView>,
}

#[derive(Debug, thiserror::Error)]
pub enum TeamError {
    #[error(transparent)]
    Invalid(#[from] ValidationError),
    #[error("team not found")]
    NotFound,
    #[error("{0}")]
    Conflict(&'static str),
    #[error(transparent)]
    Agent(#[from] AgentError),
    #[error("storage failed: {0}")]
    Storage(#[from] sqlx::Error),
}

pub async fn list(pool: &SqlitePool, include_deleted: bool) -> Result<TeamList, TeamError> {
    let mut tx = pool.begin().await?;
    let ids = sqlx::query_scalar!(
        "SELECT id FROM teams WHERE ?1 OR deleted_at IS NULL ORDER BY created_at, id",
        include_deleted
    )
    .fetch_all(&mut *tx)
    .await?;
    let mut teams = Vec::with_capacity(ids.len());
    for id in ids {
        teams.push(view(&mut tx, &id).await?.ok_or(TeamError::NotFound)?);
    }
    Ok(TeamList { teams })
}

/// A team, deleted ones included.
pub async fn get(pool: &SqlitePool, team: TeamId) -> Result<TeamView, TeamError> {
    let mut conn = pool.acquire().await?;
    view(&mut conn, &text(&team)?)
        .await?
        .ok_or(TeamError::NotFound)
}

/// The team and its default Lead in one write transaction.
pub async fn create(pool: &SqlitePool, input: TeamInput) -> Result<TeamView, TeamError> {
    let name = valid_name(&input.name)?;
    let mut tx = begin_write(pool).await?;
    name_free(&mut tx, name, None).await?;
    let team = insert(&mut tx, name).await?;
    agents::insert(&mut tx, team, seed::default_lead()).await?;
    let view = view(&mut tx, &text(&team)?)
        .await?
        .ok_or(TeamError::NotFound)?;
    tx.commit().await?;
    Ok(view)
}

/// Renames a live team.
pub async fn rename(
    pool: &SqlitePool,
    team: TeamId,
    input: TeamInput,
) -> Result<TeamView, TeamError> {
    let name = valid_name(&input.name)?;
    let mut tx = begin_write(pool).await?;
    if !is_live(&mut tx, team).await? {
        return Err(TeamError::NotFound);
    }
    name_free(&mut tx, name, Some(team)).await?;
    let id = text(&team)?;
    let now = timestamp(Utc::now());
    sqlx::query!(
        "UPDATE teams SET name = ?, updated_at = ? WHERE id = ?",
        name,
        now,
        id
    )
    .execute(&mut *tx)
    .await?;
    let view = view(&mut tx, &id).await?.ok_or(TeamError::NotFound)?;
    tx.commit().await?;
    Ok(view)
}

/// Soft-deletes a live team and its live agents. `is_busy` is checked while the write lock is
/// held: a start registers its task before its `run_started` commits, so either the delete sees
/// it, or the start's projection finds the team gone and its emit fails.
pub async fn delete(
    pool: &SqlitePool,
    team: TeamId,
    is_busy: impl Fn() -> bool,
) -> Result<(), TeamError> {
    let mut tx = begin_write(pool).await?;
    if !is_live(&mut tx, team).await? {
        return Err(TeamError::NotFound);
    }
    if is_busy() {
        return Err(TeamError::Conflict("this team has a task running"));
    }
    let live = sqlx::query_scalar!("SELECT COUNT(*) FROM teams WHERE deleted_at IS NULL")
        .fetch_one(&mut *tx)
        .await?;
    if live <= 1 {
        return Err(TeamError::Conflict("keep at least one team"));
    }
    let id = text(&team)?;
    let now = timestamp(Utc::now());
    sqlx::query!(
        "UPDATE agents SET deleted_at = ? WHERE team_id = ? AND deleted_at IS NULL",
        now,
        id
    )
    .execute(&mut *tx)
    .await?;
    sqlx::query!("UPDATE teams SET deleted_at = ? WHERE id = ?", now, id)
        .execute(&mut *tx)
        .await?;
    sqlx::query!(
        "DELETE FROM team_file_grants WHERE team_id = ?1 OR of_team_id = ?1",
        id
    )
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(())
}

/// What every other live team's folder `team` may open. 404 for an unknown or deleted team.
pub async fn file_access(pool: &SqlitePool, team: TeamId) -> Result<FileAccessList, TeamError> {
    let mut tx = pool.begin().await?;
    if !is_live(&mut tx, team).await? {
        return Err(TeamError::NotFound);
    }
    let id = text(&team)?;
    let rows = sqlx::query!(
        r#"SELECT t.id, t.name, t.slug, g.access AS "access?"
           FROM teams t
           LEFT JOIN team_file_grants g ON g.of_team_id = t.id AND g.team_id = ?1
           WHERE t.deleted_at IS NULL AND t.id != ?1
           ORDER BY t.created_at, t.id"#,
        id
    )
    .fetch_all(&mut *tx)
    .await?;
    let teams = rows
        .into_iter()
        .map(|row| {
            let access = match row.access {
                Some(access) => from_text(&access).map_err(decode)?,
                None => FileAccess::None,
            };
            Ok(FileAccessView {
                team_id: from_text(&row.id).map_err(decode)?,
                name: row.name,
                slug: row.slug,
                access,
            })
        })
        .collect::<Result<_, sqlx::Error>>()?;
    Ok(FileAccessList { teams })
}

/// Sets how much of `of`'s folder `team` may open; `none` removes the grant. Both teams must be
/// live and differ.
pub async fn set_file_access(
    pool: &SqlitePool,
    team: TeamId,
    of: TeamId,
    access: FileAccess,
) -> Result<(), TeamError> {
    if team == of {
        return Err(
            ValidationError::new("of_team_id", "a team always opens its own folder").into(),
        );
    }
    let mut tx = begin_write(pool).await?;
    if !is_live(&mut tx, team).await? || !is_live(&mut tx, of).await? {
        return Err(TeamError::NotFound);
    }
    let (team, of) = (text(&team)?, text(&of)?);
    match access {
        FileAccess::None => {
            sqlx::query!(
                "DELETE FROM team_file_grants WHERE team_id = ? AND of_team_id = ?",
                team,
                of
            )
            .execute(&mut *tx)
            .await?;
        }
        FileAccess::Read | FileAccess::ReadWrite => {
            let access = text(&access)?;
            let now = timestamp(Utc::now());
            sqlx::query!(
                "INSERT INTO team_file_grants (team_id, of_team_id, access, updated_at)
                 VALUES (?, ?, ?, ?)
                 ON CONFLICT (team_id, of_team_id) DO UPDATE SET
                   access = excluded.access, updated_at = excluded.updated_at",
                team,
                of,
                access,
                now
            )
            .execute(&mut *tx)
            .await?;
        }
    }
    tx.commit().await?;
    Ok(())
}

fn valid_name(raw: &str) -> Result<&str, ValidationError> {
    let name = raw.trim();
    if name.is_empty() || name.chars().count() > NAME_CHARS {
        return Err(ValidationError::new("name", "use 1 to 60 characters"));
    }
    Ok(name)
}

/// Live names are unique ignoring case; the unique index is the backstop.
async fn name_free(
    conn: &mut SqliteConnection,
    name: &str,
    except: Option<TeamId>,
) -> Result<(), TeamError> {
    let except = except.as_ref().map(text).transpose()?;
    let taken = sqlx::query_scalar!(
        "SELECT COUNT(*) FROM teams
         WHERE name = ?1 COLLATE NOCASE AND deleted_at IS NULL AND (?2 IS NULL OR id != ?2)",
        name,
        except
    )
    .fetch_one(&mut *conn)
    .await?;
    if taken > 0 {
        return Err(ValidationError::new("name", "a team already has this name").into());
    }
    Ok(())
}

async fn view(conn: &mut SqliteConnection, id: &str) -> Result<Option<TeamView>, TeamError> {
    let lead_role = text(&Role::Lead)?;
    let Some(row) = sqlx::query!(
        r#"SELECT t.id, t.name, t.slug, t.created_at, t.updated_at, t.deleted_at,
                  (SELECT a.id FROM agents a WHERE a.team_id = t.id AND a.role = ?
                   ORDER BY a.deleted_at IS NOT NULL, a.created_at DESC LIMIT 1) AS "lead_agent_id?: String",
                  (SELECT COUNT(*) FROM agents a WHERE a.team_id = t.id AND a.deleted_at IS NULL) AS "agent_count!: i64"
           FROM teams t WHERE t.id = ?"#,
        lead_role,
        id
    )
    .fetch_optional(&mut *conn)
    .await?
    else {
        return Ok(None);
    };
    let lead = row
        .lead_agent_id
        .ok_or_else(|| decode("a team has no Lead"))?;
    Ok(Some(TeamView {
        id: from_text(&row.id).map_err(decode)?,
        name: row.name,
        slug: row.slug,
        lead_agent_id: from_text(&lead).map_err(decode)?,
        agent_count: row.agent_count,
        created_at: parse_timestamp(&row.created_at).map_err(decode)?,
        updated_at: parse_timestamp(&row.updated_at).map_err(decode)?,
        deleted_at: row
            .deleted_at
            .as_deref()
            .map(parse_timestamp)
            .transpose()
            .map_err(decode)?,
    }))
}

fn text<T: Serialize>(value: &T) -> Result<String, sqlx::Error> {
    to_text(value).map_err(decode)
}

/// The oldest live team, the default for a request that names none.
pub async fn first_live(conn: &mut SqliteConnection) -> Result<Option<TeamId>, sqlx::Error> {
    let id = sqlx::query_scalar!(
        "SELECT id FROM teams WHERE deleted_at IS NULL ORDER BY created_at, id LIMIT 1"
    )
    .fetch_optional(&mut *conn)
    .await?;
    id.map(|id| from_text(&id).map_err(decode)).transpose()
}

pub async fn is_live(conn: &mut SqliteConnection, team: TeamId) -> Result<bool, sqlx::Error> {
    let id = to_text(&team).map_err(decode)?;
    let found = sqlx::query_scalar!(
        "SELECT COUNT(*) FROM teams WHERE id = ? AND deleted_at IS NULL",
        id
    )
    .fetch_one(&mut *conn)
    .await?;
    Ok(found > 0)
}

/// Inserts a team row inside the caller's write transaction; the caller adds its Lead.
pub async fn insert(conn: &mut SqliteConnection, name: &str) -> Result<TeamId, sqlx::Error> {
    let team = TeamId::new();
    let id = to_text(&team).map_err(decode)?;
    let slug = free_slug(&mut *conn, &slug_of(name)).await?;
    let now = timestamp(Utc::now());
    sqlx::query!(
        "INSERT INTO teams (id, name, slug, created_at, updated_at) VALUES (?, ?, ?, ?, ?)",
        id,
        name,
        slug,
        now,
        now
    )
    .execute(&mut *conn)
    .await?;
    Ok(team)
}

/// Lowercase ASCII letters and digits; every other run becomes one `-`; `team` when nothing is left.
fn slug_of(name: &str) -> String {
    let mut slug = String::new();
    for c in name.chars() {
        if c.is_ascii_alphanumeric() {
            slug.push(c.to_ascii_lowercase());
        } else if !slug.is_empty() && !slug.ends_with('-') {
            slug.push('-');
        }
    }
    let slug: String = slug.chars().take(SLUG_CHARS).collect();
    let slug = slug.trim_end_matches('-');
    if slug.is_empty() {
        "team".to_string()
    } else {
        slug.to_string()
    }
}

/// `base`, or `base-2`, `base-3`, ... when a live team holds it; called inside the write lock.
async fn free_slug(conn: &mut SqliteConnection, base: &str) -> Result<String, sqlx::Error> {
    let mut candidate = base.to_string();
    for n in 2.. {
        let taken = sqlx::query_scalar!(
            "SELECT COUNT(*) FROM teams WHERE slug = ? AND deleted_at IS NULL",
            candidate
        )
        .fetch_one(&mut *conn)
        .await?;
        if taken == 0 {
            break;
        }
        candidate = format!("{base}-{n}");
    }
    Ok(candidate)
}

/// The live team with this slug.
pub async fn by_slug(
    conn: &mut SqliteConnection,
    slug: &str,
) -> Result<Option<TeamId>, sqlx::Error> {
    let id = sqlx::query_scalar!(
        "SELECT id FROM teams WHERE slug = ? AND deleted_at IS NULL",
        slug
    )
    .fetch_optional(&mut *conn)
    .await?;
    id.map(|id| from_text(&id).map_err(decode)).transpose()
}

fn decode(error: impl std::fmt::Display) -> sqlx::Error {
    sqlx::Error::Decode(error.to_string().into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slug_keeps_ascii_letters_and_digits_and_joins_the_rest() {
        assert_eq!(slug_of("Access desk"), "access-desk");
        assert_eq!(slug_of("  Access-desk 2!"), "access-desk-2");
        assert_eq!(slug_of("Zespół łódź"), "zesp-d");
        assert_eq!(slug_of("!!!"), "team");
        assert_eq!(slug_of(&"a".repeat(80)).len(), SLUG_CHARS);
    }
}
