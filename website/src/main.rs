//! Builds olo-desktop.com, Olo's website, as a static site in
//! `website/dist`, to host anywhere (any web server or static host: plain
//! files, pretty URLs as `NAME/index.html`, a `404.html`).
//!
//! ```sh
//! cargo run --release                 # build into dist/
//! cargo run --release -- serve [PORT] # build, then serve dist/ on localhost (8000)
//! ```
//!
//! What it does (see website/README.md):
//! - each page in `pages/` (front matter: `title`, `description`) is put
//!   in `templates/layout.html`, its shortcodes expanded:
//!   `{{shot NAME | ALT [| eager]}}` a screenshot from `docs/img/NAME.png`
//!   (and `NAME-dark.png`, shown in dark mode when there is one),
//!   `{{icon NAME [PX]}}` an icon from `data/icons`, `{{wallpaper NAME |
//!   LABEL}}` a picture from the wallpaper pack (with its dark version);
//! - screenshots and wallpapers are made at the widths the site shows,
//!   as lossless WebP (no compression artifacts, on a page that shows
//!   how sharp Olo is), and kept: one is made again only when its source
//!   is newer;
//! - `static/` is copied as it is (styles, fonts, the brand kit made by
//!   tools/mkbrand.py), `sitemap.xml`, `robots.txt` and `CNAME` are
//!   written, and what an earlier build left in `dist` that this one
//!   doesn't make is removed.
//!
//! Its pictures come from the repository itself (docs/img, data/icons,
//! data/wallpapers), so the site shows what Olo is now.

use std::{
    collections::BTreeMap,
    fs,
    io::{BufRead, BufReader, Write},
    net::{TcpListener, TcpStream},
    path::{Path, PathBuf},
    sync::Mutex,
    time::SystemTime,
};

use image::{ImageEncoder, codecs::webp::WebPEncoder, imageops::FilterType};

const SITE: &str = "https://olo-desktop.com";
const DOMAIN: &str = "olo-desktop.com";
/// Widths a screenshot is made at (each no wider than its source).
const SHOT_WIDTHS: &[u32] = &[960, 1600, 2400, 3200];
/// Widths a wallpaper's picture is made at.
const WALLPAPER_WIDTHS: &[u32] = &[640, 1280];

fn site() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn repo() -> PathBuf {
    site().parent().expect("the website is in Olo's repository").to_path_buf()
}

fn dist() -> PathBuf {
    site().join("dist")
}

/// A picture to make: `src` at `width`, as lossless WebP, to `out`.
struct Job {
    src: PathBuf,
    out: PathBuf,
    width: u32,
}

#[derive(Default)]
struct Build {
    jobs: Vec<Job>,
    /// Files to copy as they are: destination → source.
    copies: BTreeMap<PathBuf, PathBuf>,
    /// Pages for the sitemap, by URL path.
    urls: Vec<String>,
    /// Every file this build puts in dist/ (what else is there goes).
    made: std::collections::BTreeSet<PathBuf>,
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let started = std::time::Instant::now();
    if let Err(e) = build() {
        eprintln!("olo-website: {e}");
        std::process::exit(1);
    }
    println!("built {} in {:.1} s", dist().display(), started.elapsed().as_secs_f32());
    if args.first().map(String::as_str) == Some("serve") {
        let port: u16 = args.get(1).and_then(|p| p.parse().ok()).unwrap_or(8000);
        serve(port);
    }
}

