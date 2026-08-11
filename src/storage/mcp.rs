use chrono::Utc;
use rusqlite::{Connection, params};
use std::collections::HashSet;
use uuid::Uuid;

use crate::{
    domain::{
        McpAgentOverrides, McpAgentSettings, McpDiagnostics, McpServer, McpServerDraft,
        McpServerOverride, McpServerStatus, McpTool, McpToolOverride, McpTransport,
    },
    error::{AppError, AppResult},
};

use super::{
    sql_util::{bool_to_i64, now_rfc3339},
    store::Store,
};

const MCP_SCHEMA_VERSION: i64 = 15;

const MCP_SCHEMA_SQL: &str = r#"
CREATE TABLE IF NOT EXISTS tool_mcp_servers (
    server_id TEXT PRIMARY KEY NOT NULL,
    name TEXT NOT NULL CHECK (length(trim(name)) > 0),
    description TEXT NOT NULL DEFAULT '',
    enabled INTEGER NOT NULL CHECK (enabled IN (0, 1)),
    supports_parallel_tool_calls INTEGER NOT NULL DEFAULT 0 CHECK (supports_parallel_tool_calls IN (0, 1)),
    provider TEXT,
    provider_url TEXT,
    logo_url TEXT,
    tags_json TEXT NOT NULL DEFAULT '[]' CHECK (json_valid(tags_json)),
    transport_json TEXT NOT NULL CHECK (json_valid(transport_json)),
    status TEXT NOT NULL DEFAULT 'unknown',
    last_synced_at TEXT,
    last_error TEXT,
    diagnostics_json TEXT NOT NULL DEFAULT '{}' CHECK (json_valid(diagnostics_json)),
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
) WITHOUT ROWID;

CREATE TABLE IF NOT EXISTS tool_mcp_tools (
    server_id TEXT NOT NULL,
    tool_name TEXT NOT NULL,
    qualified_name TEXT NOT NULL UNIQUE,
    title TEXT,
    description TEXT NOT NULL DEFAULT '',
    input_schema_json TEXT NOT NULL CHECK (json_valid(input_schema_json)),
    enabled INTEGER NOT NULL CHECK (enabled IN (0, 1)),
    updated_at TEXT,
    PRIMARY KEY (server_id, tool_name),
    FOREIGN KEY (server_id) REFERENCES tool_mcp_servers(server_id) ON DELETE CASCADE
) WITHOUT ROWID;

CREATE TABLE IF NOT EXISTS tool_mcp_agent_servers (
    agent_id TEXT NOT NULL,
    server_id TEXT NOT NULL,
    enabled INTEGER NOT NULL CHECK (enabled IN (0, 1)),
    customized INTEGER NOT NULL DEFAULT 0 CHECK (customized IN (0, 1)),
    PRIMARY KEY (agent_id, server_id),
    FOREIGN KEY (server_id) REFERENCES tool_mcp_servers(server_id) ON DELETE CASCADE
) WITHOUT ROWID;

CREATE TABLE IF NOT EXISTS tool_mcp_agent_tools (
    agent_id TEXT NOT NULL,
    server_id TEXT NOT NULL,
    tool_name TEXT NOT NULL,
    enabled INTEGER NOT NULL CHECK (enabled IN (0, 1)),
    PRIMARY KEY (agent_id, server_id, tool_name),
    FOREIGN KEY (server_id, tool_name) REFERENCES tool_mcp_tools(server_id, tool_name) ON DELETE CASCADE
) WITHOUT ROWID;

CREATE INDEX IF NOT EXISTS idx_tool_mcp_tools_server ON tool_mcp_tools(server_id);
CREATE INDEX IF NOT EXISTS idx_tool_mcp_agent_servers_agent ON tool_mcp_agent_servers(agent_id);
CREATE INDEX IF NOT EXISTS idx_tool_mcp_agent_tools_agent ON tool_mcp_agent_tools(agent_id);
"#;

