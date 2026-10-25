//! Agent CRUD with the team invariants, each checked inside the write's transaction.
//! Every agent read back from the database goes through `AgentSpec::validated` again.

use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use orchestrator_core::agents::{AgentInput, AgentSpec, Role, Slug, ValidationError};
use orchestrator_core::ids::AgentId;
use orchestrator_core::policy::Decision;
use orchestrator_core::tools::ToolName;
use serde::Serialize;
use sqlx::{SqliteConnection, SqlitePool};
use ts_rs::TS;

use super::begin_write;
use super::text::{from_text, parse_timestamp, timestamp, to_text};

/// An agent as the API returns it: the spec plus its timestamps.
#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
pub struct AgentRecord {
    #[serde(flatten)]
    pub spec: AgentSpec,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, thiserror::Error)]
pub enum AgentError {
    #[error(transparent)]
    Invalid(#[from] ValidationError),
    #[error("agent not found")]
    NotFound,
    #[error("storage failed: {0}")]
    Storage(#[from] sqlx::Error),
    #[error("stored agent is unreadable: {0}")]
    Corrupt(String),
}

/// Reads the ids and every agent in one transaction, so a delete in between cannot drop a row.
pub async fn list(pool: &SqlitePool) -> Result<Vec<AgentRecord>, AgentError> {
    let mut tx = pool.begin().await?;
    let ids = sqlx::query_scalar!(
        "SELECT id FROM agents WHERE deleted_at IS NULL ORDER BY created_at, id"
    )
    .fetch_all(&mut *tx)
    .await?;
    let mut records = Vec::with_capacity(ids.len());
    for id in ids {
        records.push(load(&mut tx, &id).await?.ok_or(AgentError::NotFound)?);
    }
    Ok(records)
}

pub async fn get(pool: &SqlitePool, id: AgentId) -> Result<AgentRecord, AgentError> {
    let mut conn = pool.acquire().await?;
    load(&mut conn, &text(&id)?)
        .await?
        .ok_or(AgentError::NotFound)
}

pub async fn create(pool: &SqlitePool, input: AgentInput) -> Result<AgentRecord, AgentError> {
    let mut tx = begin_write(pool).await?;
    let spec = AgentSpec::validated(AgentId::new(), 1, input)?;
    check_team(&mut tx, &spec).await?;

    let now = timestamp(Utc::now());
    let id = text(&spec.id())?;
    let role = text(&spec.role())?;
    let slug = spec.slug().as_str();
    let model = text(&spec.model())?;
    let effort = spec.reasoning_effort().as_ref().map(text).transpose()?;
    let max_steps = i64::from(spec.max_steps());
    let targets = json(spec.delegate_targets())?;
    let (name, description, prompt) = (spec.name(), spec.description(), spec.system_prompt());
    let (icon, colour) = (spec.icon(), spec.colour());
    sqlx::query!(
        "INSERT INTO agents (id, role, slug, name, description, system_prompt, model, reasoning_effort, max_steps,
                             icon, colour, delegate_targets, version, created_at, updated_at)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, 1, ?, ?)",
        id,
        role,
        slug,
        name,
        description,
        prompt,
        model,
        effort,
        max_steps,
        icon,
        colour,
        targets,
        now,
        now
    )
    .execute(&mut *tx)
    .await
    ?;
    write_tools(&mut tx, &id, spec.tools()).await?;

    let record = load(&mut tx, &id).await?.ok_or(AgentError::NotFound)?;
    tx.commit().await?;
    Ok(record)
}

