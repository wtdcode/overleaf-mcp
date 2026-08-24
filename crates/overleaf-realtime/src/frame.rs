use overleaf_types::{OverleafError, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// socket.io 0.9 message separator used when several frames share one
/// xhr-polling response body.
const BATCH_SEP: char = '\u{fffd}';

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventPayload {
    pub name: String,
    #[serde(default)]
    pub args: Vec<Value>,
}

/// One socket.io 0.9 frame. Wire format is `type:id:endpoint:data`; we never
/// use endpoints, so that slot stays empty on both directions.
#[derive(Debug, Clone)]
pub enum Frame {
    Disconnect,
    Connect,
    Heartbeat,
    Message(String),
    JsonMsg(Value),
    Event(EventPayload),
    Ack { id: u64, args: Vec<Value> },
    ProtoError { reason: String },
    Noop,
}

impl Frame {
    /// Splits a poll response body into individual frame strings. Batched
    /// bodies look like `\u{fffd}LEN\u{fffd}FRAME...` with LEN counted in
    /// characters, not bytes.
    pub fn split_batch(raw: &str) -> Vec<String> {
        if !raw.starts_with(BATCH_SEP) {
            if raw.is_empty() {
                return Vec::new();
            }
            return vec![raw.to_string()];
        }
        let mut frames = Vec::new();
        let chars: Vec<char> = raw.chars().collect();
        let mut idx = 0;
        while idx < chars.len() {
            if chars[idx] != BATCH_SEP {
                tracing::warn!("malformed frame batch near char {idx}");
                break;
            }
            idx += 1;
            let mut len: usize = 0;
            let mut have_digits = false;
            while idx < chars.len() && chars[idx].is_ascii_digit() {
                len = len * 10 + (chars[idx] as usize - '0' as usize);
                have_digits = true;
                idx += 1;
            }
            if !have_digits || idx >= chars.len() || chars[idx] != BATCH_SEP {
                tracing::warn!("malformed frame batch length near char {idx}");
                break;
            }
            idx += 1;
            let end = (idx + len).min(chars.len());
            frames.push(chars[idx..end].iter().collect());
            idx = end;
        }
        frames
    }

    pub fn parse(raw: &str) -> Result<Frame> {
        let mut parts = raw.splitn(4, ':');
        let kind = parts.next().unwrap_or("");
        let id_part = parts.next().unwrap_or("");
        let _endpoint = parts.next().unwrap_or("");
        let data = parts.next().unwrap_or("");
        match kind {
            "0" => Ok(Frame::Disconnect),
            "1" => Ok(Frame::Connect),
            "2" => Ok(Frame::Heartbeat),
            "3" => Ok(Frame::Message(data.to_string())),
            "4" => Ok(Frame::JsonMsg(serde_json::from_str(data)?)),
            "5" => {
                let _ = id_part; // server events carry no ack id we need to honor
                Ok(Frame::Event(serde_json::from_str(data)?))
            }
            "6" => {
                // Ack data: `<id>` or `<id>+<json array>`.
                let (id_str, args) = match data.split_once('+') {
                    Some((id_str, json)) => (id_str, serde_json::from_str(json)?),
                    None => (data, Vec::new()),
                };
                let id = id_str.parse::<u64>().map_err(|_| {
                    OverleafError::Protocol(format!("bad ack id in frame: {raw}"))
                })?;
                Ok(Frame::Ack { id, args })
            }
            "7" => Ok(Frame::ProtoError {
                reason: data.to_string(),
            }),
            "8" => Ok(Frame::Noop),
            other => Err(OverleafError::Protocol(format!(
                "unknown frame type {other}: {raw}"
            ))),
        }
    }

    /// Encodes an event frame; `ack_id` requests a data ack (`5:<id>+::…`).
    pub fn encode_event(payload: &EventPayload, ack_id: Option<u64>) -> Result<String> {
        let json = serde_json::to_string(payload)?;
        Ok(match ack_id {
            Some(id) => format!("5:{id}+::{json}"),
            None => format!("5:::{json}"),
        })
    }
}
