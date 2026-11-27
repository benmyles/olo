//! Real-host integration tests and release-build benchmarks for
//! [`StreamScreenSource`] (Steps 3b & 3c).
//!
//! All tests here are `#[ignore]` so `jato test -p olo-screen-pane` stays fast
//! and self-contained; `/var/tmp/olo-agents/screens/verify_3b.py` and
//! `/var/tmp/olo-agents/screens/verify_3c.py` run them on release builds
//! against real headless screens and `olo-rex-link screen`.

use std::{
    collections::VecDeque,
    io::{self, BufReader, Read, Write},
    net::Shutdown,
    os::unix::net::UnixStream,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc::channel,
    },
    time::{Duration, Instant},
};

use olo_protocol::SyntheticEvent;
use olo_rex_link::screen_wire::{ClientMsg, ServerMsg, read_client_msg, read_server_msg, write_server_msg};
use olo_screen_codec::{
    Frame, FrameIndex, TileEncoding, Writer, olof,
    tiles::{EncodedDelta, apply_delta, encode_delta},
};
use olo_screen_pane::{
    CommandOutput, HistoryEvent, HostProtocolCache, LiveEvent, LiveStream, PaneCapabilities, ScreenId, ScreenInfo, ScreenSource, ScreenState, Shot, StreamOpener, StreamScreenSource, history::Action,
};

struct CountingReader<R> {
    inner: R,
    bytes: Arc<AtomicU64>,
}

impl<R: Read> Read for CountingReader<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let n = self.inner.read(buf)?;
        self.bytes.fetch_add(n as u64, Ordering::Relaxed);
        Ok(n)
    }
}

/// Opener that spawns the release `olo-rex-link screen ...` child over plain
/// OS pipes (stdin/stdout) with no SSH or QUIC in between.
struct PipeStreamOpener {
    helper: PathBuf,
    env_overrides: Vec<(String, String)>,
    bytes_read: Arc<AtomicU64>,
    children: Arc<Mutex<Vec<Child>>>,
    cmd_calls: Arc<Mutex<Vec<Vec<String>>>>,
}

impl PipeStreamOpener {
    fn new(helper: PathBuf) -> Self {
        Self { helper, env_overrides: Vec::new(), bytes_read: Arc::new(AtomicU64::new(0)), children: Arc::new(Mutex::new(Vec::new())), cmd_calls: Arc::new(Mutex::new(Vec::new())) }
    }

    fn protocol_call_count(&self) -> usize {
        self.cmd_calls.lock().unwrap().iter().filter(|a| a.as_slice() == ["--protocol"]).count()
    }

    fn with_env(mut self, envs: Vec<(String, String)>) -> Self {
        self.env_overrides = envs;
        self
    }

    fn base_cmd(&self) -> Command {
        let mut cmd = Command::new(&self.helper);
        for (k, v) in &self.env_overrides {
            cmd.env(k, v);
        }
        cmd
    }

    fn spawn_stream(&self, args: &[&str], cancelled: Arc<AtomicBool>) -> Result<LiveStream, String> {
        if cancelled.load(Ordering::Acquire) {
            return Err("cancelled".into());
        }
        let mut child = self.base_cmd().args(args).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::inherit()).spawn().map_err(|e| format!("spawn {}: {e}", self.helper.display()))?;
        let writer = child.stdin.take().ok_or("missing child stdin")?;
        let reader = CountingReader { inner: child.stdout.take().ok_or("missing child stdout")?, bytes: self.bytes_read.clone() };
        self.children.lock().unwrap().push(child);
        Ok(LiveStream::new(reader, writer))
    }

    fn wait_all(&self) {
        for mut c in self.children.lock().unwrap().drain(..) {
            let deadline = Instant::now() + Duration::from_secs(5);
            loop {
                match c.try_wait() {
                    Ok(Some(_)) | Err(_) => break,
                    Ok(None) if Instant::now() >= deadline => {
                        let _ = c.kill();
                        let _ = c.wait();
                        break;
                    }
                    Ok(None) => std::thread::sleep(Duration::from_millis(10)),
                }
            }
        }
    }
}

impl StreamOpener for PipeStreamOpener {
    fn open_live(&self, name: &str, cancelled: Arc<AtomicBool>) -> Result<LiveStream, String> {
        self.spawn_stream(&["screen", "stream", name], cancelled)
    }

    fn open_history(&self, name: &str, cancelled: Arc<AtomicBool>) -> Result<LiveStream, String> {
        self.spawn_stream(&["screen", "history-stream", name], cancelled)
    }

    fn open_export(&self, name: &str, args: &[String], cancelled: Arc<AtomicBool>) -> Result<LiveStream, String> {
        let mut argv = vec!["screen", "export", name];
        for a in args {
            argv.push(a.as_str());
        }
        self.spawn_stream(&argv, cancelled)
    }

    fn run_command(&self, args: &[String], cancelled: Arc<AtomicBool>) -> Result<CommandOutput, String> {
        if cancelled.load(Ordering::Acquire) {
            return Err("cancelled".into());
        }
        self.cmd_calls.lock().unwrap().push(args.to_vec());
        let out = self.base_cmd().arg("screen").args(args).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).output().map_err(|e| format!("spawn {}: {e}", self.helper.display()))?;
        Ok(CommandOutput { status: out.status.code().unwrap_or(1), stdout: String::from_utf8_lossy(&out.stdout).into_owned(), stderr: String::from_utf8_lossy(&out.stderr).into_owned() })
    }
}

impl Drop for PipeStreamOpener {
    fn drop(&mut self) {
        self.wait_all();
    }
}

/// Require and verify a dedicated scratch root under `/var/tmp` so real-host
/// integration tests can NEVER touch the invoking account's HOME or live
/// `/run/user/<uid>/olo-screens`.
fn require_isolated_env(screen: &str) -> (PathBuf, Vec<(String, String)>) {
    let root = PathBuf::from(std::env::var("OLO_TEST_ROOT").expect("OLO_TEST_ROOT must be set to a scratch directory under /var/tmp"));
    assert!(root.starts_with("/var/tmp"), "OLO_TEST_ROOT must live under /var/tmp: {}", root.display());
    let mut envs = Vec::new();
    for key in ["HOME", "XDG_RUNTIME_DIR", "XDG_STATE_HOME", "XDG_CONFIG_HOME", "XDG_CACHE_HOME", "XDG_DATA_HOME"] {
        let val = std::env::var(key).unwrap_or_else(|_| panic!("{key} must be set inside OLO_TEST_ROOT"));
        let p = PathBuf::from(&val);
        assert!(p.starts_with(&root), "{key}={} must stay inside OLO_TEST_ROOT={}", p.display(), root.display());
        envs.push((key.to_owned(), val));
    }
    let rt = PathBuf::from(std::env::var("XDG_RUNTIME_DIR").unwrap());
    let desc = rt.join("olo-screens").join(format!("{screen}.json"));
    assert!(desc.starts_with(&root) && desc.is_file(), "test screen descriptor {} missing inside OLO_TEST_ROOT", desc.display());
    (root, envs)
}

fn isolated_cmd(helper: &Path, envs: &[(String, String)]) -> Command {
    let mut cmd = Command::new(helper);
    for (k, v) in envs {
        cmd.env(k, v);
    }
    cmd
}