impl Store {
    pub(super) async fn initialize_mcp_settings(&self) -> AppResult<()> {
        self.database.call(|conn| {
            conn.execute_batch("CREATE TABLE IF NOT EXISTS app_schema_migrations (version INTEGER PRIMARY KEY NOT NULL, name TEXT NOT NULL, applied_at TEXT NOT NULL);")?;
            let applied = conn.query_row(
                "SELECT EXISTS(SELECT 1 FROM app_schema_migrations WHERE version = ?1)",
                [MCP_SCHEMA_VERSION],
                |row| row.get::<_, bool>(0),
            )?;
            if !applied {
                let tx = conn.transaction()?;
                tx.execute(
                    "INSERT OR IGNORE INTO app_schema_migrations(version, name, applied_at) VALUES (?1, ?2, ?3)",
                    params![13_i64, "mcp_tools_v1", Utc::now().to_rfc3339()],
                )?;
                tx.execute(
                    "INSERT OR IGNORE INTO app_schema_migrations(version, name, applied_at) VALUES (?1, ?2, ?3)",
                    params![14_i64, "mcp_transport_auth_v14", Utc::now().to_rfc3339()],
                )?;
                tx.execute_batch(MCP_SCHEMA_SQL)?;
                for (column, definition) in [
                    ("provider", "TEXT"),
                    ("provider_url", "TEXT"),
                    ("logo_url", "TEXT"),
                    ("tags_json", "TEXT NOT NULL DEFAULT '[]'"),
                    ("supports_parallel_tool_calls", "INTEGER NOT NULL DEFAULT 0"),
                    ("diagnostics_json", "TEXT NOT NULL DEFAULT '{}'"),
                ] {
                    if !has_column(&tx, "tool_mcp_servers", column)? {
                        tx.execute(
                            &format!("ALTER TABLE tool_mcp_servers ADD COLUMN {column} {definition}"),
                            [],
                        )?;
                    }
                }
                if !has_column(&tx, "tool_mcp_agent_servers", "customized")? {
                    tx.execute(
                        "ALTER TABLE tool_mcp_agent_servers ADD COLUMN customized INTEGER NOT NULL DEFAULT 0",
                        [],
                    )?;
                }
                tx.execute(
                    "INSERT INTO app_schema_migrations(version, name, applied_at) VALUES (?1, ?2, ?3)",
                    params![MCP_SCHEMA_VERSION, "mcp_tools_v1", Utc::now().to_rfc3339()],
                )?;
                tx.commit()?;
            } else {
                conn.execute_batch(MCP_SCHEMA_SQL)?;
                conn.execute(
                    "INSERT OR IGNORE INTO app_schema_migrations(version, name, applied_at) VALUES (?1, ?2, ?3)",
                    params![14_i64, "mcp_transport_auth_v14", Utc::now().to_rfc3339()],
                )?;
            }
            Ok(())
        }).await
    }

    pub async fn list_mcp_servers(&self) -> AppResult<Vec<McpServer>> {
        let _guard = self.mcp_lock.lock().await;
        self.database.call(|conn| read_servers_conn(conn)).await
    }

    pub async fn create_mcp_server(&self, draft: McpServerDraft) -> AppResult<McpServer> {
        self.save_mcp_server(draft).await
    }

    pub async fn update_mcp_server(&self, draft: McpServerDraft) -> AppResult<McpServer> {
        self.save_mcp_server(draft).await
    }

    pub async fn update_mcp_server_enabled(
        &self,
        server_id: Uuid,
        enabled: bool,
    ) -> AppResult<McpServer> {
        let _guard = self.mcp_lock.lock().await;
        self.database
            .call(move |conn| {
                let changed = conn.execute(
                    "UPDATE tool_mcp_servers SET enabled = ?1, updated_at = ?2 WHERE server_id = ?3",
                    params![bool_to_i64(enabled), now_rfc3339(), server_id.to_string()],
                )?;
                if changed == 0 {
                    return Err(AppError::NotFound);
                }
                read_server_conn(conn, server_id)?.ok_or(AppError::NotFound)
            })
            .await
    }

