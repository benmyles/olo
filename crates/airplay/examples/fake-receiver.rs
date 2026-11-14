//! A receiver of a test's own, run by hand, for looking at what shows
//! around AirPlay (Preferences ▸ AirPlay, "Play On" in the apps'
//! menus, Deskbar's tray, `olo-airplay`) without a real one:
//! `cargo run --release -p olo-airplay --example fake-receiver --
//! [SECONDS [NAME…]]`. It listens on the loopback only, shows nothing,
//! pairs with the code 1234 and "plays" each film for SECONDS (600 if
//! not said). Each NAME is a receiver ("Front Room" if none is given).
//!
//! It prints the line that points Olo's programs at it in place of the
//! network: `OLO_AIRPLAY_RECEIVERS=…`. With that, `OLO_AIRPLAY_DIR`
//! (where plays are said) and `XDG_STATE_HOME` (the pairings) set to
//! folders of the test's own for what is looked at, nothing of it
//! reaches the person's pairings or tray, and the network isn't asked.
//!
//! A screen shown on it (`olo-airplay show`) is kept, with
//! `OLO_AIRPLAY_KEEP=DIR`: once the sender stops, what came is written
//! there as it came, the pictures as an MP4 (NAME-N.mp4, each shown for
//! as long as its stamp says) and the sound as a WAV (NAME-N.wav), with a
//! line saying how many pictures, how far apart, and how much sound.

use std::{
    path::PathBuf,
    sync::{Arc, Mutex},
    time::Duration,
};

use olo_airplay::{mirror::fake::Shown, pairing, play};

/// The parameter sets out of an `avcC` record.
fn sets(avcc: &[u8]) -> Option<(Vec<u8>, Vec<u8>)> {
    let sps_len = u16::from_be_bytes([*avcc.get(6)?, *avcc.get(7)?]) as usize;
    let sps = avcc.get(8..8 + sps_len)?.to_vec();
    let at = 8 + sps_len + 1;
    let pps_len = u16::from_be_bytes([*avcc.get(at)?, *avcc.get(at + 1)?]) as usize;
    Some((sps, avcc.get(at + 2..at + 2 + pps_len)?.to_vec()))
}

/// Length-prefixed units as Annex B.
fn annex_b(units: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(units.len() + 16);
    let mut at = 0;
    while at + 4 <= units.len() {
        let n = u32::from_be_bytes(units[at..at + 4].try_into().expect("four bytes")) as usize;
        let Some(unit) = units.get(at + 4..at + 4 + n) else { break };
        out.extend([0, 0, 0, 1]);
        out.extend(unit);
        at += 4 + n;
    }
    out
}

fn wav(samples: &[i16]) -> Vec<u8> {
    let data = samples.len() as u32 * 2;
    let mut out = Vec::with_capacity(44 + data as usize);
    out.extend(b"RIFF");
    out.extend((36 + data).to_le_bytes());
    out.extend(b"WAVEfmt ");
    out.extend(16u32.to_le_bytes());
    out.extend(1u16.to_le_bytes());
    out.extend(2u16.to_le_bytes());
    out.extend(44_100u32.to_le_bytes());
    out.extend((44_100u32 * 4).to_le_bytes());
    out.extend(4u16.to_le_bytes());
    out.extend(16u16.to_le_bytes());
    out.extend(b"data");
    out.extend(data.to_le_bytes());
    for s in samples {
        out.extend(s.to_le_bytes());
    }
    out
}

/// What a finished showing sent, written into `dir`.
fn keep(dir: &std::path::Path, name: &str, n: usize, s: &Shown) -> Result<String, String> {
    let Some(((avcc, (w, h)), _)) = s.configurations.first().zip(s.pictures.first()) else { return Ok("nothing shown".into()) };
    let (sps, pps) = sets(avcc).ok_or("a configuration that isn't avcC")?;
    let path = dir.join(format!("{name}-{n}.mp4"));
    let mut mp4 = olo_video::Mp4::create(&path, *w as u32, *h as u32, 1000).map_err(|e| e.to_string())?;
    let ms = |stamp: u64| (stamp >> 32) as f64 * 1000.0 + (stamp & 0xffff_ffff) as f64 * 1000.0 / 4_294_967_296.0;
    let mut gaps = Vec::new();
    for (i, (key, stamp, _, units)) in s.pictures.iter().enumerate() {
        let next = s.pictures.get(i + 1).map_or(ms(*stamp) + 33.0, |p| ms(p.1));
        let shown = (next - ms(*stamp)).max(1.0);
        gaps.push(shown);
        let mut coded = Vec::new();
        if *key {
            for set in [&sps, &pps] {
                coded.extend([0, 0, 0, 1]);
                coded.extend(set);
            }
        }
        coded.extend(annex_b(units));
        mp4.add(&coded, *key, shown.round() as u32).map_err(|e| e.to_string())?;
    }
    mp4.finish().map_err(|e| e.to_string())?;
    std::fs::write(dir.join(format!("{name}-{n}.wav")), wav(&s.samples)).map_err(|e| e.to_string())?;
    gaps.sort_by(f64::total_cmp);
    let keys = s.pictures.iter().filter(|p| p.0).count();
    Ok(format!(
        "{} pictures ({keys} key frames) of {w}×{h}, {:.0} ms apart at the median, {} heartbeats, {:.1} s of sound, {} syncs; in {}",
        s.pictures.len(),
        gaps.get(gaps.len() / 2).copied().unwrap_or(0.0),
        s.heartbeats,
        s.samples.len() as f64 / 2.0 / 44_100.0,
        s.syncs.len(),
        path.display()
    ))
}

fn main() {
    let mut args = std::env::args().skip(1);
    let seconds = args.next().and_then(|s| s.parse::<u64>().ok()).unwrap_or(600);
    let mut names: Vec<String> = args.collect();
    if names.is_empty() {
        names.push("Front Room".into());
    }
    let kept_in = std::env::var_os("OLO_AIRPLAY_KEEP").map(PathBuf::from);
    let receivers: Vec<String> = names
        .iter()
        .map(|name| {
            let tv = Arc::new(Mutex::new(play::fake::Tv::new(Arc::new(Mutex::new(pairing::fake::Receiver::new("1234"))))));
            tv.lock().unwrap().film = Duration::from_secs(seconds);
            if let Some(dir) = kept_in.clone() {
                let (tv, name) = (tv.clone(), name.clone());
                std::thread::spawn(move || {
                    let mut n = 0;
                    loop {
                        std::thread::sleep(Duration::from_millis(500));
                        let shown = tv.lock().unwrap().shown.clone();
                        let mut s = shown.lock().unwrap();
                        if !s.closed {
                            continue;
                        }
                        n += 1;
                        match keep(&dir, &name, n, &s) {
                            Ok(said) => println!("{name}: {said}"),
                            Err(e) => println!("{name}: not kept: {e}"),
                        }
                        *s = Shown::default();
                    }
                });
            }
            format!("{name}={}", play::fake::serve_many(tv))
        })
        .collect();
    println!("{}={}", olo_airplay::RECEIVERS_ENV, receivers.join(";"));
    loop {
        std::thread::park();
    }
}
