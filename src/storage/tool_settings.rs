use super::tool_settings_schema::ensure_tool_settings_schema;
use std::collections::HashSet;

use chrono::Utc;
use rusqlite::{Connection, OptionalExtension, params};

use super::sql_util::{
    bool_to_i64, i64_to_bool, i64_to_u16, i64_to_u32, i64_to_u64, now_rfc3339, parse_datetime,
    parse_optional_datetime, u64_to_i64, usize_to_i64,
};
use uuid::Uuid;

use crate::domain::tools::{
    MAX_TASK_PLAN_PROGRESS_INTERVAL, TaskPlanningToolOverrides, TerminalToolOverrides,
    WebContentExtractionConfig, WebSearchKeyOperation,
};
use crate::{
    domain::{
        AgentToolsConfig, AgentToolsOverrides, AgentToolsSettings, EvidenceStorageConfig,
        TaskPlanningToolConfig, TerminalApprovalGrant, TerminalPermissionMode, TerminalToolConfig,
        WebSearchKeySelection, WebSearchKeyStatusRecord, WebSearchProviderKind,
        WebSearchSourceConfig, WebSearchSourceOverrides, WebSearchToolConfig,
        WebSearchToolOverrides,
    },
    error::{AppError, AppResult},
    storage::store::Store,
};

// Migration versions share one application-wide namespace with conversations
// and provider migrations, so task planning must use the next free version.

impl Store {
    pub(super) async fn initialize_tool_settings(&self) -> AppResult<()> {
        let agents = self.read_agents_unlocked().await?;
        let default_exists = self
            .database
            .call(|conn| {
                ensure_tool_settings_schema(conn)?;
                compact_default_exists(conn)
            })
            .await?;

        if !default_exists {
            let mut defaults = AgentToolsConfig::default();
            defaults.web_search.ensure_builtin_sources(None);
            defaults.web_search.disable_unavailable_sources();
            validate_tools_config(&defaults)?;

            self.database
                .call(move |conn| {
                    let tx = conn.transaction()?;
                    write_default_settings_conn(&tx, &defaults)?;
                    tx.commit()?;
                    Ok(())
                })
                .await?;
        }

        self.prune_orphan_agent_tool_settings(&agents).await?;
        Ok(())
    }

    pub async fn read_tool_defaults(&self) -> AppResult<AgentToolsConfig> {
        let _guard = self.tool_settings_lock.lock().await;
        self.read_tool_defaults_unlocked().await
    }

    pub async fn update_tool_defaults(
        &self,
        mut config: AgentToolsConfig,
    ) -> AppResult<AgentToolsConfig> {
        if !config.terminal.approval_grants.is_empty() {
            return Err(AppError::BadRequest(
                "global tool defaults cannot contain agent approval grants".to_string(),
            ));
        }
        config.terminal.approval_grants.clear();
        let _guard = self.tool_settings_lock.lock().await;
        let existing = self.read_tool_defaults_unlocked().await?;
        config.web_search = existing.web_search.merge_secret_update(&config.web_search);
        config
            .web_search
            .ensure_builtin_sources(Some(&existing.web_search));
        config.web_search.disable_unavailable_sources();
        normalize_key_status_slots(&mut config);
        validate_tools_config(&config)?;
        self.write_tool_defaults_unlocked(&config).await?;
        Ok(config)
    }

    pub async fn update_web_search_key_statuses(
        &self,
        updates: &[crate::domain::tools::WebSearchKeyStatusUpdate],
    ) -> AppResult<usize> {
        if updates.is_empty() {
            return Ok(0);
        }

        let _guard = self.tool_settings_lock.lock().await;
        let mut config = self.read_tool_defaults_unlocked().await?;
        let mut updated = 0;
        for update in updates {
            let Some(source) = config
                .web_search
                .sources
                .iter_mut()
                .find(|source| source.id == update.source_id)
            else {
                continue;
            };
            let blocked_recorded = update.record.status == "officially_blocked"
                && source.remember_blocked_api_key(&update.api_key);
            let key_index = source
                .api_keys
                .get(update.key_index)
                .filter(|key| key.trim() == update.api_key)
                .map(|_| update.key_index)
                .or_else(|| {
                    source
                        .api_keys
                        .iter()
                        .position(|key| key.trim() == update.api_key)
                });
            let Some(key_index) = key_index else {
                if blocked_recorded {
                    updated += 1;
                }
                continue;
            };
            let statuses = match update.operation {
                WebSearchKeyOperation::Search => &mut source.key_statuses,
                WebSearchKeyOperation::Extract => &mut source.extract_key_statuses,
            };
            statuses.resize_with(source.api_keys.len(), || None);
            statuses.truncate(source.api_keys.len());
            statuses[key_index] = Some(update.record.clone());
            updated += 1;
        }

        if updated > 0 {
            config.web_search.disable_unavailable_sources();
            validate_tools_config(&config)?;
            self.write_tool_defaults_unlocked(&config).await?;
        }
        Ok(updated)
    }

    pub async fn read_agent_tools_settings(&self, id: Uuid) -> AppResult<AgentToolsSettings> {
        let _guard = self.tool_settings_lock.lock().await;
        self.ensure_agent_exists_unlocked(id).await?;
        let defaults = self.read_tool_defaults_unlocked().await?;
        let overrides = self
            .read_agent_tools_overrides_unlocked(id, &defaults)
            .await?;
        let effective = overrides.apply_to(&defaults);
        Ok(AgentToolsSettings {
            defaults,
            overrides,
            effective,
        })
    }

    pub async fn read_agent_tools_config(&self, id: Uuid) -> AppResult<AgentToolsConfig> {
        Ok(self.read_agent_tools_settings(id).await?.effective)
    }

    pub async fn update_agent_tools_overrides(
        &self,
        id: Uuid,
        mut overrides: AgentToolsOverrides,
    ) -> AppResult<AgentToolsSettings> {
        let _guard = self.tool_settings_lock.lock().await;
        self.ensure_agent_exists_unlocked(id).await?;
        let defaults = self.read_tool_defaults_unlocked().await?;
        overrides.remove_expired_approval_grants(Utc::now());
        validate_tools_overrides(&overrides, &defaults)?;
        self.write_agent_tools_overrides_unlocked(id, &overrides)
            .await?;
        let effective = overrides.apply_to(&defaults);
        Ok(AgentToolsSettings {
            defaults,
            overrides,
            effective,
        })
    }

