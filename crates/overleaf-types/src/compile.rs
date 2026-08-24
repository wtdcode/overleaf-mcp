use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize)]
pub struct CompileRequest {
    #[serde(rename = "rootDoc_id")]
    pub root_doc_id: Option<String>,
    pub draft: bool,
    pub check: String,
    #[serde(rename = "incrementalCompilesEnabled")]
    pub incremental_compiles_enabled: bool,
    #[serde(rename = "stopOnFirstError")]
    pub stop_on_first_error: bool,
}

impl CompileRequest {
    pub fn full(root_doc_id: Option<String>) -> Self {
        CompileRequest {
            root_doc_id,
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
