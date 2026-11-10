//! GitHub REST transport. Credentials stay in this module, redirects are refused,
//! writes are never retried, and responses are bounded before allocation.
use crate::{model::ToolCall, tools};
use anyhow::{Context, Result, bail, ensure};
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

pub async fn run(call: &ToolCall, cancel: &CancellationToken) -> Result<Value> {
    let credentials = tokio::task::spawn_blocking(olo_system::github::token).await?;
    request("https://api.github.com", credentials.as_ref().map(|c| c.0.as_str()), call, cancel).await
}
#[doc(hidden)]
pub async fn request(base: &str, token: Option<&str>, call: &ToolCall, cancel: &CancellationToken) -> Result<Value> {
    let a = &call.arguments;
    let repository = tools::string(a, "repository")?;
    let parts: Vec<_> = repository.split('/').collect();
    ensure!(parts.len() == 2 && parts.iter().all(|s| !s.is_empty() && *s != "." && *s != ".." && s.bytes().all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b))), "repository must be OWNER/REPO");
    let op = tools::string(a, "operation")?;
    let n = || a["number"].as_u64().filter(|n| *n > 0).context("positive issue or pull request number required");
    let (method, path) = match op {
        "repository" => ("GET", String::new()),
        "pulls" | "issues" => ("GET", format!("/{op}")),
        "pull" => ("GET", format!("/pulls/{}", n()?)),
        "issue" => ("GET", format!("/issues/{}", n()?)),
        "comments" => ("GET", format!("/issues/{}/comments", n()?)),
        "files" | "reviews" => ("GET", format!("/pulls/{}/{op}", n()?)),
        "checks" => {
            let revision = tools::string(a, "revision")?;
            ensure!((7..=64).contains(&revision.len()) && revision.bytes().all(|b| b.is_ascii_hexdigit()), "checks require a commit SHA");
            ("GET", format!("/commits/{revision}/check-runs"))
        }
        "create_pull" => ("POST", "/pulls".into()),
        "create_issue" => ("POST", "/issues".into()),
        "comment" => ("POST", format!("/issues/{}/comments", n()?)),
        "review" => ("POST", format!("/pulls/{}/reviews", n()?)),
        "update_pull" => ("PATCH", format!("/pulls/{}", n()?)),
        "merge" => ("PUT", format!("/pulls/{}/merge", n()?)),
        _ => bail!("unknown GitHub operation {op}"),
    };
    ensure!(method == "GET" || call.name == "github_write", "publishing requires github_write");
    let mut body = a["body"].clone();
    if method != "GET" {
        ensure!(token.is_some(), "sign in through Preferences → Integrations → GitHub before publishing");
        ensure!(body.is_object(), "body must be a JSON object");
        if op == "create_pull" && body.get("draft").is_none() {
            body["draft"] = true.into();
        }
        if op == "merge" {
            ensure!(body["sha"].as_str().is_some_and(|v| v.len() >= 40 && v.bytes().all(|c| c.is_ascii_hexdigit())), "merge requires the expected head sha");
        }
        ensure!(body.to_string().len() <= 1_000_000, "request body exceeds 1 MB");
    }
    let client = reqwest::Client::builder().user_agent("olo-coder").redirect(reqwest::redirect::Policy::none()).no_proxy().timeout(std::time::Duration::from_secs(60)).build()?;
    let mut req = client.request(method.parse()?, format!("{base}/repos/{repository}{path}")).header("Accept", "application/vnd.github+json").header("X-GitHub-Api-Version", "2026-03-10");
    if let Some(token) = token {
        req = req.bearer_auth(token);
    }
    if method == "GET" {
        let page = a["page"].as_u64().unwrap_or(1).max(1);
        req = req.query(&[("per_page", 100), ("page", page)]);
        if let Some(state) = a["state"].as_str() {
            ensure!(matches!(state, "open" | "closed" | "all"), "invalid state");
            req = req.query(&[("state", state)]);
        }
    } else {
        req = req.json(&body);
    }
    let future = async {
        let mut response = req.send().await?;
        let status = response.status();
        let next = response.headers().get("link").and_then(|v| v.to_str().ok()).is_some_and(|v| v.contains("rel=\"next\""));
        let remaining = response.headers().get("x-ratelimit-remaining").and_then(|v| v.to_str().ok()).unwrap_or("").to_owned();
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await? {
            ensure!(bytes.len() + chunk.len() <= 4 * 1024 * 1024, "GitHub response exceeds 4 MB; request a narrower page");
            bytes.extend_from_slice(&chunk);
        }
        let data: Value = serde_json::from_slice(&bytes).unwrap_or_else(|_| json!({"message":tools::clip(&String::from_utf8_lossy(&bytes),4000)}));
        ensure!(status.is_success(), "GitHub HTTP {}: {}", status.as_u16(), data["message"].as_str().unwrap_or("request failed"));
        Ok(json!({"repository":repository,"operation":op,"data":data,"has_next_page":next,"rate_limit_remaining":remaining}))
    };
    tokio::select! { r=future=>r, _=cancel.cancelled()=>bail!("cancelled; if this was a publishing operation, inspect GitHub before retrying") }
}