fn build() -> Result<(), String> {
    let out = dist();
    fs::create_dir_all(&out).map_err(|e| format!("{}: {e}", out.display()))?;
    let mut b = Build::default();
    let layout = read(&site().join("templates/layout.html"))?;

    let mut pages: Vec<PathBuf> = fs::read_dir(site().join("pages")).map_err(|e| e.to_string())?.flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x == "html")).collect();
    pages.sort();
    for page in pages {
        let stem = page.file_stem().unwrap().to_string_lossy().into_owned();
        let (meta, body) = front_matter(&read(&page)?)?;
        let body = shortcodes(&body, &mut b).map_err(|e| format!("{}: {e}", page.display()))?;
        let (url, dest) = match stem.as_str() {
            "index" => ("/".to_owned(), out.join("index.html")),
            "404" => ("/404.html".to_owned(), out.join("404.html")),
            s => (format!("/{s}/"), out.join(s).join("index.html")),
        };
        let mut html = layout.clone();
        for (k, v) in &meta {
            html = html.replace(&format!("{{{{{k}}}}}"), v);
        }
        html = html.replace("{{url}}", &format!("{SITE}{url}")).replace("{{content}}", &body).replace("{{page}}", &stem);
        // The navigation's current page.
        for name in ["features", "apps", "agents", "install", "brand"] {
            let current = if name == stem { " aria-current=\"page\"" } else { "" };
            html = html.replace(&format!("{{{{current:{name}}}}}"), current);
        }
        if let Some(left) = html.find("{{") {
            return Err(format!("{}: unknown placeholder at {:?}", page.display(), &html[left..(left + 40).min(html.len())]));
        }
        write(&dest, html.as_bytes())?;
        b.made.insert(dest);
        if stem != "404" {
            b.urls.push(url);
        }
    }

    copy_tree(&site().join("static"), &out, &mut b)?;
    b.made.extend(b.copies.keys().cloned());
    b.made.extend(b.jobs.iter().map(|j| j.out.clone()));
    for (to, from) in &b.copies {
        if newer(from, to) {
            fs::create_dir_all(to.parent().unwrap()).map_err(|e| e.to_string())?;
            fs::copy(from, to).map_err(|e| format!("{} → {}: {e}", from.display(), to.display()))?;
        }
    }
    make_pictures(std::mem::take(&mut b.jobs))?;
    hdr_mark(&mut b)?;

    let today = date_today();
    let mut sitemap = String::from("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<urlset xmlns=\"http://www.sitemaps.org/schemas/sitemap/0.9\">\n");
    for u in &b.urls {
        sitemap += &format!("  <url><loc>{SITE}{u}</loc><lastmod>{today}</lastmod></url>\n");
    }
    sitemap += "</urlset>\n";
    write(&out.join("sitemap.xml"), sitemap.as_bytes())?;
    write(&out.join("robots.txt"), format!("User-agent: *\nAllow: /\nSitemap: {SITE}/sitemap.xml\n").as_bytes())?;
    write(&out.join("CNAME"), format!("{DOMAIN}\n").as_bytes())?;
    b.made.extend(["sitemap.xml", "robots.txt", "CNAME"].map(|f| out.join(f)));
    // What an earlier build left that this one doesn't make (a picture
    // no page shows any more, a page that went): gone, so dist/ is the
    // site and nothing else.
    let stale = sweep(&out, &b.made)?;
    if stale > 0 {
        println!("removed {stale} files this build doesn't make");
    }
    Ok(())
}

/// Removes the files under `dir` that aren't in `made`, and the folders
/// that leaves empty; how many files.
fn sweep(dir: &Path, made: &std::collections::BTreeSet<PathBuf>) -> Result<usize, String> {
    let mut n = 0;
    for e in fs::read_dir(dir).map_err(|e| format!("{}: {e}", dir.display()))?.flatten() {
        let p = e.path();
        if p.is_dir() {
            n += sweep(&p, made)?;
            // (Only if it's empty now.)
            let _ = fs::remove_dir(&p);
        } else if !made.contains(&p) {
            fs::remove_file(&p).map_err(|e| format!("{}: {e}", p.display()))?;
            n += 1;
        }
    }
    Ok(n)
}

fn read(p: &Path) -> Result<String, String> {
    fs::read_to_string(p).map_err(|e| format!("{}: {e}", p.display()))
}

fn write(p: &Path, data: &[u8]) -> Result<(), String> {
    fs::create_dir_all(p.parent().unwrap()).map_err(|e| e.to_string())?;
    // Unchanged files keep their time (a host syncing by it sends less).
    if fs::read(p).is_ok_and(|old| old == data) {
        return Ok(());
    }
    fs::write(p, data).map_err(|e| format!("{}: {e}", p.display()))
}

/// `---` lines `key: value` `---`, then the page.
fn front_matter(text: &str) -> Result<(BTreeMap<String, String>, String), String> {
    let mut meta = BTreeMap::new();
    let Some(rest) = text.strip_prefix("---\n") else {
        return Ok((meta, text.to_owned()));
    };
    let end = rest.find("\n---\n").ok_or("front matter without its closing ---")?;
    for line in rest[..end].lines() {
        if let Some((k, v)) = line.split_once(':') {
            meta.insert(k.trim().to_owned(), escape(v.trim()));
        }
    }
    for k in ["title", "description"] {
        if !meta.contains_key(k) {
            return Err(format!("front matter without {k}"));
        }
    }
    Ok((meta, rest[end + 5..].to_owned()))
}

