//! A live page's `<video>` elements, played by Olo's own player
//! (`olo_preview::video`: H.264, HEVC, AV1 and VP9 in MP4 or Matroska,
//! decoded natively): each fetched on a thread of its own, its pictures
//! bound to its element as a live image (shown through a surface layer,
//! nothing around it repainted), its state and events (`loadedmetadata`,
//! `playing`, `timeupdate`, `ended`) given to the page's scripts, and what
//! they ask (`play()`, `pause()`, `currentTime`) done.
//!
//! A video plays without sound for now: Chrome lets a page start sound
//! only after the person has interacted with it, and the pages that matter
//! here play muted.
use olo_js::{Media, MediaCommand};
use olo_web::dom::{Document, NodeId};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

/// The largest video fetched whole (bytes).
const MEDIA_LIMIT: usize = 256 << 20;
/// How often `timeupdate` fires while playing (HTML: every 15 to 250 ms).
const TIME_UPDATE: Duration = Duration::from_millis(250);

enum Loading {
    /// `preload="none"`: nothing fetched until it is played.
    Idle,
    Fetching,
    Ready(olo_preview::video::Video, f64),
    Failed(String),
}
struct Player {
    src: String,
    loading: Arc<Mutex<Loading>>,
    /// The player, once fetched and probed.
    video: Option<(olo_preview::video::Video, f64)>,
    image: Option<Arc<olo_web::image::Image>>,
    media: Media,
    /// Asked to play before the player was ready.
    wanted_playing: bool,
    told_ready: bool,
    last_time_update: Instant,
    /// Its element has a poster: shown (the page binds it) until it plays.
    poster: bool,
    played: bool,
    /// What fetching it takes, while it waits to be played (`preload="none"`).
    deferred: Option<(Arc<crate::network::Loader>, url::Url, crate::network::Cancel)>,
}
impl Drop for Player {
    fn drop(&mut self) {
        // The decoding thread goes on its own; it is not waited for.
        if let Some((video, _)) = self.video.take() {
            drop(video.close());
        }
    }
}
#[derive(Default)]
pub struct Players {
    players: HashMap<NodeId, Player>,
    /// The document revision the elements were last looked for at.
    scanned: Option<u64>,
    /// Videos with nothing to play but a poster, whose poster was asked for.
    posters: std::collections::HashSet<NodeId>,
}
/// What a step did: whether a picture or a state changed (the page renders
/// again), and when the next picture is due.
pub struct Step {
    pub changed: bool,
    pub next: Option<Duration>,
}
impl Players {
    pub fn is_empty(&self) -> bool {
        self.players.is_empty()
    }
    /// The pictures to bind, by element.
    pub fn images(&self) -> Vec<(NodeId, Arc<olo_web::image::Image>)> {
        // (Not over its poster before it has played, as browsers show it.)
        self.players.iter().filter(|(_, p)| !p.poster || p.played).filter_map(|(n, p)| p.image.clone().map(|i| (*n, i))).collect()
    }
    /// Follow the document's `<video>` elements (new ones fetched, gone ones
    /// stopped), do what the scripts asked, show each player's picture due
    /// now and tell the scripts what changed.
    /// `shown`: the device pixels an element's video shows in (its content
    /// box), as the last layout had it; its pictures are made that size.
    pub fn step(&mut self, realm: &mut olo_js::Realm, loader: &Arc<crate::network::Loader>, base: &url::Url, cancel: &crate::network::Cancel, shown: &dyn Fn(NodeId) -> Option<[u32; 2]>) -> Step {
        let revision = realm.revision();
        if self.scanned != Some(revision) {
            self.scanned = Some(revision);
            let doc = realm.snapshot();
            let found = videos(&doc, base);
            self.players.retain(|n, p| found.iter().any(|(m, src)| m == n && *src == p.src));
            for (node, src) in found {
                if self.players.contains_key(&node) {
                    continue;
                }
                let autoplay = doc.attribute(node, "autoplay").ok().flatten().is_some();
                let poster = doc.attribute(node, "poster").ok().flatten().map(str::trim).filter(|p| !p.is_empty()).and_then(|p| base.join(p).ok());
                // Its poster, loaded for it (an element the scripts made
                // has none bound from the page's preparation).
                if let Some(url) = &poster
                    && !autoplay
                {
                    realm.request_image(node, url.as_str());
                }
                let poster = poster.is_some();
                let lazy = !autoplay && doc.attribute(node, "preload").ok().flatten().is_some_and(|p| p.trim().eq_ignore_ascii_case("none"));
                let mut player = Player::new(src, autoplay, poster);
                if lazy {
                    player.deferred = Some((loader.clone(), base.clone(), cancel.clone()));
                    *player.loading.lock().unwrap() = Loading::Idle;
                } else {
                    player.fetch(loader.clone(), base.clone(), cancel.clone());
                }
                self.players.insert(node, player);
            }
            // A video with nothing to play shows its poster all the same
            // (Wikipedia's player takes its sources away until it is
            // clicked, in a new element: it showed nothing).
            for (node, poster) in posters_only(&doc, base) {
                if !self.players.contains_key(&node) && self.posters.insert(node) {
                    realm.request_image(node, poster.as_str());
                }
            }
            if self.posters.len() > 256 {
                self.posters.retain(|n| doc.is_connected(*n).unwrap_or(false));
            }
        }
        for (node, command) in realm.take_media_commands() {
            if let Some(p) = self.players.get_mut(&node) {
                p.command(command);
            }
        }
        let mut step = Step { changed: false, next: None };
        let nodes: Vec<NodeId> = self.players.keys().copied().collect();
        for node in nodes {
            let Some(p) = self.players.get_mut(&node) else { continue };
            let looping = realm.has_attribute(node, "loop");
            let (changed, events, next) = p.advance(looping, shown(node));
            // Playing: its frames, not its poster.
            if p.poster && p.played && p.image.is_some() {
                realm.drop_image(node);
            }
            if changed {
                step.changed = true;
            }
            if !events.is_empty() || changed {
                realm.media_update(node, p.media, &events);
            }
            if let Some(n) = next {
                step.next = Some(step.next.map_or(n, |m| m.min(n)));
            }
        }
        step
    }
}
impl Player {
    fn new(src: String, autoplay: bool, poster: bool) -> Player {
        Player {
            src,
            loading: Arc::new(Mutex::new(Loading::Fetching)),
            video: None,
            image: None,
            media: Media { paused: !autoplay, ..Default::default() },
            wanted_playing: autoplay,
            told_ready: false,
            last_time_update: Instant::now(),
            poster,
            played: autoplay,
            deferred: None,
        }
    }
    /// Fetch and probe it on a thread of its own.
    fn fetch(&mut self, loader: Arc<crate::network::Loader>, base: url::Url, cancel: crate::network::Cancel) {
        *self.loading.lock().unwrap() = Loading::Fetching;
        let autoplay = self.wanted_playing;
        let loading = self.loading.clone();
        let (slot, url) = (loading.clone(), self.src.clone());
        let started = std::thread::Builder::new().name("video fetch".into()).spawn(move || {
            let result = (|| -> Result<(olo_preview::video::Video, f64), String> {
                let parsed = url::Url::parse(&url).map_err(|e| e.to_string())?;
                let (bytes, mime) = loader.media(&base, parsed, MEDIA_LIMIT, &cancel).map_err(|e| e.to_string())?;
                let bytes = Arc::new(bytes);
                let ext = if mime.contains("webm") || url.ends_with(".webm") {
                    "webm"
                } else if mime.contains("matroska") || url.ends_with(".mkv") {
                    "mkv"
                } else {
                    "mp4"
                };
                let held = bytes.clone();
                let input = olo_preview::input::Input::new(ext, &url, move || {
                    let size = held.len() as u64;
                    Ok((Box::new(std::io::Cursor::new(SharedBytes(held.clone()))) as Box<dyn olo_preview::input::ReadSeek>, size))
                });
                let info = olo_preview::video::probe(&input)?;
                if !info.picture {
                    return Err("no picture track in a codec the player takes".into());
                }
                let duration = info.duration.unwrap_or(f64::NAN);
                // Planes where a GPU decodes (VA-API, NVDEC; else the processor),
                // made RGB here at the size shown.
                let options = olo_preview::video::Options { playing: autoplay, picture: true, sound: false, bgra: true, yuv: true, device: olo_preview::video::Device::Automatic };
                Ok((olo_preview::video::Video::start_with(&input, if duration.is_finite() { duration } else { 0. }, options), duration))
            })();
            *slot.lock().unwrap() = match result {
                Ok((video, duration)) => Loading::Ready(video, duration),
                Err(e) => Loading::Failed(e),
            };
        });
        if let Err(e) = started {
            *loading.lock().unwrap() = Loading::Failed(e.to_string());
        }
    }
    fn command(&mut self, command: MediaCommand) {
        match command {
            MediaCommand::Play => {
                self.wanted_playing = true;
                self.played = true;
                self.media.paused = false;
                if let Some((loader, base, cancel)) = self.deferred.take() {
                    self.fetch(loader, base, cancel);
                }
                if let Some((v, _)) = &self.video {
                    if v.finished() {
                        v.seek(0.);
                    }
                    v.set_playing(true);
                }
            }
            MediaCommand::Pause => {
                self.wanted_playing = false;
                self.media.paused = true;
                if let Some((v, _)) = &self.video {
                    v.set_playing(false);
                }
            }
            MediaCommand::Seek(t) => {
                if let Some((v, _)) = &self.video {
                    v.seek(t);
                }
                self.media.time = t;
            }
            // The media element load algorithm (HTML §4.8.11.5), for one
            // already loaded: back to its start, paused, its readiness told
            // again (apple.com's tiles call load() and wait for
            // `canplaythrough` before playing; it never came, and the video
            // was taken out).
            MediaCommand::Load => {
                self.wanted_playing = false;
                self.told_ready = false;
                self.media = Media { paused: true, ..Default::default() };
                if let Some((v, _)) = &self.video {
                    v.set_playing(false);
                    v.seek(0.);
                }
            }
        }
    }
    /// Whether the picture or state changed, the events to fire, and when
    /// the next picture is due.
    fn advance(&mut self, looping: bool, shown: Option<[u32; 2]>) -> (bool, Vec<&'static str>, Option<Duration>) {
        let mut events = Vec::new();
        if self.video.is_none() {
            let mut slot = self.loading.lock().unwrap();
            match std::mem::replace(&mut *slot, Loading::Fetching) {
                Loading::Ready(video, duration) => {
                    video.set_playing(self.wanted_playing);
                    self.video = Some((video, duration));
                }
                Loading::Failed(e) => {
                    if !e.is_empty() {
                        eprintln!("Browser video {}: {e}", self.src);
                    }
                    // Said once; it stays failed.
                    *slot = Loading::Failed(String::new());
                    return (false, events, None);
                }
                Loading::Fetching => return (false, events, Some(Duration::from_millis(50))),
                // Waiting to be played: nothing to do, nothing to poll.
                Loading::Idle => {
                    *slot = Loading::Idle;
                    return (false, events, None);
                }
            }
        }
        let Some((video, duration)) = &self.video else { return (false, events, None) };
        let (picture, next) = video.tick_picture();
        let mut changed = false;
        // Made the size it shows at (contained in its box), keeping its
        // natural size through its density.
        let made = match picture {
            Some(olo_preview::video::Picture::Bgra { size: [w, h], data, .. }) => {
                let [ow, oh] = fitted([w, h], shown);
                let density = w as f64 / ow as f64;
                olo_web::image::Image::from_premultiplied(ow as u32, oh as u32, reduce(&data, [w, h], [ow, oh]), density, Default::default(), None).ok().map(|i| (w, h, ow, oh, i))
            }
            // Its planes as they are: the window's GPU makes RGB of them
            // and scales them where it shows as a surface layer (made RGB
            // here, at the size shown, only to be painted with the page).
            // Made RGB on the processor each picture, GitHub's videos took
            // 4-13 ms a picture, on eight threads made for it.
            Some(olo_preview::video::Picture::Yuv(frame)) => {
                let (w, h) = (frame.width as usize, frame.height as usize);
                let [ow, oh] = fitted([w, h], shown);
                let density = w as f64 / ow as f64;
                let source = frame.clone();
                Some((w, h, ow, oh, olo_web::image::Image::deferred(ow as u32, oh as u32, density, source, move || reduce_yuv(&frame, [ow, oh]))))
            }
            _ => None,
        };
        if let Some((w, h, ow, oh, mut image)) = made {
            {
                // After its first picture, a surface of its own (nothing
                // around it repainted as it plays).
                if self.image.is_some() {
                    olo_web::image::Image::set_live(&mut image);
                }
                self.image = Some(image);
                changed = true;
                if !self.told_ready {
                    if olo_web::env_on!("OLO_BROWSER_TIMING") {
                        eprintln!("  video {}: first picture {w}x{h} shown at {ow}x{oh}, {:.1} s", self.src, duration);
                    }
                    self.told_ready = true;
                    self.media.width = w as u32;
                    self.media.height = h as u32;
                    self.media.duration = *duration;
                    self.media.ready = 4;
                    events.extend(["durationchange", "loadedmetadata", "loadeddata", "canplay", "canplaythrough"]);
                    if self.wanted_playing {
                        events.push("playing");
                    }
                }
            }
        }
        // Loaded again (`load()`) with its picture still shown: ready again.
        if !self.told_ready
            && let Some(image) = &self.image
        {
            self.told_ready = true;
            let density = image.density();
            self.media.width = (f64::from(image.width()) * density).round() as u32;
            self.media.height = (f64::from(image.height()) * density).round() as u32;
            self.media.duration = *duration;
            self.media.ready = 4;
            events.extend(["loadstart", "durationchange", "loadedmetadata", "loadeddata", "canplay", "canplaythrough"]);
            changed = true;
        }
        let playing = video.playing();
        self.media.time = video.position();
        if playing && self.last_time_update.elapsed() >= TIME_UPDATE {
            self.last_time_update = Instant::now();
            events.push("timeupdate");
        }
        if video.finished() && self.wanted_playing {
            if looping {
                video.seek(0.);
                video.set_playing(true);
                events.push("seeked");
            } else {
                self.wanted_playing = false;
                self.media.paused = true;
                self.media.ended = true;
                events.extend(["timeupdate", "pause", "ended"]);
            }
        }
        let next = if self.wanted_playing { Some(next.unwrap_or(Duration::from_millis(16))) } else { next };
        (changed, events, next)
    }
}
/// The size a `[w, h]` picture shows at in a box `shown` device pixels
/// big, contained (no larger than it is).
fn fitted([w, h]: [usize; 2], shown: Option<[u32; 2]>) -> [usize; 2] {
    let Some([bw, bh]) = shown.filter(|[a, b]| *a > 0 && *b > 0) else { return [w, h] };
    let k = (bw as f64 / w as f64).min(bh as f64 / h as f64).min(1.);
    [((w as f64 * k).round() as usize).max(1), ((h as f64 * k).round() as usize).max(1)]
}
/// A BGRA picture as RGBA `[ow, oh]` big: each output pixel the average of
/// the source pixels under it (an area filter; a copy when the size is
/// the same), in bands on a few threads.
fn reduce(bgra: &[u8], [w, h]: [usize; 2], [ow, oh]: [usize; 2]) -> Vec<u8> {
    let mut out = vec![0u8; ow * oh * 4];
    if [w, h] == [ow, oh] {
        for (d, s) in out.chunks_exact_mut(4).zip(bgra.chunks_exact(4)) {
            d.copy_from_slice(&[s[2], s[1], s[0], s[3]]);
        }
        return out;
    }
    // Each output column's source columns, [start, end).
    let spans = |n: usize, o: usize| -> Vec<(usize, usize)> { (0..o).map(|i| ((i * n) / o, (((i + 1) * n) / o).max((i * n) / o + 1).min(n))).collect() };
    let (xs, ys) = (spans(w, ow), spans(h, oh));
    let threads = std::thread::available_parallelism().map_or(4, |n| n.get()).min(8);
    let rows_per = oh.div_ceil(threads);
    std::thread::scope(|scope| {
        for (band, chunk) in out.chunks_mut(rows_per * ow * 4).enumerate() {
            let (xs, ys) = (&xs, &ys);
            scope.spawn(move || {
                let mut sums = vec![0u32; ow * 4];
                for (r, row) in chunk.chunks_exact_mut(ow * 4).enumerate() {
                    let (y0, y1) = ys[band * rows_per + r];
                    sums.fill(0);
                    for y in y0..y1 {
                        let line = &bgra[y * w * 4..(y + 1) * w * 4];
                        for (i, &(x0, x1)) in xs.iter().enumerate() {
                            let s = &mut sums[i * 4..i * 4 + 4];
                            for px in line[x0 * 4..x1 * 4].chunks_exact(4) {
                                s[0] += u32::from(px[2]);
                                s[1] += u32::from(px[1]);
                                s[2] += u32::from(px[0]);
                                s[3] += u32::from(px[3]);
                            }
                        }
                    }
                    for (i, &(x0, x1)) in xs.iter().enumerate() {
                        let n = ((x1 - x0) * (y1 - y0)) as u32;
                        for c in 0..4 {
                            row[i * 4 + c] = ((sums[i * 4 + c] + n / 2) / n) as u8;
                        }
                    }
                }
            });
        }
    });
    out
}
/// A picture's Y'CbCr planes (4:2:0, 8- or 16-bit, wherever its decoder
/// left them) as RGBA `[ow, oh]` big: each output pixel from the average
/// of the luma and chroma samples under it, made RGB by the picture's own
/// matrix (`YuvFrame::rgb_matrix`); in bands on a few threads.
fn reduce_yuv(f: &olo_ui::gpui::YuvFrame, [ow, oh]: [usize; 2]) -> Vec<u8> {
    let (w, h) = (f.width as usize, f.height as usize);
    let (cw, ch) = f.chroma_size();
    let (cw, ch) = (cw as usize, ch as usize);
    let (luma, lp) = f.luma();
    let (chroma, cp) = f.chroma();
    let deep = f.deep;
    let m = f.rgb_matrix();
    let sample = |plane: &[u8], i: usize| -> f32 {
        if deep { f32::from(u16::from_le_bytes([plane.get(i).copied().unwrap_or(0), plane.get(i + 1).copied().unwrap_or(0)])) / 65535. } else { f32::from(plane.get(i).copied().unwrap_or(0)) / 255. }
    };
    let bps = if deep { 2 } else { 1 };
    let spans = |n: usize, o: usize| -> Vec<(usize, usize)> { (0..o).map(|i| ((i * n) / o, (((i + 1) * n) / o).max((i * n) / o + 1).min(n))).collect() };
    let (xs, ys) = (spans(w, ow), spans(h, oh));
    let mut out = vec![0u8; ow * oh * 4];
    let threads = std::thread::available_parallelism().map_or(4, |n| n.get()).min(8);
    let rows_per = oh.div_ceil(threads);
    std::thread::scope(|scope| {
        for (band, chunk) in out.chunks_mut(rows_per * ow * 4).enumerate() {
            let (xs, ys, sample) = (&xs, &ys, &sample);
            scope.spawn(move || {
                for (r, row) in chunk.chunks_exact_mut(ow * 4).enumerate() {
                    let (y0, y1) = ys[band * rows_per + r];
                    let (cy0, cy1) = (y0 / 2, (y1.div_ceil(2)).max(y0 / 2 + 1).min(ch));
                    for (i, &(x0, x1)) in xs.iter().enumerate() {
                        let mut yl = 0.;
                        for y in y0..y1 {
                            for x in x0..x1 {
                                yl += sample(luma, y * lp + x * bps);
                            }
                        }
                        yl /= ((y1 - y0) * (x1 - x0)) as f32;
                        let (cx0, cx1) = (x0 / 2, (x1.div_ceil(2)).max(x0 / 2 + 1).min(cw));
                        let (mut cb, mut cr) = (0., 0.);
                        for y in cy0..cy1 {
                            for x in cx0..cx1 {
                                cb += sample(chroma, y * cp + x * 2 * bps);
                                cr += sample(chroma, y * cp + x * 2 * bps + bps);
                            }
                        }
                        let n = ((cy1 - cy0) * (cx1 - cx0)).max(1) as f32;
                        let (cb, cr) = (cb / n, cr / n);
                        for c in 0..3 {
                            let v = m[0][c] * yl + m[1][c] * cb + m[2][c] * cr + m[3][c];
                            row[i * 4 + c] = (v.clamp(0., 1.) * 255. + 0.5) as u8;
                        }
                        row[i * 4 + 3] = 255;
                    }
                }
            });
        }
    });
    out
}
/// Bytes shared with the player's reader (it asks for a fresh reader at
/// each seek).
struct SharedBytes(Arc<Vec<u8>>);
impl AsRef<[u8]> for SharedBytes {
    fn as_ref(&self) -> &[u8] {
        &self.0
    }
}
/// The `<video>` elements of `doc` with their source: the `src`, else the
/// first `<source>` of a type the player takes, resolved against `base`.
/// The `<video>` elements with a poster and nothing to play (no source
/// `videos` takes), with their posters' URLs.
fn posters_only(doc: &Document, base: &url::Url) -> Vec<(NodeId, url::Url)> {
    let playable: Vec<NodeId> = videos(doc, base).into_iter().map(|(n, _)| n).collect();
    let mut out = Vec::new();
    let mut stack = vec![doc.root()];
    while let Some(n) = stack.pop() {
        if out.len() >= 64 {
            break;
        }
        if doc.name(n).ok().flatten() == Some("video")
            && !playable.contains(&n)
            && let Some(url) = doc.attribute(n, "poster").ok().flatten().map(str::trim).filter(|p| !p.is_empty()).and_then(|p| base.join(p).ok())
        {
            out.push((n, url));
        }
        if let Ok(children) = doc.children(n) {
            let children: Vec<NodeId> = children.collect();
            stack.extend(children.into_iter().rev());
        }
    }
    out
}
fn videos(doc: &Document, base: &url::Url) -> Vec<(NodeId, String)> {
    let mut out = Vec::new();
    let mut stack = vec![doc.root()];
    while let Some(n) = stack.pop() {
        if out.len() >= 64 {
            break;
        }
        if doc.name(n).ok().flatten() == Some("video") {
            let attr = |m: NodeId, a: &str| doc.attribute(m, a).ok().flatten().map(str::to_owned).filter(|s| !s.trim().is_empty());
            let src = attr(n, "src").or_else(|| {
                doc.children(n).ok()?.into_iter().filter(|&c| doc.name(c).ok().flatten() == Some("source")).find_map(|c| {
                    let kind = attr(c, "type").unwrap_or_default().to_ascii_lowercase();
                    (kind.is_empty() || kind.starts_with("video/mp4") || kind.starts_with("video/webm")).then(|| attr(c, "src")).flatten()
                })
            });
            if let Some(src) = src.and_then(|s| base.join(s.trim()).ok()) {
                out.push((n, src.to_string()));
            }
        }
        if let Ok(children) = doc.children(n) {
            let children: Vec<NodeId> = children.collect();
            stack.extend(children.into_iter().rev());
        }
    }
    out
}