/// A full replace: bumps the version; the slug and role cannot change.
pub async fn replace(
    pool: &SqlitePool,
    id: AgentId,
    input: AgentInput,
) -> Result<AgentRecord, AgentError> {
    let mut tx = begin_write(pool).await?;
    let id_text = text(&id)?;
    let current = load(&mut tx, &id_text).await?.ok_or(AgentError::NotFound)?;
    if input.slug != current.spec.slug().as_str() {
        return Err(ValidationError::new("slug", "cannot change after create").into());
    }
    if input.role != current.spec.role() {
        return Err(ValidationError::new("role", "cannot change after create").into());
    }
    let spec = AgentSpec::validated(id, current.spec.version() + 1, input)?;
    check_team(&mut tx, &spec).await?;

    let now = timestamp(Utc::now());
    let model = text(&spec.model())?;
    let effort = spec.reasoning_effort().as_ref().map(text).transpose()?;
    let targets = json(spec.delegate_targets())?;
    let version = spec.version();
    let (name, description, prompt) = (spec.name(), spec.description(), spec.system_prompt());
    let (icon, colour) = (spec.icon(), spec.colour());
    sqlx::query!(
        "UPDATE agents SET name = ?, description = ?, system_prompt = ?, model = ?, reasoning_effort = ?,
                           icon = ?, colour = ?, delegate_targets = ?, version = ?, updated_at = ?
         WHERE id = ?",
        name,
        description,
        prompt,
        model,
        effort,
        icon,
        colour,
        targets,
        version,
        now,
        id_text
    )
    .execute(&mut *tx)
    .await?;
    sqlx::query!("DELETE FROM agent_tools WHERE agent_id = ?", id_text)
        .execute(&mut *tx)
        .await?;
    write_tools(&mut tx, &id_text, spec.tools()).await?;

    let record = load(&mut tx, &id_text).await?.ok_or(AgentError::NotFound)?;
    tx.commit().await?;
    Ok(record)
}

/// A soft delete. The Lead cannot be deleted; the slug leaves the Lead's targets in the same
/// transaction, so a later agent reusing the slug never inherits the target.
pub async fn delete(pool: &SqlitePool, id: AgentId) -> Result<(), AgentError> {
    let mut tx = begin_write(pool).await?;
    let id_text = text(&id)?;
    let current = load(&mut tx, &id_text).await?.ok_or(AgentError::NotFound)?;
    if current.spec.role() == Role::Lead {
        return Err(ValidationError::new("role", "the Lead cannot be deleted").into());
    }

    let now = timestamp(Utc::now());
    sqlx::query!(
        "UPDATE agents SET deleted_at = ? WHERE id = ?",
        now,
        id_text
    )
    .execute(&mut *tx)
    .await?;

    let lead_role = text(&Role::Lead)?;
    let lead = sqlx::query!(
        "SELECT id, delegate_targets FROM agents WHERE role = ? AND deleted_at IS NULL",
        lead_role
    )
    .fetch_optional(&mut *tx)
    .await?;
    if let Some(lead) = lead {
        let targets: Vec<Slug> = serde_json::from_str(&lead.delegate_targets).map_err(corrupt)?;
        let kept: Vec<&Slug> = targets
            .iter()
            .filter(|t| *t != current.spec.slug())
            .collect();
        if kept.len() != targets.len() {
            let kept = json(&kept)?;
            sqlx::query!(
                "UPDATE agents SET delegate_targets = ?, version = version + 1, updated_at = ? WHERE id = ?",
                kept,
                now,
                lead.id
            )
            .execute(&mut *tx)
            .await?;
        }
    }
    tx.commit().await?;
    Ok(())
}

pub async fn live_team(pool: &SqlitePool) -> Result<Vec<AgentSpec>, AgentError> {
    Ok(list(pool)
        .await?
        .into_iter()
        .map(|record| record.spec)
        .collect())
}

/// The slug is free among live agents, there is at most one live Lead, and every delegate
/// target is a live specialist. The unique indexes are backstops for these checks.
async fn check_team(conn: &mut SqliteConnection, spec: &AgentSpec) -> Result<(), AgentError> {
    let id = text(&spec.id())?;
    let slug = spec.slug().as_str();
    let slug_users = sqlx::query_scalar!(
        "SELECT COUNT(*) FROM agents WHERE slug = ? AND deleted_at IS NULL AND id != ?",
        slug,
        id
    )
    .fetch_one(&mut *conn)
    .await?;
    if slug_users > 0 {
        return Err(ValidationError::new("slug", "is taken").into());
    }

    let lead_role = text(&Role::Lead)?;
    if spec.role() == Role::Lead {
        let other_leads = sqlx::query_scalar!(
            "SELECT COUNT(*) FROM agents WHERE role = ? AND deleted_at IS NULL AND id != ?",
            lead_role,
            id
        )
        .fetch_one(&mut *conn)
        .await?;
        if other_leads > 0 {
            return Err(ValidationError::new("role", "the team already has a Lead").into());
        }
    }

    let specialist_role = text(&Role::Specialist)?;
    for target in spec.delegate_targets() {
        let slug = target.as_str();
        let found = sqlx::query_scalar!(
            "SELECT COUNT(*) FROM agents WHERE slug = ? AND role = ? AND deleted_at IS NULL",
            slug,
            specialist_role
        )
        .fetch_one(&mut *conn)
        .await?;
        if found == 0 {
            let message = format!("{slug} is not a live specialist");
            return Err(ValidationError::new("delegate_targets", &message).into());
        }
    }
    Ok(())
}