    #[cfg(test)]
    pub async fn update_agent_tools_config(
        &self,
        id: Uuid,
        config: AgentToolsConfig,
    ) -> AppResult<AgentToolsConfig> {
        validate_tools_config(&config)?;
        let _guard = self.tool_settings_lock.lock().await;
        self.ensure_agent_exists_unlocked(id).await?;
        let overrides = AgentToolsOverrides::full_override(&config);
        self.write_agent_tools_overrides_unlocked(id, &overrides)
            .await?;
        Ok(config)
    }

    pub async fn always_allow_terminal_command(
        &self,
        id: Uuid,
        command: &str,
        cwd: Option<&str>,
    ) -> AppResult<AgentToolsConfig> {
        let _guard = self.tool_settings_lock.lock().await;
        self.ensure_agent_exists_unlocked(id).await?;
        let defaults = self.read_tool_defaults_unlocked().await?;
        let mut overrides = self
            .read_agent_tools_overrides_unlocked(id, &defaults)
            .await?;
        let now = Utc::now();
        let grant = TerminalApprovalGrant {
            command: command.to_string(),
            cwd: cwd.map(str::to_string),
            permission_mode: TerminalPermissionMode::RequestApproval,
            created_at: Some(now),
            expires_at: Some(now + chrono::Duration::hours(24)),
        };
        overrides.remove_expired_approval_grants(now);
        if !overrides.terminal.approval_grants.iter().any(|existing| {
            existing.command == grant.command
                && existing.cwd == grant.cwd
                && existing.permission_mode == grant.permission_mode
                && existing
                    .expires_at
                    .is_some_and(|expires_at| expires_at > now)
        }) {
            overrides.terminal.approval_grants.push(grant);
        }
        validate_tools_overrides(&overrides, &defaults)?;
        self.write_agent_tools_overrides_unlocked(id, &overrides)
            .await?;
        Ok(overrides.apply_to(&defaults))
    }

    pub(super) async fn delete_agent_tool_settings_unlocked(&self, id: Uuid) -> AppResult<()> {
        self.database
            .call(move |conn| {
                conn.execute(
                    "DELETE FROM tool_web_search_agent_sources WHERE agent_id = ?1",
                    [id.to_string()],
                )?;
                conn.execute(
                    "DELETE FROM tool_settings WHERE scope_type = 'agent' AND scope_id = ?1",
                    [id.to_string()],
                )?;
                Ok(())
            })
            .await
    }

    async fn read_tool_defaults_unlocked(&self) -> AppResult<AgentToolsConfig> {
        self.database
            .call(|conn| {
                let mut config = read_default_settings_conn(conn)?.ok_or_else(|| {
                    AppError::Database("default tool settings row is missing".to_string())
                })?;
                let mut repaired = config.web_search.ensure_builtin_sources(None);
                repaired |= config.web_search.disable_unavailable_sources();
                repaired |= normalize_key_status_slots(&mut config);
                validate_tools_config(&config)?;
                if repaired {
                    let tx = conn.transaction()?;
                    write_default_settings_conn(&tx, &config)?;
                    tx.commit()?;
                }
                Ok(config)
            })
            .await
    }

    async fn read_agent_tools_overrides_unlocked(
        &self,
        id: Uuid,
        defaults: &AgentToolsConfig,
    ) -> AppResult<AgentToolsOverrides> {
        let defaults = defaults.clone();
        self.database
            .call(move |conn| {
                let mut overrides = read_agent_overrides_conn(conn, id)?.unwrap_or_default();
                if overrides.remove_expired_approval_grants(Utc::now()) {
                    let tx = conn.transaction()?;
                    write_agent_overrides_conn(&tx, id, &overrides)?;
                    tx.commit()?;
                }
                validate_tools_overrides(&overrides, &defaults)?;
                Ok(overrides)
            })
            .await
    }

    async fn write_tool_defaults_unlocked(&self, config: &AgentToolsConfig) -> AppResult<()> {
        let config = config.clone();
        self.database
            .call(move |conn| {
                let tx = conn.transaction()?;
                write_default_settings_conn(&tx, &config)?;
                tx.commit()?;
                Ok(())
            })
            .await
    }

    async fn write_agent_tools_overrides_unlocked(
        &self,
        id: Uuid,
        overrides: &AgentToolsOverrides,
    ) -> AppResult<()> {
        let overrides = overrides.clone();
        self.database
            .call(move |conn| {
                let tx = conn.transaction()?;
                write_agent_overrides_conn(&tx, id, &overrides)?;
                tx.commit()?;
                Ok(())
            })
            .await
    }

    async fn ensure_agent_exists_unlocked(&self, id: Uuid) -> AppResult<()> {
        if self
            .read_agents_unlocked()
            .await?
            .iter()
            .any(|agent| agent.id == id)
        {
            Ok(())
        } else {
            Err(AppError::NotFound)
        }
    }

    async fn prune_orphan_agent_tool_settings(
        &self,
        agents: &[crate::domain::Agent],
    ) -> AppResult<()> {
        let valid_ids = agents
            .iter()
            .map(|agent| agent.id.to_string())
            .collect::<HashSet<_>>();
        self.database
            .call(move |conn| {
                let tx = conn.transaction()?;
                let mut stmt = tx.prepare(
                    "SELECT scope_id FROM tool_settings WHERE scope_type = 'agent' ORDER BY scope_id",
                )?;
                let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
                let mut orphan_ids = Vec::new();
                for row in rows {
                    let id = row?;
                    if !valid_ids.contains(&id) {
                        orphan_ids.push(id);
                    }
                }
                drop(stmt);
                for id in orphan_ids {
                    tx.execute(
                        "DELETE FROM tool_settings WHERE scope_type = 'agent' AND scope_id = ?1",
                        [id],
                    )?;
                }
                tx.commit()?;
                Ok(())
            })
            .await
    }
}

fn normalize_key_status_slots(config: &mut AgentToolsConfig) -> bool {
    let mut changed = false;
    for source in &mut config.web_search.sources {
        if source.key_statuses.len() != source.api_keys.len() {
            source
                .key_statuses
                .resize_with(source.api_keys.len(), || None);
            source.key_statuses.truncate(source.api_keys.len());
            changed = true;
        }
        if source.extract_key_statuses.len() != source.api_keys.len() {
            source
                .extract_key_statuses
                .resize_with(source.api_keys.len(), || None);
            source.extract_key_statuses.truncate(source.api_keys.len());
            changed = true;
        }
    }
    changed
}

fn compact_default_exists(conn: &Connection) -> AppResult<bool> {
    Ok(conn
        .query_row(
            "SELECT 1 FROM tool_settings WHERE scope_type = 'default' AND scope_id = 'default'",
            [],
            |_| Ok(()),
        )
        .optional()?
        .is_some())
}