    async fn save_mcp_server(&self, draft: McpServerDraft) -> AppResult<McpServer> {
        let _guard = self.mcp_lock.lock().await;
        let existing = if let Some(id) = draft.id {
            self.database
                .call(move |conn| read_server_conn(conn, id))
                .await?
        } else {
            None
        };
        let id = draft.id.unwrap_or_else(Uuid::new_v4);
        let transport = existing
            .as_ref()
            .map(|server| {
                draft
                    .transport
                    .clone()
                    .merge_redacted_secrets(&server.transport)
            })
            .unwrap_or(draft.transport);
        let name = draft.name.trim().to_string();
        if name.is_empty() {
            return Err(AppError::BadRequest(
                "MCP server name is required".to_string(),
            ));
        }
        let current_id = draft.id.map(|id| id.to_string());
        let name_for_lookup = name.clone();
        let duplicate = self
            .database
            .call(move |conn| {
                let count = conn.query_row(
                    "SELECT COUNT(*) FROM tool_mcp_servers
                     WHERE name = ?1 COLLATE NOCASE
                       AND (?2 IS NULL OR server_id <> ?2)",
                    params![name_for_lookup, current_id],
                    |row| row.get::<_, i64>(0),
                )?;
                Ok(count > 0)
            })
            .await?;
        if duplicate {
            return Err(AppError::Conflict(format!(
                "MCP server name '{}' is already configured",
                name
            )));
        }
        let now = now_rfc3339();
        self.database.call(move |conn| {
            conn.execute(
                "INSERT INTO tool_mcp_servers(server_id, name, description, enabled, supports_parallel_tool_calls, provider, provider_url, logo_url, tags_json, transport_json, status, last_synced_at, last_error, diagnostics_json, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, 'unknown', NULL, NULL, '{}', ?11, ?11)
                 ON CONFLICT(server_id) DO UPDATE SET name=excluded.name, description=excluded.description,
                 enabled=excluded.enabled, supports_parallel_tool_calls=excluded.supports_parallel_tool_calls, provider=excluded.provider, provider_url=excluded.provider_url,
                 logo_url=excluded.logo_url, tags_json=excluded.tags_json, transport_json=excluded.transport_json, updated_at=excluded.updated_at",
                params![id.to_string(), name, draft.description.trim(), bool_to_i64(draft.enabled), bool_to_i64(draft.supports_parallel_tool_calls), draft.provider, draft.provider_url, draft.logo_url, serde_json::to_string(&draft.tags)?, serde_json::to_string(&transport)?, now],
            )?;
            read_server_conn(conn, id)?.ok_or(AppError::NotFound)
        }).await
    }

    pub async fn delete_mcp_server(&self, id: Uuid) -> AppResult<()> {
        let _guard = self.mcp_lock.lock().await;
        self.database
            .call(move |conn| {
                let changed = conn.execute(
                    "DELETE FROM tool_mcp_servers WHERE server_id = ?1",
                    [id.to_string()],
                )?;
                if changed == 0 {
                    return Err(AppError::NotFound);
                }
                Ok(())
            })
            .await
    }

    pub async fn replace_mcp_tools(&self, server_id: Uuid, tools: &[McpTool]) -> AppResult<()> {
        let _guard = self.mcp_lock.lock().await;
        let tools = tools.to_vec();
        self.database.call(move |conn| {
            let tx = conn.transaction()?;
            let existing_names = {
                let mut statement = tx.prepare("SELECT tool_name FROM tool_mcp_tools WHERE server_id = ?1")?;
                statement
                    .query_map([server_id.to_string()], |row| row.get::<_, String>(0))?
                    .collect::<Result<HashSet<_>, _>>()?
            };
            let current_names = tools.iter().map(|tool| tool.name.as_str()).collect::<HashSet<_>>();
            for tool_name in existing_names.iter().filter(|name| !current_names.contains(name.as_str())) {
                tx.execute(
                    "DELETE FROM tool_mcp_tools WHERE server_id = ?1 AND tool_name = ?2",
                    params![server_id.to_string(), tool_name],
                )?;
            }
            for tool in tools {
                tx.execute(
                    "INSERT INTO tool_mcp_tools(server_id, tool_name, qualified_name, title, description, input_schema_json, enabled, updated_at)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
                     ON CONFLICT(server_id, tool_name) DO UPDATE SET qualified_name=excluded.qualified_name,
                     title=excluded.title, description=excluded.description, input_schema_json=excluded.input_schema_json, updated_at=excluded.updated_at",
                    params![server_id.to_string(), tool.name, tool.qualified_name, tool.title, tool.description,
                        serde_json::to_string(&tool.input_schema)?, bool_to_i64(tool.enabled), tool.updated_at],
                )?;
            }
            tx.commit()?;
            Ok(())
        }).await
    }