fn escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('"', "&quot;").replace('<', "&lt;").replace('>', "&gt;")
}

/// Expands `{{shot …}}`, `{{icon …}}` and `{{wallpaper …}}`.
fn shortcodes(body: &str, b: &mut Build) -> Result<String, String> {
    let mut out = String::with_capacity(body.len());
    let mut rest = body;
    while let Some(i) = rest.find("{{") {
        out += &rest[..i];
        let j = rest[i..].find("}}").ok_or("a {{ without its }}")? + i;
        let inner = rest[i + 2..j].trim();
        let (kind, args) = inner.split_once(' ').unwrap_or((inner, ""));
        let parts: Vec<&str> = args.split('|').map(str::trim).collect();
        out += &match kind {
            "shot" => shot(parts[0], parts.get(1).copied().unwrap_or(""), parts.get(2) == Some(&"eager"), b)?,
            "icon" => {
                let (name, size) = parts[0].split_once(' ').map_or((parts[0], 64), |(n, s)| (n, s.trim().parse().unwrap_or(64)));
                icon(name, size, b)?
            }
            "wallpaper" => wallpaper(parts[0], parts.get(1).copied().unwrap_or(parts[0]), b)?,
            // Not ours (the layout's): left for it.
            _ => rest[i..j + 2].to_owned(),
        };
        rest = &rest[j + 2..];
    }
    out += rest;
    Ok(out)
}

fn dimensions(p: &Path) -> Result<(u32, u32), String> {
    image::image_dimensions(p).map_err(|e| format!("{}: {e}", p.display()))
}

/// The widths `widths` a picture `w` wide is made at (at least its own
/// width when it's narrower than them all).
fn widths_for(w: u32, widths: &[u32]) -> Vec<u32> {
    let mut v: Vec<u32> = widths.iter().copied().filter(|&x| x <= w).collect();
    if v.is_empty() {
        v.push(w);
    }
    v
}

/// A `srcset` of `src` at its widths, made into `dir` as `stem-W.webp`.
fn srcset(src: &Path, dir: &str, stem: &str, widths: &[u32], b: &mut Build) -> Result<(String, (u32, u32)), String> {
    let (w, h) = dimensions(src)?;
    let mut set = Vec::new();
    for width in widths_for(w, widths) {
        let name = format!("{stem}-{width}.webp");
        b.jobs.push(Job { src: src.to_owned(), out: dist().join(dir.trim_start_matches('/')).join(&name), width });
        set.push(format!("{dir}/{name} {width}w"));
    }
    Ok((set.join(", "), (w, h)))
}

fn shot(name: &str, alt: &str, eager: bool, b: &mut Build) -> Result<String, String> {
    let img = repo().join("docs/img");
    let light = ["png", "jpg"].iter().map(|x| img.join(format!("{name}.{x}"))).find(|p| p.is_file()).ok_or_else(|| format!("no docs/img/{name}.png"))?;
    let (set, (w, h)) = srcset(&light, "/img/shots", name, SHOT_WIDTHS, b)?;
    let sizes = "(min-width: 1240px) 1200px, 100vw";
    // A link to the picture at its largest: a click shows it as large as
    // the window has room for (js/site.js, in a <dialog>); with no
    // script, the link opens the picture itself.
    let largest = widths_for(w, SHOT_WIDTHS).last().copied().unwrap_or(w);
    let mut html = format!("<a class=\"zoom\" href=\"/img/shots/{name}-{largest}.webp\" aria-label=\"Enlarge: {}\"><picture>", escape(alt));
    if let Some(dark) = ["png", "jpg"].iter().map(|x| img.join(format!("{name}-dark.{x}"))).find(|p| p.is_file()) {
        let (dset, _) = srcset(&dark, "/img/shots", &format!("{name}-dark"), SHOT_WIDTHS, b)?;
        html += &format!("<source media=\"(prefers-color-scheme: dark)\" srcset=\"{dset}\" sizes=\"{sizes}\">");
    }
    let loading = if eager { "eager\" fetchpriority=\"high" } else { "lazy" };
    html += &format!(
        "<img src=\"/img/shots/{name}-{}.webp\" srcset=\"{set}\" sizes=\"{sizes}\" width=\"{w}\" height=\"{h}\" alt=\"{}\" loading=\"{loading}\" decoding=\"async\"></picture></a>",
        widths_for(w, SHOT_WIDTHS)[0],
        escape(alt)
    );
    Ok(html)
}

