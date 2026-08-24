use std::time::Duration;

use overleaf_types::{
    CompileRequest, CompileResponse, EntityKind, EntityRefJson, HistoryLabel, OverleafConfig,
    OverleafCredentials, OverleafError, ProjectList, Result, UpdatesResponse, UploadResponse,
};
use reqwest::header::{ACCEPT, CONTENT_TYPE};
use reqwest::{Method, Response};
use serde::Serialize;
use tokio::sync::RwLock;

#[derive(Serialize)]
struct LoginBody {
    #[serde(rename = "_csrf")]
    csrf: String,
    email: String,
    password: String,
}

#[derive(Serialize)]
struct CreateEntityBody {
    name: String,
    parent_folder_id: String,
}

#[derive(Serialize)]
struct RenameBody {
    name: String,
}

#[derive(Serialize)]
struct MoveBody {
    folder_id: String,
}

#[derive(Serialize)]
struct CreateLabelBody {
    comment: String,
    version: i64,
}

/// Session-authenticated HTTP client for an Overleaf Community Edition
/// instance. Handles cookie-session login, CSRF tokens, the JSON/web routes,
/// and the raw socket.io xhr-polling transport used by the realtime layer.
pub struct OverleafClient {
    cfg: OverleafConfig,
    base: String,
    http: reqwest::Client,
    csrf: RwLock<Option<String>>,
    csrf_re: regex::Regex,
}

impl OverleafClient {
    pub fn new(cfg: OverleafConfig) -> Result<Self> {
        let mut builder = reqwest::Client::builder()
            .user_agent(concat!("overleaf-mcp/", env!("CARGO_PKG_VERSION")));
        match &cfg.credentials {
            OverleafCredentials::SessionCookie(raw) => {
                let jar = std::sync::Arc::new(reqwest::cookie::Jar::default());
                let url: reqwest::Url = cfg.base().parse().map_err(|e| {
                    OverleafError::Protocol(format!("bad endpoint url: {e}"))
                })?;
                for pair in raw.split(';').map(str::trim).filter(|p| !p.is_empty()) {
                    jar.add_cookie_str(&format!("{pair}; Path=/; Secure"), &url);
                }
                builder = builder.cookie_provider(jar);
            }
            OverleafCredentials::Password { .. } => {
                builder = builder.cookie_store(true);
            }
        }
        let http = builder.build()?;
        let csrf_re = regex::Regex::new(r#"name="ol-csrfToken" content="([^"]+)""#)
            .map_err(|e| OverleafError::Protocol(format!("csrf regex: {e}")))?;
        Ok(OverleafClient {
            base: cfg.base(),
            cfg,
            http,
            csrf: RwLock::new(None),
            csrf_re,
        })
    }

    pub fn endpoint(&self) -> &str {
        &self.base
    }

    fn now_ms() -> u128 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or(0)
    }

    async fn ok_status(resp: Response) -> Result<Response> {
        let status = resp.status();
        if status.is_success() {
            return Ok(resp);
        }
        let url = resp.url().to_string();
        let body: String = resp.text().await.unwrap_or_default().chars().take(500).collect();
        Err(OverleafError::Status {
            status: status.as_u16(),
            url,
            body,
        })
    }

    /// A 401/403 or a ride back to the login page means the session (or the
    /// CSRF token) went stale; one re-login retry is enough for both.
    fn needs_reauth(resp: &Response) -> bool {
        resp.status() == 401 || resp.status() == 403 || resp.url().path() == "/login"
    }

    async fn page_csrf(&self, path: &str) -> Result<String> {
        let resp = self
            .http
            .get(format!("{}{}", self.base, path))
            .send()
            .await?;
        let resp = Self::ok_status(resp).await?;
        let text = resp.text().await?;
        self.csrf_re
            .captures(&text)
            .and_then(|c| c.get(1))
            .map(|m| m.as_str().to_string())
            .ok_or_else(|| OverleafError::Auth(format!("no CSRF token found on {path}")))
    }

