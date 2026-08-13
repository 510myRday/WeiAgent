use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize, ser::SerializeStruct};
use sha2::{Digest, Sha256};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TaskPlanItemStatus {
    Pending,
    InProgress,
    Completed,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TaskPlanItem {
    pub step: String,
    pub status: TaskPlanItemStatus,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskPlan {
    pub conversation_id: Uuid,
    pub planning_id: Uuid,
    pub public_id: String,
    pub conversation_message_id: Option<Uuid>,
    pub explanation: Option<String>,
    pub plan: Vec<TaskPlanItem>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub active: bool,
}

impl TaskPlan {
    /// Stable six-character lowercase alphanumeric identifier shown to the model and UI.
    pub fn public_id(&self) -> &str {
        &self.public_id
    }
}

impl Serialize for TaskPlan {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        let mut state = serializer.serialize_struct("TaskPlan", 8)?;
        state.serialize_field("conversation_id", &self.conversation_id)?;
        state.serialize_field("planning_id", self.public_id())?;
        state.serialize_field("conversation_message_id", &self.conversation_message_id)?;
        state.serialize_field("explanation", &self.explanation)?;
        state.serialize_field("plan", &self.plan)?;
        state.serialize_field("created_at", &self.created_at)?;
        state.serialize_field("updated_at", &self.updated_at)?;
        state.serialize_field("active", &self.active)?;
        state.end()
    }
}

pub fn task_plan_public_id_candidate(
    planning_id: Uuid,
    explanation: Option<&str>,
    plan_json: &str,
    created_at: &str,
    nonce: u32,
) -> String {
    let material = format!(
        "weiagent-task-plan-v1\nplanning_id={planning_id}\ncreated_at={created_at}\nnonce={nonce}\nexplanation={}\nplan_json={plan_json}",
        explanation.unwrap_or_default(),
    );
    let digest = Sha256::digest(material.as_bytes());
    const ALPHABET: &[u8; 36] = b"0123456789abcdefghijklmnopqrstuvwxyz";
    const SPACE: u32 = 36_u32.pow(6);
    let mut value = u32::from_be_bytes([digest[0], digest[1], digest[2], digest[3]]) % SPACE;
    let mut output = [b'0'; 6];
    for index in (0..output.len()).rev() {
        output[index] = ALPHABET[(value % 36) as usize];
        value /= 36;
    }
    String::from_utf8(output.to_vec()).expect("task plan alphabet is valid UTF-8")
}