/// An icon from Olo's set, `size` CSS pixels, sharp at 1× and 2×.
fn icon(name: &str, size: u32, b: &mut Build) -> Result<String, String> {
    let icons = repo().join("data/icons");
    let mut pick = |px: u32| -> Result<String, String> {
        let px = [16, 24, 32, 48, 64, 96, 128, 256].into_iter().find(|&s| s >= px).unwrap_or(256);
        let src = icons.join(px.to_string()).join(format!("{name}.png"));
        if !src.is_file() {
            return Err(format!("no icon {name} at {px}"));
        }
        let rel = format!("img/icons/{px}/{name}.png");
        b.copies.insert(dist().join(&rel), src);
        Ok(format!("/{rel}"))
    };
    let (one, two) = (pick(size)?, pick(size * 2)?);
    Ok(format!("<img class=\"icon\" src=\"{one}\" srcset=\"{one} 1x, {two} 2x\" width=\"{size}\" height=\"{size}\" alt=\"\" loading=\"lazy\">"))
}

fn wallpaper(name: &str, label: &str, b: &mut Build) -> Result<String, String> {
    let dir = repo().join("data/wallpapers/4k");
    let find = |stem: &str| ["webp", "png", "jpg"].iter().map(|x| dir.join(format!("{stem}.{x}"))).find(|p| p.is_file());
    let light = find(name).ok_or_else(|| format!("no wallpaper {name}"))?;
    let sizes = "(min-width: 1240px) 380px, (min-width: 700px) 45vw, 100vw";
    let (set, (w, h)) = srcset(&light, "/img/wallpapers", name, WALLPAPER_WIDTHS, b)?;
    let mut html = String::from("<figure class=\"wallpaper\"><picture>");
    if let Some(dark) = find(&format!("{name}-dark")) {
        let (dset, _) = srcset(&dark, "/img/wallpapers", &format!("{name}-dark"), WALLPAPER_WIDTHS, b)?;
        html += &format!("<source media=\"(prefers-color-scheme: dark)\" srcset=\"{dset}\" sizes=\"{sizes}\">");
    }
    html += &format!(
        "<img src=\"/img/wallpapers/{name}-{}.webp\" srcset=\"{set}\" sizes=\"{sizes}\" width=\"{w}\" height=\"{h}\" alt=\"The {} wallpaper\" loading=\"lazy\" decoding=\"async\"></picture><figcaption>{}</figcaption></figure>",
        WALLPAPER_WIDTHS[0],
        escape(label),
        escape(label)
    );
    Ok(html)
}

fn copy_tree(from: &Path, to: &Path, b: &mut Build) -> Result<(), String> {
    for e in fs::read_dir(from).map_err(|e| format!("{}: {e}", from.display()))?.flatten() {
        let p = e.path();
        if p.is_dir() {
            copy_tree(&p, &to.join(e.file_name()), b)?;
        } else {
            b.copies.insert(to.join(e.file_name()), p);
        }
    }
    Ok(())
}

fn modified(p: &Path) -> Option<SystemTime> {
    fs::metadata(p).and_then(|m| m.modified()).ok()
}

/// Whether `to` is missing or older than `from`.
fn newer(from: &Path, to: &Path) -> bool {
    match (modified(from), modified(to)) {
        (Some(a), Some(b)) => a > b,
        _ => true,
    }
}

/// Makes the pictures that are missing or out of date, on every core.
fn make_pictures(mut jobs: Vec<Job>) -> Result<(), String> {
    jobs.retain(|j| newer(&j.src, &j.out));
    jobs.sort_by(|a, b| a.out.cmp(&b.out));
    jobs.dedup_by(|a, b| a.out == b.out);
    if jobs.is_empty() {
        return Ok(());
    }
    println!("making {} pictures", jobs.len());
    // Each source decoded once, for all its widths.
    let mut by_src: BTreeMap<PathBuf, Vec<Job>> = BTreeMap::new();
    for j in jobs {
        by_src.entry(j.src.clone()).or_default().push(j);
    }
    let queue = Mutex::new(by_src.into_iter().collect::<Vec<_>>());
    let errors = Mutex::new(Vec::new());
    let threads = std::thread::available_parallelism().map_or(4, |n| n.get());
    std::thread::scope(|s| {
        for _ in 0..threads {
            s.spawn(|| {
                loop {
                    let Some((src, jobs)) = queue.lock().unwrap().pop() else { break };
                    if let Err(e) = make(&src, &jobs) {
                        errors.lock().unwrap().push(e);
                    }
                }
            });
        }
    });
    let errors = errors.into_inner().unwrap();
    if errors.is_empty() { Ok(()) } else { Err(errors.join("\n")) }
}