    pub async fn login(&self) -> Result<()> {
        let (account, password) = match &self.cfg.credentials {
            OverleafCredentials::SessionCookie(_) => {
                // The cookies came from a browser session; there is nothing to
                // log in to, only to validate. A CAPTCHA-gated server
                // (overleaf.com) cannot be re-authenticated headlessly once
                // the session dies.
                let resp = self
                    .http
                    .get(format!("{}/project", self.base))
                    .send()
                    .await?;
                if resp.url().path() == "/login" {
                    return Err(OverleafError::Auth(
                        "session cookie rejected or expired — refresh OVERLEAF_COOKIE from a logged-in browser".to_string(),
                    ));
                }
                let resp = Self::ok_status(resp).await?;
                let text = resp.text().await?;
                let token = self
                    .csrf_re
                    .captures(&text)
                    .and_then(|c| c.get(1))
                    .map(|m| m.as_str().to_string())
                    .ok_or_else(|| {
                        OverleafError::Auth("no CSRF token on /project page".to_string())
                    })?;
                *self.csrf.write().await = Some(token);
                tracing::info!("authenticated to {} via session cookie", self.base);
                return Ok(());
            }
            OverleafCredentials::Password { account, password } => {
                (account.clone(), password.clone())
            }
        };
        let token = self.page_csrf("/login").await?;
        let body = LoginBody {
            csrf: token,
            email: account.clone(),
            password,
        };
        let resp = self
            .http
            .post(format!("{}/login", self.base))
            .header(ACCEPT, "application/json")
            .json(&body)
            .send()
            .await?;
        let status = resp.status();
        let is_json = resp
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .is_some_and(|v| v.contains("json"));
        let text = resp.text().await.unwrap_or_default();
        if !status.is_success() {
            let brief: String = text.chars().take(300).collect();
            return Err(OverleafError::Auth(format!("login failed ({status}): {brief}")));
        }
        if is_json {
            let value: serde_json::Value = serde_json::from_str(&text)?;
            if value.get("redir").is_none() {
                return Err(OverleafError::Auth(format!("login rejected: {value}")));
            }
        }
        // The session rotates on login, and the CSRF token is tied to it.
        let fresh = self.page_csrf("/project").await?;
        *self.csrf.write().await = Some(fresh);
        tracing::info!(account = %account, "logged in to {}", self.base);
        Ok(())
    }

    async fn csrf_token(&self) -> Result<String> {
        if let Some(token) = self.csrf.read().await.clone() {
            return Ok(token);
        }
        let fresh = self.page_csrf("/project").await?;
        *self.csrf.write().await = Some(fresh.clone());
        Ok(fresh)
    }

    async fn get_authed(&self, path_and_query: &str) -> Result<Response> {
        for attempt in 0..2 {
            let resp = self
                .http
                .get(format!("{}{}", self.base, path_and_query))
                .header(ACCEPT, "application/json")
                .send()
                .await?;
            if attempt == 0 && Self::needs_reauth(&resp) {
                tracing::debug!("session stale on GET {path_and_query}, re-login");
                self.login().await?;
                continue;
            }
            return Self::ok_status(resp).await;
        }
        Err(OverleafError::Auth("re-login did not restore access".to_string()))
    }

    async fn send_mutating(
        &self,
        method: Method,
        path_and_query: &str,
        json: Option<serde_json::Value>,
    ) -> Result<Response> {
        for attempt in 0..2 {
            let token = self.csrf_token().await?;
            let mut rb = self
                .http
                .request(method.clone(), format!("{}{}", self.base, path_and_query))
                .header("x-csrf-token", token)
                .header(ACCEPT, "application/json");
            if let Some(value) = &json {
                rb = rb.json(value);
            }
            let resp = rb.send().await?;
            if attempt == 0 && Self::needs_reauth(&resp) {
                tracing::debug!("session stale on {method} {path_and_query}, re-login");
                self.login().await?;
                continue;
            }
            return Self::ok_status(resp).await;
        }
        Err(OverleafError::Auth("re-login did not restore access".to_string()))
    }

    pub async fn list_projects(&self) -> Result<ProjectList> {
        Ok(self.get_authed("/user/projects").await?.json().await?)
    }

    // --- realtime (socket.io 0.9) transport -------------------------------

