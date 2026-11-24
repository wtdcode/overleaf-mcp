use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize)]
pub struct CompileRequest {
    #[serde(rename = "rootDoc_id")]
    pub root_doc_id: Option<String>,
    /// The server schema accepts this field as absent but not as null.
    #[serde(rename = "rootResourcePath", skip_serializing_if = "Option::is_none")]
    pub root_resource_path: Option<String>,
    pub draft: bool,
    pub check: String,
    #[serde(rename = "incrementalCompilesEnabled")]
    pub incremental_compiles_enabled: bool,
    #[serde(rename = "stopOnFirstError")]
    pub stop_on_first_error: bool,
}

/// Compiles a specific doc as the main file for one compile, leaving the
/// project's main document setting untouched. Some Overleaf versions honor
/// the override only when the doc id and its path arrive together, so the two
/// are always sent as a pair.
#[derive(Debug, Clone)]
pub struct MainFileOverride {
    pub doc_id: String,
    /// Project path without the leading slash, e.g. `thesis/main.tex`.
    pub path: String,
}

/// Whether a compile log shows that a main file override took effect.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MainFileCheck {
    Confirmed,
    /// The log name fits both the override and the project's main document.
    Ambiguous,
    Mismatch,
}

impl MainFileOverride {
    /// Judges the input name TeX logged (its `**<name>` line) against this
    /// override. Overleaf runs TeX from the main file's directory, so a main
    /// file in a subfolder is logged by its bare file name; when the
    /// project's own main document would be logged under the same name, the
    /// log cannot tell which of the two was compiled.
    pub fn check_log_root(&self, logged: &str, default_main: Option<&str>) -> MainFileCheck {
        let fits = |path: &str| {
            logged == path || (!logged.contains('/') && path.rsplit('/').next() == Some(logged))
        };
        let fits_default = default_main.is_some_and(|d| d != self.path && fits(d));
        match (fits(&self.path), fits_default) {
            (true, false) => MainFileCheck::Confirmed,
            (true, true) => MainFileCheck::Ambiguous,
            (false, _) => MainFileCheck::Mismatch,
        }
    }
}

impl CompileRequest {
    pub fn full(main_file: Option<MainFileOverride>) -> Self {
        let (root_doc_id, root_resource_path) = match main_file {
            Some(main) => (Some(main.doc_id), Some(main.path)),
            None => (None, None),
        };
        CompileRequest {
            root_doc_id,
            root_resource_path,
            draft: false,
            check: "silent".to_string(),
            incremental_compiles_enabled: false,
            stop_on_first_error: false,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct CompileResponse {
    pub status: String,
    #[serde(rename = "outputFiles", default)]
    pub output_files: Vec<OutputFile>,
    #[serde(rename = "clsiServerId", default)]
    pub clsi_server_id: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct OutputFile {
    pub path: String,
    pub url: String,
    #[serde(rename = "type", default)]
    pub kind: Option<String>,
    #[serde(default)]
    pub build: Option<String>,
}
