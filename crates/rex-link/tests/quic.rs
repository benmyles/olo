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
