/// How to authenticate against the Overleaf instance. The two modes are
/// mutually exclusive: a self-hosted CE server takes a password login, while a
/// CAPTCHA-gated server (www.overleaf.com) only works with cookies copied from
/// a logged-in browser session.
#[derive(Debug, Clone)]
pub enum OverleafCredentials {
    Password { account: String, password: String },
    /// Cookie header content, e.g. `overleaf_session2=...` (multiple cookies
    /// separated by `;` are accepted).
    SessionCookie(String),
}

#[derive(Debug, Clone)]
pub struct OverleafConfig {
    pub endpoint: String,
    pub credentials: OverleafCredentials,
}

impl OverleafConfig {
    /// Base URL without a trailing slash, so paths can be appended verbatim.
    pub fn base(&self) -> String {
        self.endpoint.trim_end_matches('/').to_string()
    }
}

#[derive(Debug, Clone)]
pub struct RealtimeSettings {
    pub connect_timeout_secs: u64,
    pub op_timeout_secs: u64,
    pub poll_timeout_secs: u64,
    pub edit_retries: u32,
}