fn write_default_settings_conn(conn: &Connection, config: &AgentToolsConfig) -> AppResult<()> {
    let now = now_rfc3339();
    let roots_json = serde_json::to_string(&config.terminal.allowed_cwd_roots)?;
    conn.execute(
        "INSERT INTO tool_settings (
            scope_type, scope_id, config_version, max_llm_iterations,
            terminal_enabled, terminal_permission_mode, terminal_default_timeout_ms,
            terminal_allowed_cwd_roots_json, web_search_enabled, web_search_max_results,
            web_search_timeout_ms, web_search_key_selection, web_extract_remote_enabled,
            web_extract_max_remote_attempts, web_extract_allow_grok_fallback,
            evidence_storage_enabled, evidence_retention_days, evidence_max_total_mb,
            task_planning_enabled, task_planning_progress_interval,
            created_at, updated_at
         ) VALUES (
             'default', 'default', 1, ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?18
         )
         ON CONFLICT(scope_type, scope_id) DO UPDATE SET
            config_version = 1,
            max_llm_iterations = excluded.max_llm_iterations,
            terminal_enabled = excluded.terminal_enabled,
            terminal_permission_mode = excluded.terminal_permission_mode,
            terminal_default_timeout_ms = excluded.terminal_default_timeout_ms,
            terminal_allowed_cwd_roots_json = excluded.terminal_allowed_cwd_roots_json,
            web_search_enabled = excluded.web_search_enabled,
            web_search_max_results = excluded.web_search_max_results,
            web_search_timeout_ms = excluded.web_search_timeout_ms,
            web_search_key_selection = excluded.web_search_key_selection,
            web_extract_remote_enabled = excluded.web_extract_remote_enabled,
            web_extract_max_remote_attempts = excluded.web_extract_max_remote_attempts,
            web_extract_allow_grok_fallback = excluded.web_extract_allow_grok_fallback,
             evidence_storage_enabled = excluded.evidence_storage_enabled,
             evidence_retention_days = excluded.evidence_retention_days,
             evidence_max_total_mb = excluded.evidence_max_total_mb,
             task_planning_enabled = excluded.task_planning_enabled,
             task_planning_progress_interval = excluded.task_planning_progress_interval,
             updated_at = excluded.updated_at",
        params![
            i64::from(config.max_llm_iterations),
            bool_to_i64(config.terminal.enabled),
            permission_mode_as_str(&config.terminal.permission_mode),
            u64_to_i64(
                config.terminal.default_timeout_ms,
                "terminal_default_timeout_ms"
            )?,
            roots_json,
            bool_to_i64(config.web_search.enabled),
            i64::from(config.web_search.max_results),
            u64_to_i64(config.web_search.timeout_ms, "web_search_timeout_ms")?,
            key_selection_as_str(&config.web_search.key_selection),
            bool_to_i64(config.web_search.extraction.remote_enabled),
            i64::from(config.web_search.extraction.max_remote_attempts_per_url),
            bool_to_i64(config.web_search.extraction.allow_grok_fallback),
            bool_to_i64(config.evidence_storage.enabled),
            i64::from(config.evidence_storage.retention_days),
            u64_to_i64(
                config.evidence_storage.max_total_mb,
                "evidence_max_total_mb"
            )?,
            bool_to_i64(config.task_planning.enabled),
            i64::from(config.task_planning.progress_interval.clamp(
                1,
                MAX_TASK_PLAN_PROGRESS_INTERVAL,
            )),
            now,
        ],
    )?;

    conn.execute("DELETE FROM tool_web_search_sources", [])?;
    for (position, source) in config.web_search.sources.iter().enumerate() {
        let roles_json = serde_json::to_string(&source.roles)?;
        let blocked_json = serde_json::to_string(&source.blocked_api_key_hashes)?;
        conn.execute(
            "INSERT INTO tool_web_search_sources (
                source_id, scope_id, position, provider, display_name, enabled,
                deep_enabled, extract_enabled, priority, independence_group, roles_json,
                blocked_api_key_hashes_json, proxy_url
             ) VALUES (?1, 'default', ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
            params![
                source.id,
                usize_to_i64(position)?,
                source.provider.as_str(),
                source.display_name,
                bool_to_i64(source.enabled),
                bool_to_i64(source.deep_enabled),
                bool_to_i64(source.extract_enabled),
                i64::from(source.priority),
                source.independence_group,
                roles_json,
                blocked_json,
                source.proxy_url,
            ],
        )?;
        for (key_position, api_key) in source.api_keys.iter().enumerate() {
            let status = source
                .key_statuses
                .get(key_position)
                .and_then(Option::as_ref);
            conn.execute(
                "INSERT INTO tool_web_search_api_keys (
                    source_id, position, api_key, key_status, status_detail, checked_at,
                    extract_key_status, extract_status_detail, extract_checked_at, key_label
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
                params![
                    source.id,
                    usize_to_i64(key_position)?,
                    api_key,
                    status.map(|record| record.status.as_str()),
                    status.map(|record| record.detail.as_str()),
                    status.map(|record| record.checked_at.as_str()),
                    source
                        .extract_key_statuses
                        .get(key_position)
                        .and_then(Option::as_ref)
                        .map(|record| record.status.as_str()),
                    source
                        .extract_key_statuses
                        .get(key_position)
                        .and_then(Option::as_ref)
                        .map(|record| record.detail.as_str()),
                    source
                        .extract_key_statuses
                        .get(key_position)
                        .and_then(Option::as_ref)
                        .map(|record| record.checked_at.as_str()),
                    source
                        .api_key_labels
                        .get(key_position)
                        .map(String::as_str)
                        .unwrap_or(""),
                ],
            )?;
        }
    }
    Ok(())
}