    pub async fn update_mcp_tool_enabled(
        &self,
        server_id: Uuid,
        tool_name: &str,
        enabled: bool,
    ) -> AppResult<()> {
        let _guard = self.mcp_lock.lock().await;
        let tool_name = tool_name.to_string();
        self.database.call(move |conn| {
            let changed = conn.execute("UPDATE tool_mcp_tools SET enabled = ?1 WHERE server_id = ?2 AND tool_name = ?3", params![bool_to_i64(enabled), server_id.to_string(), tool_name])?;
            if changed == 0 { return Err(AppError::NotFound); }
            Ok(())
        }).await
    }

    pub async fn update_mcp_tools_enabled(
        &self,
        server_id: Uuid,
        tool_names: &[String],
        enabled: bool,
    ) -> AppResult<()> {
        let _guard = self.mcp_lock.lock().await;
        let names = tool_names.to_vec();
        self.database.call(move |conn| {
            let tx = conn.transaction()?;
            for tool_name in names {
                let changed = tx.execute(
                    "UPDATE tool_mcp_tools SET enabled = ?1 WHERE server_id = ?2 AND tool_name = ?3",
                    params![bool_to_i64(enabled), server_id.to_string(), tool_name],
                )?;
                if changed == 0 {
                    return Err(AppError::NotFound);
                }
            }
            tx.commit()?;
            Ok(())
        }).await
    }

    pub async fn update_mcp_server_status(
        &self,
        server_id: Uuid,
        status: McpServerStatus,
        last_error: Option<String>,
        synced_at: Option<String>,
    ) -> AppResult<()> {
        let _guard = self.mcp_lock.lock().await;
        let status = match status {
            McpServerStatus::Unknown => "unknown",
            McpServerStatus::Connecting => "connecting",
            McpServerStatus::Ready => "ready",
            McpServerStatus::Offline => "offline",
            McpServerStatus::Error => "error",
        };
        self.database.call(move |conn| {
            conn.execute("UPDATE tool_mcp_servers SET status = ?1, last_error = ?2, last_synced_at = COALESCE(?3, last_synced_at), updated_at = ?4 WHERE server_id = ?5", params![status, last_error, synced_at, now_rfc3339(), server_id.to_string()])?;
            Ok(())
        }).await
    }

    pub async fn update_mcp_server_diagnostics(
        &self,
        server_id: Uuid,
        diagnostics: McpDiagnostics,
    ) -> AppResult<()> {
        let _guard = self.mcp_lock.lock().await;
        self.database.call(move |conn| {
            let changed = conn.execute(
                "UPDATE tool_mcp_servers SET diagnostics_json = ?1, updated_at = ?2 WHERE server_id = ?3",
                params![serde_json::to_string(&diagnostics)?, now_rfc3339(), server_id.to_string()],
            )?;
            if changed == 0 {
                return Err(AppError::NotFound);
            }
            Ok(())
        }).await
    }

    pub async fn read_mcp_agent_settings(&self, agent_id: Uuid) -> AppResult<McpAgentSettings> {
        let _guard = self.mcp_lock.lock().await;
        let defaults = self.database.call(|conn| read_servers_conn(conn)).await?;
        let overrides = self
            .database
            .call(move |conn| read_overrides_conn(conn, agent_id))
            .await?;
        let effective = apply_overrides(&defaults, &overrides);
        Ok(McpAgentSettings {
            defaults,
            overrides,
            effective,
        })
    }

