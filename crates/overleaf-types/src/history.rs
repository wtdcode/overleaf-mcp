use schemars::JsonSchema;
use serde::Deserialize;

/// A project history version as given by a tool caller: either the version
/// number itself, or the comment of a label attached to it.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum HistoryVersionRef {
    Number(i64),
    Label(String),
}

/// Response of `GET /project/:id/diff?pathname=&from=&to=`. With `from ==
/// to` the segments are all unchanged text, i.e. the whole file content at
/// that version; binary files yield a marker instead.
#[derive(Debug, Clone, Deserialize)]
pub struct HistoryDiffResponse {
    pub diff: HistoryDiffBody,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum HistoryDiffBody {
    Text(Vec<HistoryDiffSegment>),
    Binary(HistoryBinaryMarker),
}

impl HistoryDiffBody {
    /// Text as of the `to` version (unchanged plus inserted segments), or
    /// `None` for binary files.
    pub fn text_at_to(&self) -> Option<String> {
        match self {
            HistoryDiffBody::Text(segments) => Some(
                segments
                    .iter()
                    .flat_map(|s| [s.u.as_deref(), s.i.as_deref()])
                    .flatten()
                    .collect(),
            ),
            HistoryDiffBody::Binary(_) => None,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct HistoryDiffSegment {
    #[serde(default)]
    pub u: Option<String>,
    #[serde(default)]
    pub i: Option<String>,
    #[serde(default)]
    pub d: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct HistoryBinaryMarker {
    pub binary: bool,
}

/// Response of `GET /project/:id/filetree/diff?from=&to=`: the files present
/// at that version.
#[derive(Debug, Clone, Deserialize)]
pub struct HistoryFiletreeResponse {
    pub diff: Vec<HistoryFiletreeEntry>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct HistoryFiletreeEntry {
    pub pathname: String,
    #[serde(default)]
    pub editable: bool,
}

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