fn write_agent_overrides_conn(
    conn: &Connection,
    agent_id: Uuid,
    overrides: &AgentToolsOverrides,
) -> AppResult<()> {
    let agent_id = agent_id.to_string();
    if agent_overrides_are_empty(overrides) {
        conn.execute(
            "DELETE FROM tool_web_search_agent_sources WHERE agent_id = ?1",
            [&agent_id],
        )?;
        conn.execute(
            "DELETE FROM tool_settings WHERE scope_type = 'agent' AND scope_id = ?1",
            [&agent_id],
        )?;
        return Ok(());
    }

    let roots_json = overrides
        .terminal
        .allowed_cwd_roots
        .as_ref()
        .map(serde_json::to_string)
        .transpose()?;
    let now = now_rfc3339();
    conn.execute(
        "INSERT INTO tool_settings (
            scope_type, scope_id, config_version, max_llm_iterations,
            terminal_enabled, terminal_permission_mode, terminal_default_timeout_ms,
            terminal_allowed_cwd_roots_json, web_search_enabled, web_search_max_results,
            web_search_timeout_ms, web_search_key_selection, evidence_storage_enabled,
            evidence_retention_days, evidence_max_total_mb,
            task_planning_enabled, task_planning_progress_interval,
            created_at, updated_at
         ) VALUES (
             'agent', ?1, 1, ?2, ?3, ?4, ?5, ?6, ?7, ?8,
             NULL, NULL, NULL, NULL, NULL, ?9, ?10, ?11, ?11
         )
         ON CONFLICT(scope_type, scope_id) DO UPDATE SET
            config_version = 1,
            max_llm_iterations = excluded.max_llm_iterations,
            terminal_enabled = excluded.terminal_enabled,
            terminal_permission_mode = excluded.terminal_permission_mode,
            terminal_default_timeout_ms = excluded.terminal_default_timeout_ms,
            terminal_allowed_cwd_roots_json = excluded.terminal_allowed_cwd_roots_json,
            web_search_enabled = excluded.web_search_enabled,
            web_search_max_results = excluded.web_search_max_results,
            web_search_timeout_ms = NULL,
            web_search_key_selection = NULL,
             evidence_storage_enabled = NULL,
             evidence_retention_days = NULL,
             evidence_max_total_mb = NULL,
             task_planning_enabled = excluded.task_planning_enabled,
             task_planning_progress_interval = excluded.task_planning_progress_interval,
             updated_at = excluded.updated_at",
        params![
            agent_id,
            overrides.max_llm_iterations.map(i64::from),
            overrides.terminal.enabled.map(bool_to_i64),
            overrides
                .terminal
                .permission_mode
                .as_ref()
                .map(permission_mode_as_str),
            overrides
                .terminal
                .default_timeout_ms
                .map(|value| u64_to_i64(value, "terminal_default_timeout_ms"))
                .transpose()?,
            roots_json,
            overrides.web_search.enabled.map(bool_to_i64),
            overrides.web_search.max_results.map(i64::from),
            overrides.task_planning.enabled.map(bool_to_i64),
            overrides
                .task_planning
                .progress_interval
                .map(|value| i64::from(value.clamp(1, MAX_TASK_PLAN_PROGRESS_INTERVAL))),
            now,
        ],
    )?;

    conn.execute(
        "DELETE FROM tool_terminal_approval_grants WHERE agent_id = ?1",
        [&agent_id],
    )?;
    for (position, grant) in overrides.terminal.approval_grants.iter().enumerate() {
        conn.execute(
            "INSERT INTO tool_terminal_approval_grants (
                agent_id, position, command, cwd, permission_mode, created_at, expires_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                agent_id,
                usize_to_i64(position)?,
                grant.command,
                grant.cwd,
                permission_mode_as_str(&grant.permission_mode),
                grant.created_at.map(|value| value.to_rfc3339()),
                grant.expires_at.map(|value| value.to_rfc3339()),
            ],
        )?;
    }
    conn.execute(
        "DELETE FROM tool_web_search_agent_sources WHERE agent_id = ?1",
        [&agent_id],
    )?;
    for source_override in &overrides.web_search.source_overrides {
        if source_override_is_empty(source_override) {
            continue;
        }
        let roles_json = source_override
            .roles
            .as_ref()
            .map(serde_json::to_string)
            .transpose()?;
        let proxy_url_override = bool_to_i64(source_override.proxy_url_override);
        conn.execute(
            "INSERT INTO tool_web_search_agent_sources (
                agent_id, source_id, enabled, deep_enabled, extract_enabled, priority,
                roles_json, proxy_url_override, proxy_url
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                agent_id,
                source_override.source_id,
                source_override.enabled.map(bool_to_i64),
                source_override.deep_enabled.map(bool_to_i64),
                source_override.extract_enabled.map(bool_to_i64),
                source_override.priority.map(i64::from),
                roles_json,
                proxy_url_override,
                source_override.proxy_url,
            ],
        )?;
    }
    Ok(())
}

struct DefaultSettingsRow {
    max_llm_iterations: i64,
    terminal_enabled: i64,
    terminal_permission_mode: String,
    terminal_default_timeout_ms: i64,
    terminal_allowed_cwd_roots_json: String,
    web_search_enabled: i64,
    web_search_max_results: i64,
    web_search_timeout_ms: i64,
    web_search_key_selection: String,
    web_extract_remote_enabled: i64,
    web_extract_max_remote_attempts: i64,
    web_extract_allow_grok_fallback: i64,
    evidence_storage_enabled: i64,
    evidence_retention_days: i64,
    evidence_max_total_mb: i64,
    task_planning_enabled: i64,
    task_planning_progress_interval: i64,
}

