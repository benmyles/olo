//! Native async Rust Rex protocol client (API revision 19).
//!
//! Covers:
//! - Control plane (`rex.control.v1`) with uncompressed `< 512 B` `hello`,
//!   `session.bootstrap`, `seq`/`revision` reconciliation, keepalive pings,
//!   and typed RPC methods for every server, session, host, Tailscale, client,
//!   keymap, prompt, terminal, tunnel, and onboarding API.
//! - Binary data plane (`rex.data.terminal.v1` and `rex.data.bytes.v1`) with
//!   `HELLO`/`RESUME`/`WELCOME`/`REJECT`, exact byte accounting, nonce-verified
//!   inline APC demuxing, and 0-byte half-close EOF.
//! - Multi-transport connectivity: local Unix socket discovery & autostart,
//!   explicit `unix://`, `http://`, `https://`/Tailscale TLS, and native Rust
//!   `ssh://` (`russh` + `ssh2-config` + `SSH_AUTH_SOCK` + `known_hosts` +
//!   `direct-streamlocal@openssh.com` + `rex __internal dial-stdio` fallback).
//! - Bidirectional `-L` (`dial`) and `-R` (`listen`) port-forwarding tunnels.

pub mod accounts;
pub mod carry;
pub mod control;
pub mod data;
pub mod error;
pub mod helper;
pub mod manage;
pub mod model;
pub mod plain;
pub mod provision;
pub mod quic;
pub mod ready;
pub mod repos;
pub mod sessions;
pub mod ssh;
pub mod transport;
pub mod tune;
pub mod tunnel;

pub use control::{Asked, ControlClient, RpcResult, answered};
pub use data::{BytesDataStream, DataWelcome, DemuxItem, InlineApcDemuxer, TerminalDataStream, TerminalStreamFrame, TerminalUpgrade};
pub use error::{ApiError, DataRejectReason, RexError};
pub use model::*;
pub use provision::ServerMode;
pub use ssh::{AgentKey, AgentStatus, SshAskpass, SshSessionPool, SshTarget, inspect_ssh_agent};
pub use transport::{RexEndpoint, RexStream, discover_or_start_local_socket, find_rex_binary, local_socket_candidates};
pub use tunnel::{LocalForwardHandle, open_bytes_stream, relay_reverse_to_tcp, relay_stream_with_bytes_ws, resume_local_forward, start_local_forward};

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;

    #[test]
    fn inline_apc_demuxer_verifies_nonce_and_passes_spoofed_through() {
        let mut demux = InlineApcDemuxer::new(12345);
        // Split across arbitrary chunk boundaries, with a spoofed wrong-nonce APC too!
        let stream = concat!(
            "hello ",
            "\x1b_rex1;99999;{\"type\":\"resize\",\"cols\":10,\"rows\":10}\x1b\\",
            "\x1b_rex1;12345;{\"type\":\"resize\",\"cols\":120,\"rows\":40,\"cell_width_px\":9,\"cell_height_px\":18}\x1b\\",
            "world",
            "\x1b_rex1;12345;{\"type\":\"child_exited\",\"exit_code\":0}\x1b\\",
        );
        let mut items = Vec::new();
        for ch in stream.as_bytes().chunks(7) {
            items.extend(demux.push(ch));
        }
        let mut vt_all = Vec::new();
        let mut apcs = Vec::new();
        for it in items {
            match it {
                DemuxItem::Vt(b) => vt_all.extend_from_slice(&b),
                DemuxItem::Apc(a) => apcs.push(a),
            }
        }
        let vt_str = String::from_utf8_lossy(&vt_all);
        assert!(vt_str.contains("hello "));
        assert!(vt_str.contains("rex1;99999;")); // spoofed passed untouched to VT
        assert!(vt_str.contains("world"));
        assert!(!vt_str.contains("12345"));
        assert_eq!(apcs.len(), 2);
        assert_eq!(apcs[0], InlineApc::Resize { cols: 120, rows: 40, cell_width_px: 9, cell_height_px: 18, owner: None });
        assert_eq!(apcs[1], InlineApc::ChildExited { exit_code: 0 });
    }

    #[test]
    fn endpoint_parsing_covers_all_schemes() {
        let pool = Arc::new(SshSessionPool::default());
        assert!(matches!(RexEndpoint::parse("local", pool.clone(), None).unwrap(), RexEndpoint::Local { .. }));
        assert!(matches!(RexEndpoint::parse("unix:///tmp/a.sock", pool.clone(), None).unwrap(), RexEndpoint::Unix(_)));
        assert!(matches!(RexEndpoint::parse("http://127.0.0.1:7297", pool.clone(), None).unwrap(), RexEndpoint::Http { port: 7297, .. }));
        assert!(matches!(RexEndpoint::parse("https://box.tail1234.ts.net", pool.clone(), None).unwrap(), RexEndpoint::Https { port: 443, .. }));
        assert!(matches!(RexEndpoint::parse("box.tail1234.ts.net", pool.clone(), None).unwrap(), RexEndpoint::Https { port: 443, .. }));
        assert!(matches!(RexEndpoint::parse("ssh://alice@devbox:2222", pool.clone(), None).unwrap(), RexEndpoint::Ssh { .. }));
        assert!(matches!(RexEndpoint::parse("alice@devbox", pool.clone(), None).unwrap(), RexEndpoint::Ssh { .. }));
    }
}
