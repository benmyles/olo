//! Error types for the native Rex protocol client.

use thiserror::Error;

/// Structured error returned by the Rex server over `rex.control.v1`.
#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
pub struct ApiError {
    pub code: String,
    pub message: String,
    #[serde(default)]
    pub status: u16,
}

impl std::fmt::Display for ApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} ({}): {}", self.code, self.status, self.message)
    }
}

/// Reason code sent in a binary `REJECT` (`0x82 <reason>`) frame on `/api/data`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DataRejectReason {
    InvalidOrExpiredTicket = 0x01,
    AlreadyConnected = 0x02,
    StreamUnavailable = 0x03,
    LimitReached = 0x04,
    MalformedHandshake = 0x05,
    ServerFailed = 0x06,
    WrongSubprotocol = 0x07,
    Unknown = 0xff,
}

impl DataRejectReason {
    pub fn from_byte(b: u8) -> Self {
        match b {
            0x01 => Self::InvalidOrExpiredTicket,
            0x02 => Self::AlreadyConnected,
            0x03 => Self::StreamUnavailable,
            0x04 => Self::LimitReached,
            0x05 => Self::MalformedHandshake,
            0x06 => Self::ServerFailed,
            0x07 => Self::WrongSubprotocol,
            _ => Self::Unknown,
        }
    }
}

#[derive(Debug, Error)]
pub enum RexError {
    #[error("rex api error: {0}")]
    Api(ApiError),
    #[error("transport i/o error: {0}")]
    Io(#[from] std::io::Error),
    #[error("websocket error: {0}")]
    WebSocket(#[source] tungstenite::Error),
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("ssh error: {0}")]
    Ssh(String),
    /// The host let nobody in with what was offered (`user@host:port`):
    /// no key of the agent's, no key file, no password asked for.
    #[error("ssh error: SSH authentication failed for {0}")]
    SshRefused(String),
    #[error("tls error: {0}")]
    Tls(String),
    #[error("protocol error: {0}")]
    Protocol(String),
    #[error("data handshake rejected: {0:?}")]
    DataRejected(DataRejectReason),
    #[error("connection closed: {0}")]
    Closed(String),
    #[error("message size {size} exceeds server limit {limit}")]
    MessageTooLarge { size: usize, limit: usize },
    #[error("request timed out")]
    Timeout,
    #[error("invalid endpoint: {0}")]
    InvalidEndpoint(String),
    #[error("local rex server unavailable: {0}")]
    ServerUnavailable(String),
}

impl From<tungstenite::Error> for RexError {
    fn from(error: tungstenite::Error) -> Self {
        use std::io::ErrorKind;
        use tungstenite::{Error, error::ProtocolError};
        match &error {
            Error::ConnectionClosed | Error::AlreadyClosed | Error::Protocol(ProtocolError::ResetWithoutClosingHandshake) => Self::Closed(error.to_string()),
            Error::Io(io) if matches!(io.kind(), ErrorKind::ConnectionReset | ErrorKind::ConnectionAborted | ErrorKind::BrokenPipe | ErrorKind::UnexpectedEof) => Self::Closed(error.to_string()),
            _ => Self::WebSocket(error),
        }
    }
}