fn read_default_settings_conn(conn: &Connection) -> AppResult<Option<AgentToolsConfig>> {
    let row = conn
        .query_row(
            "SELECT max_llm_iterations, terminal_enabled, terminal_permission_mode,
                    terminal_default_timeout_ms, terminal_allowed_cwd_roots_json,
                    web_search_enabled, web_search_max_results, web_search_timeout_ms,
                    web_search_key_selection, web_extract_remote_enabled,
                    web_extract_max_remote_attempts, web_extract_allow_grok_fallback,
                    evidence_storage_enabled, evidence_retention_days, evidence_max_total_mb,
                    task_planning_enabled, task_planning_progress_interval
             FROM tool_settings
             WHERE scope_type = 'default' AND scope_id = 'default'",
            [],
            |row| {
                Ok(DefaultSettingsRow {
                    max_llm_iterations: row.get(0)?,
                    terminal_enabled: row.get(1)?,
                    terminal_permission_mode: row.get(2)?,
                    terminal_default_timeout_ms: row.get(3)?,
                    terminal_allowed_cwd_roots_json: row.get(4)?,
                    web_search_enabled: row.get(5)?,
                    web_search_max_results: row.get(6)?,
                    web_search_timeout_ms: row.get(7)?,
                    web_search_key_selection: row.get(8)?,
                    web_extract_remote_enabled: row.get(9)?,
                    web_extract_max_remote_attempts: row.get(10)?,
                    web_extract_allow_grok_fallback: row.get(11)?,
                    evidence_storage_enabled: row.get(12)?,
                    evidence_retention_days: row.get(13)?,
                    evidence_max_total_mb: row.get(14)?,
                    task_planning_enabled: row.get(15)?,
                    task_planning_progress_interval: row.get(16)?,
                })
            },
        )
        .optional()?;
    let Some(row) = row else {
        return Ok(None);
    };
    let allowed_cwd_roots = serde_json::from_str(&row.terminal_allowed_cwd_roots_json)?;
    Ok(Some(AgentToolsConfig {
        max_llm_iterations: i64_to_u32(row.max_llm_iterations, "max_llm_iterations")?,
        terminal: TerminalToolConfig {
            enabled: i64_to_bool(row.terminal_enabled, "terminal_enabled")?,
            permission_mode: permission_mode_from_str(&row.terminal_permission_mode)?,
            approval_grants: Vec::new(),
            default_timeout_ms: i64_to_u64(
                row.terminal_default_timeout_ms,
                "terminal_default_timeout_ms",
            )?,
            allowed_cwd_roots,
        },
        web_search: WebSearchToolConfig {
            enabled: i64_to_bool(row.web_search_enabled, "web_search_enabled")?,
            max_results: i64_to_u32(row.web_search_max_results, "web_search_max_results")?,
            timeout_ms: i64_to_u64(row.web_search_timeout_ms, "web_search_timeout_ms")?,
            key_selection: key_selection_from_str(&row.web_search_key_selection)?,
            extraction: WebContentExtractionConfig {
                remote_enabled: i64_to_bool(
                    row.web_extract_remote_enabled,
                    "web_extract_remote_enabled",
                )?,
                max_remote_attempts_per_url: i64_to_u32(
                    row.web_extract_max_remote_attempts,
                    "web_extract_max_remote_attempts",
                )?,
                allow_grok_fallback: i64_to_bool(
                    row.web_extract_allow_grok_fallback,
                    "web_extract_allow_grok_fallback",
                )?,
            },
            sources: read_compact_web_search_sources(conn)?,
        },
        evidence_storage: EvidenceStorageConfig {
            enabled: i64_to_bool(row.evidence_storage_enabled, "evidence_storage_enabled")?,
            retention_days: i64_to_u32(row.evidence_retention_days, "evidence_retention_days")?,
            max_total_mb: i64_to_u64(row.evidence_max_total_mb, "evidence_max_total_mb")?,
        },
        task_planning: TaskPlanningToolConfig {
            enabled: i64_to_bool(row.task_planning_enabled, "task_planning_enabled")?,
            progress_interval: i64_to_u32(
                row.task_planning_progress_interval,
                "task_planning_progress_interval",
            )?,
        },
    }))
}

struct AgentSettingsRow {
    max_llm_iterations: Option<i64>,
    terminal_enabled: Option<i64>,
    terminal_permission_mode: Option<String>,
    terminal_default_timeout_ms: Option<i64>,
    terminal_allowed_cwd_roots_json: Option<String>,
    web_search_enabled: Option<i64>,
    web_search_max_results: Option<i64>,
    task_planning_enabled: Option<i64>,
    task_planning_progress_interval: Option<i64>,
}

fn read_agent_overrides_conn(
    conn: &Connection,
    agent_id: Uuid,
) -> AppResult<Option<AgentToolsOverrides>> {
    let agent_id = agent_id.to_string();
    let row = conn
        .query_row(
            "SELECT max_llm_iterations, terminal_enabled, terminal_permission_mode,
                    terminal_default_timeout_ms, terminal_allowed_cwd_roots_json,
                    web_search_enabled, web_search_max_results,
                    task_planning_enabled, task_planning_progress_interval
             FROM tool_settings
             WHERE scope_type = 'agent' AND scope_id = ?1",
            [&agent_id],
            |row| {
                Ok(AgentSettingsRow {
                    max_llm_iterations: row.get(0)?,
                    terminal_enabled: row.get(1)?,
                    terminal_permission_mode: row.get(2)?,
                    terminal_default_timeout_ms: row.get(3)?,
                    terminal_allowed_cwd_roots_json: row.get(4)?,
                    web_search_enabled: row.get(5)?,
                    web_search_max_results: row.get(6)?,
                    task_planning_enabled: row.get(7)?,
                    task_planning_progress_interval: row.get(8)?,
                })
            },
        )
        .optional()?;
    let Some(row) = row else {
        return Ok(None);
    };
    let allowed_cwd_roots = row
        .terminal_allowed_cwd_roots_json
        .as_deref()
        .map(serde_json::from_str)
        .transpose()?;
    Ok(Some(AgentToolsOverrides {
        version: 1,
        max_llm_iterations: row
            .max_llm_iterations
            .map(|value| i64_to_u32(value, "max_llm_iterations"))
            .transpose()?,
        terminal: TerminalToolOverrides {
            enabled: row
                .terminal_enabled
                .map(|value| i64_to_bool(value, "terminal_enabled"))
                .transpose()?,
            permission_mode: row
                .terminal_permission_mode
                .as_deref()
                .map(permission_mode_from_str)
                .transpose()?,
            approval_grants: read_compact_terminal_grants(conn, &agent_id)?,
            default_timeout_ms: row
                .terminal_default_timeout_ms
                .map(|value| i64_to_u64(value, "terminal_default_timeout_ms"))
                .transpose()?,
            allowed_cwd_roots,
        },
        web_search: WebSearchToolOverrides {
            enabled: row
                .web_search_enabled
                .map(|value| i64_to_bool(value, "web_search_enabled"))
                .transpose()?,
            max_results: row
                .web_search_max_results
                .map(|value| i64_to_u32(value, "web_search_max_results"))
                .transpose()?,
            source_overrides: read_web_search_source_overrides(conn, &agent_id)?,
        },
        task_planning: TaskPlanningToolOverrides {
            enabled: row
                .task_planning_enabled
                .map(|value| i64_to_bool(value, "task_planning_enabled"))
                .transpose()?,
            progress_interval: row
                .task_planning_progress_interval
                .map(|value| i64_to_u32(value, "task_planning_progress_interval"))
                .transpose()?,
        },
    }))
}

