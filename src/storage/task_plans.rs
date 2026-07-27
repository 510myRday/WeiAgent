use chrono::{DateTime, Utc};
use rusqlite::{OptionalExtension, params};
use uuid::Uuid;

use crate::{
    domain::{TaskPlan, TaskPlanItem},
    error::AppResult,
};

impl super::Store {
    pub async fn read_task_plan(&self, conversation_id: Uuid) -> AppResult<Option<TaskPlan>> {
        self.database
            .call(move |conn| {
                let row = conn
                    .query_row(
                        "SELECT planning_id, conversation_message_id, explanation, plan_json,
                                created_at, updated_at, active
                         FROM task_plans
                         WHERE conversation_id = ?1 AND active = 1
                         ORDER BY updated_at DESC
                         LIMIT 1",
                        [conversation_id.to_string()],
                        |row| {
                            Ok((
                                row.get::<_, String>(0)?,
                                row.get::<_, Option<String>>(1)?,
                                row.get::<_, Option<String>>(2)?,
                                row.get::<_, String>(3)?,
                                row.get::<_, String>(4)?,
                                row.get::<_, String>(5)?,
                                row.get::<_, i64>(6)?,
                            ))
                        },
                    )
                    .optional()?;
                row.map(
                    |(
                        planning_id,
                        conversation_message_id,
                        explanation,
                        plan_json,
                        created_at,
                        updated_at,
                        active,
                    )| {
                        Ok(TaskPlan {
                            conversation_id,
                            planning_id: Uuid::parse_str(&planning_id).map_err(|error| {
                                rusqlite::Error::FromSqlConversionFailure(
                                    0,
                                    rusqlite::types::Type::Text,
                                    Box::new(error),
                                )
                            })?,
                            conversation_message_id: conversation_message_id
                                .map(|value| {
                                    Uuid::parse_str(&value).map_err(|error| {
                                        rusqlite::Error::FromSqlConversionFailure(
                                            1,
                                            rusqlite::types::Type::Text,
                                            Box::new(error),
                                        )
                                    })
                                })
                                .transpose()?,
                            explanation,
                            plan: serde_json::from_str(&plan_json).map_err(|error| {
                                rusqlite::Error::FromSqlConversionFailure(
                                    0,
                                    rusqlite::types::Type::Text,
                                    Box::new(error),
                                )
                            })?,
                            created_at: DateTime::parse_from_rfc3339(&created_at)
                                .map_err(|error| {
                                    rusqlite::Error::FromSqlConversionFailure(
                                        0,
                                        rusqlite::types::Type::Text,
                                        Box::new(error),
                                    )
                                })?
                                .with_timezone(&Utc),
                            updated_at: DateTime::parse_from_rfc3339(&updated_at)
                                .map_err(|error| {
                                    rusqlite::Error::FromSqlConversionFailure(
                                        0,
                                        rusqlite::types::Type::Text,
                                        Box::new(error),
                                    )
                                })?
                                .with_timezone(&Utc),
                            active: active != 0,
                        })
                    },
                )
                .transpose()
            })
            .await
    }

