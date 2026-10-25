//! Teams: the unit that groups agents and tasks. No policy reads a team (ADR 0007).

use chrono::Utc;
use orchestrator_core::ids::TeamId;
use sqlx::SqliteConnection;

use super::text::{from_text, timestamp, to_text};

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