fn make(src: &Path, jobs: &[Job]) -> Result<(), String> {
    let img = image::open(src).map_err(|e| format!("{}: {e}", src.display()))?.to_rgb8();
    for j in jobs {
        let h = (img.height() as f64 * j.width as f64 / img.width() as f64).round() as u32;
        let small = if j.width == img.width() { img.clone() } else { image::imageops::resize(&img, j.width, h, FilterType::Lanczos3) };
        let mut data = Vec::new();
        WebPEncoder::new_lossless(&mut data).write_image(small.as_raw(), small.width(), small.height(), image::ExtendedColorType::Rgb8).map_err(|e| e.to_string())?;
        write(&j.out, &data)?;
        println!("  {}", j.out.strip_prefix(dist()).unwrap_or(&j.out).display());
    }
    Ok(())
}

/// The ring in HDR (Ben, 2026-11-17: "make the large o ring logo on the
/// website hdr enabled"), for screens that show HDR (the page asks for it
/// with `media="(dynamic-range: high)"`; others never load it): the same
/// picture, its colours where they are (SDR white at BT.2408's 203 nits),
/// its brighter parts lifted towards `HDR_HEADROOM` times that: the
/// glossy highlights to about 810 nits, the yellow's light a little, the
/// blue not at all. A 16-bit PNG in PQ (SMPTE ST 2084) with BT.709's
/// primaries, which its `cICP` chunk says (PNG's third edition: Chrome
/// and Safari show it as HDR), and its brightest in `cLLi`. 512 points
/// across: the hero shows it at most 184 CSS pixels wide, 460 device
/// pixels at 2.5x. Made again when the ring is newer (a new way of
/// making it: a new `HDR_MARK` name, which caches see as new too).
const HDR_MARK: &str = "brand/olo-mark-hdr-1.png";
const HDR_MARK_SIZE: u32 = 512;
/// SDR white, in nits (ITU-R BT.2408).
const SDR_WHITE: f32 = 203.0;
/// The brightest the ring goes, as a multiple of SDR white.
const HDR_HEADROOM: f32 = 4.0;
/// From where (linear luminance, 0 to 1) the light is lifted, up to the
/// full headroom at 1.
const HDR_FROM: f32 = 0.6;

