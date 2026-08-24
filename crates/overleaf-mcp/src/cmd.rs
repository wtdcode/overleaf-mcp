use std::net::SocketAddr;

use clap::Parser;
use color_eyre::eyre::eyre;
use llmy::agent::mcp::McpToolBox;
use llmy::agent::rcmp::model::{Implementation, ServerCapabilities, ServerInfo};
use llmy::agent::rcmp::transport::StreamableHttpServerConfig;
use overleaf_tools::{Workspace, WorkspaceSettings};
use overleaf_types::{OverleafConfig, OverleafCredentials, RealtimeSettings};

/// MCP server exposing an Overleaf Community Edition instance: list projects,
/// read/edit/write docs via the realtime OT protocol, search, manage the file
/// tree, compile, and fetch compile outputs.
#[derive(Parser, Debug)]
#[command(name = "overleaf-mcp", version, about)]
pub struct ProgramCommand {
    /// Overleaf base URL, e.g. https://overleaf.example.com
    #[arg(long, env = "OVERLEAF_ENDPOINT")]
    pub endpoint: String,

    /// Account email used to log in (not needed with --session-cookie).
    #[arg(long, env = "OVERLEAF_ACCOUNT")]
    pub account: Option<String>,

    /// Account password (not needed with --session-cookie).
    #[arg(long, env = "OVERLEAF_PASSWORD", hide_env_values = true)]
    pub password: Option<String>,

    /// Pre-authenticated browser cookies (e.g. "overleaf_session2=...").
    /// Required for servers whose login is CAPTCHA-gated, such as
    /// www.overleaf.com: log in with a browser and copy the cookie value.
    #[arg(long, env = "OVERLEAF_COOKIE", hide_env_values = true)]
    pub session_cookie: Option<String>,

    /// Default project (name or id) used when a tool call omits `project`.
    /// Without --allow-all-projects this is a sandbox: other projects are denied.
    #[arg(long, env = "OVERLEAF_PROJECT")]
    pub project: Option<String>,

    /// Allow tool calls to name projects other than --project, which then
    /// only serves as the default when `project` is omitted.
    #[arg(long, env = "OVERLEAF_ALLOW_ALL_PROJECTS")]
    pub allow_all_projects: bool,

    /// Serve MCP over Streamable HTTP on this address instead of stdio.
    #[arg(long)]
    pub listen: Option<SocketAddr>,

    /// Seconds to wait for the realtime connection to become ready.
    #[arg(long, default_value_t = 15)]
    pub connect_timeout: u64,

    /// Seconds to wait for a realtime request (joinDoc, applyOtUpdate) to settle.
    #[arg(long, default_value_t = 15)]
    pub op_timeout: u64,

    /// Seconds before an idle realtime poll request is considered stuck.
    #[arg(long, default_value_t = 70)]
    pub poll_timeout: u64,

    /// How often an edit is retried after a concurrent-edit resync.
    #[arg(long, default_value_t = 2)]
    pub edit_retries: u32,

    /// Maximum number of matches a search returns.
    #[arg(long, default_value_t = 200)]
    pub search_max_matches: usize,
}

impl ProgramCommand {
    pub async fn run(self) -> color_eyre::Result<()> {
        let credentials = match (&self.session_cookie, &self.account, &self.password) {
            (Some(cookie), _, _) => {
                if self.account.is_some() || self.password.is_some() {
                    tracing::info!("session cookie provided; account/password are ignored");
                }
                OverleafCredentials::SessionCookie(cookie.clone())
            }
            (None, Some(account), Some(password)) => OverleafCredentials::Password {
                account: account.clone(),
                password: password.clone(),
            },
            _ => {
                return Err(eyre!(
                    "credentials missing: set OVERLEAF_ACCOUNT and OVERLEAF_PASSWORD, or OVERLEAF_COOKIE for CAPTCHA-gated servers"
                ));
            }
        };
        let cfg = OverleafConfig {
            endpoint: self.endpoint.clone(),
            credentials,
        };
        let settings = WorkspaceSettings {
            default_project: self.project.clone(),
            allow_all_projects: self.allow_all_projects,
            realtime: RealtimeSettings {
                connect_timeout_secs: self.connect_timeout,
                op_timeout_secs: self.op_timeout,
                poll_timeout_secs: self.poll_timeout,
                edit_retries: self.edit_retries,
            },
            search_max_matches: self.search_max_matches,
        };
        let workspace = Workspace::connect(cfg, settings).await?;
        let toolbox = workspace.toolbox();
        let mut server_info = ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new(
                "overleaf-mcp",
                env!("CARGO_PKG_VERSION"),
            ));
        if let Some(pinned) = workspace.pinned_project() {
            server_info.instructions = Some(match self.allow_all_projects {
                false => format!(
                    "This server is restricted to the Overleaf project '{}' (id {}). Every tool call operates on that project, so omit the `project` parameter; naming any other project is denied.",
                    pinned.name, pinned.id
                ),
                true => format!(
                    "The default Overleaf project is '{}' (id {}): tool calls that omit the `project` parameter operate on it. Other accessible projects may still be named explicitly.",
                    pinned.name, pinned.id
                ),
            });
        }
        let server = McpToolBox::new(toolbox, server_info);
        match self.listen {
            Some(addr) => {
                tracing::info!("serving MCP over http on {addr}");
                server
                    .serve_http(addr, StreamableHttpServerConfig::default())
                    .await
                    .map_err(|e| eyre!("mcp http server failed: {e}"))?;
            }
            None => {
                tracing::info!("serving MCP over stdio");
                server
                    .serve_stdio()
                    .await
                    .map_err(|e| eyre!("mcp stdio server failed: {e}"))?;
            }
        }
        Ok(())
    }
}
