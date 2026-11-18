//! Render one of Olo's icon drawings as Olo does, to look at it:
//! `svgicon IN.svg OUT.png SIZE [SIZE…]` writes OUT.png at SIZE device
//! pixels (several sizes: OUT-SIZE.png each), drawn for the size
//! (`olo_xdg::svg`); `--plain` first renders it as any other program's.
//! `svgicon --time NAME…` measures what drawing costs beside decoding.

/// `svgicon --time NAME…`: how long each of Olo's icons NAME takes to be
/// had at 16, 32, 64 and 256 pixels, its drawing rendered and (where it
/// still has them) its pre-scaled picture decoded: the median of 200.
fn time(names: &[String]) {
    let data = olo_xdg::data_dirs().first().cloned().expect("Olo's data");
    let median = |f: &dyn Fn()| {
        let mut all: Vec<f64> = (0..200)
            .map(|_| {
                let t = std::time::Instant::now();
                f();
                t.elapsed().as_secs_f64() * 1e6
            })
            .collect();
        all.sort_by(f64::total_cmp);
        all[all.len() / 2]
    };
    // (What's kept, kept in scratch: never in the person's cache.)
    let cache = std::path::Path::new("/var/tmp").join(format!("olo-svgicon-{}", std::process::id()));
    println!("{:<16} {:>6} {:>12} {:>12} {:>12}", "icon", "px", "drawing µs", "kept µs", "picture µs");
    for name in names {
        let path = data.join("icons/svg").join(format!("{name}.svg"));
        let svg = std::fs::read(&path).expect("its drawing");
        for px in [16u32, 32, 64, 256] {
            let drawn = median(&|| {
                olo_xdg::svg::render(&svg, px, true);
            });
            let kept = median(&|| {
                olo_xdg::svg::own_in(&cache, &path, px);
            });
            let png = data.join("icons").join(px.to_string()).join(format!("{name}.png"));
            let decoded = png.is_file().then(|| {
                median(&|| {
                    olo_xdg::decode_png(&png);
                })
            });
            println!("{name:<16} {px:>6} {drawn:>12.0} {kept:>12.0} {:>12}", decoded.map_or("-".into(), |d| format!("{d:.0}")));
        }
    }
    let _ = std::fs::remove_dir_all(&cache);
}

fn main() {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().is_some_and(|a| a == "--time") {
        return time(&args[1..]);
    }
    let own = if args.first().is_some_and(|a| a == "--plain") {
        args.remove(0);
        false
    } else {
        true
    };
    let [input, output, sizes @ ..] = &args[..] else {
        eprintln!("usage: svgicon [--plain] IN.svg OUT.png SIZE [SIZE…]");
        std::process::exit(2);
    };
    let data = std::fs::read(input).expect("the drawing");
    for size in sizes {
        let px: u32 = size.parse().expect("a size in pixels");
        let Some(img) = olo_xdg::svg::render(&data, px, own) else {
            eprintln!("svgicon: {input} isn't an SVG that can be drawn");
            std::process::exit(1);
        };
        // Straight alpha, as PNG has it.
        let mut rgba = img.rgba;
        for p in rgba.chunks_exact_mut(4) {
            let a = u32::from(p[3]);
            if a > 0 && a < 255 {
                for c in &mut p[..3] {
                    *c = ((u32::from(*c) * 255 + a / 2) / a).min(255) as u8;
                }
            }
        }
        let path = if sizes.len() > 1 { output.replace(".png", &format!("-{px}.png")) } else { output.clone() };
        let mut encoder = png::Encoder::new(std::io::BufWriter::new(std::fs::File::create(&path).expect("the picture's file")), px, px);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        encoder.write_header().and_then(|mut w| w.write_image_data(&rgba)).expect("the picture written");
    }
}
