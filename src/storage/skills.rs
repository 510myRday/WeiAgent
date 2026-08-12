use std::collections::HashMap;

use rusqlite::{Connection, params};
use uuid::Uuid;

use crate::{
    domain::{SkillGroupOverride, SkillOverride, SkillOverrides},
    error::AppResult,
};

use super::{
    sql_util::{bool_to_i64, now_rfc3339},
    store::Store,
};

const SKILLS_SCHEMA_VERSION: i64 = 18;

impl Store {
    pub(super) async fn initialize_skill_settings(&self) -> AppResult<()> {
        self.database.call(|conn| {
            conn.execute_batch("CREATE TABLE IF NOT EXISTS app_schema_migrations (version INTEGER PRIMARY KEY NOT NULL, name TEXT NOT NULL, applied_at TEXT NOT NULL);")?;
            conn.execute_batch(
                "CREATE TABLE IF NOT EXISTS skill_global_overrides (
                    skill_name TEXT PRIMARY KEY NOT NULL,
                    enabled INTEGER NOT NULL CHECK (enabled IN (0, 1))
                 ) WITHOUT ROWID;
                 CREATE TABLE IF NOT EXISTS skill_global_group_overrides (
                    group_name TEXT PRIMARY KEY NOT NULL,
                    enabled INTEGER NOT NULL CHECK (enabled IN (0, 1))
                 ) WITHOUT ROWID;
                 CREATE TABLE IF NOT EXISTS skill_agent_overrides (
                    agent_id TEXT NOT NULL,
                    skill_name TEXT NOT NULL,
                    enabled INTEGER NOT NULL CHECK (enabled IN (0, 1)),
                    PRIMARY KEY (agent_id, skill_name)
                 ) WITHOUT ROWID;
                 CREATE TABLE IF NOT EXISTS skill_agent_group_overrides (
                    agent_id TEXT NOT NULL,
                    group_name TEXT NOT NULL,
                    enabled INTEGER NOT NULL CHECK (enabled IN (0, 1)),
                    PRIMARY KEY (agent_id, group_name)
                 ) WITHOUT ROWID;
                 CREATE TABLE IF NOT EXISTS skill_agent_baselines (
                    agent_id TEXT PRIMARY KEY NOT NULL
                 ) WITHOUT ROWID;
                 CREATE TABLE IF NOT EXISTS skill_agent_seen (
                    agent_id TEXT NOT NULL,
                    skill_name TEXT NOT NULL,
                    PRIMARY KEY (agent_id, skill_name)
                 ) WITHOUT ROWID;
                 CREATE TABLE IF NOT EXISTS skill_icons (
                    skill_name TEXT PRIMARY KEY NOT NULL,
                    icon_url TEXT NOT NULL
                 ) WITHOUT ROWID;
                 CREATE TABLE IF NOT EXISTS skill_sources (
                    skill_name TEXT PRIMARY KEY NOT NULL,
                    source_url TEXT NOT NULL
                 ) WITHOUT ROWID;
                 CREATE INDEX IF NOT EXISTS idx_skill_agent_overrides_agent ON skill_agent_overrides(agent_id);
                 CREATE INDEX IF NOT EXISTS idx_skill_agent_group_overrides_agent ON skill_agent_group_overrides(agent_id);",
            )?;
            let now = now_rfc3339();
            conn.execute(
                "INSERT OR IGNORE INTO app_schema_migrations(version, name, applied_at) VALUES (?1, ?2, ?3)",
                params![17_i64, "skills_v1", now.clone()],
            )?;
            conn.execute(
                "INSERT OR IGNORE INTO app_schema_migrations(version, name, applied_at) VALUES (?1, ?2, ?3)",
                params![SKILLS_SCHEMA_VERSION, "skills_v2", now],
            )?;
            Ok(())
        }).await
    }

    pub async fn read_skill_icons(&self) -> AppResult<HashMap<String, String>> {
        let _guard = self.skill_lock.lock().await;
        self.database
            .call(|conn| {
                let mut statement = conn.prepare("SELECT skill_name, icon_url FROM skill_icons")?;
                let rows = statement
                    .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
                    .collect::<Result<HashMap<String, String>, _>>()?;
                Ok(rows)
            })
            .await
    }

    pub async fn update_skill_icon(&self, name: &str, icon_url: Option<String>) -> AppResult<()> {
        let _guard = self.skill_lock.lock().await;
        let name = name.trim().to_ascii_lowercase();
        self.database
            .call(move |conn| {
                if let Some(icon_url) = icon_url.filter(|value| !value.trim().is_empty()) {
                    conn.execute(
                        "INSERT INTO skill_icons(skill_name, icon_url) VALUES (?1, ?2)
                         ON CONFLICT(skill_name) DO UPDATE SET icon_url = excluded.icon_url",
                        params![name, icon_url.trim()],
                    )?;
                } else {
                    conn.execute("DELETE FROM skill_icons WHERE skill_name = ?1", [name])?;
                }
                Ok(())
            })
            .await
    }

    pub async fn read_skill_sources(&self) -> AppResult<HashMap<String, String>> {
        let _guard = self.skill_lock.lock().await;
        self.database
            .call(|conn| {
                let mut statement =
                    conn.prepare("SELECT skill_name, source_url FROM skill_sources")?;
                let rows = statement
                    .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
                    .collect::<Result<HashMap<String, String>, _>>()?;
                Ok(rows)
            })
            .await
    }

    pub async fn update_skill_source(
        &self,
        name: &str,
        source_url: Option<String>,
    ) -> AppResult<()> {
        let _guard = self.skill_lock.lock().await;
        let name = name.trim().to_ascii_lowercase();
        self.database
            .call(move |conn| {
                if let Some(source_url) = source_url.filter(|value| !value.trim().is_empty()) {
                    conn.execute(
                        "INSERT INTO skill_sources(skill_name, source_url) VALUES (?1, ?2)
                         ON CONFLICT(skill_name) DO UPDATE SET source_url = excluded.source_url",
                        params![name, source_url.trim()],
                    )?;
                } else {
                    conn.execute("DELETE FROM skill_sources WHERE skill_name = ?1", [name])?;
                }
                Ok(())
            })
            .await
    }

    pub async fn delete_skill_metadata(&self, name: &str) -> AppResult<()> {
        let _guard = self.skill_lock.lock().await;
        let name = name.trim().to_ascii_lowercase();
        self.database
            .call(move |conn| {
                let tx = conn.transaction()?;
                tx.execute(
                    "DELETE FROM skill_global_overrides WHERE skill_name = ?1",
                    [&name],
                )?;
                tx.execute(
                    "DELETE FROM skill_agent_overrides WHERE skill_name = ?1",
                    [&name],
                )?;
                tx.execute(
                    "DELETE FROM skill_agent_seen WHERE skill_name = ?1",
                    [&name],
                )?;
                tx.execute("DELETE FROM skill_icons WHERE skill_name = ?1", [&name])?;
                tx.execute("DELETE FROM skill_sources WHERE skill_name = ?1", [&name])?;
                tx.commit()?;
                Ok(())
            })
            .await
    }

    pub async fn read_skill_global_overrides(&self) -> AppResult<SkillOverrides> {
        let _guard = self.skill_lock.lock().await;
        self.database
            .call(|conn| read_global_overrides_conn(conn))
            .await
    }

    pub async fn read_skill_agent_overrides(&self, agent_id: Uuid) -> AppResult<SkillOverrides> {
        let _guard = self.skill_lock.lock().await;
        self.database
            .call(move |conn| read_agent_overrides_conn(conn, agent_id))
            .await
    }

    pub async fn sync_skill_agent_catalog(
        &self,
        agent_id: Uuid,
        skill_names: &[String],
    ) -> AppResult<()> {
        let _guard = self.skill_lock.lock().await;
        let skill_names = skill_names.to_vec();
        self.database
            .call(move |conn| {
                let agent_id_text = agent_id.to_string();
                let initialized = conn.query_row(
                    "SELECT EXISTS(SELECT 1 FROM skill_agent_baselines WHERE agent_id = ?1)",
                    [&agent_id_text],
                    |row| row.get::<_, bool>(0),
                )?;
                let tx = conn.transaction()?;
                tx.execute(
                    "INSERT OR IGNORE INTO skill_agent_baselines(agent_id) VALUES (?1)",
                    [&agent_id_text],
                )?;
                for name in skill_names {
                    let name = name.trim().to_ascii_lowercase();
                    let inserted = tx.execute(
                        "INSERT OR IGNORE INTO skill_agent_seen(agent_id, skill_name) VALUES (?1, ?2)",
                        params![agent_id_text, name],
                    )?;
                    if initialized && inserted > 0 {
                        tx.execute(
                            "INSERT OR IGNORE INTO skill_agent_overrides(agent_id, skill_name, enabled) VALUES (?1, ?2, 0)",
                            params![agent_id_text, name],
                        )?;
                    }
                }
                tx.commit()?;
                Ok(())
            })
            .await
    }

    pub async fn update_skill_global_override(
        &self,
        name: &str,
        enabled: Option<bool>,
    ) -> AppResult<()> {
        let _guard = self.skill_lock.lock().await;
        let name = name.trim().to_ascii_lowercase();
        self.database
            .call(move |conn| {
                if let Some(enabled) = enabled {
                    conn.execute(
                        "INSERT INTO skill_global_overrides(skill_name, enabled) VALUES (?1, ?2)
                     ON CONFLICT(skill_name) DO UPDATE SET enabled = excluded.enabled",
                        params![name, bool_to_i64(enabled)],
                    )?;
                } else {
                    conn.execute(
                        "DELETE FROM skill_global_overrides WHERE skill_name = ?1",
                        [name],
                    )?;
                }
                Ok(())
            })
            .await
    }

    pub async fn update_skill_global_group_override(
        &self,
        group: &str,
        enabled: Option<bool>,
    ) -> AppResult<()> {
        let _guard = self.skill_lock.lock().await;
        let group = group.trim().to_string();
        self.database
            .call(move |conn| {
                if let Some(enabled) = enabled {
                    conn.execute(
                    "INSERT INTO skill_global_group_overrides(group_name, enabled) VALUES (?1, ?2)
                     ON CONFLICT(group_name) DO UPDATE SET enabled = excluded.enabled",
                    params![group, bool_to_i64(enabled)],
                )?;
                } else {
                    conn.execute(
                        "DELETE FROM skill_global_group_overrides WHERE group_name = ?1",
                        [group],
                    )?;
                }
                Ok(())
            })
            .await
    }

    pub async fn update_skill_agent_overrides(
        &self,
        agent_id: Uuid,
        overrides: SkillOverrides,
    ) -> AppResult<()> {
        let _guard = self.skill_lock.lock().await;
        self.database.call(move |conn| {
            let tx = conn.transaction()?;
            tx.execute("DELETE FROM skill_agent_overrides WHERE agent_id = ?1", [agent_id.to_string()])?;
            tx.execute("DELETE FROM skill_agent_group_overrides WHERE agent_id = ?1", [agent_id.to_string()])?;
            for item in overrides.skills {
                if let Some(enabled) = item.enabled {
                    tx.execute(
                        "INSERT INTO skill_agent_overrides(agent_id, skill_name, enabled) VALUES (?1, ?2, ?3)",
                        params![agent_id.to_string(), item.name.trim().to_ascii_lowercase(), bool_to_i64(enabled)],
                    )?;
                }
            }
            for item in overrides.groups {
                if let Some(enabled) = item.enabled {
                    tx.execute(
                        "INSERT INTO skill_agent_group_overrides(agent_id, group_name, enabled) VALUES (?1, ?2, ?3)",
                        params![agent_id.to_string(), item.group.trim(), bool_to_i64(enabled)],
                    )?;
                }
            }
            tx.commit()?;
            Ok(())
        }).await
    }

    pub async fn cleanup_skill_orphans(
        &self,
        names: &[String],
        groups: &[String],
    ) -> AppResult<()> {
        let _guard = self.skill_lock.lock().await;
        let names = names.to_vec();
        let groups = groups.to_vec();
        self.database
            .call(move |conn| {
                let tx = conn.transaction()?;
                for name in names {
                    tx.execute(
                        "DELETE FROM skill_global_overrides WHERE skill_name = ?1",
                        [name.clone()],
                    )?;
                    tx.execute(
                        "DELETE FROM skill_agent_overrides WHERE skill_name = ?1",
                        [name],
                    )?;
                }
                for group in groups {
                    tx.execute(
                        "DELETE FROM skill_global_group_overrides WHERE group_name = ?1",
                        [group.clone()],
                    )?;
                    tx.execute(
                        "DELETE FROM skill_agent_group_overrides WHERE group_name = ?1",
                        [group],
                    )?;
                }
                tx.commit()?;
                Ok(())
            })
            .await
    }
}