    pub async fn create_task_plan(
        &self,
        conversation_id: Uuid,
        explanation: Option<String>,
        plan: Vec<TaskPlanItem>,
    ) -> AppResult<TaskPlan> {
        let planning_id = Uuid::new_v4();
        let now = Utc::now();
        let plan_json = serde_json::to_string(&plan)?;
        let explanation_for_db = explanation.clone();
        self.database
            .call(move |conn| {
                let tx = conn.unchecked_transaction()?;
                tx.execute(
                    "UPDATE task_plans SET active = 0
                     WHERE conversation_id = ?1 AND active = 1",
                    [conversation_id.to_string()],
                )?;
                tx.execute(
                    "INSERT INTO task_plans (
                        planning_id, conversation_id, conversation_message_id,
                        explanation, plan_json, created_at, updated_at, active
                     ) VALUES (?1, ?2, NULL, ?3, ?4, ?5, ?5, 1)",
                    params![
                        planning_id.to_string(),
                        conversation_id.to_string(),
                        explanation_for_db,
                        plan_json,
                        now.to_rfc3339(),
                    ],
                )?;
                tx.commit()?;
                Ok(TaskPlan {
                    conversation_id,
                    planning_id,
                    conversation_message_id: None,
                    explanation,
                    plan,
                    created_at: now,
                    updated_at: now,
                    active: true,
                })
            })
            .await
    }

    pub async fn update_task_plan(
        &self,
        conversation_id: Uuid,
        planning_id: Uuid,
        explanation: Option<String>,
        plan: Vec<TaskPlanItem>,
    ) -> AppResult<Option<TaskPlan>> {
        let now = Utc::now();
        let plan_json = serde_json::to_string(&plan)?;
        let explanation_for_db = explanation.clone();
        self.database
            .call(move |conn| {
                let changed = conn.execute(
                    "UPDATE task_plans
                     SET explanation = ?1, plan_json = ?2, updated_at = ?3
                     WHERE conversation_id = ?4 AND planning_id = ?5 AND active = 1",
                    params![
                        explanation_for_db,
                        plan_json,
                        now.to_rfc3339(),
                        conversation_id.to_string(),
                        planning_id.to_string(),
                    ],
                )?;
                if changed == 0 {
                    return Ok(None);
                }
                let (created_at, conversation_message_id) = conn.query_row(
                    "SELECT created_at, conversation_message_id FROM task_plans
                     WHERE conversation_id = ?1 AND planning_id = ?2",
                    params![conversation_id.to_string(), planning_id.to_string()],
                    |row| Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?)),
                )?;
                let created_at = DateTime::parse_from_rfc3339(&created_at)
                    .map_err(|error| rusqlite::Error::ToSqlConversionFailure(Box::new(error)))?
                    .with_timezone(&Utc);
                Ok(Some(TaskPlan {
                    conversation_id,
                    planning_id,
                    conversation_message_id: conversation_message_id
                        .map(|value| {
                            Uuid::parse_str(&value).map_err(|error| {
                                rusqlite::Error::FromSqlConversionFailure(
                                    1,
                                    rusqlite::types::Type::Text,
                                    Box::new(error),
                                )
                            })
                        })
                        .transpose()?,
                    explanation,
                    plan,
                    created_at,
                    updated_at: now,
                    active: true,
                }))
            })
            .await
    }

    pub async fn deactivate_task_plan(
        &self,
        conversation_id: Uuid,
        planning_id: Option<Uuid>,
    ) -> AppResult<bool> {
        self.database
            .call(move |conn| {
                let changed = match planning_id {
                    Some(planning_id) => conn.execute(
                        "UPDATE task_plans SET active = 0
                         WHERE conversation_id = ?1 AND planning_id = ?2 AND active = 1",
                        params![conversation_id.to_string(), planning_id.to_string()],
                    )?,
                    None => conn.execute(
                        "UPDATE task_plans SET active = 0
                         WHERE conversation_id = ?1 AND active = 1",
                        [conversation_id.to_string()],
                    )?,
                };
                Ok(changed > 0)
            })
            .await
    }

    pub async fn associate_task_plan_with_message(
        &self,
        conversation_id: Uuid,
        planning_id: Uuid,
        message_id: Uuid,
    ) -> AppResult<bool> {
        self.database
            .call(move |conn| {
                let changed = conn.execute(
                    "UPDATE task_plans SET conversation_message_id = ?1
                     WHERE conversation_id = ?2 AND planning_id = ?3
                       AND EXISTS (
                           SELECT 1 FROM conversation_messages
                           WHERE id = ?1 AND conversation_id = ?2
                       )",
                    params![
                        message_id.to_string(),
                        conversation_id.to_string(),
                        planning_id.to_string()
                    ],
                )?;
                Ok(changed > 0)
            })
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::TaskPlanItemStatus;

    async fn insert_conversation_and_message(
        store: &crate::storage::Store,
        conversation_id: Uuid,
        message_id: Uuid,
    ) {
        let agent_id = Uuid::new_v4();
        let now = Utc::now().to_rfc3339();
        store
            .database()
            .call(move |conn| {
                conn.execute(
                    "INSERT INTO conversations (
                        id, agent_id, title, message_count, is_pinned,
                        context_checkpoint_json, created_at, updated_at
                     ) VALUES (?1, ?2, 'Test', 1, 0, NULL, ?3, ?3)",
                    params![conversation_id.to_string(), agent_id.to_string(), now],
                )?;
                conn.execute(
                    "INSERT INTO conversation_messages (
                        seq, id, conversation_id, agent_id, role, content,
                        status, created_at
                     ) VALUES (1, ?1, ?2, ?3, 'tool', '{}', 'done', ?4)",
                    params![
                        message_id.to_string(),
                        conversation_id.to_string(),
                        agent_id.to_string(),
                        Utc::now().to_rfc3339(),
                    ],
                )?;
                Ok(())
            })
            .await
            .unwrap();
    }

    async fn count_plans(store: &crate::storage::Store, planning_id: Uuid) -> i64 {
        store
            .database()
            .call(move |conn| {
                Ok(conn.query_row(
                    "SELECT COUNT(*) FROM task_plans WHERE planning_id = ?1",
                    [planning_id.to_string()],
                    |row| row.get(0),
                )?)
            })
            .await
            .unwrap()
    }

    fn item(step: &str, status: TaskPlanItemStatus) -> TaskPlanItem {
        TaskPlanItem {
            step: step.to_string(),
            status,
        }
    }

    #[tokio::test]
    async fn task_plans_persist_and_stay_isolated_by_conversation() {
        let temp_dir = tempfile::tempdir().unwrap();
        let store = crate::storage::Store::new(temp_dir.path()).await.unwrap();
        let first_conversation = Uuid::new_v4();
        let second_conversation = Uuid::new_v4();
        let created = store
            .create_task_plan(
                first_conversation,
                Some("Ship the feature".to_string()),
                vec![item("Implement", TaskPlanItemStatus::InProgress)],
            )
            .await
            .unwrap();

        assert_eq!(
            store.read_task_plan(second_conversation).await.unwrap(),
            None
        );
        drop(store);

        let reopened = crate::storage::Store::new(temp_dir.path()).await.unwrap();
        let loaded = reopened
            .read_task_plan(first_conversation)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(loaded.planning_id, created.planning_id);
        assert_eq!(loaded.explanation.as_deref(), Some("Ship the feature"));
        assert_eq!(loaded.plan[0].step, "Implement");

        let updated = reopened
            .update_task_plan(
                first_conversation,
                created.planning_id,
                None,
                vec![item("Finish", TaskPlanItemStatus::Completed)],
            )
            .await
            .unwrap()
            .unwrap();
        assert_eq!(updated.plan[0].status, TaskPlanItemStatus::Completed);
        assert!(
            reopened
                .deactivate_task_plan(first_conversation, Some(created.planning_id))
                .await
                .unwrap()
        );
        assert_eq!(
            reopened.read_task_plan(first_conversation).await.unwrap(),
            None
        );

        let retained = reopened
            .database()
            .call(move |conn| {
                Ok(conn.query_row(
                    "SELECT active FROM task_plans WHERE planning_id = ?1",
                    [created.planning_id.to_string()],
                    |row| row.get::<_, i64>(0),
                )?)
            })
            .await
            .unwrap();
        assert_eq!(retained, 0);
    }

    #[tokio::test]
    async fn task_plan_history_is_retained_and_cascades_from_its_message() {
        let temp_dir = tempfile::tempdir().unwrap();
        let store = crate::storage::Store::new(temp_dir.path()).await.unwrap();
        let conversation_id = Uuid::new_v4();
        let first_message_id = Uuid::new_v4();
        let unrelated_message_id = Uuid::new_v4();
        insert_conversation_and_message(&store, conversation_id, first_message_id).await;

        let first = store
            .create_task_plan(
                conversation_id,
                Some("First plan".to_string()),
                vec![item("First", TaskPlanItemStatus::Completed)],
            )
            .await
            .unwrap();
        assert!(
            store
                .associate_task_plan_with_message(
                    conversation_id,
                    first.planning_id,
                    first_message_id
                )
                .await
                .unwrap()
        );
        assert!(
            store
                .deactivate_task_plan(conversation_id, Some(first.planning_id))
                .await
                .unwrap()
        );

        let second = store
            .create_task_plan(
                conversation_id,
                Some("Second plan".to_string()),
                vec![item("Second", TaskPlanItemStatus::InProgress)],
            )
            .await
            .unwrap();
        assert_eq!(count_plans(&store, first.planning_id).await, 1);
        assert_eq!(count_plans(&store, second.planning_id).await, 1);

        store
            .database()
            .call(move |conn| {
                conn.execute(
                    "INSERT INTO conversation_messages (
                        seq, id, conversation_id, agent_id, role, content,
                        status, created_at
                     ) SELECT 2, ?1, conversation_id, agent_id, 'tool', '{}',
                              'done', created_at
                       FROM conversation_messages WHERE id = ?2",
                    params![
                        unrelated_message_id.to_string(),
                        first_message_id.to_string()
                    ],
                )?;
                Ok(())
            })
            .await
            .unwrap();
        store
            .database()
            .call(move |conn| {
                conn.execute(
                    "DELETE FROM conversation_messages WHERE id = ?1",
                    [unrelated_message_id.to_string()],
                )?;
                Ok(())
            })
            .await
            .unwrap();
        assert_eq!(count_plans(&store, first.planning_id).await, 1);

        store
            .database()
            .call(move |conn| {
                conn.execute(
                    "DELETE FROM conversation_messages WHERE id = ?1",
                    [first_message_id.to_string()],
                )?;
                Ok(())
            })
            .await
            .unwrap();
        assert_eq!(count_plans(&store, first.planning_id).await, 0);
        assert_eq!(count_plans(&store, second.planning_id).await, 1);
    }
}