    pub async fn update_mcp_agent_overrides(
        &self,
        agent_id: Uuid,
        overrides: McpAgentOverrides,
    ) -> AppResult<McpAgentSettings> {
        let _guard = self.mcp_lock.lock().await;
        let defaults = self.database.call(|conn| read_servers_conn(conn)).await?;
        let copy = overrides.clone();
        self.database.call(move |conn| {
            let tx = conn.transaction()?;
            tx.execute("DELETE FROM tool_mcp_agent_servers WHERE agent_id = ?1", [agent_id.to_string()])?;
            tx.execute("DELETE FROM tool_mcp_agent_tools WHERE agent_id = ?1", [agent_id.to_string()])?;
            for item in copy.servers.iter().filter(|item| item.enabled.is_some()) {
                tx.execute("INSERT INTO tool_mcp_agent_servers(agent_id, server_id, enabled, customized) VALUES (?1, ?2, ?3, ?4)", params![agent_id.to_string(), item.server_id.to_string(), bool_to_i64(item.enabled.unwrap_or(true)), bool_to_i64(item.customized)])?;
            }
            for item in copy.tools.iter().filter(|item| item.enabled.is_some()) {
                tx.execute("INSERT INTO tool_mcp_agent_tools(agent_id, server_id, tool_name, enabled) VALUES (?1, ?2, ?3, ?4)", params![agent_id.to_string(), item.server_id.to_string(), item.tool_name, bool_to_i64(item.enabled.unwrap_or(true))])?;
            }
            tx.commit()?;
            Ok(())
        }).await?;
        let effective = apply_overrides(&defaults, &overrides);
        Ok(McpAgentSettings {
            defaults,
            overrides,
            effective,
        })
    }
}

fn read_servers_conn(conn: &Connection) -> AppResult<Vec<McpServer>> {
    let mut statement = conn.prepare("SELECT server_id, name, description, enabled, supports_parallel_tool_calls, provider, provider_url, logo_url, tags_json, transport_json, status, last_synced_at, last_error, diagnostics_json FROM tool_mcp_servers ORDER BY name COLLATE NOCASE")?;
    let rows = statement.query_map([], |row| {
        let id = Uuid::parse_str(&row.get::<_, String>(0)?)
            .map_err(|error| rusqlite::Error::ToSqlConversionFailure(Box::new(error)))?;
        let tags =
            serde_json::from_str::<Vec<String>>(&row.get::<_, String>(8)?).unwrap_or_default();
        let transport = serde_json::from_str::<McpTransport>(&row.get::<_, String>(9)?)
            .map_err(|error| rusqlite::Error::ToSqlConversionFailure(Box::new(error)))?;
        let status = match row.get::<_, String>(10)?.as_str() {
            "connecting" => McpServerStatus::Connecting,
            "ready" => McpServerStatus::Ready,
            "offline" => McpServerStatus::Offline,
            "error" => McpServerStatus::Error,
            _ => McpServerStatus::Unknown,
        };
        let diagnostics = serde_json::from_str::<McpDiagnostics>(&row.get::<_, String>(13)?)
            .unwrap_or_default();
        Ok((
            id,
            McpServer {
                id,
                name: row.get(1)?,
                description: row.get(2)?,
                enabled: row.get::<_, i64>(3)? != 0,
                supports_parallel_tool_calls: row.get::<_, i64>(4)? != 0,
                provider: row.get(5)?,
                provider_url: row.get(6)?,
                logo_url: row.get(7)?,
                tags,
                transport,
                tools: Vec::new(),
                status,
                last_synced_at: row.get(11)?,
                last_error: row.get(12)?,
                diagnostics,
            },
        ))
    })?;
    let mut servers = Vec::new();
    for row in rows {
        let (id, mut server) = row?;
        server.tools = read_tools_conn(conn, id)?;
        servers.push(server);
    }
    Ok(servers)
}

fn read_server_conn(conn: &Connection, id: Uuid) -> AppResult<Option<McpServer>> {
    Ok(read_servers_conn(conn)?
        .into_iter()
        .find(|server| server.id == id))
}

fn read_tools_conn(conn: &Connection, server_id: Uuid) -> AppResult<Vec<McpTool>> {
    let mut statement = conn.prepare("SELECT tool_name, qualified_name, title, description, input_schema_json, enabled, updated_at FROM tool_mcp_tools WHERE server_id = ?1 ORDER BY tool_name COLLATE NOCASE")?;
    let rows = statement.query_map([server_id.to_string()], |row| {
        let schema = serde_json::from_str(&row.get::<_, String>(4)?)
            .map_err(|error| rusqlite::Error::ToSqlConversionFailure(Box::new(error)))?;
        Ok(McpTool {
            name: row.get(0)?,
            qualified_name: row.get(1)?,
            title: row.get(2)?,
            description: row.get(3)?,
            input_schema: schema,
            enabled: row.get::<_, i64>(5)? != 0,
            updated_at: row.get(6)?,
        })
    })?;
    Ok(rows.collect::<Result<Vec<_>, _>>()?)
}