fn decode_png_rgba(path: &Path) -> (usize, usize, Vec<u8>) {
    let file = std::fs::File::open(path).expect("open look png");
    let mut decoder = png::Decoder::new(BufReader::new(file));
    decoder.set_transformations(png::Transformations::EXPAND | png::Transformations::ALPHA);
    let mut reader = decoder.read_info().expect("png info");
    let mut buf = vec![0u8; reader.output_buffer_size().expect("png buf size")];
    let info = reader.next_frame(&mut buf).expect("png frame");
    let raw = &buf[..info.buffer_size()];
    let rgba = match info.color_type {
        png::ColorType::Rgba => raw.to_vec(),
        png::ColorType::Rgb => {
            let mut out = Vec::with_capacity((info.width * info.height * 4) as usize);
            for c in raw.as_chunks::<3>().0 {
                out.extend_from_slice(&[c[0], c[1], c[2], 255]);
            }
            out
        }
        other => panic!("unexpected png color type {other:?}"),
    };
    (info.width as usize, info.height as usize, rgba)
}

fn evdev_us(ch: char) -> u32 {
    match ch {
        'a' => 30,
        'b' => 48,
        'c' => 46,
        'd' => 32,
        'e' => 18,
        'f' => 33,
        'g' => 34,
        'h' => 35,
        'i' => 23,
        'j' => 36,
        'k' => 37,
        'l' => 38,
        'm' => 50,
        'n' => 49,
        'o' => 24,
        'p' => 25,
        'q' => 16,
        'r' => 19,
        's' => 31,
        't' => 20,
        'u' => 22,
        'v' => 47,
        'w' => 17,
        'x' => 45,
        'y' => 21,
        'z' => 44,
        ' ' => 57,
        '\n' => 28,
        _ => panic!("unsupported test char {ch:?}"),
    }
}

/// Real end-to-end test against a live headless screen and release
/// `olo-rex-link screen stream`, driven by `/var/tmp/olo-agents/screens/verify_3b.py`.
#[test]
#[ignore]
fn real_stream_matches_look_and_types_under_control() {
    let helper = PathBuf::from(std::env::var("OLO_TEST_HELPER").expect("OLO_TEST_HELPER"));
    let screen = std::env::var("OLO_TEST_SCREEN").expect("OLO_TEST_SCREEN");
    let look_png = PathBuf::from(std::env::var("OLO_TEST_LOOK_PNG").expect("OLO_TEST_LOOK_PNG"));
    let notes_path = PathBuf::from(std::env::var("OLO_TEST_NOTES").expect("OLO_TEST_NOTES"));
    let control_file = PathBuf::from(std::env::var("OLO_TEST_CONTROL_FILE").expect("OLO_TEST_CONTROL_FILE"));
    let idle_secs: u64 = std::env::var("OLO_TEST_IDLE_SECS").ok().and_then(|v| v.parse().ok()).unwrap_or(0);
    let styled_edit_bin = std::env::var("OLO_TEST_STYLED_EDIT").unwrap_or_else(|_| "olo-styled-edit".into());

    let (test_root, iso_envs) = require_isolated_env(&screen);
    assert!(control_file.starts_with(&test_root), "control_file must live inside OLO_TEST_ROOT: {}", control_file.display());
    assert!(notes_path.starts_with(&test_root), "notes_path must live inside OLO_TEST_ROOT: {}", notes_path.display());

    let opener = Arc::new(PipeStreamOpener::new(helper.clone()).with_env(iso_envs.clone()));
    let proto_cache = Arc::new(HostProtocolCache::new());
    let mut caps = PaneCapabilities::remote_default();
    caps.can_control = true;
    caps.control_disabled_reason = None;
    let info = ScreenInfo {
        id: ScreenId::host("scratch-host", "Scratch Host", &screen),
        title: None,
        state: ScreenState::Running,
        size: Some((1440, 900)),
        scale: 1.0,
        sound: false,
        started_ms: None,
        last_used_ms: None,
        controlled: false,
        capabilities: caps.clone(),
    };
    let source = StreamScreenSource::with_host_cache(info.clone(), opener.clone(), proto_cache.clone());
    let latest: Arc<Mutex<Option<Arc<Shot>>>> = Arc::new(Mutex::new(None));
    let latest_cb = latest.clone();
    let control_held = Arc::new(AtomicBool::new(false));
    let control_held_cb = control_held.clone();
    let (frame_tx, frame_rx) = channel::<()>();

    let handle = source.watch_live(
        Vec::new(),
        Box::new(move |ev| match ev {
            LiveEvent::Rgba(shot) => {
                *latest_cb.lock().unwrap() = Some(shot);
                let _ = frame_tx.send(());
            }
            LiveEvent::Control { held, .. } => {
                control_held_cb.store(held, Ordering::Release);
            }
            LiveEvent::Failed(e) => panic!("real stream failed unexpectedly: {e}"),
            _ => {}
        }),
    );

    // 1. Wait for initial keyframe and let the static screen settle.
    frame_rx.recv_timeout(Duration::from_secs(15)).expect("initial keyframe from olo-rex-link screen stream");
    let probed = proto_cache.get().expect("HostProtocolCache must have probed --protocol");
    assert!(
        probed.control_ack && probed.warning_status && probed.history_frame_error && probed.history_snapshot_done && probed.stop_expected_started && probed.discontiguous_part,
        "unexpected probed host capabilities: {probed:?}"
    );
    assert_eq!(opener.protocol_call_count(), 1, "--protocol must be queried once per host");
    println!("REAL_PROTOCOL_PROBED calls={} caps={:?}", opener.protocol_call_count(), probed);
    std::thread::sleep(Duration::from_millis(600));
    while frame_rx.try_recv().is_ok() {}

    // 2. Compare reconstructed picture against `olo-rex-link screen look`.
    let look_out = isolated_cmd(&helper, &iso_envs).args(["screen", "look", &screen, &look_png.display().to_string()]).output().expect("run screen look");
    assert!(look_out.status.success(), "screen look failed: {}", String::from_utf8_lossy(&look_out.stderr));
    std::thread::sleep(Duration::from_millis(300));
    while frame_rx.try_recv().is_ok() {}

    let shot = latest.lock().unwrap().clone().expect("latest shot");
    let (lw, lh, look_rgba) = decode_png_rgba(&look_png);
    assert_eq!(shot.size, [lw, lh], "stream size {:?} != screen look {}x{}", shot.size, lw, lh);
    let diff_px = shot.rgba.as_chunks::<4>().0.iter().zip(look_rgba.as_chunks::<4>().0.iter()).filter(|(a, b)| a[..3] != b[..3]).count();
    let total_px = lw * lh;
    println!("REAL_PICTURE_MATCH size={}x{} diff_pixels={} total_pixels={}", lw, lh, diff_px, total_px);
    assert_eq!(diff_px, 0, "rebuilt stream canvas differed from screen look by {diff_px}/{total_px} pixels");

    // 3. Optional idle window while the screen is static (no cursor blink) so
    //    `verify_3b.py` can measure the desktop StreamScreenSource process via `tools/dev/cost`.
    if idle_secs > 0 {
        println!("IDLE_READY pid={} secs={}", std::process::id(), idle_secs);
        let _ = std::io::stdout().flush();
        std::thread::sleep(Duration::from_secs(idle_secs + 2));
    }

    // 4. Launch olo-styled-edit, verify input gating before control, take
    //    control, type, save (Ctrl+S), and verify text arrived on disk.
    std::fs::write(&notes_path, "").expect("clear notes");
    let run_out = isolated_cmd(&helper, &iso_envs).args(["screen", "run", &screen, &styled_edit_bin, &notes_path.display().to_string()]).output().expect("run styled-edit");
    assert!(run_out.status.success(), "run styled-edit failed: {}", String::from_utf8_lossy(&run_out.stderr));
    std::thread::sleep(Duration::from_millis(1200));

    let win_out = isolated_cmd(&helper, &iso_envs).args(["screen", "windows", &screen]).output().expect("screen windows");
    assert!(win_out.status.success(), "screen windows failed");
    let wins: serde_json::Value = serde_json::from_slice(&win_out.stdout).expect("parse windows json");
    let arr = wins.as_array().expect("windows array");
    let se_win = arr
        .iter()
        .find(|w| w["app_id"].as_str().is_some_and(|a| a.to_lowercase().contains("styled")) || w["title"].as_str().is_some_and(|t| t.to_lowercase().contains("notes")))
        .or_else(|| arr.first())
        .expect("styled-edit window");
    let rect = if se_win["content"].is_object() { &se_win["content"] } else { &se_win["frame"] };
    let cx = rect["x"].as_f64().unwrap() + rect["width"].as_f64().unwrap() / 2.0;
    let cy = rect["y"].as_f64().unwrap() + rect["height"].as_f64().unwrap() / 2.0;

    // Input BEFORE control must not be sent and control lock must not exist.
    source.send_input(vec![SyntheticEvent::Key { key: evdev_us('x'), pressed: true }, SyntheticEvent::Key { key: evdev_us('x'), pressed: false }]);
    std::thread::sleep(Duration::from_millis(200));
    assert!(!control_file.exists(), "control file must not exist before set_controlled(true)");

    // Take control (ClientMsg::ControlSeq -> ServerMsg::Control { held: true, seq }),
    // verify second viewer gets an explicit refusal with reason words, click StyledEdit,
    // send a wheel notch + type "hello stream", save with Ctrl+S, and release via ack.
    source.set_controlled(true).expect("take control");
    let deadline = Instant::now() + Duration::from_secs(3);
    while (!control_file.exists() || !control_held.load(Ordering::Acquire)) && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(control_file.exists(), "control file missing after set_controlled(true)");
    assert!(control_held.load(Ordering::Acquire), "explicit ControlSeq grant not received after set_controlled(true)");

    // Second viewer sharing `proto_cache` must receive `Control { held: false, seq, reason }` refusal
    // without re-probing `--protocol` and without revoking viewer #1's control.
    let source2 = StreamScreenSource::with_host_cache(info, opener.clone(), proto_cache.clone());
    let (f2_tx, f2_rx) = channel::<()>();
    let (c2_tx, c2_rx) = channel::<(bool, Option<String>)>();
    let handle2 = source2.watch_live(
        Vec::new(),
        Box::new(move |ev| match ev {
            LiveEvent::Rgba(_) => {
                let _ = f2_tx.send(());
            }
            LiveEvent::Control { held, reason } => {
                let _ = c2_tx.send((held, reason));
            }
            _ => {}
        }),
    );
    f2_rx.recv_timeout(Duration::from_secs(10)).expect("viewer 2 initial frame");
    source2.set_controlled(true).expect("viewer 2 request control");
    let (held2, reason2) = c2_rx.recv_timeout(Duration::from_secs(5)).expect("viewer 2 control refusal");
    assert!(!held2, "second viewer must not be granted control while viewer 1 holds it");
    assert!(reason2.as_deref().is_some_and(|r| !r.is_empty()), "refusal must carry host words: {reason2:?}");
    assert!(control_held.load(Ordering::Acquire), "viewer 1 must retain control after viewer 2 refusal");
    println!("REAL_CONTROL_REFUSAL_OK reason={reason2:?}");
    drop(handle2);
    assert_eq!(opener.protocol_call_count(), 1, "second viewer must reuse HostProtocolCache");

    source.send_input(vec![
        SyntheticEvent::Move { x: cx, y: cy },
        SyntheticEvent::Button { button: 0x110, pressed: true },
        SyntheticEvent::Button { button: 0x110, pressed: false },
        SyntheticEvent::Wheel { dx: 0, dy: -1 },
    ]);
    std::thread::sleep(Duration::from_millis(350));

    for ch in "hello stream".chars() {
        let code = evdev_us(ch);
        source.send_input(vec![SyntheticEvent::Key { key: code, pressed: true }, SyntheticEvent::Key { key: code, pressed: false }]);
        std::thread::sleep(Duration::from_millis(35));
    }
    std::thread::sleep(Duration::from_millis(200));
    // Ctrl+S to save notes.txt.
    source.send_input(vec![
        SyntheticEvent::Key { key: 29, pressed: true },
        SyntheticEvent::Key { key: evdev_us('s'), pressed: true },
        SyntheticEvent::Key { key: evdev_us('s'), pressed: false },
        SyntheticEvent::Key { key: 29, pressed: false },
    ]);
    std::thread::sleep(Duration::from_millis(600));

    let saved = std::fs::read_to_string(&notes_path).expect("read notes");
    println!("REAL_TYPING_RESULT saved={saved:?}");
    assert!(saved.contains("hello stream"), "typed text did not arrive: {saved:?}");
    assert!(!saved.starts_with('x'), "pre-control 'x' leaked into input: {saved:?}");

    // Synchronous release ack: `control_file` must ALREADY be unlinked when `release_control_and_wait` returns.
    source.release_control_and_wait(Vec::new()).expect("release_control_and_wait");
    assert!(!control_file.exists(), "control file must be unlinked before release_control_and_wait returns");
    drop(handle);
    opener.wait_all();
    assert!(!control_file.exists(), "control file leaked after drop(handle)");
    println!("REAL_STREAM_OK");
}

