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
    /// Stays set on a deleted team: it names the soft-deleted Lead, so history can show it.
    pub lead_agent_id: AgentId,
    /// Live agents, the Lead included; 0 on a deleted team.
    pub agent_count: i64,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub deleted_at: Option<DateTime<Utc>>,
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
        r#"SELECT t.id, t.name, t.created_at, t.updated_at, t.deleted_at,
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
    let now = timestamp(Utc::now());
    sqlx::query!(
        "INSERT INTO teams (id, name, created_at, updated_at) VALUES (?, ?, ?, ?)",
        id,
        name,
        now,
        now
    )
    .execute(&mut *conn)
    .await?;
    Ok(team)
}

fn decode(error: impl std::fmt::Display) -> sqlx::Error {
    sqlx::Error::Decode(error.to_string().into())
}
