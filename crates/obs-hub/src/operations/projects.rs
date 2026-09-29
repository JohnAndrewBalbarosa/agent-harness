//! `project.register` and `agent.register` (server.mjs:144-160).

use anyhow::{bail, Result};
use rusqlite::{params, Connection};

use super::fields::Fields;
use super::Operation;

const SLUG_MAX_LENGTH: usize = 120;

pub fn register_project(connection: &Connection, operation: &Operation) -> Result<()> {
    let data = operation.fields();
    let project_id = data.uuid("projectId")?;
    let slug = data.text("slug", SLUG_MAX_LENGTH);

    ensure_valid_slug(&slug)?;
    upsert_project(connection, operation, project_id.as_deref(), &slug)?;

    if let Some(aliases) = data.raw("aliases").as_array() {
        for alias in aliases {
            upsert_alias(
                connection,
                operation,
                project_id.as_deref(),
                Fields::of(alias),
            )?;
        }
    }
    Ok(())
}

pub fn register_agent(connection: &Connection, operation: &Operation) -> Result<()> {
    let data = operation.fields();
    connection.execute(
        "INSERT INTO agent_instances(agent_instance_id,instance_key,source_kind,display_name,last_seen_at)
         VALUES(?1,?2,?3,?4,?5)
         ON CONFLICT(instance_key) DO UPDATE SET display_name=excluded.display_name,last_seen_at=excluded.last_seen_at",
        params![
            data.uuid("agentInstanceId")?,
            data.text("instanceKey", 80),
            data.integer_or("sourceKind", 1)?,
            data.text("displayName", 160),
            operation.occurred_at,
        ],
    )?;
    Ok(())
}

fn upsert_project(
    connection: &Connection,
    operation: &Operation,
    project_id: Option<&str>,
    slug: &str,
) -> Result<()> {
    let data = operation.fields();
    connection.execute(
        "INSERT INTO projects(project_id,display_name,slug,last_seen_at) VALUES(?1,?2,?3,?4)
         ON CONFLICT(project_id) DO UPDATE SET display_name=excluded.display_name,slug=excluded.slug,last_seen_at=excluded.last_seen_at",
        params![project_id, data.text("displayName", 160), slug, operation.occurred_at],
    )?;
    Ok(())
}

fn upsert_alias(
    connection: &Connection,
    operation: &Operation,
    project_id: Option<&str>,
    alias: Fields<'_>,
) -> Result<()> {
    connection.execute(
        "INSERT INTO project_aliases(project_id,alias_kind,alias_hash,last_seen_at) VALUES(?1,?2,?3,?4)
         ON CONFLICT(project_id,alias_kind,alias_hash) DO UPDATE SET is_current=1,last_seen_at=excluded.last_seen_at",
        params![
            project_id,
            alias.integer("kind")?,
            alias.hex_bytes("hash", 32)?,
            operation.occurred_at,
        ],
    )?;
    Ok(())
}

/// Postgres CHECK `slug ~ '^[a-z0-9][a-z0-9._-]{0,119}$'`.
fn ensure_valid_slug(slug: &str) -> Result<()> {
    if !is_valid_slug(slug) {
        bail!(
            "new row for relation \"projects\" violates check constraint \"projects_slug_check\""
        );
    }
    Ok(())
}

#[inline]
fn is_valid_slug(slug: &str) -> bool {
    let mut characters = slug.chars();
    characters
        .next()
        .is_some_and(|first| first.is_ascii_lowercase() || first.is_ascii_digit())
        && slug.chars().count() <= SLUG_MAX_LENGTH
        && characters.all(is_slug_character)
}

#[inline]
fn is_slug_character(character: char) -> bool {
    character.is_ascii_lowercase()
        || character.is_ascii_digit()
        || matches!(character, '.' | '_' | '-')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slug_rules_match_the_postgres_check() {
        assert!(is_valid_slug("codex-system-harness"));
        assert!(is_valid_slug("a"));
        assert!(!is_valid_slug(""));
        assert!(!is_valid_slug("-lead"));
        assert!(!is_valid_slug("Upper"));
    }
}
