//! The helper's own link over UDP, both ends on this computer: the
//! program in its `quic` mode, dialling `cat` for each stream (what is
//! sent comes back), and the app's side of it. No SSH between: the
//! token goes to the helper's input and its word comes from its
//! output, as over an SSH channel.

use std::process::Stdio;
use std::time::Duration;

use olo_rex_client::quic::{QuicLink, announced, hex, new_token};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};

struct Helper {
    child: tokio::process::Child,
    input: Option<tokio::process::ChildStdin>,
    to: std::net::SocketAddr,
    print: String,
}

async fn helper(token: &[u8]) -> Helper {
    let mut child = tokio::process::Command::new(env!("CARGO_BIN_EXE_olo-rex-link")).args(["quic", "--", "cat"]).stdin(Stdio::piped()).stdout(Stdio::piped()).kill_on_drop(true).spawn().unwrap();
    let mut input = child.stdin.take().unwrap();
    input.write_all(format!("{}\n", hex(token)).as_bytes()).await.unwrap();
    let mut line = String::new();
    tokio::time::timeout(Duration::from_secs(10), BufReader::new(child.stdout.take().unwrap()).read_line(&mut line)).await.expect("the helper's word").unwrap();
    let (port, print) = announced(&line).unwrap_or_else(|| panic!("the helper said {line:?}"));
    assert!(olo_rex_link::quic_wire::PORTS.contains(&port) || port > 1024, "{port}");
    Helper { child, input: Some(input), to: (std::net::Ipv4Addr::LOCALHOST, port).into(), print }
}

#[tokio::test(flavor = "multi_thread")]
async fn streams_over_the_link_reach_what_is_dialled_each_by_itself() {
    let token = new_token().unwrap();
    let mut h = helper(&token).await;
    let began = std::time::Instant::now();
    let link = QuicLink::connect(h.to, &h.print, token).await.expect("the link");
    println!("the link: up in {:.1} ms on this computer", began.elapsed().as_secs_f64() * 1000.0);
    assert!(link.alive());

    // One stream: what is sent comes back, and its end ends it.
    let mut one = link.open(false).await.unwrap();
    one.write_all(b"hello\n").await.unwrap();
    let mut back = [0u8; 6];
    tokio::time::timeout(Duration::from_secs(5), one.read_exact(&mut back)).await.expect("an answer").unwrap();
    assert_eq!(&back, b"hello\n");

    // Many at once, a megabyte through each, every byte in its place:
    // none waits on another.
    let link = std::sync::Arc::new(link);
    let began = std::time::Instant::now();
    let mut all = Vec::new();
    for n in 0..8u8 {
        let link = link.clone();
        all.push(tokio::spawn(async move {
            let stream = link.open(false).await.unwrap();
            let (mut from, mut to) = tokio::io::split(stream);
            let sent: Vec<u8> = (0..1_048_576usize).map(|i| (i as u8).wrapping_mul(31).wrapping_add(n)).collect();
            let expect = sent.clone();
            let writer = tokio::spawn(async move {
                to.write_all(&sent).await.unwrap();
                to.shutdown().await.unwrap();
            });
            let mut got = Vec::new();
            tokio::time::timeout(Duration::from_secs(30), from.read_to_end(&mut got)).await.expect("the whole of it back").unwrap();
            writer.await.unwrap();
            assert!(got == expect, "stream {n}: {} bytes back of {}", got.len(), expect.len());
        }));
    }
    for one in all {
        one.await.unwrap();
    }
    println!("the link: 8 streams of 1 MiB each way in {:.0} ms", began.elapsed().as_secs_f64() * 1000.0);

    // The helper lives while the channel that started it is open or a
    // link is up: with both gone, it ends by itself.
    drop(one);
    drop(link);
    drop(h.input.take());
    let ended = tokio::time::timeout(Duration::from_secs(40), h.child.wait()).await.expect("the helper ends once nothing holds it").unwrap();
    assert!(ended.success());
}