    pub async fn rt_handshake(&self, project_id: &str) -> Result<String> {
        let path = format!(
            "/socket.io/1/?projectId={}&t={}",
            project_id,
            Self::now_ms()
        );
        let text = self.get_authed(&path).await?.text().await?;
        let sid = text.split(':').next().unwrap_or("");
        if sid.is_empty() {
            return Err(OverleafError::Protocol(format!(
                "bad socket.io handshake response: {text}"
            )));
        }
        Ok(sid.to_string())
    }

    pub async fn rt_poll(&self, sid: &str, timeout: Duration) -> Result<String> {
        let resp = self
            .http
            .get(format!(
                "{}/socket.io/1/xhr-polling/{}?t={}",
                self.base,
                sid,
                Self::now_ms()
            ))
            .timeout(timeout)
            .send()
            .await?;
        Ok(Self::ok_status(resp).await?.text().await?)
    }

    pub async fn rt_send(&self, sid: &str, payload: String) -> Result<()> {
        let resp = self
            .http
            .post(format!(
                "{}/socket.io/1/xhr-polling/{}?t={}",
                self.base,
                sid,
                Self::now_ms()
            ))
            .header(CONTENT_TYPE, "text/plain;charset=UTF-8")
            .body(payload)
            .send()
            .await?;
        Self::ok_status(resp).await?;
        Ok(())
    }

    // --- content ----------------------------------------------------------

    pub async fn download_doc(&self, project_id: &str, doc_id: &str) -> Result<String> {
        let path = format!("/Project/{project_id}/doc/{doc_id}/download");
        Ok(self.get_authed(&path).await?.text().await?)
    }

    pub async fn download_file(&self, project_id: &str, file_id: &str) -> Result<Vec<u8>> {
        let path = format!("/Project/{project_id}/file/{file_id}");
        Ok(self.get_authed(&path).await?.bytes().await?.to_vec())
    }

