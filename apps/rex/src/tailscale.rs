//! Mutation replies carry a watermark. Events after it may already have
//! reached the workspace when the worker returns from the request.
use olo_rex_client::TailscaleStatus;

#[derive(Clone, PartialEq, Eq)]
pub enum Action {
    Enable,
    Disable,
    Login,
    OpenLink(String),
}

impl Action {
    fn problem(&self, reason: &str) -> String {
        let reason = reason.trim().trim_end_matches('.');
        match self {
            Self::Enable => olo_kit::tr!("Couldn’t turn on Tailscale: {}.", reason),
            Self::Disable => olo_kit::tr!("Couldn’t turn off Tailscale: {}.", reason),
            Self::Login => olo_kit::tr!("Couldn’t get a sign-in link: {}.", reason),
            Self::OpenLink(_) => olo_kit::tr!("Couldn’t open the sign-in page: {}.", reason),
        }
    }

    pub fn failed(&self, error: &olo_rex_client::RexError) -> (String, Option<String>) {
        use olo_rex_client::RexError;
        let (reason, detail) = match error {
            RexError::Api(e) if e.status == 403 || e.code == "permission_denied" => ("permission was denied", Some(e.message.clone())),
            RexError::Api(e) => return self.server_problem(&e.message),
            RexError::Timeout => ("the server took too long to respond", None),
            RexError::Closed(_) | RexError::ServerUnavailable(_) => ("the Rex server disconnected", None),
            _ => ("the server couldn’t complete the request", Some(error.to_string())),
        };
        let detail =
            detail.filter(|s| !s.trim().trim_end_matches('.').eq_ignore_ascii_case(reason) && !s.trim().trim_end_matches('.').eq_ignore_ascii_case("permission denied") && !s.trim().is_empty());
        (self.problem(olo_kit::words::tr(reason)), detail)
    }

    pub fn server_problem(&self, message: &str) -> (String, Option<String>) {
        let msg = message.trim();
        if msg.is_empty() {
            return (self.problem(olo_kit::words::tr("the server couldn’t complete the request")), None);
        }
        if msg.len() > 100 || msg.contains(['\n', ':']) { (self.problem(olo_kit::words::tr("the server couldn’t complete the request")), Some(msg.into())) } else { (self.problem(msg), None) }
    }
}

pub fn state_line(status: Option<&TailscaleStatus>, connected: bool) -> String {
    if !connected {
        return olo_kit::words::tr("The Rex server isn’t connected.").into();
    }
    let Some(s) = status else { return olo_kit::words::tr("Tailscale status is unavailable.").into() };
    match s.state.as_str() {
        "disabled" | "stopped" => olo_kit::words::tr("Only this computer can reach these sessions.").into(),
        "starting" => olo_kit::words::tr("Starting Tailscale…").into(),
        "needs_login" => olo_kit::words::tr("Sign in to connect this server to Tailscale.").into(),
        "running" | "connected" => s
            .dns_name
            .as_deref()
            .filter(|v| !v.is_empty())
            .or(s.tailnet_name.as_deref())
            .map(|name| olo_kit::tr!("On the tailnet as {}.", name.trim_end_matches('.')))
            .unwrap_or_else(|| olo_kit::words::tr("Connected to Tailscale.").into()),
        _ => s.error.clone().unwrap_or_else(|| olo_kit::tr!("Tailscale: {}.", s.state)),
    }
}

pub fn apply(slot: &mut Option<TailscaleStatus>, watermark: &mut u64, seq: u64, status: TailscaleStatus) -> bool {
    if seq < *watermark {
        return false;
    }
    *watermark = seq;
    *slot = Some(status);
    true
}

pub fn login_link(status: &TailscaleStatus) -> Option<&str> {
    let url = status.auth_url.as_deref()?;
    (status.state == "needs_login" && url.strip_prefix("https://").is_some_and(|s| !s.is_empty() && !s.starts_with('/')) && !url.chars().any(char::is_whitespace)).then_some(url)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn shows_the_server_state_and_only_additional_error_detail() {
        use olo_rex_client::{ApiError, RexError};
        let status = TailscaleStatus { state: "running".into(), dns_name: Some("studio.example.".into()), ..Default::default() };
        assert_eq!(state_line(Some(&status), true), "On the tailnet as studio.example.");
        assert_eq!(state_line(Some(&status), false), "The Rex server isn’t connected.");
        let denied = |message: &str| RexError::Api(ApiError { status: 403, code: "permission_denied".into(), message: message.into() });
        assert_eq!(Action::Enable.failed(&denied("Permission denied.")), ("Couldn’t turn on Tailscale: permission was denied.".into(), None));
        assert_eq!(Action::Enable.failed(&denied("Requires administrator approval.")).1.as_deref(), Some("Requires administrator approval."));
        assert_eq!(Action::Login.failed(&RexError::Timeout).1, None);
    }
    #[test]
    fn a_late_starting_reply_cannot_hide_the_sign_in_action() {
        let mut slot = None;
        let mut seq = 0;
        let status = |state: &str, url: Option<&str>| TailscaleStatus { state: state.into(), auth_url: url.map(str::to_owned), ..Default::default() };
        assert!(apply(&mut slot, &mut seq, 2, status("needs_login", None)));
        assert!(!apply(&mut slot, &mut seq, 1, status("starting", None)));
        assert!(apply(&mut slot, &mut seq, 2, status("needs_login", Some("https://example.invalid/fixture"))));
        assert_eq!(login_link(slot.as_ref().unwrap()), Some("https://example.invalid/fixture"));
        assert!(apply(&mut slot, &mut seq, 3, status("disabled", None)));
        assert!(login_link(slot.as_ref().unwrap()).is_none());
    }
}