fn read_global_overrides_conn(conn: &Connection) -> AppResult<SkillOverrides> {
    let mut skills = Vec::new();
    let mut stmt =
        conn.prepare("SELECT skill_name, enabled FROM skill_global_overrides ORDER BY skill_name")?;
    for row in stmt.query_map([], |row| {
        Ok(SkillOverride {
            name: row.get(0)?,
            enabled: Some(row.get::<_, bool>(1)?),
        })
    })? {
        skills.push(row?);
    }
    let mut groups = Vec::new();
    let mut stmt = conn.prepare(
        "SELECT group_name, enabled FROM skill_global_group_overrides ORDER BY group_name",
    )?;
    for row in stmt.query_map([], |row| {
        Ok(SkillGroupOverride {
            group: row.get(0)?,
            enabled: Some(row.get::<_, bool>(1)?),
        })
    })? {
        groups.push(row?);
    }
    Ok(SkillOverrides { skills, groups })
}

fn read_agent_overrides_conn(conn: &Connection, agent_id: Uuid) -> AppResult<SkillOverrides> {
    let agent_id = agent_id.to_string();
    let mut skills = Vec::new();
    let mut stmt = conn.prepare("SELECT skill_name, enabled FROM skill_agent_overrides WHERE agent_id = ?1 ORDER BY skill_name")?;
    for row in stmt.query_map([agent_id.clone()], |row| {
        Ok(SkillOverride {
            name: row.get(0)?,
            enabled: Some(row.get::<_, bool>(1)?),
        })
    })? {
        skills.push(row?);
    }
    let mut groups = Vec::new();
    let mut stmt = conn.prepare("SELECT group_name, enabled FROM skill_agent_group_overrides WHERE agent_id = ?1 ORDER BY group_name")?;
    for row in stmt.query_map([agent_id], |row| {
        Ok(SkillGroupOverride {
            group: row.get(0)?,
            enabled: Some(row.get::<_, bool>(1)?),
        })
    })? {
        groups.push(row?);
    }
    Ok(SkillOverrides { skills, groups })
}
