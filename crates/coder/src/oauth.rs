//! MCP OAuth: resource/issuer discovery, S256 PKCE, loopback callbacks, encrypted
//! credentials and serialized refresh. No credential is part of a session event.
use crate::{
    model::{McpServer, id, now},
    tools,
};
use anyhow::{Context, Result, bail, ensure};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD as B64};
use chacha20poly1305::{
    XChaCha20Poly1305, XNonce,
    aead::{Aead, KeyInit, Payload},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{collections::HashMap, path::PathBuf, sync::Arc};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio_util::sync::CancellationToken;
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Auth {
    BearerEnv {
        variable: String,
    },
    Password {
        reference: String,
    },
    Oauth {
        #[serde(flatten)]
        config: OAuthConfig,
    },
}
#[derive(Debug, Clone, Default, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct OAuthConfig {
    pub client_id: Option<String>,
    pub client_secret_env: Option<String>,
    pub scopes: Vec<String>,
    pub callback_port: u16,
}
#[derive(Clone, Serialize, Deserialize)]
struct Credential {
    access_token: String,
    refresh_token: Option<String>,
    expires: u64,
    client_id: String,
    client_secret: Option<String>,
    issuer: String,
    token_endpoint: String,
    resource: String,
    auth_method: String,
}
impl Drop for Credential {
    fn drop(&mut self) {
        use zeroize::Zeroize;
        self.access_token.zeroize();
        self.refresh_token.zeroize();
        self.client_secret.zeroize();
    }
}
struct Flow {
    cancel: CancellationToken,
    status: Value,
    identity: String,
}
pub struct Manager {
    directory: PathBuf,
    key: tokio::sync::Mutex<Option<zeroize::Zeroizing<Vec<u8>>>>,
    credentials: tokio::sync::Mutex<HashMap<String, Credential>>,
    flows: tokio::sync::Mutex<HashMap<String, Flow>>,
    changed: tokio::sync::watch::Sender<u64>,
}
impl Manager {
    pub fn new(directory: PathBuf) -> Arc<Self> {
        Arc::new(Self { directory, key: Default::default(), credentials: Default::default(), flows: Default::default(), changed: tokio::sync::watch::channel(0).0 })
    }
    pub async fn finished(&self, id: &str, cancel: &CancellationToken) -> Result<Value> {
        let mut changes = self.changed.subscribe();
        loop {
            let status = self.flows.lock().await.get(id).context("login not found")?.status.clone();
            if status["state"] != "waiting" {
                return Ok(status);
            }
            tokio::select! {_ = cancel.cancelled()=>bail!("cancelled"),r=changes.changed()=>r?}
        }
    }
    pub async fn status(&self) -> Value {
        json!(self.flows.lock().await.iter().map(|(id, f)| json!({"id":id,"status":f.status})).collect::<Vec<_>>())
    }
    pub async fn cancel(&self, id: &str) -> Result<Value> {
        let mut flows = self.flows.lock().await;
        let f = flows.get_mut(id).context("login not found")?;
        f.cancel.cancel();
        f.status = json!({"state":"cancelled"});
        self.changed.send_modify(|v| *v += 1);
        Ok(f.status.clone())
    }
    fn identity(server: &McpServer) -> String {
        hex(&Sha256::digest(format!("{}\n{}", server.url.as_deref().unwrap_or(""), serde_json::to_string(&server.auth).unwrap_or_default())))
    }
    async fn key(&self) -> Result<zeroize::Zeroizing<Vec<u8>>> {
        let mut cached = self.key.lock().await;
        if let Some(key) = cached.as_ref() {
            return Ok(key.clone());
        }
        let title = format!("Olo Coder MCP encryption {}", hex(&Sha256::digest(self.directory.to_string_lossy().as_bytes())));
        let answer = crate::integrations::keychain(olo_keychain::Request::Secrets { title: title.clone() }).await?;
        let mut key = match &answer {
            olo_keychain::Response::Secrets { values } => values.first().map(|v| B64.decode(v)).transpose()?,
            _ => bail!("Unlock Olo Passwords to access MCP sign-ins"),
        };
        if key.is_none() {
            let mut bytes = vec![0; 32];
            getrandom::fill(&mut bytes).map_err(|_| anyhow::anyhow!("secure randomness unavailable"))?;
            let reply = crate::integrations::keychain(olo_keychain::Request::KeepSecret {
                title,
                secret: B64.encode(&bytes),
                note: "Encrypts this Olo Coder service's MCP OAuth credentials. No conversation data is stored here.".into(),
            })
            .await?;
            ensure!(matches!(reply, olo_keychain::Response::Saved { .. }), "Passwords could not save the MCP encryption key");
            key = Some(bytes);
        }
        let key = zeroize::Zeroizing::new(key.unwrap());
        ensure!(key.len() == 32, "invalid MCP encryption key");
        *cached = Some(key.clone());
        Ok(key)
    }
    async fn save(&self, identity: &str, credential: &Credential) -> Result<()> {
        let key = self.key().await?;
        let cipher = XChaCha20Poly1305::new_from_slice(&key).map_err(|_| anyhow::anyhow!("invalid key"))?;
        let mut nonce = [0; 24];
        getrandom::fill(&mut nonce).map_err(|_| anyhow::anyhow!("secure randomness unavailable"))?;
        let plaintext = zeroize::Zeroizing::new(serde_json::to_vec(credential)?);
        let encrypted = cipher.encrypt(&XNonce::from(nonce), Payload { msg: &plaintext, aad: identity.as_bytes() }).map_err(|_| anyhow::anyhow!("credential encryption failed"))?;
        let mut bytes = nonce.to_vec();
        bytes.extend(encrypted);
        let path = self.directory.join(format!("oauth-{identity}.sealed"));
        tokio::task::spawn_blocking(move || tools::atomic_write(&path, &bytes)).await??;
        Ok(())
    }
    pub async fn token(self: &Arc<Self>, server: &McpServer) -> Result<String> {
        // Rotation must finish durably even if a cancelled tool drops its waiter.
        let manager = self.clone();
        let server = server.clone();
        tokio::spawn(async move { manager.token_inner(&server).await }).await?
    }
    async fn token_inner(&self, server: &McpServer) -> Result<String> {
        let identity = Self::identity(server);
        let mut credentials = self.credentials.lock().await;
        if !credentials.contains_key(&identity) {
            let bytes = tokio::fs::read(self.directory.join(format!("oauth-{identity}.sealed"))).await.context("MCP is not signed in; use Integrations → Sign In")?;
            ensure!(bytes.len() > 24, "invalid credential file");
            let key = self.key().await?;
            let cipher = XChaCha20Poly1305::new_from_slice(&key).map_err(|_| anyhow::anyhow!("invalid key"))?;
            let plain = zeroize::Zeroizing::new(
                cipher
                    .decrypt(&XNonce::try_from(&bytes[..24]).map_err(|_| anyhow::anyhow!("invalid nonce"))?, Payload { msg: &bytes[24..], aad: identity.as_bytes() })
                    .map_err(|_| anyhow::anyhow!("credential decryption failed"))?,
            );
            credentials.insert(identity.clone(), serde_json::from_slice(&plain)?);
        }
        let c = credentials.get_mut(&identity).unwrap();
        if c.expires <= now() + 60 {
            let refresh = c.refresh_token.clone().context("MCP sign-in expired; sign in again")?;
            let form = vec![("grant_type", "refresh_token".to_owned()), ("refresh_token", refresh), ("resource", c.resource.clone())];
            let value = token_request(c, form).await?;
            update(c, value)?;
            self.save(&identity, c).await?;
        }
        Ok(c.access_token.clone())
    }
    pub async fn forget(&self, server: &McpServer) -> Result<Value> {
        let identity = Self::identity(server);
        {
            let mut flows = self.flows.lock().await;
            for flow in flows.values_mut().filter(|f| f.identity == identity) {
                flow.cancel.cancel();
                flow.status = json!({"state":"cancelled"});
            }
        }
        self.changed.send_modify(|v| *v += 1);
        let mut credentials = self.credentials.lock().await;
        credentials.remove(&identity);
        match tokio::fs::remove_file(self.directory.join(format!("oauth-{identity}.sealed"))).await {
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.into()),
        };
        Ok(json!({"signed_out":true}))
    }
    pub async fn begin(self: &Arc<Self>, server: &McpServer) -> Result<Value> {
        let Some(Auth::Oauth { config }) = &server.auth else { bail!("configure OAuth for this server first") };
        let resource = server.url.as_ref().context("OAuth requires a remote MCP")?;
        secure_url(resource)?;
        let (issuer, metadata, scopes, canonical_resource) = discover(resource).await?;
        let resource = &canonical_resource;
        ensure!(metadata["code_challenge_methods_supported"].as_array().is_some_and(|a| a.iter().any(|v| v == "S256")), "authorization server must advertise S256 PKCE");
        let auth_endpoint = metadata["authorization_endpoint"].as_str().context("missing authorization endpoint")?;
        secure_url(auth_endpoint)?;
        let token_endpoint = metadata["token_endpoint"].as_str().context("missing token endpoint")?;
        secure_url(token_endpoint)?;
        let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, config.callback_port)).await?;
        let redirect = format!("http://127.0.0.1:{}/callback", listener.local_addr()?.port());
        let mut secret = config.client_secret_env.as_ref().map(|name| std::env::var(name).with_context(|| format!("missing client secret environment variable {name}"))).transpose()?;
        let client_id = if let Some(id) = &config.client_id {
            id.clone()
        } else {
            let registration =
                metadata["registration_endpoint"].as_str().context("This provider requires a registered app. Configure its client ID and secret environment reference, then Sign In.")?;
            secure_url(registration)?;
            let response = http()?
                .post(registration)
                .json(&json!({"client_name":"Olo Coder","redirect_uris":[redirect],"grant_types":["authorization_code","refresh_token"],"response_types":["code"],"token_endpoint_auth_method":"none"}))
                .send()
                .await?;
            let value = bounded_json(response).await?;
            secret = value["client_secret"].as_str().map(str::to_owned);
            value["client_id"].as_str().context("registration omitted client_id")?.to_owned()
        };
        let mut random = [0; 32];
        getrandom::fill(&mut random).map_err(|_| anyhow::anyhow!("secure randomness unavailable"))?;
        let verifier = B64.encode(random);
        let challenge = B64.encode(Sha256::digest(verifier.as_bytes()));
        let state = id();
        let flow = id();
        let scopes = if config.scopes.is_empty() { scopes } else { config.scopes.clone() };
        let mut url = reqwest::Url::parse(auth_endpoint)?;
        url.query_pairs_mut().extend_pairs([
            ("response_type", "code"),
            ("client_id", &client_id),
            ("redirect_uri", &redirect),
            ("code_challenge", &challenge),
            ("code_challenge_method", "S256"),
            ("state", &state),
            ("resource", resource),
            ("scope", &scopes.join(" ")),
        ]);
        let method = if secret.is_none() {
            "none"
        } else if metadata["token_endpoint_auth_methods_supported"].as_array().is_some_and(|a| a.iter().any(|v| v == "client_secret_post")) {
            "client_secret_post"
        } else {
            "client_secret_basic"
        };
        let mut credential = Credential {
            access_token: String::new(),
            refresh_token: None,
            expires: 0,
            client_id,
            client_secret: secret,
            issuer,
            token_endpoint: token_endpoint.into(),
            resource: resource.clone(),
            auth_method: method.into(),
        };
        let required_issuer = metadata["authorization_response_iss_parameter_supported"] == true;
        let identity = Self::identity(server);
        let manager = self.clone();
        let cancel = CancellationToken::new();
        let token = cancel.clone();
        let fid = flow.clone();
        {
            let mut flows = self.flows.lock().await;
            ensure!(flows.values().filter(|f| f.status["state"] == "waiting").count() < 8, "too many pending sign-ins");
            if flows.len() >= 64 {
                flows.retain(|_, f| f.status["state"] == "waiting");
            }
            flows.insert(flow.clone(), Flow { cancel, identity: identity.clone(), status: json!({"state":"waiting","server":server.name}) });
        }
        tokio::spawn(async move {
            let work = async {
                let code = callback(&listener, &state, &credential.issuer, required_issuer).await?;
                let value = token_request(
                    &credential,
                    vec![("grant_type", "authorization_code".into()), ("code", code), ("redirect_uri", redirect), ("code_verifier", verifier), ("resource", credential.resource.clone())],
                )
                .await?;
                update(&mut credential, value)?;
                Ok::<_, anyhow::Error>(credential)
            };
            let result = tokio::select! {_ = token.cancelled()=>Err(anyhow::anyhow!("sign-in cancelled")),v=tokio::time::timeout(std::time::Duration::from_secs(600),work)=>v.context("sign-in timed out").and_then(|v|v)};
            // Commit outside the cancellable select. Logout takes the same lock;
            // a cancelled atomic write must never resurrect signed-out tokens.
            let result = match result {
                Ok(credential) => {
                    let mut credentials = manager.credentials.lock().await;
                    if token.is_cancelled() {
                        Err(anyhow::anyhow!("sign-in cancelled"))
                    } else {
                        let saved = manager.save(&identity, &credential).await;
                        if token.is_cancelled() {
                            let _ = tokio::fs::remove_file(manager.directory.join(format!("oauth-{identity}.sealed"))).await;
                            Err(anyhow::anyhow!("sign-in cancelled"))
                        } else {
                            if saved.is_ok() {
                                credentials.insert(identity, credential);
                            }
                            saved
                        }
                    }
                }
                Err(error) => Err(error),
            };
            if let Some(f) = manager.flows.lock().await.get_mut(&fid) {
                f.status = if token.is_cancelled() {
                    json!({"state":"cancelled"})
                } else {
                    match result {
                        Ok(()) => json!({"state":"connected"}),
                        Err(e) => json!({"state":"failed","message":e.to_string()}),
                    }
                };
            }
            manager.changed.send_modify(|v| *v += 1);
        });
        Ok(json!({"flow":flow,"authorization_url":url.as_str(),"state":"waiting","expires_in":600}))
    }
}
pub fn secure_url(value: &str) -> Result<reqwest::Url> {
    let url = reqwest::Url::parse(value)?;
    ensure!(
        url.username().is_empty()
            && url.password().is_none()
            && url.fragment().is_none()
            && url.host_str().is_some()
            && (url.scheme() == "https" || url.scheme() == "http" && matches!(url.host_str(), Some("127.0.0.1" | "localhost" | "[::1]"))),
        "MCP/auth URL must use HTTPS (loopback HTTP allowed), without userinfo or fragment"
    );
    Ok(url)
}
fn http() -> Result<reqwest::Client> {
    Ok(reqwest::Client::builder().redirect(reqwest::redirect::Policy::none()).timeout(std::time::Duration::from_secs(30)).build()?)
}
async fn bounded_json(mut response: reqwest::Response) -> Result<Value> {
    ensure!(response.status().is_success(), "authentication endpoint returned HTTP {}", response.status());
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        ensure!(bytes.len() + chunk.len() <= 1024 * 1024, "authentication response too large");
        bytes.extend(chunk);
    }
    Ok(serde_json::from_slice(&bytes)?)
}
fn validate_resource(endpoint: &str, resource: &str) -> Result<()> {
    let endpoint = secure_url(endpoint)?;
    let resource = secure_url(resource)?;
    let base = resource.path().trim_end_matches('/');
    ensure!(
        endpoint.origin() == resource.origin() && resource.query().is_none() && (endpoint.path().trim_end_matches('/') == base || endpoint.path().starts_with(&format!("{base}/"))),
        "protected-resource metadata does not identify this MCP endpoint"
    );
    Ok(())
}
async fn discover(resource: &str) -> Result<(String, Value, Vec<String>, String)> {
    let url = secure_url(resource)?;
    let origin = url.origin().ascii_serialization();
    let client = http()?;
    let mut urls = vec![format!("{origin}/.well-known/oauth-protected-resource{}", url.path().trim_end_matches('/')), format!("{origin}/.well-known/oauth-protected-resource")];
    let challenge = client.get(resource).send().await?;
    if let Some(header) = challenge.headers().get("www-authenticate").and_then(|v| v.to_str().ok()) {
        if let Some(value) = header.split("resource_metadata=\"").nth(1).and_then(|s| s.split('"').next()) {
            secure_url(value)?;
            urls.insert(0, value.into());
        }
    }
    let mut metadata = None;
    for candidate in urls {
        if let Ok(response) = client.get(&candidate).send().await {
            if response.status().is_success() {
                metadata = Some(bounded_json(response).await?);
                break;
            }
        }
    }
    let protected = metadata.context("MCP protected-resource metadata is unavailable")?;
    let canonical = protected["resource"].as_str().context("metadata omitted resource identifier")?;
    validate_resource(resource, canonical)?;
    let issuer = protected["authorization_servers"].as_array().and_then(|a| a.first()).and_then(Value::as_str).context("MCP has no authorization server")?.to_owned();
    let i = secure_url(&issuer)?;
    let origin = i.origin().ascii_serialization();
    let suffix = i.path().trim_end_matches('/');
    let candidates = [
        format!("{origin}/.well-known/oauth-authorization-server{suffix}"),
        format!("{origin}/.well-known/openid-configuration{suffix}"),
        format!("{}/.well-known/openid-configuration", issuer.trim_end_matches('/')),
    ];
    for candidate in candidates {
        if let Ok(response) = client.get(candidate).send().await {
            if response.status().is_success() {
                let value = bounded_json(response).await?;
                ensure!(value["issuer"] == issuer, "authorization issuer mismatch");
                let scopes = protected["scopes_supported"].as_array().into_iter().flatten().filter_map(|v| v.as_str().map(str::to_owned)).collect();
                return Ok((issuer, value, scopes, canonical.into()));
            }
        }
    }
    bail!("authorization server metadata is unavailable")
}
async fn token_request(c: &Credential, mut form: Vec<(&str, String)>) -> Result<Value> {
    form.push(("client_id", c.client_id.clone()));
    let mut request = http()?.post(&c.token_endpoint);
    if let Some(secret) = &c.client_secret {
        if c.auth_method == "client_secret_post" {
            form.push(("client_secret", secret.clone()));
        } else {
            request = request.basic_auth(&c.client_id, Some(secret));
        }
    }
    bounded_json(request.form(&form).send().await?).await
}
fn update(c: &mut Credential, value: Value) -> Result<()> {
    let slack = c.issuer == "https://mcp.slack.com" && c.token_endpoint == "https://slack.com/api/oauth.v2.user.access";
    ensure!(value["ok"] != false && value.get("error").is_none(), "OAuth provider rejected the token exchange");
    ensure!(value["token_type"].as_str().is_some_and(|v| v.eq_ignore_ascii_case("bearer") || slack && v == "user"), "OAuth token type must be Bearer (Slack user tokens are supported)");
    let access = value["access_token"].as_str().filter(|v| !v.is_empty()).context("OAuth response omitted access token")?;
    c.access_token = access.into();
    if let Some(refresh) = value["refresh_token"].as_str() {
        c.refresh_token = Some(refresh.into());
    }
    c.expires = if let Some(expires) = value.get("expires_in") { now().saturating_add(expires.as_u64().context("invalid OAuth expiry")?) } else { u64::MAX };
    Ok(())
}
async fn callback(listener: &tokio::net::TcpListener, state: &str, issuer: &str, required_issuer: bool) -> Result<String> {
    loop {
        let (mut stream, peer) = listener.accept().await?;
        if !peer.ip().is_loopback() {
            continue;
        }
        let parsed = tokio::time::timeout(std::time::Duration::from_secs(5), async {
            let mut bytes = Vec::new();
            loop {
                let mut chunk = [0; 1024];
                let n = stream.read(&mut chunk).await?;
                ensure!(n > 0 && bytes.len() + n <= 16384, "invalid callback");
                bytes.extend_from_slice(&chunk[..n]);
                if bytes.windows(4).any(|w| w == b"\r\n\r\n") {
                    break;
                }
            }
            let text = std::str::from_utf8(&bytes)?;
            let path = text.lines().next().and_then(|s| s.strip_prefix("GET ")).and_then(|s| s.split_whitespace().next()).context("invalid request")?;
            ensure!(path.starts_with("/callback?"), "invalid callback path");
            let url = reqwest::Url::parse(&format!("http://127.0.0.1{path}"))?;
            let mut pairs = std::collections::HashMap::new();
            for (key, value) in url.query_pairs() {
                ensure!(pairs.insert(key.into_owned(), value.into_owned()).is_none(), "duplicate callback parameter");
            }
            ensure!(pairs.get("state").map(String::as_str) == Some(state), "invalid state");
            ensure!(pairs.get("iss").map_or(!required_issuer, |v| v == issuer), "invalid issuer");
            ensure!(pairs.contains_key("code") || pairs.contains_key("error"), "missing code");
            Ok::<_, anyhow::Error>(pairs)
        })
        .await;
        let Ok(Ok(pairs)) = parsed else {
            let _ = stream.write_all(b"HTTP/1.1 400 Bad Request\r\nContent-Length: 16\r\nConnection: close\r\n\r\nInvalid callback").await;
            continue;
        };
        let body = "Authorization received. Return to Olo Coder to check connection status.";
        let _ = stream.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).as_bytes()).await;
        ensure!(!pairs.contains_key("error"), "authorization was declined");
        return pairs.get("code").filter(|v| !v.is_empty()).cloned().context("callback omitted code");
    }
}
fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|v| format!("{v:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        Form, Json, Router,
        extract::State,
        routing::{get, post},
    };
    use std::sync::atomic::{AtomicUsize, Ordering};
    #[derive(Clone)]
    struct Fixture {
        base: String,
        calls: Arc<AtomicUsize>,
        forms: Arc<olo_sync::Mutex<Vec<HashMap<String, String>>>>,
    }
    async fn metadata(State(f): State<Fixture>) -> Json<Value> {
        Json(
            json!({"issuer":f.base,"authorization_endpoint":format!("{}/authorize",f.base),"token_endpoint":format!("{}/token",f.base),"registration_endpoint":format!("{}/register",f.base),"code_challenge_methods_supported":["S256"],"token_endpoint_auth_methods_supported":["none"],"authorization_response_iss_parameter_supported":true}),
        )
    }
    async fn resource(State(f): State<Fixture>) -> Json<Value> {
        Json(json!({"resource":format!("{}/mcp",f.base),"authorization_servers":[f.base],"scopes_supported":["read"]}))
    }
    async fn token(State(f): State<Fixture>, Form(form): Form<HashMap<String, String>>) -> Json<Value> {
        f.forms.lock().unwrap().push(form);
        let count = f.calls.fetch_add(1, Ordering::SeqCst);
        Json(json!({"access_token":format!("secret-access-{count}"),"refresh_token":format!("secret-refresh-{count}"),"expires_in":if count==0{1}else{3600},"token_type":"Bearer"}))
    }
    async fn fixture() -> (Fixture, McpServer, tokio::task::JoinHandle<()>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let f = Fixture { base: format!("http://{}", listener.local_addr().unwrap()), calls: Arc::new(AtomicUsize::new(0)), forms: Default::default() };
        let app = Router::new()
            .route("/mcp", get(|| async { axum::http::StatusCode::UNAUTHORIZED }))
            .route("/.well-known/oauth-protected-resource/mcp", get(resource))
            .route("/.well-known/oauth-authorization-server", get(metadata))
            .route("/register", post(|| async { Json(json!({"client_id":"registered-coder"})) }))
            .route("/token", post(token))
            .with_state(f.clone());
        let task = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        let server = McpServer { name: "fixture".into(), url: Some(format!("{}/mcp", f.base)), auth: Some(Auth::Oauth { config: Default::default() }), ..Default::default() };
        (f, server, task)
    }
    #[tokio::test]
    async fn oauth_pkce_refresh_rotation_sealed_restart_and_logout() {
        let (f, server, task) = fixture().await;
        let dir = PathBuf::from("/var/tmp").join(format!("coder-oauth-{}", id()));
        std::fs::create_dir_all(&dir).unwrap();
        let manager = Manager::new(dir.clone());
        *manager.key.lock().await = Some(zeroize::Zeroizing::new(vec![9; 32]));
        let started = manager.begin(&server).await.unwrap();
        let auth = reqwest::Url::parse(started["authorization_url"].as_str().unwrap()).unwrap();
        let pairs = auth.query_pairs().into_owned().collect::<HashMap<_, _>>();
        assert_eq!(pairs["resource"], server.url.as_ref().unwrap().as_str());
        assert_eq!(pairs["scope"], "read");
        assert_eq!(pairs["code_challenge_method"], "S256");
        let redirect = &pairs["redirect_uri"];
        // Malformed and wrong-state callbacks cannot destroy a pending login.
        let address = reqwest::Url::parse(redirect).unwrap();
        let mut socket = tokio::net::TcpStream::connect(("127.0.0.1", address.port().unwrap())).await.unwrap();
        socket.write_all(b"malformed\r\n\r\n").await.unwrap();
        drop(socket);
        assert_eq!(http().unwrap().get(format!("{redirect}?state=wrong&code=x")).send().await.unwrap().status(), 400);
        assert_eq!(http().unwrap().get(redirect).query(&[("state", pairs["state"].as_str()), ("state", "duplicate"), ("code", "x")]).send().await.unwrap().status(), 400);
        let response = http().unwrap().get(redirect).query(&[("state", pairs["state"].as_str()), ("code", "fixture-code"), ("iss", f.base.as_str())]).send().await.unwrap();
        assert!(response.status().is_success());
        let status = tokio::time::timeout(std::time::Duration::from_secs(5), manager.finished(started["flow"].as_str().unwrap(), &CancellationToken::new())).await.unwrap().unwrap();
        assert_eq!(status["state"], "connected", "{status}");
        let form = f.forms.lock().unwrap()[0].clone();
        assert_eq!(B64.encode(Sha256::digest(form["code_verifier"].as_bytes())), pairs["code_challenge"]);
        let (a, b) = tokio::join!(manager.token(&server), manager.token(&server));
        assert_eq!(a.unwrap(), "secret-access-1");
        assert_eq!(b.unwrap(), "secret-access-1");
        assert_eq!(f.calls.load(Ordering::SeqCst), 2, "refresh must be serialized");
        assert_eq!(f.forms.lock().unwrap()[1]["refresh_token"], "secret-refresh-0");
        let file = dir.join(format!("oauth-{}.sealed", Manager::identity(&server)));
        let sealed = std::fs::read(&file).unwrap();
        assert!(!String::from_utf8_lossy(&sealed).contains("secret-"));
        let restarted = Manager::new(dir.clone());
        *restarted.key.lock().await = Some(zeroize::Zeroizing::new(vec![9; 32]));
        assert_eq!(restarted.token(&server).await.unwrap(), "secret-access-1");
        restarted.forget(&server).await.unwrap();
        assert!(restarted.token(&server).await.is_err());
        assert!(!file.exists());
        task.abort();
        std::fs::remove_dir_all(dir).unwrap();
    }
    #[tokio::test]
    async fn cancellation_wakes_waiters_and_closes_callback() {
        let (_, server, task) = fixture().await;
        let manager = Manager::new(PathBuf::from("/var/tmp"));
        let started = manager.begin(&server).await.unwrap();
        let flow = started["flow"].as_str().unwrap();
        manager.cancel(flow).await.unwrap();
        assert_eq!(manager.finished(flow, &CancellationToken::new()).await.unwrap()["state"], "cancelled");
        tokio::task::yield_now().await;
        assert_eq!(manager.status().await[0]["status"]["state"], "cancelled");
        task.abort();
    }
    #[test]
    fn authentication_configuration_roundtrips_and_rejects_unsafe_urls() {
        assert!(validate_resource("https://mcp.slack.com/mcp", "https://mcp.slack.com").is_ok());
        assert!(validate_resource("https://host/mcp/child", "https://host/mcp").is_ok());
        assert!(validate_resource("https://host/mcp", "https://host/other").is_err());
        assert!(validate_resource("https://host/mcp", "https://evil").is_err());
        for auth in [json!({"kind":"oauth","client_id":"client","scopes":["read"]}), json!({"kind":"bearer_env","variable":"TOKEN"}), json!({"kind":"password","reference":"op://vault/item/password"})]
        {
            let parsed: Auth = serde_json::from_value(auth).unwrap();
            serde_json::from_value::<Auth>(serde_json::to_value(parsed).unwrap()).unwrap();
        }
        for url in ["http://example.com/mcp", "https://user:secret@example.com/mcp", "https://example.com/mcp#fragment", "file:///private"] {
            assert!(secure_url(url).is_err());
        }
        for url in ["http://127.0.0.1:8080/mcp", "https://example.com/mcp"] {
            assert!(secure_url(url).is_ok());
        }
    }
}

#[cfg(test)]
mod provider_quirks {
    use super::*;
    #[test]
    fn only_slacks_official_endpoint_can_return_user_tokens_without_expiry() {
        let mut credential = Credential {
            access_token: String::new(),
            refresh_token: None,
            expires: 0,
            client_id: "client".into(),
            client_secret: None,
            issuer: "https://mcp.slack.com".into(),
            token_endpoint: "https://slack.com/api/oauth.v2.user.access".into(),
            resource: "https://mcp.slack.com".into(),
            auth_method: "none".into(),
        };
        update(&mut credential, json!({"ok":true,"access_token":"fixture-token","token_type":"user"})).unwrap();
        assert_eq!(credential.expires, u64::MAX);
        credential.issuer = "https://other.example".into();
        assert!(update(&mut credential, json!({"access_token":"fixture-token","token_type":"user"})).is_err());
        assert!(update(&mut credential, json!({"ok":false,"access_token":"fixture-token","token_type":"Bearer"})).is_err());
    }
}