/// Real end-to-end Step 3c test against a driven headless screen and release
/// `olo-rex-link screen history|export|<cmd>`, driven by
/// `/var/tmp/olo-agents/screens/verify_3c.py`.
#[test]
#[ignore]
fn real_history_scrub_20_frames_and_playable_export() {
    let helper = PathBuf::from(std::env::var("OLO_TEST_HELPER").expect("OLO_TEST_HELPER"));
    let screen = std::env::var("OLO_TEST_SCREEN").expect("OLO_TEST_SCREEN");
    let state_dir = PathBuf::from(std::env::var("OLO_TEST_STATE_DIR").expect("OLO_TEST_STATE_DIR"));
    let export_mp4 = PathBuf::from(std::env::var("OLO_TEST_EXPORT_MP4").expect("OLO_TEST_EXPORT_MP4"));
    let idle_secs: u64 = std::env::var("OLO_TEST_IDLE_SECS").ok().and_then(|v| v.parse().ok()).unwrap_or(0);

    let (test_root, iso_envs) = require_isolated_env(&screen);
    assert!(state_dir.starts_with(&test_root), "state_dir must live inside OLO_TEST_ROOT: {}", state_dir.display());
    assert!(export_mp4.starts_with(&test_root), "export_mp4 must live inside OLO_TEST_ROOT: {}", export_mp4.display());

    let opener = Arc::new(PipeStreamOpener::new(helper.clone()).with_env(iso_envs.clone()));
    let proto_cache = Arc::new(HostProtocolCache::new());
    let rt_dir = PathBuf::from(std::env::var("XDG_RUNTIME_DIR").unwrap());
    let desc_raw = std::fs::read_to_string(rt_dir.join("olo-screens").join(format!("{screen}.json"))).expect("read runtime descriptor");
    let desc_json: serde_json::Value = serde_json::from_str(&desc_raw).expect("parse runtime descriptor");
    let real_started = desc_json["started"].as_u64().expect("runtime screen started timestamp");
    let info = ScreenInfo {
        id: ScreenId::host("scratch-host", "Scratch Host", &screen),
        title: None,
        state: ScreenState::Running,
        size: Some((1440, 900)),
        scale: 1.0,
        sound: false,
        started_ms: Some(real_started.saturating_mul(1000)),
        last_used_ms: None,
        controlled: false,
        capabilities: PaneCapabilities::remote_default(),
    };
    let source = StreamScreenSource::with_host_cache(info, opener.clone(), proto_cache.clone());

    // 1. Follow history over `olo-rex-link screen history-stream <screen>`.
    let acc_actions: Arc<Mutex<Vec<Action>>> = Arc::new(Mutex::new(Vec::new()));
    let acc_texts: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let acc_index: Arc<Mutex<FrameIndex>> = Arc::new(Mutex::new(FrameIndex::default()));
    let (ev_tx, ev_rx) = channel::<()>();

    let a_cb = acc_actions.clone();
    let t_cb = acc_texts.clone();
    let i_cb = acc_index.clone();
    let handle = source.follow_history(Box::new(move |ev| match ev {
        HistoryEvent::Loaded { reset, actions, texts, frames, .. } => {
            assert_eq!(actions.len(), texts.len(), "actions/texts length mismatch");
            let mut a = a_cb.lock().unwrap();
            let mut t = t_cb.lock().unwrap();
            if reset {
                *a = actions;
                *t = texts;
            } else {
                a.extend(actions);
                t.extend(texts);
            }
            *i_cb.lock().unwrap() = (*frames).clone();
            let _ = ev_tx.send(());
        }
        HistoryEvent::Failed(e) => panic!("history stream failed unexpectedly: {e}"),
    }));

    // Wait until both actions and at least 20 recorded frames arrive.
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        let ready = !acc_actions.lock().unwrap().is_empty() && acc_index.lock().unwrap().frames.len() >= 20;
        if ready {
            break;
        }
        let rem = deadline.saturating_duration_since(Instant::now());
        if rem.is_zero() {
            panic!("timed out waiting for history (got {} actions, {} frames)", acc_actions.lock().unwrap().len(), acc_index.lock().unwrap().frames.len());
        }
        let _ = ev_rx.recv_timeout(rem.min(Duration::from_millis(250)));
    }
    // Brief settle so any trailing actions already flushed by the host arrive.
    std::thread::sleep(Duration::from_millis(300));
    while ev_rx.try_recv().is_ok() {}

    // 2. Compare StreamScreenSource's History tab data against host `olo-screen history --json`.
    let hist_out = isolated_cmd(&helper, &iso_envs).args(["screen", "history", &screen, "--json"]).output().expect("run screen history --json");
    assert!(hist_out.status.success(), "screen history --json failed: {}", String::from_utf8_lossy(&hist_out.stderr));
    let host_actions: Vec<Action> = serde_json::from_slice(&hist_out.stdout).expect("parse host history json");
    let stream_actions = acc_actions.lock().unwrap().clone();
    assert_eq!(stream_actions.len(), host_actions.len(), "stream actions {} != host actions {}", stream_actions.len(), host_actions.len());
    for (idx, (sa, ha)) in stream_actions.iter().zip(host_actions.iter()).enumerate() {
        assert_eq!(
            (sa.at, sa.ms, &sa.kind, &sa.args, &sa.result, &sa.error),
            (ha.at, ha.ms, &ha.kind, &ha.args, &ha.result, &ha.error),
            "action #{idx} mismatch between StreamScreenSource and host `olo-screen history --json`"
        );
    }
    let idx_snapshot = acc_index.lock().unwrap().clone();
    println!("REAL_HISTORY_MATCH actions={} frames={} last_size={:?}", stream_actions.len(), idx_snapshot.frames.len(), idx_snapshot.last_size);

    // 3. Optional at-rest idle window while history stream stays open (0% CPU, 0 wakeups/s).
    if idle_secs > 0 {
        println!("IDLE_HIST_READY pid={} secs={}", std::process::id(), idle_secs);
        let _ = std::io::stdout().flush();
        std::thread::sleep(Duration::from_secs(idle_secs + 2));
    }

    // 4. Scrub through 20 distinct recorded frames (mixed forward + backward jumps)
    //    and compare EVERY pixel against the host's own recorded `.olof` frame!
    let total_frames = idx_snapshot.frames.len();
    assert!(total_frames >= 20, "need at least 20 recorded frames, got {total_frames}");
    let mut pick_indices: Vec<usize> = (0..20).map(|i| i * (total_frames - 1) / 19).collect();
    // Mix order (even forward, then odd backward) to exercise deltas, keyframes, and backward seeks.
    let mut scrub_order = Vec::with_capacity(20);
    for &pi in pick_indices.iter().step_by(2) {
        scrub_order.push(pi);
    }
    for &pi in pick_indices.iter().skip(1).step_by(2).rev() {
        scrub_order.push(pi);
    }
    pick_indices.clear();

    let host_segments = olof::index_dir(&state_dir);
    let mut latencies_us = Vec::with_capacity(scrub_order.len());
    let mut total_diff_px = 0usize;
    let mut total_checked_px = 0usize;
    for &fi in &scrub_order {
        let ts = idx_snapshot.frames[fi].at;
        let t0 = Instant::now();
        let got = source.frame_at(ts).expect("stream frame_at").expect("expected recorded frame");
        latencies_us.push(t0.elapsed().as_micros() as u64);

        let expected = olof::frame_at(&host_segments, ts).expect("host frame missing");
        assert_eq!(got.0, expected.0, "timestamp mismatch at frame index {fi}");
        assert_eq!((got.1.width, got.1.height), (expected.1.width, expected.1.height), "dimension mismatch at frame index {fi}");
        let diff = got.1.rgba.as_chunks::<4>().0.iter().zip(expected.1.rgba.as_chunks::<4>().0.iter()).filter(|(a, b)| a != b).count();
        total_diff_px += diff;
        total_checked_px += expected.1.width * expected.1.height;
        assert_eq!(diff, 0, "scrubbed frame #{fi} (at={ts}) differed by {diff} pixels from host .olof");
    }
    let lat = summarize_us(latencies_us);
    let max_us = lat.4;
    println!(
        "REAL_SCRUB_MATCH frames={} diff_pixels={} total_pixels={} frame_at_us(min/p50/p95/max/mean)={}/{}/{}/{}/{}",
        scrub_order.len(),
        total_diff_px,
        total_checked_px,
        lat.0,
        lat.1,
        lat.2,
        max_us,
        lat.3
    );

    // 5. Export playable MP4 through `StreamScreenSource::export_video` and decode it with `olo_preview::video::picture_at`.
    let mut progress_steps = Vec::new();
    let made = source
        .export_video(None, 1, &export_mp4, &mut |done| {
            progress_steps.push(done);
            true
        })
        .expect("StreamScreenSource::export_video failed");
    assert!(export_mp4.is_file(), "exported MP4 missing at {}", export_mp4.display());
    let part_sibling = PathBuf::from(format!("{}.part", export_mp4.display()));
    assert!(!part_sibling.exists(), "temporary .part file left behind: {}", part_sibling.display());
    let mp4_bytes = std::fs::metadata(&export_mp4).expect("mp4 metadata").len();
    assert!(mp4_bytes > 0, "exported MP4 is empty");

    let decoded = olo_preview::video::picture_at(&olo_preview::input::Input::file(&export_mp4), 0.0).expect("decode exported MP4 frame 0");
    assert!(decoded.size[0] > 0 && decoded.size[1] > 0, "decoded MP4 frame has empty dimensions: {:?}", decoded.size);
    if let Some((sw, sh)) = idx_snapshot.last_size {
        assert_eq!(decoded.size, [sw & !1, sh & !1], "decoded MP4 size {:?} != expected even screen size {:?}", decoded.size, [sw & !1, sh & !1]);
    }
    println!("REAL_EXPORT_OK bytes={} frames={} encoder={} progress_events={} decoded_size={}x{}", mp4_bytes, made.frames, made.encoder, progress_steps.len(), decoded.size[0], decoded.size[1]);

    // 5b. Export a discontiguous action selection (`--part 0,2-4`) through real `olo-rex-link screen export`!
    assert!(source.info().capabilities.can_export_discontiguous, "real helper should advertise discontiguous_part");
    let discontig_mp4 = export_mp4.with_file_name("exported-discontig.mp4");
    let part_made = source.export_video(Some(&[0, 2, 3, 4]), 1, &discontig_mp4, &mut |_| true).expect("discontiguous --part export failed");
    assert!(discontig_mp4.is_file(), "discontiguous MP4 missing");
    let part_decoded = olo_preview::video::picture_at(&olo_preview::input::Input::file(&discontig_mp4), 0.0).expect("decode discontiguous MP4");
    assert!(part_decoded.size[0] > 0 && part_decoded.size[1] > 0);
    println!("REAL_DISCONTIG_EXPORT_OK frames={} decoded_size={}x{}", part_made.frames, part_decoded.size[0], part_decoded.size[1]);

    // 6. Exercise one-shot lifecycle commands through StreamScreenSource.
    source.rename(Some("QA Renamed")).expect("remote rename");
    assert_eq!(std::fs::read_to_string(state_dir.join("title")).expect("read title"), "QA Renamed");
    source.rename(None).expect("clear remote rename");
    assert!(!state_dir.join("title").exists(), "cleared title file should be removed");

    // Refused `start` and `archive` while running must surface the helper's own refusal words.
    let start_err = source.start().expect_err("start on already-running screen must be refused");
    assert!(start_err.contains("already"), "unexpected start refusal: {start_err:?}");
    let arch_err = source.archive(true).expect_err("archive on running screen must be refused");
    assert!(arch_err.contains("running"), "unexpected archive refusal: {arch_err:?}");
    // Generation-checked stop with stale `--expected-started` MUST be refused without stopping the screen!
    let stale_stop_err = source.stop_if_generation(Some(1)).expect_err("stale --expected-started must be refused");
    assert!(stale_stop_err.contains("replaced") || stale_stop_err.contains("restarted") || stale_stop_err.contains("newer session"), "unexpected stale stop error: {stale_stop_err:?}");
    println!("REAL_LIFECYCLE_REFUSALS start_running={start_err:?} archive_running={arch_err:?} stale_stop={stale_stop_err:?}");

    // Stop with matching `--expected-started`, archive toggle, start again, stop, clear_history, and delete.
    drop(handle);
    source.stop().expect("remote stop with matching --expected-started");
    source.archive(true).expect("remote archive(true)");
    assert!(state_dir.join("archived").is_file(), "archived marker missing after archive(true)");
    source.archive(false).expect("remote archive(false)");
    assert!(!state_dir.join("archived").exists(), "archived marker still present after archive(false)");

    let restarted = source.start().expect("remote start");
    assert!(restarted.is_running(), "restarted screen should be running");
    source.stop().expect("second remote stop");
    source.clear_history().expect("remote clear_history");
    assert!(!state_dir.join("actions.jsonl").exists(), "actions.jsonl should be cleared");
    source.delete().expect("remote delete");
    assert!(!state_dir.exists(), "deleted screen state dir should be moved to scratch Trash");
    assert_eq!(opener.protocol_call_count(), 1, "--protocol must be probed only once across the whole host session");
    opener.wait_all();
    println!("REAL_3C_OK");
}

