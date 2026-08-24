use serde::Deserialize;

/// Response of `GET /project/:id/updates` (summarized project history).
#[derive(Debug, Clone, Deserialize)]
pub struct UpdatesResponse {
    #[serde(default)]
    pub updates: Vec<HistoryUpdate>,
    #[serde(rename = "nextBeforeTimestamp", default)]
    pub next_before_timestamp: Option<i64>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct HistoryUpdate {
    #[serde(rename = "fromV")]
    pub from_v: i64,
    #[serde(rename = "toV")]
    pub to_v: i64,
    #[serde(default)]
    pub meta: Option<HistoryUpdateMeta>,
    #[serde(default)]
    pub labels: Vec<HistoryLabel>,
    #[serde(default)]
    pub pathnames: Vec<String>,
    /// Structural operations (add/remove/rename) as loosely-typed values; the
    /// exact shape varies by operation.
    #[serde(default)]
    pub project_ops: Vec<serde_json::Value>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct HistoryUpdateMeta {
    #[serde(default)]
    pub users: Vec<HistoryUser>,
    #[serde(default)]
    pub start_ts: Option<i64>,
    #[serde(default)]
    pub end_ts: Option<i64>,
}

/// History users are usually objects, but anonymous edits can appear as null.
#[derive(Debug, Clone, Deserialize)]
pub struct HistoryUser {
    #[serde(default)]
    pub first_name: Option<String>,
    #[serde(default)]
    pub last_name: Option<String>,
    #[serde(default)]
    pub email: Option<String>,
}

impl HistoryUser {
    pub fn display(&self) -> String {
        let name = [self.first_name.as_deref(), self.last_name.as_deref()]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>()
            .join(" ");
        match (name.is_empty(), self.email.as_deref()) {
            (false, _) => name,
            (true, Some(email)) => email.to_string(),
            (true, None) => "unknown".to_string(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct HistoryLabel {
    pub id: String,
    pub comment: String,
    pub version: i64,
    #[serde(default)]
    pub created_at: Option<String>,
    #[serde(default)]
    pub user_id: Option<String>,
}
