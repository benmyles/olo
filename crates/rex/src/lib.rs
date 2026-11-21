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

pub mod control;
pub mod data;
pub mod error;
pub mod model;
pub mod ssh;
pub mod transport;
pub mod tunnel;

pub use control::{ControlClient, RpcResult};
pub use data::{BytesDataStream, DataWelcome, DemuxItem, InlineApcDemuxer, TerminalDataStream, TerminalStreamFrame};
pub use error::{ApiError, DataRejectReason, RexError};
pub use model::*;
pub use ssh::{AgentStatus, SshAskpass, SshSessionPool, SshTarget, inspect_ssh_agent};
pub use transport::{RexEndpoint, RexStream, discover_or_start_local_socket, find_rex_binary, local_socket_candidates};
pub use tunnel::{LocalForwardHandle, open_bytes_stream, relay_reverse_to_tcp, relay_stream_with_bytes_ws, start_local_forward};

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

    #[tokio::test]
    async fn live_scratch_rex_server_control_terminal_snapshot_and_bytes_roundtrip() {
        let rex_bin = find_rex_binary();
        if !rex_bin.exists() {
            return;
        }
        let dir = std::path::PathBuf::from(format!("/var/tmp/olo-rex-client-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let sock = dir.join("rex.sock");
        let state_dir = dir.join("state");
        let test_file = dir.join("sample.txt");
        std::fs::write(&test_file, b"hello from olo rex download test\n").unwrap();

        let mut child = tokio::process::Command::new(&rex_bin)
            .args(["server", "run", "--listen", &format!("unix://{}", sock.display()), "--state-dir", &state_dir.display().to_string(), "--restore=false"])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .expect("spawn scratch rex server");

        let mut ready = false;
        for _ in 0..50 {
            if sock.exists() {
                ready = true;
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        assert!(ready, "scratch rex server socket never appeared");

        let result = async {
            let ep = RexEndpoint::Unix(sock.clone());
            let (stream, host_hdr) = ep.connect_stream().await?;
            let (ctrl, mut _ev_rx) = ControlClient::connect(stream, &host_hdr, ClientIdentity::olo_rex(None), None).await?;
            assert_eq!(ctrl.api_revision(), REX_API_REVISION);

            let created = ctrl.session_create(Some("it-test"), None).await?;
            let sid = created.payload["session_id"].as_str().unwrap().to_owned();
            let bid = created.payload["initial_windows"][0]["block_ids"][0].as_str().unwrap().to_owned();

            let boot = ctrl
                .session_bootstrap(
                    "all",
                    Some(&sid),
                    None,
                    &[DataTicketRequest { creator_name: "com.superlogical.terminal".into(), data_ref_id: "terminal".into(), subprotocol: Some(DATA_TERMINAL_SUBPROTOCOL.into()) }],
                )
                .await?;
            let ticket = boot.payload.data_tickets.as_ref().and_then(|t| t.first()).map(|t| t.ticket.clone()).expect("bootstrap data ticket");

            let (dstream, dhost) = ep.connect_stream().await?;
            let mut term_ws = TerminalDataStream::connect_hello(dstream, &dhost, &ticket).await?;
            assert_ne!(term_ws.welcome.channel_nonce, 0);

            // Receive initial GHOSTSNP snapshot and verify live bytes arrive
            let mut snap_buf = Vec::new();
            let mut got_ghostsnp = false;
            for _ in 0..10 {
                match tokio::time::timeout(std::time::Duration::from_secs(2), term_ws.next_frame()).await {
                    Ok(Ok(TerminalStreamFrame::Bytes(b))) => {
                        snap_buf.extend_from_slice(&b);
                        if snap_buf.starts_with(b"GHOSTSNP") {
                            got_ghostsnp = true;
                            break;
                        }
                    }
                    _ => break,
                }
            }
            assert!(got_ghostsnp, "expected GHOSTSNP header on terminal data stream");
            term_ws.close_clean().await;

            // Verify rex.data.bytes.v1 download with 0-byte half-close
            let dl = ctrl.terminal_download_begin(&sid, &bid, &format!("download:{}", uuid::Uuid::new_v4()), test_file.to_str().unwrap(), None).await?;
            let dl_ticket = dl.payload.ticket.expect("download ticket");
            let mut bws = open_bytes_stream(&ep, &dl_ticket).await?;
            let mut downloaded = Vec::new();
            while let Some(chunk) = bws.read_chunk().await? {
                downloaded.extend_from_slice(&chunk);
            }
            assert_eq!(downloaded, b"hello from olo rex download test\n");

            ctrl.session_destroy(&sid).await?;
            ctrl.close();
            Ok::<(), RexError>(())
        }
        .await;

        let _ = child.kill().await;
        let _ = child.wait().await;
        let _ = std::fs::remove_dir_all(&dir);
        result.expect("live scratch rex server test failed");
    }
}