fn synthetic_desktop_frame(w: usize, h: usize, scroll_y: usize, typed_col: Option<usize>) -> Frame {
    let mut rgba = vec![0u8; w * h * 4];
    for y in 0..h {
        let line = (y + scroll_y) / 22;
        for x in 0..w {
            let i = (y * w + x) * 4;
            let card_band = ((y + scroll_y) % 110) >= 12 && ((y + scroll_y) % 110) <= 98 && x >= 40 && x < w - 40;
            let is_text = card_band && ((y + scroll_y) % 22) >= 6 && ((y + scroll_y) % 22) <= 15 && ((x + line * 19) % 7 != 0) && ((x / 36 + line) % 5 != 0);
            let px: [u8; 4] = if is_text {
                [34, 40, 52, 255]
            } else if card_band {
                [255, 255, 255, 255]
            } else {
                [242, 244, 248, 255]
            };
            rgba[i..i + 4].copy_from_slice(&px);
        }
    }
    if let Some(col) = typed_col {
        let y0 = 220;
        for y in y0..(y0 + 18) {
            for x in col..(col + 14) {
                let i = (y * w + x) * 4;
                rgba[i..i + 4].copy_from_slice(&[28, 86, 210, 255]);
            }
        }
    }
    Frame { width: w, height: h, rgba }
}

