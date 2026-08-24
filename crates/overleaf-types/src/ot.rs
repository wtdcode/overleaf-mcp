use serde::{Deserialize, Serialize};

/// One sharejs `text` op component. Positions (`p`) count UTF-16 code units of
/// the document content, matching the JavaScript string semantics used by the
/// Overleaf document-updater. Comment components (`c`/`t`) can arrive in remote
/// updates and never change the text, so they are carried but ignored.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OtComponent {
    pub p: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub i: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub d: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub c: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub t: Option<String>,
}

impl OtComponent {
    pub fn insert(p: u64, text: String) -> Self {
        OtComponent {
            p,
            i: Some(text),
            d: None,
            c: None,
            t: None,
        }
    }

    pub fn delete(p: u64, text: String) -> Self {
        OtComponent {
            p,
            i: None,
            d: Some(text),
            c: None,
            t: None,
        }
    }

    pub fn utf16_len(text: &str) -> u64 {
        text.chars().map(|c| c.len_utf16() as u64).sum()
    }

    /// UTF-16 length of `text[..byte]`; `byte` must lie on a char boundary.
    pub fn utf16_of_byte(text: &str, byte: usize) -> u64 {
        Self::utf16_len(&text[..byte])
    }

    /// Byte offset for a UTF-16 offset; `None` when it is out of range or falls
    /// inside a surrogate pair.
    pub fn byte_of_utf16(text: &str, units: u64) -> Option<usize> {
        let mut seen: u64 = 0;
        if units == 0 {
            return Some(0);
        }
        for (byte, ch) in text.char_indices() {
            if seen == units {
                return Some(byte);
            }
            if seen > units {
                return None;
            }
            seen += ch.len_utf16() as u64;
        }
        (seen == units).then_some(text.len())
    }

    /// Builds a sequentially-applied op replacing the given byte ranges of
    /// `content` (ascending, non-overlapping) with the paired texts. Positions
    /// are adjusted by the running length delta because sharejs applies the
    /// components one after another, each seeing the previous result.
    pub fn replace_script(content: &str, edits: &[(usize, usize, String)]) -> Vec<OtComponent> {
        let mut ops = Vec::new();
        let mut delta: i64 = 0;
        for (start, end, replacement) in edits {
            let old = &content[*start..*end];
            let base = Self::utf16_of_byte(content, *start) as i64 + delta;
            let p = base.max(0) as u64;
            if !old.is_empty() {
                ops.push(OtComponent::delete(p, old.to_string()));
            }
            if !replacement.is_empty() {
                ops.push(OtComponent::insert(p, replacement.clone()));
            }
            delta += Self::utf16_len(replacement) as i64 - Self::utf16_len(old) as i64;
        }
        ops
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct OtUpdate {
    pub doc: String,
    pub op: Vec<OtComponent>,
    pub v: i64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct UpdateMeta {
    #[serde(default)]
    pub source: Option<String>,
    #[serde(default)]
    pub user_id: Option<String>,
}

/// `otUpdateApplied` payload. Our own ops come back as a short confirmation
/// with `op` absent; other clients' ops arrive with the full op body.
#[derive(Debug, Clone, Deserialize)]
pub struct AppliedOtUpdate {
    pub doc: String,
    pub v: i64,
    #[serde(default)]
    pub op: Option<Vec<OtComponent>>,
    #[serde(default)]
    pub meta: Option<UpdateMeta>,
}
