//! Connection setup and native Olo capabilities, shared by all interfaces.
use crate::{
    Engine, mcp,
    model::*,
    oauth::{Auth, OAuthConfig},
};
use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{collections::BTreeMap, sync::Arc};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt};
use tokio_util::sync::CancellationToken;
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct OloConfig {
    pub terminal: bool,
    pub screens: bool,
    pub secret_grants: BTreeMap<String, String>,
}
impl Default for OloConfig {
    fn default() -> Self {
        Self { terminal: true, screens: false, secret_grants: BTreeMap::new() }
    }
}
impl OloConfig {
    pub fn validate(&self) -> Result<()> {
        ensure!(self.secret_grants.len() <= 64, "at most 64 password grants");
        for (name, reference) in &self.secret_grants {
            ensure!(!name.trim().is_empty() && name.len() <= 128 && reference.starts_with("op://") && reference.len() <= 2048, "password grants need a name and an op:// field reference");
        }
        Ok(())
    }
}
/// Async native keychain RPC. Dropping it closes the socket, dismissing pending
/// Passwords requests. No blocking worker is left waiting after cancellation.
pub async fn keychain(request: olo_keychain::Request) -> Result<olo_keychain::Response> {
    let timeout = request.timeout();
    let operation = async {
        let mut socket = tokio::net::UnixStream::connect(olo_keychain::socket_path()).await.context("Olo Passwords is not available")?;
        let wire = zeroize::Zeroizing::new(format!("{}\n", serde_json::to_string(&request)?));
        socket.write_all(wire.as_bytes()).await?;
        let mut reader = tokio::io::BufReader::new(socket);
        let mut bytes = zeroize::Zeroizing::new(Vec::new());
        loop {
            let chunk = reader.fill_buf().await?;
            ensure!(!chunk.is_empty(), "Passwords disconnected");
            let n = chunk.iter().position(|b| *b == b'\n').map_or(chunk.len(), |n| n + 1);
            ensure!(bytes.len() + n <= 8 * 1024 * 1024, "Passwords response too large");
            let done = chunk[n - 1] == b'\n';
            bytes.extend_from_slice(&chunk[..n]);
            reader.consume(n);
            if done {
                break;
            }
        }
        let reply: olo_keychain::Response = serde_json::from_slice(&bytes)?;
        if let olo_keychain::Response::Error { message } = &reply {
            bail!("Passwords: {message}");
        }
        Ok(reply)
    };
    tokio::time::timeout(timeout, operation).await.context("Passwords request timed out")?
}
pub async fn resolve(reference: &str) -> Result<String> {
    ensure!(reference.starts_with("op://"), "expected an op:// password field reference");
    let result = keychain(olo_keychain::Request::Cli { query: olo_keychain::CliQuery::Resolve { references: vec![reference.into()] } }).await?;
    match &result {
        olo_keychain::Response::Cli { json } => json.as_array().and_then(|v| v.first()).and_then(Value::as_str).map(str::to_owned).context("Passwords did not return the selected field"),
        _ => bail!("Passwords access was not granted"),
    }
}
pub fn terminal() -> McpServer {
    McpServer {
        name: "olo_terminal".into(),
        url: Some(format!("http://127.0.0.1:{}/mcp", std::env::var("OLO_TERMINAL_MCP_PORT").ok().and_then(|s| s.parse::<u16>().ok()).unwrap_or(17893))),
        optional: true,
        ..Default::default()
    }
}
pub fn preset(name: &str) -> Result<McpServer> {
    let url = match name {
        "slack" => "https://mcp.slack.com/mcp",
        "logfire" | "logfire-us" => "https://logfire-us.pydantic.dev/mcp",
        "logfire-eu" => "https://logfire-eu.pydantic.dev/mcp",
        "terminal" => return Ok(terminal()),
        _ => bail!("unknown integration"),
    };
    let config = if name == "slack" {
        OAuthConfig { client_id: std::env::var("SLACK_CLIENT_ID").ok(), client_secret_env: Some("SLACK_CLIENT_SECRET".into()), callback_port: 8787, ..Default::default() }
    } else {
        OAuthConfig { scopes: vec!["project:read".into()], ..Default::default() }
    };
    Ok(McpServer { name: if name.starts_with("logfire") { "logfire".into() } else { name.into() }, url: Some(url.into()), auth: Some(Auth::Oauth { config }), optional: true, ..Default::default() })
}
impl Engine {
    pub(crate) async fn connect_mcp(&self, server: &McpServer, cwd: &std::path::Path) -> Result<mcp::Connection> {
        let token = match &server.auth {
            None => None,
            Some(Auth::BearerEnv { variable }) => Some(std::env::var(variable).with_context(|| format!("missing MCP token variable {variable}"))?),
            Some(Auth::Password { reference }) => Some(resolve(reference).await?),
            Some(Auth::Oauth { .. }) => Some(self.oauth.token(server).await?),
        };
        mcp::Connection::start_authenticated(server, cwd, token.as_deref()).await
    }
    pub(crate) async fn integration(self: &Arc<Self>, session: &str, action: &str, args: Value) -> Result<Value> {
        // Normalize convenience forms into the same typed operations as /api.
        let (action, args) = if action == "manage" {
            let operation = args["action"].as_str().context("action required")?;
            ensure!(matches!(operation, "login" | "test" | "inspect" | "logout" | "remove"), "unknown connection action");
            (operation.to_owned(), args.clone())
        } else if action == "configure_fields" {
            let address = args["address"].as_str().context("program or URL required")?;
            let mut server = McpServer { name: args["name"].as_str().unwrap_or("").into(), optional: true, ..Default::default() };
            if address.starts_with("http://") || address.starts_with("https://") {
                server.url = Some(address.into());
            } else {
                server.program = address.into();
            }
            server.args = serde_json::from_str(args["args"].as_str().unwrap_or("[]")).context("arguments must be a JSON array of strings")?;
            let reference = args["reference"].as_str().unwrap_or("");
            server.auth = match args["auth"].as_str().unwrap_or("none") {
                "none" => None,
                "token" => Some(Auth::BearerEnv { variable: reference.into() }),
                "password" => Some(Auth::Password { reference: reference.into() }),
                "oauth" => Some(Auth::Oauth {
                    config: OAuthConfig {
                        client_id: args["client_id"].as_str().filter(|v| !v.is_empty()).map(str::to_owned),
                        client_secret_env: args["client_secret_env"].as_str().filter(|v| !v.is_empty()).map(str::to_owned),
                        ..Default::default()
                    },
                }),
                _ => bail!("authentication must be none, token, password or oauth"),
            };
            ("configure".into(), json!({"server":server}))
        } else {
            (action.to_owned(), args)
        };
        let setup = matches!(action.as_str(), "setup" | "setup_slack");
        let args = if setup {
            let preset_name = if action == "setup_slack" { "slack" } else { args["preset"].as_str().context("preset required")? };
            let mut server = preset(preset_name)?;
            if action == "setup_slack" {
                if let Some(Auth::Oauth { config }) = &mut server.auth {
                    config.client_id = Some(args["client_id"].as_str().context("client ID required")?.into());
                    config.client_secret_env = Some(args["client_secret_env"].as_str().context("secret variable required")?.into());
                }
            }
            {
                let _idle = self.idle_guard(session)?;
                self.change(session, "integration_configured", |s| {
                    let mut config = s.config.clone();
                    if let Some(old) = config.mcp.iter_mut().find(|v| v.name == server.name) {
                        *old = server.clone();
                    } else {
                        config.mcp.push(server.clone());
                    }
                    config.validate()?;
                    s.config = config;
                    Ok(json!({"server":server,"configured":true}))
                })?;
            }
            json!({"name":server.name})
        } else {
            args
        };
        let action = if setup { "login" } else { action.as_str() };
        let s = self.get(session)?;
        match action {
            "" | "list" => Ok(json!({"servers":s.config.mcp,"olo":s.config.olo,"logins":self.oauth.status().await})),
            "catalog" => Ok(
                json!({"presets":[{"id":"slack","name":"Slack","description":"Official Slack MCP. Requires your registered Slack app client ID and secret. Callback: http://127.0.0.1:8787/callback"},{"id":"logfire","name":"Logfire · US","description":"Browser sign-in or API key"},{"id":"logfire-eu","name":"Logfire · EU","description":"Browser sign-in or API key"},{"id":"terminal","name":"Olo Terminal","description":"Native tab title MCP"}],"native":["Agent Screens","Passwords: explicit item/field grants"]}),
            ),
            "install" | "configure" => {
                let server = if action == "install" { preset(args["preset"].as_str().context("preset required")?)? } else { serde_json::from_value::<McpServer>(args["server"].clone())? };
                let _idle = self.idle_guard(session)?;
                self.change(session, "integration_configured", |s| {
                    let mut config = s.config.clone();
                    if let Some(old) = config.mcp.iter_mut().find(|v| v.name == server.name) {
                        ensure!(action == "configure", "integration is already installed; use configure");
                        *old = server.clone();
                    } else {
                        config.mcp.push(server.clone());
                    }
                    config.validate()?;
                    s.config = config;
                    Ok(json!({"server":server,"configured":true}))
                })
            }
            "remove" => {
                let name = args["name"].as_str().context("name required")?;
                let _idle = self.idle_guard(session)?;
                self.change(session, "integration_removed", |s| {
                    s.config.mcp.retain(|v| v.name != name);
                    Ok(json!({"removed":name}))
                })
            }
            "login" | "test" | "logout" | "inspect" | "request" => {
                let name = args["name"].as_str().context("name required")?;
                let server = s.config.mcp.iter().find(|v| v.name == name).context("integration is not installed")?;
                match action {
                    "login" => {
                        let result = tokio::time::timeout(std::time::Duration::from_secs(35), self.oauth.begin(server)).await.context("sign-in discovery timed out")??;
                        let engine = self.clone();
                        let session = session.to_owned();
                        let flow = result["flow"].as_str().context("missing login flow")?.to_owned();
                        let name = name.to_owned();
                        tokio::spawn(async move {
                            if let Ok(status) = engine.oauth.finished(&flow, &engine.shutdown).await {
                                let _ = engine.change(&session, "integration_login", |_| Ok(json!({"server":name,"flow":flow,"status":status})));
                            }
                        });
                        Ok(result)
                    }
                    "logout" => self.oauth.forget(server).await,
                    _ => tokio::time::timeout(std::time::Duration::from_secs(35), async {
                        let mut connection = tokio::time::timeout(std::time::Duration::from_secs(60), self.connect_mcp(server, &s.workspace)).await.context("MCP connection timed out")??;
                        if action == "request" {
                            let method = args["method"].as_str().context("method required")?;
                            connection.read(method, args["params"].clone(), &self.shutdown, s.config.command_timeout_secs).await
                        } else {
                            let tools = tokio::time::timeout(std::time::Duration::from_secs(30), connection.tools(name)).await??;
                            Ok(json!({"connected":true,"tools":tools}))
                        }
                    })
                    .await
                    .context("MCP inspection timed out; check the server and authentication")?,
                }
            }
            "status" => Ok(self.oauth.status().await),
            "cancel_login" => self.oauth.cancel(args["flow"].as_str().context("flow required")?).await,
            "olo" => {
                let config: OloConfig = serde_json::from_value(args)?;
                config.validate()?;
                let _idle = self.idle_guard(session)?;
                self.change(session, "olo_configured", |s| {
                    s.config.olo = config;
                    Ok(json!({"olo":s.config.olo}))
                })
            }
            "password_search" => {
                let result = keychain(olo_keychain::Request::Search { query: args["query"].as_str().unwrap_or("").into() }).await?;
                Ok(serde_json::to_value(&result)?)
            }
            "password_fields" => {
                let vault = args["vault"].as_str().context("vault required")?.to_owned();
                let item = args["item"].as_str().context("item required")?.to_owned();
                match tokio::time::timeout(std::time::Duration::from_secs(35), keychain(olo_keychain::Request::Cli { query: olo_keychain::CliQuery::Fields { vault, item } }))
                    .await
                    .context("Unlock Passwords and allow access, then try again")??
                {
                    olo_keychain::Response::Cli { ref json } => Ok(json.clone()),
                    _ => bail!("Unlock Passwords and allow field selection to continue"),
                }
            }
            "grant_secret" | "revoke_secret" => {
                let name = args["name"].as_str().context("grant name required")?;
                let _idle = self.idle_guard(session)?;
                self.change(session, "password_grant", |s| {
                    let mut olo = s.config.olo.clone();
                    if action == "grant_secret" {
                        olo.secret_grants.insert(name.into(), args["reference"].as_str().context("op:// reference required")?.into());
                    } else {
                        olo.secret_grants.remove(name);
                    }
                    olo.validate()?;
                    s.config.olo = olo;
                    Ok(json!({"name":name,"shared":action=="grant_secret"}))
                })
            }
            _ => bail!("unknown integration action"),
        }
    }
    pub(crate) async fn olo_tool(&self, s: &Session, name: &str, args: &Value, cancel: &CancellationToken) -> Result<Value> {
        match name {
            "set_terminal_title" => {
                ensure!(s.config.olo.terminal, "Terminal integration disabled");
                let mut c = self.connect_mcp(&terminal(), &s.workspace).await?;
                c.invoke("set_tab_title", json!({"title":args["title"],"cwd":s.workspace}), cancel, 10).await
            }
            "screen_capture" => {
                ensure!(s.config.olo.screens, "Enable Agent Screens in Integrations first");
                let artifact = id();
                let path = self.artifacts.join(&artifact);
                let screen = format!("coder-{}", s.id.replace('-', ""));
                let mut argv = vec!["look".into(), screen, path.to_string_lossy().into_owned(), "--no-pointer".into()];
                if let Some(window) = args["window"].as_u64() {
                    argv.extend(["--window".into(), window.to_string()]);
                }
                let job = tokio::task::spawn_blocking(move || olo_screen::agent_command(&argv).map_err(anyhow::Error::msg));
                tokio::select! {_ = cancel.cancelled()=>bail!("capture cancelled"),result=job=>{result??;}}
                let bytes = tokio::fs::metadata(self.artifacts.join(&artifact)).await?.len();
                ensure!(bytes <= 20 * 1024 * 1024, "capture exceeds 20 MB; capture a smaller window");
                Ok(json!({"artifact":artifact,"mime_type":"image/png","bytes":bytes,"description":"Screenshot attached for visual inspection after tool results."}))
            }
            "read_granted_secret" => {
                let name = args["name"].as_str().context("grant name required")?;
                let reference = s.config.olo.secret_grants.get(name).context("this password field has not been shared with this session")?;
                tokio::select! {_ = cancel.cancelled()=>bail!("cancelled"),value=resolve(reference)=>Ok(json!({"name":name,"value":value?}))}
            }
            "screen_read" | "screen_control" => {
                ensure!(s.config.olo.screens, "Enable Agent Screens in Integrations first");
                let operation = args["operation"].as_str().context("operation required")?;
                ensure!(
                    matches!(
                        operation,
                        "windows"
                            | "tree"
                            | "menus"
                            | "review"
                            | "start"
                            | "stop"
                            | "run"
                            | "click"
                            | "set"
                            | "select"
                            | "type"
                            | "key"
                            | "scroll"
                            | "wheel"
                            | "drag"
                            | "move"
                            | "focus"
                            | "close"
                            | "minimize"
                            | "zoom"
                            | "resize"
                            | "place"
                            | "invoke"
                    ),
                    "unsupported screen operation"
                );
                let read = matches!(operation, "windows" | "tree" | "menus" | "review");
                ensure!(read == (name == "screen_read"), "operation does not match tool permissions");
                let screen = format!("coder-{}", s.id.replace('-', ""));
                ensure!(screen.len() <= 40, "invalid session screen name");
                let mut argv = vec![operation.to_owned(), screen];
                if let Some(arguments) = args["args"].as_array() {
                    for v in arguments {
                        argv.push(v.as_str().context("screen args must be strings")?.into());
                    }
                }
                if matches!(operation, "tree" | "review") {
                    argv.push("--json".into());
                }
                let job = tokio::task::spawn_blocking(move || olo_screen::agent_command(&argv).map_err(anyhow::Error::msg));
                // Native commands finish atomically; don't advertise cancelled
                // effects as completed. Slow application launches run off runtime threads.
                tokio::select! {_ = cancel.cancelled()=>bail!("screen action cancelled; inspect the screen before retrying"),value=job=>value?}
            }
            _ => bail!("unknown Olo tool"),
        }
    }
}