async fn write_tools(
    conn: &mut SqliteConnection,
    agent_id: &str,
    tools: &BTreeMap<ToolName, Decision>,
) -> Result<(), AgentError> {
    for (tool, decision) in tools {
        let tool = text(tool)?;
        let decision = text(decision)?;
        sqlx::query!(
            "INSERT INTO agent_tools (agent_id, tool, decision) VALUES (?, ?, ?)",
            agent_id,
            tool,
            decision
        )
        .execute(&mut *conn)
        .await?;
    }
    Ok(())
}

/// A live agent with its tools, or `None` if it does not exist or is soft-deleted.
/// Rows pass `AgentSpec::validated` again, so a stricter rule needs a data migration in the same change.
async fn load(conn: &mut SqliteConnection, id: &str) -> Result<Option<AgentRecord>, AgentError> {
    let Some(row) = sqlx::query!(
        "SELECT id, role, slug, name, description, system_prompt, model, reasoning_effort, icon, colour,
                delegate_targets, version, created_at, updated_at
         FROM agents WHERE id = ? AND deleted_at IS NULL",
        id
    )
    .fetch_optional(&mut *conn)
    .await?
    else {
        return Ok(None);
    };

    let mut tools = BTreeMap::new();
    for tool in sqlx::query!(
        "SELECT tool, decision FROM agent_tools WHERE agent_id = ?",
        id
    )
    .fetch_all(&mut *conn)
    .await?
    {
        let name: ToolName = from_text(&tool.tool).map_err(corrupt)?;
        tools.insert(name, from_text(&tool.decision).map_err(corrupt)?);
    }

    let input = AgentInput {
        role: from_text(&row.role).map_err(corrupt)?,
        slug: row.slug,
        name: row.name,
        description: row.description,
        system_prompt: row.system_prompt,
        model: from_text(&row.model).map_err(corrupt)?,
        reasoning_effort: row
            .reasoning_effort
            .as_deref()
            .map(from_text)
            .transpose()
            .map_err(corrupt)?,
        icon: row.icon,
        colour: row.colour,
        tools,
        delegate_targets: serde_json::from_str(&row.delegate_targets).map_err(corrupt)?,
    };
    let spec = AgentSpec::validated(from_text(&row.id).map_err(corrupt)?, row.version, input)
        .map_err(corrupt)?;
    Ok(Some(AgentRecord {
        spec,
        created_at: parse_timestamp(&row.created_at).map_err(corrupt)?,
        updated_at: parse_timestamp(&row.updated_at).map_err(corrupt)?,
    }))
}

fn text<T: Serialize>(value: &T) -> Result<String, AgentError> {
    to_text(value).map_err(corrupt)
}

fn json<T: Serialize + ?Sized>(value: &T) -> Result<String, AgentError> {
    serde_json::to_string(value).map_err(corrupt)
}