fn read_web_search_source_overrides(
    conn: &Connection,
    agent_id: &str,
) -> AppResult<Vec<WebSearchSourceOverrides>> {
    let mut statement = conn.prepare(
        "SELECT source_id, enabled, deep_enabled, extract_enabled, priority,
                roles_json, proxy_url_override, proxy_url
         FROM tool_web_search_agent_sources
         WHERE agent_id = ?1
         ORDER BY source_id ASC",
    )?;
    let rows = statement.query_map([agent_id], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, Option<i64>>(1)?,
            row.get::<_, Option<i64>>(2)?,
            row.get::<_, Option<i64>>(3)?,
            row.get::<_, Option<i64>>(4)?,
            row.get::<_, Option<String>>(5)?,
            row.get::<_, i64>(6)?,
            row.get::<_, Option<String>>(7)?,
        ))
    })?;
    let mut overrides = Vec::new();
    for row in rows {
        let (
            source_id,
            enabled,
            deep_enabled,
            extract_enabled,
            priority,
            roles_json,
            proxy_url_override,
            proxy_url,
        ) = row?;
        overrides.push(WebSearchSourceOverrides {
            source_id,
            enabled: enabled
                .map(|value| i64_to_bool(value, "web search source override enabled"))
                .transpose()?,
            deep_enabled: deep_enabled
                .map(|value| i64_to_bool(value, "web search source override deep_enabled"))
                .transpose()?,
            extract_enabled: extract_enabled
                .map(|value| i64_to_bool(value, "web search source override extract_enabled"))
                .transpose()?,
            priority: priority
                .map(|value| i64_to_u16(value, "web search source override priority"))
                .transpose()?,
            roles: roles_json
                .map(|value| serde_json::from_str(&value))
                .transpose()?,
            proxy_url,
            proxy_url_override: i64_to_bool(
                proxy_url_override,
                "web search source proxy override",
            )?,
        });
    }
    Ok(overrides)
}

fn read_compact_terminal_grants(
    conn: &Connection,
    agent_id: &str,
) -> AppResult<Vec<TerminalApprovalGrant>> {
    let mut stmt = conn.prepare(
        "SELECT command, cwd, permission_mode, created_at, expires_at
         FROM tool_terminal_approval_grants
         WHERE agent_id = ?1 ORDER BY position ASC",
    )?;
    let rows = stmt.query_map([agent_id], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, Option<String>>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, Option<String>>(3)?,
            row.get::<_, String>(4)?,
        ))
    })?;
    let mut grants = Vec::new();
    for row in rows {
        let (command, cwd, permission_mode, created_at, expires_at) = row?;
        grants.push(TerminalApprovalGrant {
            command,
            cwd,
            permission_mode: permission_mode_from_str(&permission_mode)?,
            created_at: parse_optional_datetime(created_at, "approval created_at")?,
            expires_at: Some(parse_datetime(&expires_at, "approval expires_at")?),
        });
    }
    Ok(grants)
}

struct CompactSourceRow {
    source_id: String,
    provider: String,
    display_name: String,
    enabled: i64,
    deep_enabled: i64,
    extract_enabled: i64,
    priority: i64,
    independence_group: String,
    roles_json: String,
    blocked_api_key_hashes_json: String,
    proxy_url: Option<String>,
}

fn read_compact_web_search_sources(conn: &Connection) -> AppResult<Vec<WebSearchSourceConfig>> {
    let mut stmt = conn.prepare(
        "SELECT source_id, provider, display_name, enabled, deep_enabled, extract_enabled,
                priority, independence_group, roles_json, blocked_api_key_hashes_json, proxy_url
         FROM tool_web_search_sources ORDER BY position ASC",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok(CompactSourceRow {
            source_id: row.get(0)?,
            provider: row.get(1)?,
            display_name: row.get(2)?,
            enabled: row.get(3)?,
            deep_enabled: row.get(4)?,
            extract_enabled: row.get(5)?,
            priority: row.get(6)?,
            independence_group: row.get(7)?,
            roles_json: row.get(8)?,
            blocked_api_key_hashes_json: row.get(9)?,
            proxy_url: row.get(10)?,
        })
    })?;
    let mut source_rows = Vec::new();
    for row in rows {
        source_rows.push(row?);
    }
    drop(stmt);

    let mut sources = Vec::with_capacity(source_rows.len());
    for row in source_rows {
        let (api_keys, api_key_labels, key_statuses, extract_key_statuses) =
            read_compact_web_search_keys(conn, &row.source_id)?;
        sources.push(WebSearchSourceConfig {
            id: row.source_id,
            provider: source_provider_from_str(&row.provider)?,
            display_name: row.display_name,
            enabled: i64_to_bool(row.enabled, "web search source enabled")?,
            deep_enabled: i64_to_bool(row.deep_enabled, "web search source deep_enabled")?,
            extract_enabled: i64_to_bool(row.extract_enabled, "web search source extract_enabled")?,
            priority: i64_to_u16(row.priority, "web search source priority")?,
            independence_group: row.independence_group,
            roles: serde_json::from_str(&row.roles_json)?,
            api_keys,
            api_key_labels,
            api_key_fingerprints: Vec::new(),
            blocked_api_key_hashes: serde_json::from_str(&row.blocked_api_key_hashes_json)?,
            key_statuses,
            extract_key_statuses,
            proxy_url: row.proxy_url,
        });
    }
    Ok(sources)
}

type CompactWebSearchKeys = (
    Vec<String>,
    Vec<String>,
    Vec<Option<WebSearchKeyStatusRecord>>,
    Vec<Option<WebSearchKeyStatusRecord>>,
);

fn read_compact_web_search_keys(
    conn: &Connection,
    source_id: &str,
) -> AppResult<CompactWebSearchKeys> {
    let mut stmt = conn.prepare(
        "SELECT api_key, key_status, status_detail, checked_at,
                extract_key_status, extract_status_detail, extract_checked_at, key_label
         FROM tool_web_search_api_keys WHERE source_id = ?1 ORDER BY position ASC",
    )?;
    let rows = stmt.query_map([source_id], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, Option<String>>(1)?,
            row.get::<_, Option<String>>(2)?,
            row.get::<_, Option<String>>(3)?,
            row.get::<_, Option<String>>(4)?,
            row.get::<_, Option<String>>(5)?,
            row.get::<_, Option<String>>(6)?,
            row.get::<_, String>(7)?,
        ))
    })?;
    let mut api_keys = Vec::new();
    let mut labels = Vec::new();
    let mut statuses = Vec::new();
    let mut extract_statuses = Vec::new();
    for row in rows {
        let (
            api_key,
            status,
            detail,
            checked_at,
            extract_status,
            extract_detail,
            extract_checked_at,
            key_label,
        ) = row?;
        api_keys.push(api_key);
        labels.push(key_label);
        statuses.push(status.map(|status| WebSearchKeyStatusRecord {
            status,
            detail: detail.unwrap_or_default(),
            checked_at: checked_at.unwrap_or_default(),
        }));
        extract_statuses.push(extract_status.map(|status| WebSearchKeyStatusRecord {
            status,
            detail: extract_detail.unwrap_or_default(),
            checked_at: extract_checked_at.unwrap_or_default(),
        }));
    }
    Ok((api_keys, labels, statuses, extract_statuses))
}