fn has_column(conn: &Connection, table: &str, column: &str) -> rusqlite::Result<bool> {
    let mut statement = conn.prepare(&format!("PRAGMA table_info({table})"))?;
    let columns = statement.query_map([], |row| row.get::<_, String>(1))?;
    Ok(columns.filter_map(Result::ok).any(|name| name == column))
}

fn read_overrides_conn(conn: &Connection, agent_id: Uuid) -> AppResult<McpAgentOverrides> {
    let mut servers = Vec::new();
    let mut statement = conn.prepare(
        "SELECT server_id, enabled, customized FROM tool_mcp_agent_servers WHERE agent_id = ?1",
    )?;
    for row in statement.query_map([agent_id.to_string()], |row| {
        let id = Uuid::parse_str(&row.get::<_, String>(0)?)
            .map_err(|error| rusqlite::Error::ToSqlConversionFailure(Box::new(error)))?;
        Ok(McpServerOverride {
            server_id: id,
            enabled: Some(row.get::<_, i64>(1)? != 0),
            customized: row.get::<_, i64>(2)? != 0,
        })
    })? {
        servers.push(row?);
    }
    let mut tools = Vec::new();
    let mut statement = conn.prepare(
        "SELECT server_id, tool_name, enabled FROM tool_mcp_agent_tools WHERE agent_id = ?1",
    )?;
    for row in statement.query_map([agent_id.to_string()], |row| {
        let id = Uuid::parse_str(&row.get::<_, String>(0)?)
            .map_err(|error| rusqlite::Error::ToSqlConversionFailure(Box::new(error)))?;
        Ok(McpToolOverride {
            server_id: id,
            tool_name: row.get(1)?,
            enabled: Some(row.get::<_, i64>(2)? != 0),
        })
    })? {
        tools.push(row?);
    }
    Ok(McpAgentOverrides { servers, tools })
}