fn corrupt(error: impl std::fmt::Display) -> AgentError {
    AgentError::Corrupt(error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input(role: Role, slug: &str, targets: &[&str]) -> AgentInput {
        AgentInput {
            role,
            slug: slug.to_string(),
            name: slug.to_string(),
            description: "d".to_string(),
            system_prompt: "p".to_string(),
            model: orchestrator_core::llm::Model::Grok43,
            reasoning_effort: None,
            icon: "i".to_string(),
            colour: "c".to_string(),
            tools: BTreeMap::new(),
            delegate_targets: targets.iter().map(|t| t.to_string()).collect(),
        }
    }

    fn rejected_on(result: Result<impl std::fmt::Debug, AgentError>, field: &str) {
        match result {
            Err(AgentError::Invalid(error)) => assert_eq!(error.field, field),
            other => panic!("expected a validation error on {field}, got {other:?}"),
        }
    }

    #[sqlx::test]
    async fn second_lead_is_rejected(pool: SqlitePool) {
        create(&pool, input(Role::Lead, "lead", &[]))
            .await
            .expect("first lead");
        rejected_on(create(&pool, input(Role::Lead, "lead2", &[])).await, "role");
    }

    #[sqlx::test]
    async fn lead_cannot_be_deleted(pool: SqlitePool) {
        let lead = create(&pool, input(Role::Lead, "lead", &[]))
            .await
            .expect("lead");
        rejected_on(delete(&pool, lead.spec.id()).await, "role");
    }

    #[sqlx::test]
    async fn slug_and_role_cannot_change(pool: SqlitePool) {
        let agent = create(&pool, input(Role::Specialist, "reader", &[]))
            .await
            .expect("agent");
        let id = agent.spec.id();
        rejected_on(
            replace(&pool, id, input(Role::Specialist, "renamed", &[])).await,
            "slug",
        );
        rejected_on(
            replace(&pool, id, input(Role::Lead, "reader", &[])).await,
            "role",
        );
        let replaced = replace(&pool, id, input(Role::Specialist, "reader", &[]))
            .await
            .expect("replace");
        assert_eq!(replaced.spec.version(), 2);
    }

    #[sqlx::test]
    async fn targets_must_be_live_specialists(pool: SqlitePool) {
        rejected_on(
            create(&pool, input(Role::Lead, "lead", &["ghost"])).await,
            "delegate_targets",
        );

        let gone = create(&pool, input(Role::Specialist, "gone", &[]))
            .await
            .expect("specialist");
        delete(&pool, gone.spec.id()).await.expect("delete");
        rejected_on(
            create(&pool, input(Role::Lead, "lead", &["gone"])).await,
            "delegate_targets",
        );
    }

    #[sqlx::test]
    async fn delete_removes_slug_from_lead_targets_and_frees_it(pool: SqlitePool) {
        let reader = create(&pool, input(Role::Specialist, "reader", &[]))
            .await
            .expect("reader");
        let lead = create(&pool, input(Role::Lead, "lead", &["reader"]))
            .await
            .expect("lead");

        delete(&pool, reader.spec.id()).await.expect("delete");
        let lead = get(&pool, lead.spec.id()).await.expect("lead");
        assert!(lead.spec.delegate_targets().is_empty());

        create(&pool, input(Role::Specialist, "reader", &[]))
            .await
            .expect("slug reusable");
        let lead = get(&pool, lead.spec.id()).await.expect("lead");
        assert!(
            lead.spec.delegate_targets().is_empty(),
            "a reused slug inherits nothing"
        );
    }

    #[sqlx::test]
    async fn concurrent_lead_creates_yield_one_lead_and_one_role_error(pool: SqlitePool) {
        let first = tokio::spawn(create_owned(pool.clone(), input(Role::Lead, "lead_a", &[])));
        let second = tokio::spawn(create_owned(pool.clone(), input(Role::Lead, "lead_b", &[])));
        let results = [first.await.expect("join"), second.await.expect("join")];

        assert_eq!(results.iter().filter(|r| r.is_ok()).count(), 1);
        let failure = results
            .into_iter()
            .find(|r| r.is_err())
            .expect("one failure");
        rejected_on(failure, "role");
    }

    async fn create_owned(pool: SqlitePool, input: AgentInput) -> Result<AgentRecord, AgentError> {
        create(&pool, input).await
    }

    #[sqlx::test]
    async fn malformed_slug_is_one_message_on_slug(pool: SqlitePool) {
        match create(&pool, input(Role::Specialist, "Bad Slug", &[])).await {
            Err(AgentError::Invalid(error)) => {
                assert_eq!(error.field, "slug");
                assert_eq!(error.message, "use 1 to 32 of a-z, 0-9, _ and -");
            }
            other => panic!("expected a slug error, got {other:?}"),
        }
    }

    #[sqlx::test]
    async fn index_rejects_a_second_live_lead_written_around_the_checks(pool: SqlitePool) {
        create(&pool, input(Role::Lead, "lead", &[]))
            .await
            .expect("lead");
        let id = to_text(&AgentId::new()).expect("id");
        let role = to_text(&Role::Lead).expect("role");
        let result = sqlx::query!(
            "INSERT INTO agents (id, role, slug, name, description, system_prompt, model, max_steps, icon, colour,
                                 created_at, updated_at)
             VALUES (?, ?, 'other', 'n', 'd', 'p', 'grok-4.3', 12, 'i', 'c', 't', 't')",
            id,
            role
        )
        .execute(&pool)
        .await;
        assert!(result.is_err(), "the one-Lead index must reject this row");
    }

    #[sqlx::test]
    async fn duplicate_live_slug_is_rejected(pool: SqlitePool) {
        create(&pool, input(Role::Specialist, "reader", &[]))
            .await
            .expect("first");
        rejected_on(
            create(&pool, input(Role::Specialist, "reader", &[])).await,
            "slug",
        );
    }
}