fn read_proc_rss_kib() -> (u64, u64) {
    let status = std::fs::read_to_string("/proc/self/status").unwrap_or_default();
    let parse = |key: &str| -> u64 { status.lines().find_map(|l| l.strip_prefix(key)).and_then(|rest| rest.split_whitespace().next()).and_then(|n| n.parse::<u64>().ok()).unwrap_or(0) };
    (parse("VmRSS:"), parse("RssAnon:"))
}

fn summarize_us(mut v: Vec<u64>) -> (u64, u64, u64, u64, u64) {
    v.sort_unstable();
    let n = v.len().max(1);
    let min = *v.first().unwrap_or(&0);
    let p50 = v[n / 2];
    let p95 = v[(n * 95 / 100).min(n - 1)];
    let mean = v.iter().sum::<u64>() / n as u64;
    let max = *v.last().unwrap_or(&0);
    (min, p50, p95, mean, max)
}

struct SingleOpener(Mutex<VecDeque<UnixStream>>);
impl StreamOpener for SingleOpener {
    fn open_live(&self, _name: &str, _cancelled: Arc<AtomicBool>) -> Result<LiveStream, String> {
        let s = self.0.lock().unwrap().pop_front().ok_or("no stream")?;
        let r = s.try_clone().map_err(|e| e.to_string())?;
        let c = s.try_clone().map_err(|e| e.to_string())?;
        Ok(LiveStream::with_cancel(r, s, move || {
            let _ = c.shutdown(Shutdown::Both);
        }))
    }

    fn open_history(&self, _name: &str, _cancelled: Arc<AtomicBool>) -> Result<LiveStream, String> {
        Err("history unsupported in SingleOpener".into())
    }

    fn open_export(&self, _name: &str, _args: &[String], _cancelled: Arc<AtomicBool>) -> Result<LiveStream, String> {
        Err("export unsupported in SingleOpener".into())
    }

    fn run_command(&self, _args: &[String], _cancelled: Arc<AtomicBool>) -> Result<CommandOutput, String> {
        Err("command unsupported in SingleOpener".into())
    }
}