fn apply_overrides(defaults: &[McpServer], overrides: &McpAgentOverrides) -> Vec<McpServer> {
    defaults
        .iter()
        .cloned()
        .map(|mut server| {
            if let Some(item) = overrides
                .servers
                .iter()
                .find(|item| item.server_id == server.id)
            {
                if let Some(enabled) = item.enabled {
                    server.enabled = enabled;
                }
            }
            for tool in &mut server.tools {
                if let Some(item) = overrides
                    .tools
                    .iter()
                    .find(|item| item.server_id == server.id && item.tool_name == tool.name)
                {
                    if let Some(enabled) = item.enabled {
                        tool.enabled = enabled;
                    }
                }
            }
            server
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[tokio::test]
    async fn mcp_storage_round_trips_catalog_and_agent_overrides() {
        let directory = tempdir().unwrap();
        let store = Store::new(directory.path()).await.unwrap();
        let server = store
            .create_mcp_server(McpServerDraft {
                id: None,
                name: "Local MCP".to_string(),
                description: "Test server".to_string(),
                enabled: true,
                supports_parallel_tool_calls: false,
                provider: Some("WeiAgent".to_string()),
                provider_url: Some("https://example.test".to_string()),
                logo_url: None,
                tags: vec!["local".to_string()],
                transport: McpTransport::Stdio {
                    command: "fake-mcp".to_string(),
                    args: vec!["--stdio".to_string()],
                    cwd: None,
                    env: Default::default(),
                    inherit_env: false,
                    env_allowlist: Vec::new(),
                    long_running: false,
                    timeout_ms: 60_000,
                },
            })
            .await
            .unwrap();
        let tool = McpTool {
            name: "read_file".to_string(),
            qualified_name: "mcp__12345678__read_file".to_string(),
            title: Some("Read file".to_string()),
            description: "Reads a file".to_string(),
            input_schema: serde_json::json!({"type":"object"}),
            enabled: true,
            updated_at: Some(Utc::now().to_rfc3339()),
        };
        store
            .replace_mcp_tools(server.id, &[tool.clone()])
            .await
            .unwrap();
        store
            .update_mcp_tool_enabled(server.id, &tool.name, false)
            .await
            .unwrap();

        let agent_id = store
            .list_agents()
            .await
            .unwrap()
            .into_iter()
            .next()
            .unwrap()
            .id;
        let settings = store
            .update_mcp_agent_overrides(
                agent_id,
                McpAgentOverrides {
                    servers: vec![McpServerOverride {
                        server_id: server.id,
                        enabled: Some(false),
                        customized: true,
                    }],
                    tools: vec![McpToolOverride {
                        server_id: server.id,
                        tool_name: tool.name.clone(),
                        enabled: Some(true),
                    }],
                },
            )
            .await
            .unwrap();
        assert!(!settings.effective[0].enabled);
        assert!(settings.effective[0].tools[0].enabled);

        let cached = store.list_mcp_servers().await.unwrap();
        assert_eq!(cached[0].tools.len(), 1);
        assert!(!cached[0].tools[0].enabled);

        store.replace_mcp_tools(server.id, &[]).await.unwrap();
        assert!(store.list_mcp_servers().await.unwrap()[0].tools.is_empty());
        store.delete_mcp_server(server.id).await.unwrap();
        assert!(store.list_mcp_servers().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn mcp_server_names_are_unique_case_insensitively() {
        let directory = tempdir().unwrap();
        let store = Store::new(directory.path()).await.unwrap();
        let transport = McpTransport::Stdio {
            command: "fake-mcp".to_string(),
            args: Vec::new(),
            cwd: None,
            env: Default::default(),
            inherit_env: false,
            env_allowlist: Vec::new(),
            long_running: false,
            timeout_ms: 60_000,
        };
        let server = store
            .create_mcp_server(McpServerDraft {
                id: None,
                name: "Local MCP".to_string(),
                description: String::new(),
                enabled: true,
                supports_parallel_tool_calls: false,
                provider: None,
                provider_url: None,
                logo_url: None,
                tags: Vec::new(),
                transport: transport.clone(),
            })
            .await
            .unwrap();

        let duplicate = store
            .create_mcp_server(McpServerDraft {
                id: None,
                name: " local mcp ".to_string(),
                description: String::new(),
                enabled: true,
                supports_parallel_tool_calls: false,
                provider: None,
                provider_url: None,
                logo_url: None,
                tags: Vec::new(),
                transport: transport.clone(),
            })
            .await;
        assert!(matches!(duplicate, Err(AppError::Conflict(_))));

        let updated = store
            .update_mcp_server(McpServerDraft {
                id: Some(server.id),
                name: " local mcp ".to_string(),
                description: String::new(),
                enabled: true,
                supports_parallel_tool_calls: false,
                provider: None,
                provider_url: None,
                logo_url: None,
                tags: Vec::new(),
                transport,
            })
            .await
            .unwrap();
        assert_eq!(updated.name, "local mcp");
    }

    #[tokio::test]
    async fn mcp_server_enabled_switch_updates_only_server_state() {
        let directory = tempdir().unwrap();
        let store = Store::new(directory.path()).await.unwrap();
        let server = store
            .create_mcp_server(McpServerDraft {
                id: None,
                name: "Toggle MCP".to_string(),
                description: String::new(),
                enabled: true,
                supports_parallel_tool_calls: false,
                provider: None,
                provider_url: None,
                logo_url: None,
                tags: Vec::new(),
                transport: McpTransport::Stdio {
                    command: "fake-mcp".to_string(),
                    args: Vec::new(),
                    cwd: None,
                    env: Default::default(),
                    inherit_env: false,
                    env_allowlist: Vec::new(),
                    long_running: false,
                    timeout_ms: 60_000,
                },
            })
            .await
            .unwrap();
        let disabled = store
            .update_mcp_server_enabled(server.id, false)
            .await
            .unwrap();
        assert!(!disabled.enabled);
        assert_eq!(disabled.name, "Toggle MCP");
        assert_eq!(store.list_mcp_servers().await.unwrap()[0].enabled, false);
    }
}
