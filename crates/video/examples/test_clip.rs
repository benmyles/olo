//! A test clip for players and seeking: `test_clip OUT.mp4 [SECONDS]`
//! writes an H.264 MP4, 320 × 180 at 10 frames a second, its colour a
//! different shade each second (so where it is shows), 60 s unless given.
//! Made here, so tests needn't fetch anyone's video.

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let out = std::path::PathBuf::from(args.first().expect("test_clip OUT.mp4 [SECONDS]"));
    let secs: u32 = args.get(1).and_then(|s| s.parse().ok()).unwrap_or(60);
    let (w, h, fps) = (320u32, 180u32, 10u32);
    let mut enc = olo_video::H264::new(w, h, fps).expect("an H.264 encoder");
    let mut mp4 = olo_video::Mp4::create(&out, w, h, 1000).expect("the file");
    for f in 0..secs * fps {
        let s = f / fps;
        // Each second its own colour, and a bar along the bottom as far as
        // the clip has got.
        let (r, g, b) = ((s * 37 % 200 + 40) as u8, (s * 91 % 200 + 40) as u8, (s * 53 % 200 + 40) as u8);
        let done = (f as u64 * w as u64 / (secs * fps) as u64) as u32;
        let frame: Vec<u8> = (0..h).flat_map(|y| (0..w).flat_map(move |x| if y >= h - 12 && x < done { [255, 255, 255, 255] } else { [r, g, b, 255] })).collect();
        let (coded, key) = enc.encode(&frame, (w * 4) as usize, olo_video::Order::Rgba).expect("a frame");
        mp4.add(&coded, key, 1000 / fps).expect("written");
    }
    mp4.finish().expect("finished");
    eprintln!("{}: {secs} s", out.display());
}
