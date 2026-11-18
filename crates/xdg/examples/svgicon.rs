//! Render one of Olo's icon drawings as Olo does, to look at it:
//! `svgicon IN.svg OUT.png SIZE [SIZE…]` writes OUT.png at SIZE device
//! pixels (several sizes: OUT-SIZE.png each), drawn for the size
//! (`olo_xdg::svg`); `--plain` first renders it as any other program's;
//! `--theme ID` first, in that theme's colours (its `recolour.txt`).
//! `svgicon --time NAME…` measures what drawing costs beside decoding.

/// `svgicon --time NAME…`: how long each of Olo's icons NAME takes to be
/// had at 16, 32, 64 and 256 pixels: its drawing rendered, rendered in
/// Platinum's colours, read back kept, and a picture of that size
/// decoded (what the pre-scaled pictures cost): the median of 200.
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
    // (In a theme's colours: Platinum's, the rules applied and the drawing rendered.)
    let rules = olo_xdg::themes::recolour("platinum");
    println!("{:<16} {:>6} {:>12} {:>12} {:>12} {:>12}", "icon", "px", "drawing µs", "themed µs", "kept µs", "picture µs");
    for name in names {
        let path = data.join("icons/svg").join(format!("{name}.svg"));
        let svg = std::fs::read(&path).expect("its drawing");
        for px in [16u32, 32, 64, 256] {
            let drawn = median(&|| {
                olo_xdg::svg::render(&svg, px, true);
            });
            let themed = rules.map(|r| {
                median(&|| {
                    olo_xdg::svg::render(r.apply(name, &String::from_utf8_lossy(&svg)).as_bytes(), px, true);
                })
            });
            let kept = median(&|| {
                olo_xdg::svg::own_in(&cache, &path, px);
            });
            // (What a picture of that size takes to decode: the drawing's own, as a PNG.)
            let png = cache.join(format!("{name}-{px}.png"));
            let decoded = olo_xdg::svg::render(&svg, px, true).and_then(|i| i.to_png()).and_then(|bytes| std::fs::create_dir_all(&cache).and_then(|()| std::fs::write(&png, bytes)).ok()).map(|()| {
                median(&|| {
                    olo_xdg::decode_png(&png);
                })
            });
            let or_none = |v: Option<f64>| v.map_or("-".into(), |d| format!("{d:.0}"));
            println!("{name:<16} {px:>6} {drawn:>12.0} {:>12} {kept:>12.0} {:>12}", or_none(themed), or_none(decoded));
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
    let theme = (args.first().is_some_and(|a| a == "--theme") && args.len() > 1).then(|| {
        args.remove(0);
        args.remove(0)
    });
    let [input, output, sizes @ ..] = &args[..] else {
        eprintln!("usage: svgicon [--plain | --theme ID] IN.svg OUT.png SIZE [SIZE…]");
        std::process::exit(2);
    };
    let mut data = std::fs::read(input).expect("the drawing");
    if let Some(theme) = &theme {
        let Some(rules) = olo_xdg::themes::recolour(theme) else {
            eprintln!("svgicon: theme {theme} has no colours of its own for the drawings");
            std::process::exit(1);
        };
        let name = std::path::Path::new(input).file_stem().unwrap_or_default().to_string_lossy().into_owned();
        data = rules.apply(&name, &String::from_utf8_lossy(&data)).into_bytes();
    }
    for size in sizes {
        let px: u32 = size.parse().expect("a size in pixels");
        let Some(img) = olo_xdg::svg::render(&data, px, own) else {
            eprintln!("svgicon: {input} isn't an SVG that can be drawn");
            std::process::exit(1);
        };
        let path = if sizes.len() > 1 { output.replace(".png", &format!("-{px}.png")) } else { output.clone() };
        std::fs::write(&path, img.to_png().expect("the picture as a PNG")).expect("the picture written");
    }
}