fn agent_overrides_are_empty(overrides: &AgentToolsOverrides) -> bool {
    overrides.max_llm_iterations.is_none()
        && overrides.terminal.enabled.is_none()
        && overrides.terminal.permission_mode.is_none()
        && overrides.terminal.approval_grants.is_empty()
        && overrides.terminal.default_timeout_ms.is_none()
        && overrides.terminal.allowed_cwd_roots.is_none()
        && overrides.web_search.enabled.is_none()
        && overrides.web_search.max_results.is_none()
        && overrides.web_search.source_overrides.is_empty()
        && overrides.task_planning.enabled.is_none()
        && overrides.task_planning.progress_interval.is_none()
}

fn source_override_is_empty(source_override: &WebSearchSourceOverrides) -> bool {
    source_override.enabled.is_none()
        && source_override.deep_enabled.is_none()
        && source_override.extract_enabled.is_none()
        && source_override.priority.is_none()
        && source_override.roles.is_none()
        && !source_override.proxy_url_override
}

fn permission_mode_as_str(mode: &TerminalPermissionMode) -> &'static str {
    match mode {
        TerminalPermissionMode::RequestApproval => "request_approval",
        TerminalPermissionMode::FullAccess => "full_access",
    }
}

fn permission_mode_from_str(value: &str) -> AppResult<TerminalPermissionMode> {
    match value {
        "request_approval" => Ok(TerminalPermissionMode::RequestApproval),
        "full_access" => Ok(TerminalPermissionMode::FullAccess),
        _ => Err(AppError::Database(format!(
            "unknown terminal permission mode '{value}'"
        ))),
    }
}

fn key_selection_as_str(selection: &WebSearchKeySelection) -> &'static str {
    match selection {
        WebSearchKeySelection::Sequential => "sequential",
        WebSearchKeySelection::RoundRobin => "round_robin",
        WebSearchKeySelection::Random => "random",
    }
}

fn key_selection_from_str(value: &str) -> AppResult<WebSearchKeySelection> {
    match value {
        "sequential" => Ok(WebSearchKeySelection::Sequential),
        "round_robin" => Ok(WebSearchKeySelection::RoundRobin),
        "random" => Ok(WebSearchKeySelection::Random),
        _ => Err(AppError::Database(format!(
            "unknown web search key selection '{value}'"
        ))),
    }
}

fn source_provider_from_str(value: &str) -> AppResult<WebSearchProviderKind> {
    match value {
        "tavily" => Ok(WebSearchProviderKind::Tavily),
        "exa" => Ok(WebSearchProviderKind::Exa),
        "jina" => Ok(WebSearchProviderKind::Jina),
        "grok" => Ok(WebSearchProviderKind::Grok),
        "firecrawl" => Ok(WebSearchProviderKind::Firecrawl),
        "keenable" => Ok(WebSearchProviderKind::Keenable),
        _ => Err(AppError::Database(format!(
            "unknown web search provider '{value}'"
        ))),
    }
}

fn validate_tools_config(config: &AgentToolsConfig) -> AppResult<()> {
    const MAX_ALLOWED_ROOTS: usize = 8;
    if config.max_llm_iterations == 0
        || config.max_llm_iterations > crate::domain::tools::MAX_LLM_ITERATIONS
    {
        return Err(AppError::BadRequest(format!(
            "max_llm_iterations must be between 1 and {}",
            crate::domain::tools::MAX_LLM_ITERATIONS
        )));
    }
    if config.terminal.default_timeout_ms == 0
        || config.terminal.default_timeout_ms > crate::tools::terminal::MAX_TERMINAL_TIMEOUT_MS
    {
        return Err(AppError::BadRequest(format!(
            "terminal default_timeout_ms must be between 1 and {}",
            crate::tools::terminal::MAX_TERMINAL_TIMEOUT_MS
        )));
    }
    if config.terminal.allowed_cwd_roots.len() > MAX_ALLOWED_ROOTS {
        return Err(AppError::BadRequest(format!(
            "at most {MAX_ALLOWED_ROOTS} terminal workspace roots may be configured"
        )));
    }
    let home = crate::fsutil::home_dir().and_then(|path| std::fs::canonicalize(path).ok());
    let mut canonical_roots = HashSet::new();
    for root in &config.terminal.allowed_cwd_roots {
        let path = std::fs::canonicalize(root).map_err(|error| {
            AppError::BadRequest(format!(
                "terminal workspace root '{root}' is invalid: {error}"
            ))
        })?;
        if path.parent().is_none() {
            return Err(AppError::BadRequest(
                "filesystem roots cannot be used as terminal workspace roots".to_string(),
            ));
        }
        if home
            .as_ref()
            .is_some_and(|home| crate::fsutil::paths_equal(home, &path))
        {
            return Err(AppError::BadRequest(
                "the entire user home directory cannot be used as terminal workspace root"
                    .to_string(),
            ));
        }
        if !canonical_roots.insert(crate::fsutil::path_identity_key(&path)) {
            return Err(AppError::BadRequest(
                "terminal workspace roots must be unique".to_string(),
            ));
        }
    }
    if config.web_search.max_results == 0
        || config.web_search.max_results > crate::domain::tools::MAX_WEB_SEARCH_RESULTS
    {
        return Err(AppError::BadRequest(format!(
            "web_search max_results must be between 1 and {}",
            crate::domain::tools::MAX_WEB_SEARCH_RESULTS
        )));
    }
    if config.web_search.timeout_ms == 0
        || config.web_search.timeout_ms > crate::domain::tools::MAX_WEB_SEARCH_TIMEOUT_MS
    {
        return Err(AppError::BadRequest(format!(
            "web_search timeout_ms must be between 1 and {}",
            crate::domain::tools::MAX_WEB_SEARCH_TIMEOUT_MS
        )));
    }
    if config.web_search.extraction.max_remote_attempts_per_url == 0
        || config.web_search.extraction.max_remote_attempts_per_url
            > crate::domain::tools::MAX_WEB_EXTRACT_REMOTE_ATTEMPTS
    {
        return Err(AppError::BadRequest(format!(
            "web_extract max_remote_attempts_per_url must be between 1 and {}",
            crate::domain::tools::MAX_WEB_EXTRACT_REMOTE_ATTEMPTS
        )));
    }
    if config.evidence_storage.retention_days == 0
        || config.evidence_storage.retention_days
            > crate::domain::tools::MAX_EVIDENCE_RETENTION_DAYS
    {
        return Err(AppError::BadRequest(format!(
            "evidence_storage retention_days must be between 1 and {}",
            crate::domain::tools::MAX_EVIDENCE_RETENTION_DAYS
        )));
    }
    if config.evidence_storage.max_total_mb == 0
        || config.evidence_storage.max_total_mb > crate::domain::tools::MAX_EVIDENCE_TOTAL_MB
    {
        return Err(AppError::BadRequest(format!(
            "evidence_storage max_total_mb must be between 1 and {}",
            crate::domain::tools::MAX_EVIDENCE_TOTAL_MB
        )));
    }
    if config.web_search.sources.len() > crate::domain::tools::MAX_WEB_SEARCH_SOURCES {
        return Err(AppError::BadRequest(format!(
            "at most {} web_search sources may be configured",
            crate::domain::tools::MAX_WEB_SEARCH_SOURCES
        )));
    }
    let mut source_ids = HashSet::new();
    for source in &config.web_search.sources {
        let id = source.id.trim();
        if id.is_empty() || id.len() > 80 {
            return Err(AppError::BadRequest(
                "web_search source id must contain 1 to 80 bytes".to_string(),
            ));
        }
        if !source_ids.insert(id.to_string()) {
            return Err(AppError::BadRequest(format!(
                "duplicate web_search source id '{id}'"
            )));
        }
        if source.api_keys.len() > crate::domain::tools::MAX_WEB_SEARCH_KEYS_PER_SOURCE {
            return Err(AppError::BadRequest(format!(
                "web_search source '{id}' has too many API keys (max {})",
                crate::domain::tools::MAX_WEB_SEARCH_KEYS_PER_SOURCE
            )));
        }
        if source.enabled && !source.can_search() {
            return Err(AppError::BadRequest(format!(
                "web_search source '{id}' requires an available API key before it can be enabled"
            )));
        }
        let mut api_keys = HashSet::new();
        for api_key in &source.api_keys {
            let api_key = api_key.trim();
            if api_key.is_empty() || !api_keys.insert(api_key) {
                return Err(AppError::BadRequest(format!(
                    "web_search source '{id}' API keys must be non-empty and unique"
                )));
            }
        }
        let mut blocked_hashes = HashSet::new();
        for fingerprint in &source.blocked_api_key_hashes {
            if fingerprint.len() != 64
                || !fingerprint.bytes().all(|byte| byte.is_ascii_hexdigit())
                || !blocked_hashes.insert(fingerprint.to_ascii_lowercase())
            {
                return Err(AppError::BadRequest(format!(
                    "web_search source '{id}' blocked key fingerprints must be valid and unique"
                )));
            }
        }
        if source.display_name.chars().count() > 100
            || source.independence_group.chars().count() > 80
        {
            return Err(AppError::BadRequest(format!(
                "web_search source '{id}' display metadata is too long"
            )));
        }
        let mut roles = HashSet::new();
        if source.roles.iter().any(|role| !roles.insert(role)) {
            return Err(AppError::BadRequest(format!(
                "web_search source '{id}' roles must be unique"
            )));
        }
        validate_optional_proxy_url(id, "proxy_url", source.proxy_url.as_deref())?;
    }
    Ok(())
}