/// Release-build benchmark of the desktop side of a frame (read, decode,
/// decompress+apply, hand over) for typing and scroll frames + memory held
/// for one 1440×900 screen.
#[test]
#[ignore]
fn bench_desktop_frame_stages_and_memory() {
    let (w, h) = (1440, 900);
    let base = synthetic_desktop_frame(w, h, 0, None);
    let typed = synthetic_desktop_frame(w, h, 0, Some(120));
    let scrolled = synthetic_desktop_frame(w, h, 44, None);

    let key_delta = encode_delta(&base, None, TileEncoding::PackedStream, 1).unwrap();
    let type_delta = encode_delta(&typed, Some(&base), TileEncoding::PackedStream, 1).unwrap();
    let scroll_delta = encode_delta(&scrolled, Some(&base), TileEncoding::PackedStream, 1).unwrap();

    let key_olot = key_delta.encode_bytes();
    let type_olot = type_delta.encode_bytes();
    let scroll_olot = scroll_delta.encode_bytes();

    // Pre-serialize wire frames as sent by `olo-rex-link screen stream`.
    let make_wire = |seq: u64, olot: &[u8]| -> Vec<u8> {
        let mut out = Vec::new();
        write_server_msg(&mut out, &ServerMsg::Frame { seq, delta: olot.to_vec() }).unwrap();
        out
    };
    let type_wire = make_wire(2, &type_olot);
    let scroll_wire = make_wire(3, &scroll_olot);

    println!(
        "BENCH_PAYLOADS keyframe={}B ({} tiles) typing={}B ({} tiles) scroll={}B ({} tiles)",
        key_olot.len(),
        key_delta.rects.len(),
        type_olot.len(),
        type_delta.rects.len(),
        scroll_olot.len(),
        scroll_delta.rects.len()
    );

    // 1. Per-stage microbenchmark (200 iterations each, release build).
    let iters = 200usize;
    for (label, wire, prev_ref) in [("typing", &type_wire, &base), ("scroll", &scroll_wire, &base)] {
        let mut read_us = Vec::with_capacity(iters);
        let mut decode_apply_us = Vec::with_capacity(iters);
        let mut copy_handover_us = Vec::with_capacity(iters);
        let mut total_us = Vec::with_capacity(iters);

        let mut canvas = prev_ref.clone();
        for _ in 0..iters {
            // Reset canvas outside timing so every iteration applies the exact delta cleanly.
            canvas.rgba.copy_from_slice(&prev_ref.rgba);

            let t0 = Instant::now();
            let mut cur = std::io::Cursor::new(&wire[..]);
            let msg = read_server_msg(&mut cur).unwrap().unwrap();
            let t1 = Instant::now();

            let ServerMsg::Frame { delta, .. } = msg else { unreachable!() };
            let decoded = EncodedDelta::decode_bytes(&delta).unwrap();
            apply_delta(&decoded, &mut canvas).unwrap();
            let t2 = Instant::now();

            let shot = Arc::new(Shot { size: [canvas.width, canvas.height], rgba: canvas.rgba.clone() });
            std::hint::black_box(&shot);
            let t3 = Instant::now();

            read_us.push(t1.duration_since(t0).as_micros() as u64);
            decode_apply_us.push(t2.duration_since(t1).as_micros() as u64);
            copy_handover_us.push(t3.duration_since(t2).as_micros() as u64);
            total_us.push(t3.duration_since(t0).as_micros() as u64);
        }
        let r = summarize_us(read_us);
        let da = summarize_us(decode_apply_us);
        let ho = summarize_us(copy_handover_us);
        let tot = summarize_us(total_us);
        println!(
            "BENCH_STAGE workload={label} read_us(min/p50/p95/mean)={}/{}/{}/{} decode_apply_us={}/{}/{}/{} handover_copy_us={}/{}/{}/{} total_us={}/{}/{}/{}",
            r.0, r.1, r.2, r.3, da.0, da.1, da.2, da.3, ho.0, ho.1, ho.2, ho.3, tot.0, tot.1, tot.2, tot.3
        );
    }

    // 2. End-to-end StreamScreenSource latency (UnixStream write -> read -> decode -> apply -> deliver thread -> on_event + Ack back)
    //    AND memory held for one 1440×900 screen.
    let (rss_before, anon_before) = read_proc_rss_kib();
    let (client_sock, mut host_sock) = UnixStream::pair().unwrap();
    let opener = Arc::new(SingleOpener(Mutex::new(VecDeque::from([client_sock]))));
    let info = ScreenInfo {
        id: ScreenId::host("bench", "Bench", "qa"),
        title: None,
        state: ScreenState::Running,
        size: Some((1440, 900)),
        scale: 1.0,
        sound: false,
        started_ms: None,
        last_used_ms: None,
        controlled: false,
        capabilities: PaneCapabilities::remote_default(),
    };
    let source = StreamScreenSource::new(info, opener);
    let kept_shot: Arc<Mutex<Option<Arc<Shot>>>> = Arc::new(Mutex::new(None));
    let kept_cb = kept_shot.clone();
    let (ev_tx, ev_rx) = channel::<Instant>();
    let handle = source.watch_live(
        Vec::new(),
        Box::new(move |ev| {
            if let LiveEvent::Rgba(shot) = ev {
                *kept_cb.lock().unwrap() = Some(shot);
                let _ = ev_tx.send(Instant::now());
            }
        }),
    );

    // Send initial keyframe.
    write_server_msg(&mut host_sock, &ServerMsg::Frame { seq: 1, delta: key_olot }).unwrap();
    let _ = ev_rx.recv_timeout(Duration::from_secs(5)).unwrap();
    assert_eq!(read_client_msg(&mut host_sock).unwrap(), Some(ClientMsg::Ack { seq: 1 }));

    // Measure 100 end-to-end frames each for typing and scroll.
    let mut seq = 2u64;
    for (label, olot) in [("typing", &type_olot), ("scroll", &scroll_olot)] {
        let mut e2e_us = Vec::with_capacity(100);
        for _ in 0..100 {
            let t0 = Instant::now();
            write_server_msg(&mut host_sock, &ServerMsg::Frame { seq, delta: olot.clone() }).unwrap();
            let t_delivered = ev_rx.recv_timeout(Duration::from_secs(5)).unwrap();
            assert_eq!(read_client_msg(&mut host_sock).unwrap(), Some(ClientMsg::Ack { seq }));
            e2e_us.push(t_delivered.duration_since(t0).as_micros() as u64);
            seq += 1;
        }
        let s = summarize_us(e2e_us);
        println!("BENCH_E2E workload={label} wire_to_callback_us(min/p50/p95/mean)={}/{}/{}/{}", s.0, s.1, s.2, s.3);
    }

    let (rss_active, anon_active) = read_proc_rss_kib();
    drop(handle);
    *kept_shot.lock().unwrap() = None;
    let (rss_after, anon_after) = read_proc_rss_kib();
    println!(
        "BENCH_MEMORY_KIB before_rss={} active_rss={} delta_rss={} after_drop_rss={} | before_anon={} active_anon={} delta_anon={} after_drop_anon={}",
        rss_before,
        rss_active,
        rss_active.saturating_sub(rss_before),
        rss_after,
        anon_before,
        anon_active,
        anon_active.saturating_sub(anon_before),
        anon_after
    );
}