    /// Size of a binary file via the HEAD route, when the server reports one.
    pub async fn file_size(&self, project_id: &str, file_id: &str) -> Result<Option<u64>> {
        let resp = self
            .http
            .head(format!("{}/Project/{project_id}/file/{file_id}", self.base))
            .send()
            .await?;
        let resp = Self::ok_status(resp).await?;
        Ok(resp
            .headers()
            .get(reqwest::header::CONTENT_LENGTH)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.parse().ok()))
    }

    pub async fn create_doc(
        &self,
        project_id: &str,
        parent_folder_id: &str,
        name: &str,
    ) -> Result<EntityRefJson> {
        let body = serde_json::to_value(CreateEntityBody {
            name: name.to_string(),
            parent_folder_id: parent_folder_id.to_string(),
        })?;
        let resp = self
            .send_mutating(Method::POST, &format!("/project/{project_id}/doc"), Some(body))
            .await?;
        Ok(resp.json().await?)
    }

    pub async fn create_folder(
        &self,
        project_id: &str,
        parent_folder_id: &str,
        name: &str,
    ) -> Result<EntityRefJson> {
        let body = serde_json::to_value(CreateEntityBody {
            name: name.to_string(),
            parent_folder_id: parent_folder_id.to_string(),
        })?;
        let resp = self
            .send_mutating(
                Method::POST,
                &format!("/project/{project_id}/folder"),
                Some(body),
            )
            .await?;
        Ok(resp.json().await?)
    }

    pub async fn rename_entity(
        &self,
        project_id: &str,
        kind: EntityKind,
        entity_id: &str,
        new_name: &str,
    ) -> Result<()> {
        let body = serde_json::to_value(RenameBody {
            name: new_name.to_string(),
        })?;
        self.send_mutating(
            Method::POST,
            &format!(
                "/project/{project_id}/{}/{entity_id}/rename",
                kind.route_segment()
            ),
            Some(body),
        )
        .await?;
        Ok(())
    }

    pub async fn move_entity(
        &self,
        project_id: &str,
        kind: EntityKind,
        entity_id: &str,
        target_folder_id: &str,
    ) -> Result<()> {
        let body = serde_json::to_value(MoveBody {
            folder_id: target_folder_id.to_string(),
        })?;
        self.send_mutating(
            Method::POST,
            &format!(
                "/project/{project_id}/{}/{entity_id}/move",
                kind.route_segment()
            ),
            Some(body),
        )
        .await?;
        Ok(())
    }

    pub async fn delete_entity(
        &self,
        project_id: &str,
        kind: EntityKind,
        entity_id: &str,
    ) -> Result<()> {
        self.send_mutating(
            Method::DELETE,
            &format!(
                "/project/{project_id}/{}/{entity_id}",
                kind.route_segment()
            ),
            None,
        )
        .await?;
        Ok(())
    }

    /// Uploads (or upserts, when the name already exists in the folder) a file.
    /// Text files with recognized extensions become editable docs server-side.
    pub async fn upload_file(
        &self,
        project_id: &str,
        folder_id: &str,
        name: &str,
        bytes: Vec<u8>,
    ) -> Result<UploadResponse> {
        for attempt in 0..2 {
            let token = self.csrf_token().await?;
            let part = reqwest::multipart::Part::bytes(bytes.clone()).file_name(name.to_string());
            let form = reqwest::multipart::Form::new()
                .text("name", name.to_string())
                .text("relativePath", "null")
                .part("qqfile", part);
            let resp = self
                .http
                .post(format!(
                    "{}/Project/{project_id}/upload?folder_id={folder_id}",
                    self.base
                ))
                .header("x-csrf-token", token)
                .header(ACCEPT, "application/json")
                .multipart(form)
                .send()
                .await?;
            if attempt == 0 && Self::needs_reauth(&resp) {
                self.login().await?;
                continue;
            }
            let resp = Self::ok_status(resp).await?;
            let parsed: UploadResponse = resp.json().await?;
            if !parsed.success {
                return Err(OverleafError::Edit(format!(
                    "upload of {name} rejected by server"
                )));
            }
            return Ok(parsed);
        }
        Err(OverleafError::Auth("re-login did not restore access".to_string()))
    }

    // --- history ----------------------------------------------------------

    /// Flushes pending realtime edits into project history so the summarized
    /// updates reflect the latest state.
    pub async fn flush_history(&self, project_id: &str) -> Result<()> {
        self.send_mutating(Method::POST, &format!("/project/{project_id}/flush"), None)
            .await?;
        Ok(())
    }

    pub async fn history_updates(
        &self,
        project_id: &str,
        before: Option<i64>,
        min_count: usize,
    ) -> Result<UpdatesResponse> {
        let mut path = format!("/project/{project_id}/updates?min_count={min_count}");
        if let Some(before) = before {
            path.push_str(&format!("&before={before}"));
        }
        Ok(self.get_authed(&path).await?.json().await?)
    }

    pub async fn history_labels(&self, project_id: &str) -> Result<Vec<HistoryLabel>> {
        let path = format!("/project/{project_id}/labels");
        Ok(self.get_authed(&path).await?.json().await?)
    }

    pub async fn create_label(
        &self,
        project_id: &str,
        version: i64,
        comment: &str,
    ) -> Result<HistoryLabel> {
        let body = serde_json::to_value(CreateLabelBody {
            comment: comment.to_string(),
            version,
        })?;
        let resp = self
            .send_mutating(
                Method::POST,
                &format!("/project/{project_id}/labels"),
                Some(body),
            )
            .await?;
        Ok(resp.json().await?)
    }

    // --- compile ----------------------------------------------------------

    pub async fn compile(
        &self,
        project_id: &str,
        request: &CompileRequest,
    ) -> Result<CompileResponse> {
        let body = serde_json::to_value(request)?;
        let resp = self
            .send_mutating(
                Method::POST,
                &format!("/project/{project_id}/compile?auto_compile=false"),
                Some(body),
            )
            .await?;
        Ok(resp.json().await?)
    }

    /// Downloads a compile output file by the (relative) URL returned in
    /// `outputFiles`, forwarding the CLSI affinity id when present.
    pub async fn download_output(
        &self,
        url_path: &str,
        clsi_server_id: Option<&str>,
    ) -> Result<Vec<u8>> {
        let mut path = url_path.to_string();
        if let Some(clsi) = clsi_server_id {
            let sep = if path.contains('?') { '&' } else { '?' };
            path = format!("{path}{sep}clsiserverid={clsi}");
        }
        Ok(self.get_authed(&path).await?.bytes().await?.to_vec())
    }
}