#[tokio::test(flavor = "multi_thread")]
async fn nobody_else_gets_a_stream() {
    let token = new_token().unwrap();
    let h = helper(&token).await;
    // Another token: the handshake is anybody's, the first stream nobody's.
    let other = new_token().unwrap();
    let stranger = QuicLink::connect(h.to, &h.print, other).await.expect("the handshake is anybody's");
    assert!(tokio::time::timeout(Duration::from_secs(5), stranger.hello()).await.expect("an end to it").is_err(), "a stream with another token was answered");
    assert!(!stranger.alive(), "its link is closed with it");
    // Another certificate than the one SSH vouched for: no link at all.
    let wrong = "0".repeat(64);
    assert!(QuicLink::connect(h.to, &wrong, token).await.is_err());
    // And the right ones still do.
    QuicLink::connect(h.to, &h.print, token).await.unwrap().hello().await.expect("the app's own token is answered");
}

/// QUIC `SCREEN` stream (`quic_wire::SCREEN = 3`): dispatches `olo-rex-link screen follow`
/// over the UDP link and shuts down cleanly when the stream closes.
#[cfg(feature = "screens")]
#[tokio::test(flavor = "multi_thread")]
async fn screen_stream_dispatches_over_quic() {
    use olo_rex_link::{quic_wire as wire, screen_wire};
    use std::sync::Arc;

    let scratch = std::path::PathBuf::from(format!("/var/tmp/olo-rex-link-quic-screen-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&scratch);
    std::fs::create_dir_all(scratch.join("home")).unwrap();
    std::fs::create_dir_all(scratch.join("run")).unwrap();
    std::fs::create_dir_all(scratch.join("state")).unwrap();

    let token = new_token().unwrap();
    let mut child = tokio::process::Command::new(env!("CARGO_BIN_EXE_olo-rex-link"))
        .args(["quic", "--", "cat"])
        .env("HOME", scratch.join("home"))
        .env("XDG_RUNTIME_DIR", scratch.join("run"))
        .env("XDG_STATE_HOME", scratch.join("state"))
        .env("XDG_CONFIG_HOME", scratch.join("home/.config"))
        .env("OLO_HOST_DESKTOP", scratch.join("desktop"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let mut input = child.stdin.take().unwrap();
    input.write_all(format!("{}\n", hex(&token)).as_bytes()).await.unwrap();
    let mut line = String::new();
    tokio::time::timeout(Duration::from_secs(10), BufReader::new(child.stdout.take().unwrap()).read_line(&mut line)).await.unwrap().unwrap();
    let (port, print) = announced(&line).unwrap();

    // Connect a raw quinn client pinning the helper's certificate (matches olo_rex_client::quic).
    #[derive(Debug)]
    struct Pin(String, Arc<rustls::crypto::CryptoProvider>);
    impl rustls::client::danger::ServerCertVerifier for Pin {
        fn verify_server_cert(
            &self,
            end: &rustls::pki_types::CertificateDer<'_>,
            _: &[rustls::pki_types::CertificateDer<'_>],
            _: &rustls::pki_types::ServerName<'_>,
            _: &[u8],
            _: rustls::pki_types::UnixTime,
        ) -> Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
            use sha2::Digest as _;
            let got: String = sha2::Sha256::digest(end.as_ref()).iter().map(|b| format!("{b:02x}")).collect();
            if got == self.0 { Ok(rustls::client::danger::ServerCertVerified::assertion()) } else { Err(rustls::Error::General("wrong cert".into())) }
        }
        fn verify_tls12_signature(
            &self,
            m: &[u8],
            c: &rustls::pki_types::CertificateDer<'_>,
            d: &rustls::DigitallySignedStruct,
        ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
            rustls::crypto::verify_tls12_signature(m, c, d, &self.1.signature_verification_algorithms)
        }
        fn verify_tls13_signature(
            &self,
            m: &[u8],
            c: &rustls::pki_types::CertificateDer<'_>,
            d: &rustls::DigitallySignedStruct,
        ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
            rustls::crypto::verify_tls13_signature(m, c, d, &self.1.signature_verification_algorithms)
        }
        fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
            self.1.signature_verification_algorithms.supported_schemes()
        }
    }

    let provider = Arc::new(rustls::crypto::aws_lc_rs::default_provider());
    let mut tls = rustls::ClientConfig::builder_with_provider(provider.clone())
        .with_protocol_versions(&[&rustls::version::TLS13])
        .unwrap()
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(Pin(print, provider)))
        .with_no_client_auth();
    tls.alpn_protocols = vec![wire::ALPN.to_vec()];
    let config = quinn::ClientConfig::new(Arc::new(quinn::crypto::rustls::QuicClientConfig::try_from(tls).unwrap()));
    let endpoint = quinn::Endpoint::client((std::net::Ipv4Addr::LOCALHOST, 0).into()).unwrap();
    let conn = endpoint.connect_with(config, (std::net::Ipv4Addr::LOCALHOST, port).into(), wire::NAME).unwrap().await.unwrap();

    // Open a SCREEN stream for `follow`, read initial Snapshot, then finish write side to stop cleanly.
    let (mut send, recv) = conn.open_bi().await.unwrap();
    let _ = send.set_priority(-1);
    let mut head = [0u8; wire::TOKEN + 1];
    head[..wire::TOKEN].copy_from_slice(&token);
    head[wire::TOKEN] = wire::SCREEN;
    send.write_all(&head).await.unwrap();
    let selector = screen_wire::encode_quic_screen_args(&["follow".to_owned()]).unwrap();
    send.write_all(&selector).await.unwrap();

    let mut reader = BufReader::new(recv);
    let mut snap_line = String::new();
    tokio::time::timeout(Duration::from_secs(5), reader.read_line(&mut snap_line)).await.unwrap().unwrap();
    let ev: screen_wire::FollowEvent = serde_json::from_str(snap_line.trim()).unwrap();
    assert!(matches!(ev, screen_wire::FollowEvent::Snapshot { proto: screen_wire::PROTO, .. }), "got {ev:?}");

    // Closing our send side causes `follow` to exit cleanly and close its stdout.
    send.finish().unwrap();
    let mut rest = Vec::new();
    tokio::time::timeout(Duration::from_secs(5), reader.read_to_end(&mut rest)).await.unwrap().unwrap();
    assert!(rest.is_empty());

    // Open a SCREEN stream for `history qa`, read BOTH initial Actions + Index, then close cleanly (Finding 36).
    let (mut h_send, mut h_recv) = conn.open_bi().await.unwrap();
    let _ = h_send.set_priority(-1);
    h_send.write_all(&head).await.unwrap();
    let h_sel = screen_wire::encode_quic_screen_args(&["history".to_owned(), "qa".to_owned()]).unwrap();
    h_send.write_all(&h_sel).await.unwrap();
    let mut h_head = [0u8; 5];
    tokio::time::timeout(Duration::from_secs(5), h_recv.read_exact(&mut h_head)).await.unwrap().unwrap();
    assert_eq!(h_head[0], screen_wire::MSG_HISTORY_ACTIONS);
    let h_len = u32::from_le_bytes([h_head[1], h_head[2], h_head[3], h_head[4]]) as usize;
    let mut h_body = vec![0u8; h_len];
    tokio::time::timeout(Duration::from_secs(5), h_recv.read_exact(&mut h_body)).await.unwrap().unwrap();
    assert_eq!(screen_wire::decode_history_msg(h_head[0], &h_body).unwrap(), screen_wire::HistoryMsg::Actions { reset: true, lines: vec![] });
    // Decode initial Index message too!
    tokio::time::timeout(Duration::from_secs(5), h_recv.read_exact(&mut h_head)).await.unwrap().unwrap();
    assert_eq!(h_head[0], screen_wire::MSG_HISTORY_INDEX);
    let idx_len = u32::from_le_bytes([h_head[1], h_head[2], h_head[3], h_head[4]]) as usize;
    let mut idx_body = vec![0u8; idx_len];
    tokio::time::timeout(Duration::from_secs(5), h_recv.read_exact(&mut idx_body)).await.unwrap().unwrap();
    assert_eq!(screen_wire::decode_history_msg(h_head[0], &idx_body).unwrap(), screen_wire::HistoryMsg::Index { reset: true, last_size: None, frames: vec![] });
    // Decode initial SnapshotDone barrier (Review 2 Findings 40 & 50).
    tokio::time::timeout(Duration::from_secs(5), h_recv.read_exact(&mut h_head)).await.unwrap().unwrap();
    assert_eq!(h_head[0], screen_wire::MSG_HISTORY_SNAPSHOT_DONE);
    let done_len = u32::from_le_bytes([h_head[1], h_head[2], h_head[3], h_head[4]]) as usize;
    let mut done_body = vec![0u8; done_len];
    tokio::time::timeout(Duration::from_secs(5), h_recv.read_exact(&mut done_body)).await.unwrap().unwrap();
    assert_eq!(screen_wire::decode_history_msg(h_head[0], &done_body).unwrap(), screen_wire::HistoryMsg::SnapshotDone { actions_gen: 1, index_gen: 1 });
    h_send.finish().unwrap();
    let rest = tokio::time::timeout(Duration::from_secs(5), h_recv.read_to_end(4096)).await.unwrap().unwrap();
    assert!(rest.is_empty(), "expected clean EOF on history stream, got {} trailing bytes", rest.len());

    // Disallowed subcommand over QUIC SCREEN is reset/rejected cleanly.
    let (mut bad_send, mut bad_recv) = conn.open_bi().await.unwrap();
    bad_send.write_all(&head).await.unwrap();
    let bad_json = br#"["remove"]"#;
    bad_send.write_all(&(bad_json.len() as u16).to_le_bytes()).await.unwrap();
    bad_send.write_all(bad_json).await.unwrap();
    let res = tokio::time::timeout(Duration::from_secs(5), bad_recv.read_to_end(1024)).await.unwrap();
    assert!(res.is_err() || res.unwrap().is_empty());

    conn.close(0u32.into(), b"");
    drop(input);
    let status = tokio::time::timeout(Duration::from_secs(10), child.wait()).await.expect("quic parent helper timed out").unwrap();
    assert!(status.success(), "quic parent helper exited with {status}");
    let _ = std::fs::remove_dir_all(&scratch);
}

/// Findings 7, 20, 21: unlinking the running helper binary never breaks QUIC
/// `SCREEN` child spawning (`/proc/self/exe`), host-wide concurrency limits
/// reject excess screen children without spawning, and closing/stopping a
/// stream reaps its child and returns its permit promptly.
#[cfg(feature = "screens")]
#[tokio::test(flavor = "multi_thread")]
async fn review_7_20_21_quic_unlinked_binary_admission_limits_and_teardown() {
    use olo_rex_link::{quic_wire as wire, screen_wire};
    use std::sync::Arc;

    let scratch = std::path::PathBuf::from(format!("/var/tmp/olo-rex-link-quic-rev-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&scratch);
    std::fs::create_dir_all(scratch.join("home")).unwrap();
    std::fs::create_dir_all(scratch.join("run")).unwrap();
    std::fs::create_dir_all(scratch.join("state")).unwrap();

    // Finding 20: copy helper binary, launch it, then UNLINK the file on disk!
    let copied_bin = scratch.join("olo-rex-link-ephemeral");
    std::fs::copy(env!("CARGO_BIN_EXE_olo-rex-link"), &copied_bin).unwrap();

    let token = new_token().unwrap();
    let mut child = tokio::process::Command::new(&copied_bin)
        .args(["quic", "--", "cat"])
        .env("HOME", scratch.join("home"))
        .env("XDG_RUNTIME_DIR", scratch.join("run"))
        .env("XDG_STATE_HOME", scratch.join("state"))
        .env("XDG_CONFIG_HOME", scratch.join("home/.config"))
        .env("OLO_HOST_DESKTOP", scratch.join("desktop"))
        .env("OLO_QUIC_MAX_SCREEN_FOLLOWS", "1")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let mut input = child.stdin.take().unwrap();
    input.write_all(format!("{}\n", hex(&token)).as_bytes()).await.unwrap();
    let mut line = String::new();
    tokio::time::timeout(Duration::from_secs(10), BufReader::new(child.stdout.take().unwrap()).read_line(&mut line)).await.unwrap().unwrap();
    let (port, print) = announced(&line).unwrap();

    // Unlink the helper binary while it is running (simulating `helper.rs` pruning!).
    std::fs::remove_file(&copied_bin).unwrap();
    assert!(!copied_bin.exists());

    #[derive(Debug)]
    struct Pin(String, Arc<rustls::crypto::CryptoProvider>);
    impl rustls::client::danger::ServerCertVerifier for Pin {
        fn verify_server_cert(
            &self,
            end: &rustls::pki_types::CertificateDer<'_>,
            _: &[rustls::pki_types::CertificateDer<'_>],
            _: &rustls::pki_types::ServerName<'_>,
            _: &[u8],
            _: rustls::pki_types::UnixTime,
        ) -> Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
            use sha2::Digest as _;
            let got: String = sha2::Sha256::digest(end.as_ref()).iter().map(|b| format!("{b:02x}")).collect();
            if got == self.0 { Ok(rustls::client::danger::ServerCertVerified::assertion()) } else { Err(rustls::Error::General("wrong cert".into())) }
        }
        fn verify_tls12_signature(
            &self,
            m: &[u8],
            c: &rustls::pki_types::CertificateDer<'_>,
            d: &rustls::DigitallySignedStruct,
        ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
            rustls::crypto::verify_tls12_signature(m, c, d, &self.1.signature_verification_algorithms)
        }
        fn verify_tls13_signature(
            &self,
            m: &[u8],
            c: &rustls::pki_types::CertificateDer<'_>,
            d: &rustls::DigitallySignedStruct,
        ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
            rustls::crypto::verify_tls13_signature(m, c, d, &self.1.signature_verification_algorithms)
        }
        fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
            self.1.signature_verification_algorithms.supported_schemes()
        }
    }

    let provider = Arc::new(rustls::crypto::aws_lc_rs::default_provider());
    let mut tls = rustls::ClientConfig::builder_with_provider(provider.clone())
        .with_protocol_versions(&[&rustls::version::TLS13])
        .unwrap()
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(Pin(print, provider)))
        .with_no_client_auth();
    tls.alpn_protocols = vec![wire::ALPN.to_vec()];
    let config = quinn::ClientConfig::new(Arc::new(quinn::crypto::rustls::QuicClientConfig::try_from(tls).unwrap()));
    let endpoint = quinn::Endpoint::client((std::net::Ipv4Addr::LOCALHOST, 0).into()).unwrap();
    let conn = endpoint.connect_with(config, (std::net::Ipv4Addr::LOCALHOST, port).into(), wire::NAME).unwrap().await.unwrap();

    let mut head = [0u8; wire::TOKEN + 1];
    head[..wire::TOKEN].copy_from_slice(&token);
    head[wire::TOKEN] = wire::SCREEN;
    let follow_sel = screen_wire::encode_quic_screen_args(&["follow".to_owned()]).unwrap();

    // 1. First `follow` succeeds EVEN THOUGH the binary file was unlinked!
    let (mut s1, r1) = conn.open_bi().await.unwrap();
    s1.write_all(&head).await.unwrap();
    s1.write_all(&follow_sel).await.unwrap();
    let mut reader1 = BufReader::new(r1);
    let mut snap = String::new();
    let n = tokio::time::timeout(Duration::from_secs(5), reader1.read_line(&mut snap)).await.unwrap().unwrap();
    assert!(n > 0, "first follow after binary unlink must emit Snapshot, got EOF");
    let ev: screen_wire::FollowEvent = serde_json::from_str(snap.trim()).unwrap();
    assert!(matches!(ev, screen_wire::FollowEvent::Snapshot { .. }));

    // 2. Finding 21: second `follow` exceeds OLO_QUIC_MAX_SCREEN_FOLLOWS=1 -> rejected immediately!
    let (mut s2, mut r2) = conn.open_bi().await.unwrap();
    s2.write_all(&head).await.unwrap();
    s2.write_all(&follow_sel).await.unwrap();
    let res2 = tokio::time::timeout(Duration::from_secs(3), r2.read_to_end(1024)).await.expect("second follow should fail immediately when busy");
    assert!(res2.is_err() || res2.unwrap().is_empty(), "second follow must be refused by admission limit");

    // 3. Finding 7: closing send side (even without draining recv further) stops and reaps child 1,
    //    returning the permit so a 3rd `follow` succeeds!
    s1.finish().unwrap();
    let _ = reader1.into_inner().stop(0u32.into());
    let until = std::time::Instant::now() + Duration::from_secs(5);
    let mut third_ok = false;
    while std::time::Instant::now() < until {
        let (mut s3, r3) = conn.open_bi().await.unwrap();
        s3.write_all(&head).await.unwrap();
        s3.write_all(&follow_sel).await.unwrap();
        let mut reader3 = BufReader::new(r3);
        let mut line3 = String::new();
        if let Ok(Ok(k)) = tokio::time::timeout(Duration::from_secs(2), reader3.read_line(&mut line3)).await
            && k > 0
            && serde_json::from_str::<screen_wire::FollowEvent>(line3.trim()).is_ok()
        {
            s3.finish().unwrap();
            let mut rest3 = Vec::new();
            let _ = tokio::time::timeout(Duration::from_secs(3), reader3.read_to_end(&mut rest3)).await;
            third_ok = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(third_ok, "permit was not returned after stopping stalled follow stream");

    conn.close(0u32.into(), b"");
    drop(input);
    let status = tokio::time::timeout(Duration::from_secs(10), child.wait()).await.expect("quic parent timed out").unwrap();
    assert!(status.success(), "quic parent exited with {status}");
    let _ = std::fs::remove_dir_all(&scratch);
}

/// Finding 24: `olo-rex-link quic` must reject an oversized token line (>256 bytes)
/// immediately without unbounded buffering, and after startup must discard arbitrary
/// stdin bytes into a fixed buffer rather than retaining them in memory.
#[tokio::test]
async fn review2_w1_quic_bounded_token_and_stdin_discard() {
    use olo_rex_link::quic_wire as wire;

    let mut bad = tokio::process::Command::new(env!("CARGO_BIN_EXE_olo-rex-link"))
        .args(["quic", "--", "cat"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let mut bad_in = bad.stdin.take().unwrap();
    // Send 8 KiB WITHOUT a newline: bounded token reader must fail immediately while stdin is still open!
    let _ = bad_in.write_all(&vec![b'a'; 8192]).await;
    let _ = bad_in.flush().await;
    let status = tokio::time::timeout(Duration::from_secs(3), bad.wait()).await.expect("Finding 24: quic helper hung waiting for newline on oversized token line").unwrap();
    assert!(!status.success(), "oversized token line must exit with error");
    drop(bad_in);

    // Valid token followed by 32 MiB of extra bytes must NOT retain them in RSS and must exit cleanly on EOF.
    let token = [0x5au8; wire::TOKEN];
    let mut good = tokio::process::Command::new(env!("CARGO_BIN_EXE_olo-rex-link"))
        .args(["quic", "--", "cat"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let pid = good.id().unwrap();
    let mut good_in = good.stdin.take().unwrap();
    good_in.write_all(format!("{}\n", hex(&token)).as_bytes()).await.unwrap();
    good_in.flush().await.unwrap();
    let mut line = String::new();
    tokio::time::timeout(Duration::from_secs(5), BufReader::new(good.stdout.take().unwrap()).read_line(&mut line)).await.unwrap().unwrap();
    assert!(announced(&line).is_some(), "expected QUIC announcement, got {line:?}");
    let rss_before = std::fs::read_to_string(format!("/proc/{pid}/statm")).ok().and_then(|s| s.split_whitespace().nth(1)?.parse::<u64>().ok()).unwrap_or(0) * 4096;
    let chunk = vec![0x42u8; 64 * 1024];
    for _ in 0..512 {
        good_in.write_all(&chunk).await.unwrap();
    }
    good_in.flush().await.unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;
    let rss_after = std::fs::read_to_string(format!("/proc/{pid}/statm")).ok().and_then(|s| s.split_whitespace().nth(1)?.parse::<u64>().ok()).unwrap_or(0) * 4096;
    let rss_growth = rss_after.saturating_sub(rss_before);
    assert!(rss_growth < 16 * 1024 * 1024, "Finding 24: streaming 32 MiB to quic stdin grew RSS by {rss_growth} bytes (unbounded read_to_end retention!)");
    drop(good_in);
    let status = tokio::time::timeout(Duration::from_secs(5), good.wait()).await.unwrap().unwrap();
    assert!(status.success(), "quic helper should exit cleanly after discarding stdin: {status}");
}

/// Round 3 Req 4: QUIC `wire::SCREEN` dispatch refuses `--expected-started` /
/// `--expected-last-used` and one-shot mutating actions (`stop`, `delete`,
/// `history NAME --clear`) on every QUIC stream without touching host state.
#[cfg(feature = "screens")]
#[tokio::test(flavor = "multi_thread")]
async fn round3_req4_quic_dispatch_refuses_expected_options_and_mutations() {
    use olo_rex_link::quic_wire as wire;
    use std::sync::Arc;

    let scratch = std::path::PathBuf::from(format!("/var/tmp/olo-rex-link-quic-r3-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&scratch);
    let sdir = scratch.join("state/olo/screens/qa");
    std::fs::create_dir_all(scratch.join("home")).unwrap();
    std::fs::create_dir_all(scratch.join("run")).unwrap();
    std::fs::create_dir_all(&sdir).unwrap();
    std::fs::write(sdir.join("screen.json"), r#"{"width":800,"height":600,"scale":1.0,"sound":false}"#).unwrap();
    std::fs::write(sdir.join("actions.jsonl"), "{\"at\":5000,\"ms\":10,\"kind\":\"click\"}\n").unwrap();

    let token = new_token().unwrap();
    let mut child = tokio::process::Command::new(env!("CARGO_BIN_EXE_olo-rex-link"))
        .args(["quic", "--", "cat"])
        .env("HOME", scratch.join("home"))
        .env("XDG_RUNTIME_DIR", scratch.join("run"))
        .env("XDG_STATE_HOME", scratch.join("state"))
        .env("XDG_CONFIG_HOME", scratch.join("home/.config"))
        .env("OLO_HOST_DESKTOP", scratch.join("desktop"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let mut input = child.stdin.take().unwrap();
    input.write_all(format!("{}\n", hex(&token)).as_bytes()).await.unwrap();
    let mut line = String::new();
    tokio::time::timeout(Duration::from_secs(10), BufReader::new(child.stdout.take().unwrap()).read_line(&mut line)).await.unwrap().unwrap();
    let (port, print) = announced(&line).unwrap();

    #[derive(Debug)]
    struct Pin(String, Arc<rustls::crypto::CryptoProvider>);
    impl rustls::client::danger::ServerCertVerifier for Pin {
        fn verify_server_cert(
            &self,
            end: &rustls::pki_types::CertificateDer<'_>,
            _: &[rustls::pki_types::CertificateDer<'_>],
            _: &rustls::pki_types::ServerName<'_>,
            _: &[u8],
            _: rustls::pki_types::UnixTime,
        ) -> Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
            use sha2::Digest as _;
            let got: String = sha2::Sha256::digest(end.as_ref()).iter().map(|b| format!("{b:02x}")).collect();
            if got == self.0 { Ok(rustls::client::danger::ServerCertVerified::assertion()) } else { Err(rustls::Error::General("wrong cert".into())) }
        }
        fn verify_tls12_signature(
            &self,
            m: &[u8],
            c: &rustls::pki_types::CertificateDer<'_>,
            d: &rustls::DigitallySignedStruct,
        ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
            rustls::crypto::verify_tls12_signature(m, c, d, &self.1.signature_verification_algorithms)
        }
        fn verify_tls13_signature(
            &self,
            m: &[u8],
            c: &rustls::pki_types::CertificateDer<'_>,
            d: &rustls::DigitallySignedStruct,
        ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
            rustls::crypto::verify_tls13_signature(m, c, d, &self.1.signature_verification_algorithms)
        }
        fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
            self.1.signature_verification_algorithms.supported_schemes()
        }
    }

    let provider = Arc::new(rustls::crypto::aws_lc_rs::default_provider());
    let mut tls = rustls::ClientConfig::builder_with_provider(provider.clone())
        .with_protocol_versions(&[&rustls::version::TLS13])
        .unwrap()
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(Pin(print, provider)))
        .with_no_client_auth();
    tls.alpn_protocols = vec![wire::ALPN.to_vec()];
    let config = quinn::ClientConfig::new(Arc::new(quinn::crypto::rustls::QuicClientConfig::try_from(tls).unwrap()));
    let endpoint = quinn::Endpoint::client((std::net::Ipv4Addr::LOCALHOST, 0).into()).unwrap();
    let conn = endpoint.connect_with(config, (std::net::Ipv4Addr::LOCALHOST, port).into(), wire::NAME).unwrap().await.unwrap();

    let mut head = [0u8; wire::TOKEN + 1];
    head[..wire::TOKEN].copy_from_slice(&token);
    head[wire::TOKEN] = wire::SCREEN;

    let rejected_cases: &[&[&str]] = &[
        &["stop", "qa", "--expected-started", "1000"],
        &["stop", "qa", "--expected-last-used", "5010"],
        &["delete", "qa", "--expected-started", "0", "--expected-last-used", "5010"],
        &["history", "qa", "--clear", "--expected-last-used", "5010"],
        &["history", "qa", "--expected-last-used", "5010"],
        &["history-stream", "qa", "--expected-last-used", "5010"],
        &["follow", "--expected-started", "1000"],
        &["stream", "qa", "--expected-started", "1000"],
        &["export", "qa", "--expected-last-used", "5010"],
    ];
    for case in rejected_cases {
        let body = serde_json::to_vec(case).unwrap();
        let (mut send, mut recv) = conn.open_bi().await.unwrap();
        send.write_all(&head).await.unwrap();
        send.write_all(&(body.len() as u16).to_le_bytes()).await.unwrap();
        send.write_all(&body).await.unwrap();
        let res = tokio::time::timeout(Duration::from_secs(3), recv.read_to_end(1024)).await.unwrap();
        assert!(matches!(res, Err(quinn::ReadToEndError::Read(quinn::ReadError::Reset(code))) if code == 2u32.into()), "QUIC dispatch must reset {case:?} with selector-rejection code 2, got {res:?}");
    }

    // Confirm a valid `follow` selector on the SAME connection still works cleanly after rejections.
    let (mut ok_send, ok_recv) = conn.open_bi().await.unwrap();
    ok_send.write_all(&head).await.unwrap();
    let ok_sel = olo_rex_link::screen_wire::encode_quic_screen_args(&["follow".to_owned()]).unwrap();
    ok_send.write_all(&ok_sel).await.unwrap();
    let mut ok_reader = BufReader::new(ok_recv);
    let mut snap_line = String::new();
    tokio::time::timeout(Duration::from_secs(3), ok_reader.read_line(&mut snap_line)).await.unwrap().unwrap();
    let ev: olo_rex_link::screen_wire::FollowEvent = serde_json::from_str(snap_line.trim()).unwrap();
    assert!(matches!(ev, olo_rex_link::screen_wire::FollowEvent::Snapshot { .. }));
    ok_send.finish().unwrap();
    let mut rest = Vec::new();
    tokio::time::timeout(Duration::from_secs(3), ok_reader.read_to_end(&mut rest)).await.unwrap().unwrap();
    assert!(rest.is_empty());

    // Host state must remain completely untouched after every rejected QUIC request!
    assert!(sdir.join("actions.jsonl").exists(), "rejected QUIC requests must not clear actions.jsonl");
    assert!(sdir.join("screen.json").exists(), "rejected QUIC requests must not delete screen.json");

    conn.close(0u32.into(), b"");
    drop(input);
    let status = tokio::time::timeout(Duration::from_secs(10), child.wait()).await.unwrap().unwrap();
    assert!(status.success());
    let _ = std::fs::remove_dir_all(&scratch);
}