fn hdr_mark(b: &mut Build) -> Result<(), String> {
    let src = site().join("static/brand/olo-mark.png");
    let out = dist().join(HDR_MARK);
    b.made.insert(out.clone());
    if !newer(&src, &out) {
        return Ok(());
    }
    let img = image::open(&src).map_err(|e| format!("{}: {e}", src.display()))?.to_rgba8();
    // Linear light, its alpha premultiplied, to be made smaller without
    // dark edges.
    let linear = |v: u8| {
        let c = f32::from(v) / 255.0;
        if c <= 0.04045 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) }
    };
    let mut light = image::Rgba32FImage::new(img.width(), img.height());
    for (x, y, p) in img.enumerate_pixels() {
        let a = f32::from(p[3]) / 255.0;
        light.put_pixel(x, y, image::Rgba([linear(p[0]) * a, linear(p[1]) * a, linear(p[2]) * a, a]));
    }
    let (w, h) = (HDR_MARK_SIZE, (HDR_MARK_SIZE as f64 * img.height() as f64 / img.width() as f64).round() as u32);
    let small = image::imageops::resize(&light, w, h, FilterType::Lanczos3);
    // PQ: nits to its signal, 0 to 1.
    let pq = |nits: f32| {
        let (m1, m2, c1, c2, c3) = (0.159_301_76_f32, 78.843_75_f32, 0.835_937_5_f32, 18.851_562_f32, 18.6875_f32);
        let l = (nits / 10_000.0).clamp(0.0, 1.0).powf(m1);
        ((c1 + c2 * l) / (1.0 + c3 * l)).powf(m2)
    };
    let mut data = Vec::with_capacity((w * h * 8) as usize);
    let mut brightest: f32 = 0.0;
    for p in small.pixels() {
        let a = p[3].clamp(0.0, 1.0);
        let rgb = if a > 0.0 { [p[0] / a, p[1] / a, p[2] / a].map(|c| c.clamp(0.0, 1.0)) } else { [0.0; 3] };
        let y = 0.2126 * rgb[0] + 0.7152 * rgb[1] + 0.0722 * rgb[2];
        let t = ((y - HDR_FROM) / (1.0 - HDR_FROM)).clamp(0.0, 1.0);
        let gain = 1.0 + (HDR_HEADROOM - 1.0) * t * t * (3.0 - 2.0 * t);
        for c in rgb {
            let nits = c * SDR_WHITE * gain;
            if a > 0.5 {
                brightest = brightest.max(nits);
            }
            data.extend_from_slice(&((pq(nits) * 65535.0).round() as u16).to_be_bytes());
        }
        data.extend_from_slice(&((a * 65535.0).round() as u16).to_be_bytes());
    }
    let mut bytes = Vec::new();
    {
        let mut e = png::Encoder::new(&mut bytes, w, h);
        e.set_color(png::ColorType::Rgba);
        e.set_depth(png::BitDepth::Sixteen);
        e.set_compression(png::Compression::High);
        let mut out = e.write_header().map_err(|e| e.to_string())?;
        // BT.709 primaries (1), PQ (16), RGB (0), full range (1).
        out.write_chunk(png::chunk::ChunkType(*b"cICP"), &[1, 16, 0, 1]).map_err(|e| e.to_string())?;
        // Its brightest pixel and its average, in 0.0001 nits.
        let cll = (brightest.round() as u32) * 10_000;
        let mut cl = cll.to_be_bytes().to_vec();
        cl.extend_from_slice(&((SDR_WHITE as u32) * 10_000).to_be_bytes());
        out.write_chunk(png::chunk::ChunkType(*b"cLLi"), &cl).map_err(|e| e.to_string())?;
        out.write_image_data(&data).map_err(|e| e.to_string())?;
    }
    write(&out, &bytes)?;
    println!("  {HDR_MARK} (HDR, its brightest {} nits)", brightest.round());
    Ok(())
}

/// Today as YYYY-MM-DD (UTC), from the clock (no calendar crate).
fn date_today() -> String {
    let days = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).map_or(0, |d| d.as_secs() / 86_400) as i64;
    // Howard Hinnant's days-to-civil.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02}")
}

// ---- serve: dist/ on localhost, to look at the site before it's hosted

fn serve(port: u16) {
    let listener = match TcpListener::bind(("127.0.0.1", port)) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("olo-website: port {port}: {e}");
            std::process::exit(1);
        }
    };
    println!("serving {} at http://localhost:{port}/ (Ctrl+C to stop)", dist().display());
    for stream in listener.incoming().flatten() {
        std::thread::spawn(move || {
            let _ = answer(stream);
        });
    }
}

fn answer(mut stream: TcpStream) -> std::io::Result<()> {
    let mut line = String::new();
    BufReader::new(&stream).read_line(&mut line)?;
    let mut words = line.split_whitespace();
    let (method, target) = (words.next().unwrap_or(""), words.next().unwrap_or("/"));
    let path = target.split(['?', '#']).next().unwrap_or("/");
    let root = dist();
    let mut file = root.join(path.trim_start_matches('/'));
    // Never outside dist/.
    if path.split('/').any(|c| c == "..") {
        file = root.join("404.html");
    }
    if file.is_dir() {
        if !path.ends_with('/') {
            return write!(stream, "HTTP/1.1 301 Moved Permanently\r\nLocation: {path}/\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
        }
        file = file.join("index.html");
    }
    let (status, file) = if file.is_file() { ("200 OK", file) } else { ("404 Not Found", root.join("404.html")) };
    let body = fs::read(&file).unwrap_or_default();
    let kind = match file.extension().and_then(|x| x.to_str()).unwrap_or("") {
        "html" => "text/html; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "js" => "text/javascript; charset=utf-8",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "webp" => "image/webp",
        "ico" => "image/x-icon",
        "woff2" => "font/woff2",
        "xml" => "application/xml",
        "webmanifest" => "application/manifest+json",
        "txt" => "text/plain; charset=utf-8",
        _ => "application/octet-stream",
    };
    write!(stream, "HTTP/1.1 {status}\r\nContent-Type: {kind}\r\nContent-Length: {}\r\nCache-Control: no-cache\r\nConnection: close\r\n\r\n", body.len())?;
    if method != "HEAD" {
        stream.write_all(&body)?;
    }
    Ok(())
}
