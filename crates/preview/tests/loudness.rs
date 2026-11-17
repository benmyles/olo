//! A sound of a set loudness (`audio::Player::start_at`, the start-up
//! chime's way) against the stand-in sound server (`olo-fakesound`,
//! which plays nothing and says what it was sent): the stream's own
//! volume makes up for the output's, follows it when it changes, isn't
//! made at all when the output is muted, and an output's volume lent to
//! a sound is put back. One test: the server's address is the process's
//! environment.

use std::{
    io::{BufRead, BufReader},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::{Arc, Mutex, atomic::Ordering},
    time::{Duration, Instant},
};

use olo_preview::{audio, input::Input};

/// A stand-in server on `socket` with `settings`, and what it has said.
struct Server {
    child: Child,
    said: Arc<Mutex<Vec<String>>>,
}

impl Server {
    fn start(socket: &Path, settings: &[(&str, &str)]) -> Server {
        let _ = std::fs::remove_file(socket);
        let mut child = Command::new(env!("CARGO_BIN_EXE_olo-fakesound")).arg(socket).envs(settings.iter().copied()).stderr(Stdio::piped()).spawn().expect("olo-fakesound");
        let said = Arc::new(Mutex::new(Vec::new()));
        let (lines, err) = (said.clone(), child.stderr.take().unwrap());
        std::thread::spawn(move || {
            for line in BufReader::new(err).lines().map_while(Result::ok) {
                lines.lock().unwrap().push(line);
            }
        });
        let start = Instant::now();
        while !socket.exists() {
            assert!(start.elapsed() < Duration::from_secs(10), "olo-fakesound didn't start");
            std::thread::sleep(Duration::from_millis(20));
        }
        Server { child, said }
    }

    /// Whether it has said a line with `words` in it, waited for `within`.
    fn says(&self, words: &str, within: Duration) -> bool {
        let start = Instant::now();
        loop {
            if self.said.lock().unwrap().iter().any(|l| l.contains(words)) {
                return true;
            }
            if start.elapsed() >= within {
                return false;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    fn all(&self) -> String {
        self.said.lock().unwrap().join("\n")
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// `seconds` of a 440 Hz tone at half scale, as a WAV file.
fn tone(path: &Path, seconds: f32) {
    let rate = 48_000u32;
    let samples: Vec<i16> = (0..(seconds * rate as f32) as u32).flat_map(|i| [((i as f32 * 440.0 * std::f32::consts::TAU / rate as f32).sin() * 16384.0) as i16; 2]).collect();
    let data = (samples.len() * 2) as u32;
    let mut wav = Vec::new();
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&(36 + data).to_le_bytes());
    wav.extend_from_slice(b"WAVEfmt ");
    for field in [16u32.to_le_bytes().as_slice(), &1u16.to_le_bytes(), &2u16.to_le_bytes(), &rate.to_le_bytes(), &(rate * 4).to_le_bytes(), &4u16.to_le_bytes(), &16u16.to_le_bytes()] {
        wav.extend_from_slice(field);
    }
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&data.to_le_bytes());
    wav.extend(samples.iter().flat_map(|s| s.to_le_bytes()));
    std::fs::write(path, wav).unwrap();
}

/// Play `file` so that it comes out at a quarter of the output's full
/// volume (none muted), to its end.
fn play(file: &Path) {
    let player = audio::Player::start_at(&Input::file(file), Arc::new(|o: audio::Output| (!o.muted).then(|| 0.25 / o.level)));
    let start = Instant::now();
    while !player.state.finished.load(Ordering::Relaxed) && player.state.error.lock().unwrap().is_none() {
        assert!(start.elapsed() < Duration::from_secs(20), "it didn't finish");
        std::thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(*player.state.error.lock().unwrap(), None);
}

#[test]
fn a_sound_of_a_set_loudness() {
    let dir = PathBuf::from(format!("/var/tmp/olo-preview-loudness-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let (socket, file) = (dir.join("sound.sock"), dir.join("tone.wav"));
    tone(&file, 2.5);
    // SAFETY: the only test in this process; set before any thread of
    // ours reads the environment.
    unsafe {
        std::env::set_var("PULSE_SERVER", format!("unix:{}", socket.display()));
        std::env::set_var("PULSE_RUNTIME_PATH", &dir);
    }
    let soon = Duration::from_secs(5);

    // The output at 44%: the stream at 56.8% of it, a quarter in all.
    {
        let server = Server::start(&socket, &[("FAKESOUND_VOLUME", "44")]);
        assert_eq!(audio::output().map(|o| ((o.level * 1000.0).round(), o.muted, o.hardware)), Ok((440.0, false, false)));
        play(&file);
        assert!(server.says("at 56.8% of the speaker's 44.0%", soon), "{}", server.all());
        // (Any other sound: as it is, at the output's volume.)
        let plain = audio::Player::start(&Input::file(&file));
        assert!(server.says("at 100.0% of the speaker's 44.0%", soon), "{}", server.all());
        drop(plain);
    }
    // The output turned down while it plays: the stream follows.
    {
        let server = Server::start(&socket, &[("FAKESOUND_VOLUME", "100"), ("FAKESOUND_VOLUME_THEN", "30@700")]);
        play(&file);
        assert!(server.says("at 25.0% of the speaker's 100.0%", soon), "{}", server.all());
        assert!(server.says("now at 83.3% of the speaker's 30.0%", soon), "{}", server.all());
    }
    // Muted: no stream at all.
    {
        let server = Server::start(&socket, &[("FAKESOUND_VOLUME", "44"), ("FAKESOUND_MUTED", "1")]);
        play(&file);
        assert!(!server.says("stream", Duration::from_millis(300)), "{}", server.all());
    }
    // An output's volume lent and put back; left alone if it was changed
    // meanwhile (here, by a second lending).
    {
        let server = Server::start(&socket, &[("FAKESOUND_VOLUME", "10"), ("FAKESOUND_HARDWARE", "1")]);
        assert_eq!(audio::output().map(|o| o.hardware), Ok(true));
        let lent = audio::lend_output(0.25).expect("lent");
        assert!(server.says("set to 25.0% (it was 10.0%)", soon), "{}", server.all());
        drop(lent);
        assert!(server.says("set to 10.0% (it was 25.0%)", soon), "{}", server.all());
        let first = audio::lend_output(0.25).expect("lent");
        let second = audio::lend_output(0.5).expect("lent again");
        drop(first);
        assert!(!server.says("set to 10.0% (it was 50.0%)", Duration::from_millis(300)), "changed since: not put back\n{}", server.all());
        drop(second);
        assert!(server.says("set to 25.0% (it was 50.0%)", soon), "{}", server.all());
    }
    let _ = std::fs::remove_dir_all(&dir);
}