/// Release-build benchmark for Step 3c:
/// - wire bytes and wall-clock time to load a history of 1,000 actions + its
///   `FrameIndex` (and build search texts off the UI thread) from real release
///   `olo-rex-link screen history`;
/// - `frame_at` asked-to-answered latency across 50 requests (initial keyframe
///   and subsequent tile deltas over the persistent history stream).
#[test]
#[ignore]
fn bench_history_1000_actions_and_frame_at_release() {
    let helper = PathBuf::from(std::env::var("OLO_TEST_HELPER").expect("OLO_TEST_HELPER"));
    let scratch = PathBuf::from(format!("/var/tmp/olo-agents/screens/bench-3c-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&scratch);
    let home = scratch.join("home");
    let state_home = home.join(".local/state");
    let rt = scratch.join("rt");
    let screen_name = "bench-hist";
    let screen_dir = state_home.join("olo/screens").join(screen_name);
    std::fs::create_dir_all(&screen_dir).unwrap();
    std::fs::create_dir_all(&rt).unwrap();

    // 1. Write 1,000 realistic actions into `actions.jsonl`.
    let base_at = 1_795_000_000_000u64;
    {
        let mut f = std::fs::File::create(screen_dir.join("actions.jsonl")).unwrap();
        let cmds = [
            ("click", vec!["--at".into(), "420,310".into(), "--button".into(), "left".into()]),
            ("type", vec!["hello world from agent screen benchmark".into()]),
            ("look", vec!["/var/tmp/shot.png".into()]),
            ("key", vec!["Return".into()]),
            ("scroll", vec!["--dy".into(), "-120".into()]),
            ("windows", vec![]),
        ];
        for i in 0..1000usize {
            let (kind, args) = &cmds[i % cmds.len()];
            let action = Action {
                at: base_at + (i as u64) * 250,
                ms: 12 + (i % 45) as u64,
                kind: (*kind).to_owned(),
                args: args.clone(),
                result: None,
                error: (i % 50 == 49).then(|| "window did not respond in time".to_owned()),
            };
            serde_json::to_writer(&mut f, &action).unwrap();
            writeln!(f).unwrap();
        }
        f.flush().unwrap();
    }

    // 2. Write 30 recorded 1440×900 `.olof` frames across the timeline.
    let (w, h) = (1440, 900);
    let mut writer = Writer::new(screen_dir.clone(), 512 << 20);
    let mut frame_ts = Vec::with_capacity(30);
    for i in 0..30usize {
        let ts = base_at + (i as u64) * 8_000;
        let frame = if i % 5 == 0 { synthetic_desktop_frame(w, h, (i / 5) * 44, None) } else { synthetic_desktop_frame(w, h, (i / 5) * 44, Some(100 + (i % 5) * 24)) };
        writer.add(frame, ts).unwrap();
        frame_ts.push(ts);
    }
    drop(writer);

    let actions_file_bytes = std::fs::metadata(screen_dir.join("actions.jsonl")).unwrap().len();
    let opener = Arc::new(PipeStreamOpener::new(helper).with_env(vec![
        ("HOME".into(), home.display().to_string()),
        ("XDG_STATE_HOME".into(), state_home.display().to_string()),
        ("XDG_RUNTIME_DIR".into(), rt.display().to_string()),
        ("XDG_CONFIG_HOME".into(), home.join(".config").display().to_string()),
        ("XDG_CACHE_HOME".into(), home.join(".cache").display().to_string()),
        ("XDG_DATA_HOME".into(), home.join(".local/share").display().to_string()),
    ]));
    let info = ScreenInfo {
        id: ScreenId::host("bench", "Bench", screen_name),
        title: None,
        state: ScreenState::Stopped,
        size: Some((w as i32, h as i32)),
        scale: 1.0,
        sound: false,
        started_ms: None,
        last_used_ms: None,
        controlled: false,
        capabilities: PaneCapabilities::remote_default(),
    };
    let source = StreamScreenSource::new(info, opener.clone());

    // 3. Measure wire bytes and wall-clock time to load all 1,000 actions + FrameIndex.
    let (done_tx, done_rx) = channel::<(usize, usize, Instant)>();
    let seen_actions = Arc::new(Mutex::new(0usize));
    let seen_frames = Arc::new(Mutex::new(0usize));
    let sa_cb = seen_actions.clone();
    let sf_cb = seen_frames.clone();
    let t_start = Instant::now();
    let handle = source.follow_history(Box::new(move |ev| {
        if let HistoryEvent::Loaded { actions, texts, reset, frames, .. } = ev {
            assert_eq!(actions.len(), texts.len());
            let mut na = sa_cb.lock().unwrap();
            if reset {
                *na = actions.len();
            } else {
                *na += actions.len();
            }
            let mut nf = sf_cb.lock().unwrap();
            *nf = frames.frames.len();
            if *na >= 1000 && *nf >= 30 {
                let _ = done_tx.send((*na, *nf, Instant::now()));
            }
        }
    }));

    let (n_actions, n_frames, t_loaded) = done_rx.recv_timeout(Duration::from_secs(15)).expect("1000-action history load");
    let load_us = t_loaded.duration_since(t_start).as_micros() as u64;
    let load_wire_bytes = opener.bytes_read.load(Ordering::Relaxed);
    println!(
        "BENCH_HISTORY_1000 actions={} frames={} disk_jsonl_bytes={} wire_bytes={} load_ms={:.2} load_us={}",
        n_actions,
        n_frames,
        actions_file_bytes,
        load_wire_bytes,
        load_us as f64 / 1000.0,
        load_us
    );

    // 4. Measure 50 `frame_at` requests (1 initial keyframe + 49 deltas/seeks).
    let bytes_before_first = opener.bytes_read.load(Ordering::Relaxed);
    let t_first = Instant::now();
    let first = source.frame_at(frame_ts[0]).unwrap().unwrap();
    let first_us = t_first.elapsed().as_micros() as u64;
    let first_wire_bytes = opener.bytes_read.load(Ordering::Relaxed) - bytes_before_first;
    assert_eq!((first.1.width, first.1.height), (w, h));

    let mut typing_us = Vec::new();
    let mut typing_bytes = Vec::new();
    let mut scroll_us = Vec::new();
    let mut scroll_bytes = Vec::new();
    for step in 0..49usize {
        // Alternate between nearby typing deltas (within same scroll band) and
        // larger scroll-band jumps.
        let idx = (step + 1) % frame_ts.len();
        let is_scroll_jump = idx % 5 == 0;
        let b0 = opener.bytes_read.load(Ordering::Relaxed);
        let t0 = Instant::now();
        let got = source.frame_at(frame_ts[idx]).unwrap().unwrap();
        let dt = t0.elapsed().as_micros() as u64;
        let db = opener.bytes_read.load(Ordering::Relaxed) - b0;
        assert_eq!(got.0, frame_ts[idx]);
        if is_scroll_jump {
            scroll_us.push(dt);
            scroll_bytes.push(db);
        } else {
            typing_us.push(dt);
            typing_bytes.push(db);
        }
    }
    let all_us: Vec<u64> = typing_us.iter().chain(scroll_us.iter()).copied().collect();
    let st_all = summarize_us(all_us);
    let st_type = summarize_us(typing_us);
    let st_scroll = summarize_us(scroll_us);
    let mean_type_b = typing_bytes.iter().sum::<u64>() / typing_bytes.len().max(1) as u64;
    let mean_scroll_b = scroll_bytes.iter().sum::<u64>() / scroll_bytes.len().max(1) as u64;
    println!(
        "BENCH_FRAME_AT_50 first_keyframe_us={} first_keyframe_bytes={} | delta_all_us(min/p50/p95/max/mean)={}/{}/{}/{}/{} | typing_delta_us(p50/p95)={}/{} mean_bytes={} | scroll_delta_us(p50/p95)={}/{} mean_bytes={}",
        first_us, first_wire_bytes, st_all.0, st_all.1, st_all.2, st_all.4, st_all.3, st_type.1, st_type.2, mean_type_b, st_scroll.1, st_scroll.2, mean_scroll_b
    );

    drop(handle);
    opener.wait_all();
    let _ = std::fs::remove_dir_all(&scratch);
}

/// Real-helper wire test covering multi-chunk history snapshots (>10,000 actions),
/// oversized (>8 MiB) action placeholders at exact 0-based index, `last_size: None`
/// and per-request `HistoryMsg::FrameError` on an oversized `.olof` frame while the
/// stream stays open, and `index_gen` invalidation on equal-timestamp `.olof` replacement.
#[test]
#[ignore]
fn real_multichunk_history_oversized_frame_error_and_index_gen() {
    let helper = PathBuf::from(std::env::var("OLO_TEST_HELPER").expect("OLO_TEST_HELPER"));
    let scratch = PathBuf::from(format!("/var/tmp/olo-agents/screens/real-multichunk-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&scratch);
    let home = scratch.join("home");
    let state_home = home.join(".local/state");
    let rt = scratch.join("rt");
    let screen_name = "multichunk-qa";
    let screen_dir = state_home.join("olo/screens").join(screen_name);
    std::fs::create_dir_all(&screen_dir).unwrap();
    std::fs::create_dir_all(&rt).unwrap();

    // 1. Write 10,500 action lines (> ACTIONS_BATCH_LINES = 10,000 -> >= 2 Actions chunks
    //    before SnapshotDone), with line index 5,000 exceeding 8 MiB (MAX_ACTION_RECORD_BYTES)
    //    so real `olo-rex-link` emits the index-preserving `"oversized"` placeholder.
    let base_at = 1_796_000_000_000u64;
    let total_actions = 10_500usize;
    let oversized_idx = 5_000usize;
    {
        let mut f = io::BufWriter::new(std::fs::File::create(screen_dir.join("actions.jsonl")).unwrap());
        let giant_arg = "x".repeat(8 * 1024 * 1024 + 4096);
        for i in 0..total_actions {
            if i == oversized_idx {
                writeln!(f, r#"{{"at":{},"ms":1,"kind":"type","args":["{giant_arg}"]}}"#, base_at + i as u64).unwrap();
            } else {
                writeln!(f, r#"{{"at":{},"ms":5,"kind":"click","args":["{i}"]}}"#, base_at + (i as u64) * 10).unwrap();
            }
        }
        f.flush().unwrap();
    }

    // 2. Write two `.olof` segments:
    //    - segment 1 at `base_at`: valid 128×64 green frame;
    //    - segment 2 at `base_at + 5_000`: valid `OLOFRM1\n` header claiming 20000×64
    //      (> MAX_NET_DIM = 16384), causing `last_size: None` and per-request `FrameError`.
    let make_solid = |w: usize, h: usize, rgba: [u8; 4]| -> Frame {
        let mut buf = Vec::with_capacity(w * h * 4);
        for _ in 0..(w * h) {
            buf.extend_from_slice(&rgba);
        }
        Frame { width: w, height: h, rgba: buf }
    };
    let green = make_solid(128, 64, [20, 200, 40, 255]);
    {
        let mut w = Writer::new(screen_dir.clone(), 64 << 20);
        w.add(green.clone(), base_at).unwrap();
    }
    let oversized_ts = base_at + 5_000;
    {
        let seg2 = screen_dir.join(format!("frames-{oversized_ts:016}.olof"));
        let mut rec = Vec::with_capacity(21);
        rec.extend_from_slice(&oversized_ts.to_le_bytes());
        rec.extend_from_slice(&20_000u32.to_le_bytes());
        rec.extend_from_slice(&64u32.to_le_bytes());
        rec.push(1u8); // keyframe
        rec.extend_from_slice(&0u32.to_le_bytes()); // 0 tiles
        let mut raw = Vec::with_capacity(8 + 4 + rec.len());
        raw.extend_from_slice(b"OLOFRM1\n");
        raw.extend_from_slice(&(rec.len() as u32).to_le_bytes());
        raw.extend_from_slice(&rec);
        std::fs::write(&seg2, raw).unwrap();
    }

    let opener = Arc::new(PipeStreamOpener::new(helper).with_env(vec![
        ("HOME".into(), home.display().to_string()),
        ("XDG_STATE_HOME".into(), state_home.display().to_string()),
        ("XDG_RUNTIME_DIR".into(), rt.display().to_string()),
        ("XDG_CONFIG_HOME".into(), home.join(".config").display().to_string()),
        ("XDG_CACHE_HOME".into(), home.join(".cache").display().to_string()),
        ("XDG_DATA_HOME".into(), home.join(".local/share").display().to_string()),
    ]));
    let proto_cache = Arc::new(HostProtocolCache::new());
    let info = ScreenInfo {
        id: ScreenId::host("multi", "Multi", screen_name),
        title: None,
        state: ScreenState::Stopped,
        size: Some((128, 64)),
        scale: 1.0,
        sound: false,
        started_ms: None,
        last_used_ms: None,
        controlled: false,
        capabilities: PaneCapabilities::remote_default(),
    };
    let source = StreamScreenSource::with_host_cache(info, opener.clone(), proto_cache.clone());

    let (tx, rx) = channel::<HistoryEvent>();
    let handle = source.follow_history(Box::new(move |ev| {
        let _ = tx.send(ev);
    }));

    // Initial multi-chunk snapshot MUST arrive as ONE coherent Loaded event on SnapshotDone!
    match rx.recv_timeout(Duration::from_secs(20)).expect("multi-chunk initial snapshot") {
        HistoryEvent::Loaded { actions, texts, reset, index_reset, frames } => {
            assert!(reset);
            assert!(index_reset);
            assert_eq!(actions.len(), total_actions, "all 10,500 actions across >=2 chunks must commit together on SnapshotDone");
            assert_eq!(texts.len(), total_actions);
            assert!(actions[oversized_idx].is_oversized(), "index {oversized_idx} must be oversized placeholder");
            assert!(!actions[oversized_idx - 1].is_oversized());
            assert!(!actions[oversized_idx + 1].is_oversized());
            assert_eq!(actions[oversized_idx - 1].args, vec![(oversized_idx - 1).to_string()]);
            assert_eq!(actions[oversized_idx + 1].args, vec![(oversized_idx + 1).to_string()]);
            assert_eq!(frames.len(), 2);
            assert_eq!(frames.last_size, None, "oversized newest frame must yield last_size: None");
            println!("REAL_MULTICHUNK_OK actions={} oversized_idx={} frames={} last_size={:?}", actions.len(), oversized_idx, frames.len(), frames.last_size);
        }
        HistoryEvent::Failed(e) => panic!("unexpected failure: {e}"),
    }
    assert_eq!(opener.protocol_call_count(), 1, "--protocol must be probed once");

    // Per-request `HistoryMsg::FrameError` on the 20000×64 frame must return Err words
    // without closing the stream, and the immediately following `frame_at(base_at)` must succeed!
    let err = source.frame_at(oversized_ts).expect_err("oversized recorded frame must return FrameError");
    assert!(err.contains("limit") || err.contains("20000") || err.contains("exceeds"), "unexpected FrameError message: {err}");
    let (got_ts, got_frame) = source.frame_at(base_at).expect("stream must stay open after FrameError").expect("frame 0 present");
    assert_eq!(got_ts, base_at);
    assert_eq!(got_frame, green);
    println!("REAL_FRAME_ERROR_OK err={err:?}");

    // Equal-timestamp `.olof` replacement: delete old segments and write a brand-new blue frame
    // at the EXACT same `base_at` -> real helper bumps `index_gen` + sends SnapshotDone ->
    // StreamScreenSource emits `index_reset == true` and serves the new blue pixels!
    for ent in std::fs::read_dir(&screen_dir).unwrap().flatten() {
        if ent.file_name().to_string_lossy().ends_with(".olof") {
            let _ = std::fs::remove_file(ent.path());
        }
    }
    let blue = make_solid(128, 64, [30, 80, 230, 255]);
    {
        let mut w = Writer::new(screen_dir.clone(), 64 << 20);
        w.add(blue.clone(), base_at).unwrap();
    }
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut saw_index_reset = false;
    while Instant::now() < deadline {
        if let Ok(HistoryEvent::Loaded { index_reset, frames, .. }) = rx.recv_timeout(Duration::from_millis(250))
            && index_reset
            && frames.len() == 1
            && frames.last_size == Some((128, 64))
        {
            saw_index_reset = true;
            break;
        }
    }
    assert!(saw_index_reset, "replacing .olof at identical timestamp must emit index_reset=true via new index_gen");
    let (got_ts2, got_frame2) = source.frame_at(base_at).expect("frame_at after index_gen bump").expect("blue frame present");
    assert_eq!(got_ts2, base_at);
    assert_eq!(got_frame2, blue, "must decode fresh pixels from new index_gen, never stale pixels");
    println!("REAL_INDEX_GEN_RESET_OK");

    drop(handle);
    opener.wait_all();
    let _ = std::fs::remove_dir_all(&scratch);
}