fn validate_optional_proxy_url(source_id: &str, field: &str, value: Option<&str>) -> AppResult<()> {
    if let Some(value) = value {
        let value = value.trim();
        if !value.is_empty()
            && !(value.starts_with("http://")
                || value.starts_with("https://")
                || value.starts_with("socks4://")
                || value.starts_with("socks4a://")
                || value.starts_with("socks5://")
                || value.starts_with("socks5h://")
                || value.starts_with("socks://"))
        {
            return Err(AppError::BadRequest(format!(
                "web_search source '{source_id}' {field} must start with http://, https://, socks4://, socks4a://, socks5:// or socks5h://"
            )));
        }
    }
    Ok(())
}

fn validate_tools_overrides(
    overrides: &AgentToolsOverrides,
    defaults: &AgentToolsConfig,
) -> AppResult<()> {
    const MAX_APPROVAL_GRANTS: usize = 256;
    if overrides.version != 1 {
        return Err(AppError::BadRequest(format!(
            "unsupported agent tool settings version {}",
            overrides.version
        )));
    }
    if overrides.terminal.approval_grants.len() > MAX_APPROVAL_GRANTS {
        return Err(AppError::BadRequest(format!(
            "at most {MAX_APPROVAL_GRANTS} terminal approval grants may be stored per agent"
        )));
    }
    let now = Utc::now();
    let mut unique_grants = HashSet::new();
    for grant in &overrides.terminal.approval_grants {
        if grant.command.trim().is_empty() || grant.command.len() > 8_000 {
            return Err(AppError::BadRequest(
                "terminal approval grant command must contain 1 to 8000 bytes".to_string(),
            ));
        }
        if grant.permission_mode != TerminalPermissionMode::RequestApproval {
            return Err(AppError::BadRequest(
                "terminal approval grants require request_approval mode".to_string(),
            ));
        }
        if grant
            .cwd
            .as_deref()
            .is_some_and(|cwd| cwd.trim().is_empty())
        {
            return Err(AppError::BadRequest(
                "terminal approval grant cwd must be null or non-empty".to_string(),
            ));
        }
        let Some(expires_at) = grant.expires_at else {
            return Err(AppError::BadRequest(
                "terminal approval grants require an expiry time".to_string(),
            ));
        };
        if expires_at <= now
            || grant
                .created_at
                .is_some_and(|created_at| created_at > expires_at)
        {
            return Err(AppError::BadRequest(
                "terminal approval grant timestamps are invalid".to_string(),
            ));
        }
        if !unique_grants.insert((grant.command.clone(), grant.cwd.clone())) {
            return Err(AppError::BadRequest(
                "terminal approval grants must be unique by command and cwd".to_string(),
            ));
        }
    }
    let default_source_ids = defaults
        .web_search
        .sources
        .iter()
        .map(|source| source.id.as_str())
        .collect::<HashSet<_>>();
    let mut override_source_ids = HashSet::new();
    for source_override in &overrides.web_search.source_overrides {
        let source_id = source_override.source_id.trim();
        if source_id.is_empty() || !default_source_ids.contains(source_id) {
            return Err(AppError::BadRequest(format!(
                "web_search source override references unknown source '{source_id}'"
            )));
        }
        if !override_source_ids.insert(source_id) {
            return Err(AppError::BadRequest(format!(
                "duplicate web_search source override '{source_id}'"
            )));
        }
        if let Some(roles) = &source_override.roles {
            let mut roles_seen = HashSet::new();
            if roles.iter().any(|role| !roles_seen.insert(role)) {
                return Err(AppError::BadRequest(format!(
                    "web_search source override '{source_id}' roles must be unique"
                )));
            }
        }
        if source_override.proxy_url_override {
            validate_optional_proxy_url(
                source_id,
                "proxy_url",
                source_override.proxy_url.as_deref(),
            )?;
        }
    }
    validate_tools_config(&overrides.apply_to(defaults))
}
