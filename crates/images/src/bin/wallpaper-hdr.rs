//! The wallpaper pack's HDR versions: a gain map beside each picture
//! (`olo_images::gain`), made from the picture itself.
//!
//!   wallpaper-hdr [DIR] [--preview OUT]
//!
//! DIR is the pack (`data/wallpapers` unless given): every picture in its
//! `4k/` and `ultrawide/` gets `NAME.gain.png` beside it, made again only
//! where the picture is newer. `--preview OUT` also writes, for each, the
//! picture with what shines tinted (red at the most), to look at.
//! (Not an `olo-` program: the session doesn't install it.)

use std::path::{Path, PathBuf};

fn pictures(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for sub in ["4k", "ultrawide"] {
        let Ok(entries) = std::fs::read_dir(dir.join(sub)) else { continue };
        for e in entries.flatten() {
            let p = e.path();
            let name = p.file_name().and_then(|n| n.to_str()).unwrap_or_default();
            if name.ends_with(".png") && !name.ends_with(".gain.png") {
                out.push(p);
            }
        }
    }
    out.sort();
    out
}

fn newer(a: &Path, b: &Path) -> bool {
    let m = |p: &Path| std::fs::metadata(p).and_then(|m| m.modified()).ok();
    match (m(a), m(b)) {
        (Some(a), Some(b)) => a > b,
        _ => true,
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let preview = args.iter().position(|a| a == "--preview").and_then(|i| args.get(i + 1)).map(PathBuf::from);
    let dir = args.iter().enumerate().find(|(i, a)| !a.starts_with("--") && (*i == 0 || args[i - 1] != "--preview")).map(|(_, a)| PathBuf::from(a)).unwrap_or_else(|| PathBuf::from("data/wallpapers"));
    if let Some(p) = &preview {
        let _ = std::fs::create_dir_all(p);
    }
    let all = pictures(&dir);
    if all.is_empty() {
        eprintln!("wallpaper-hdr: no pictures in {}/4k or ultrawide", dir.display());
        std::process::exit(1);
    }
    for picture in all {
        let gain_path = olo_images::gain::path_for(&picture);
        if !newer(&picture, &gain_path) && preview.is_none() {
            continue;
        }
        let start = std::time::Instant::now();
        let img = match image::open(&picture) {
            Ok(i) => i.into_rgba8(),
            Err(e) => {
                eprintln!("{}: {e}", picture.display());
                continue;
            }
        };
        let map = olo_images::gain::make(&img);
        if let Err(e) = map.save(&gain_path) {
            eprintln!("{}: {e}", gain_path.display());
            continue;
        }
        let mean = map.pixels().map(|p| p[0] as f64).sum::<f64>() / (map.width() * map.height()) as f64;
        let lit = map.pixels().filter(|p| p[0] > 128).count() as f64 / (map.width() * map.height()) as f64;
        println!(
            "{}: {} KB, mean {:.2} stops, {:.2}% above one stop, {:.1} s",
            gain_path.display(),
            std::fs::metadata(&gain_path).map(|m| m.len() / 1024).unwrap_or(0),
            mean / 255.0 * olo_images::gain::MAX_STOPS as f64,
            lit * 100.0,
            start.elapsed().as_secs_f64()
        );
        if let Some(out) = &preview {
            // The picture at a quarter size, tinted red where it shines.
            let small = image::imageops::resize(&img, img.width() / 4, img.height() / 4, image::imageops::FilterType::Triangle);
            let g = image::imageops::resize(&map, small.width(), small.height(), image::imageops::FilterType::Triangle);
            let tinted = image::RgbaImage::from_fn(small.width(), small.height(), |x, y| {
                let p = small.get_pixel(x, y).0;
                let t = g.get_pixel(x, y)[0] as f32 / 255.0;
                let mix = |c: u8, to: f32| ((c as f32) * (1.0 - t) + to * t) as u8;
                image::Rgba([mix(p[0], 255.0), mix(p[1], 0.0), mix(p[2], 0.0), 255])
            });
            let name = picture.strip_prefix(&dir).unwrap_or(&picture).to_string_lossy().replace('/', "-");
            let _ = tinted.save(out.join(name));
        }
    }
}
