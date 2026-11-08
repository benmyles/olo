//! WebM video with alpha: Telegram's video stickers (VP9, 512 pixels on
//! the longer side, up to three seconds at up to 30 frames a second, 256
//! KB at most). The colour is the video track's VP9; the alpha a second
//! VP9 stream in each block's BlockAdditional (BlockAddID 1: the vendored
//! matroska-demuxer reads it), its luma the alpha. The whole file is
//! demuxed once, its compressed frames kept (a sticker's are a few
//! hundred kilobytes); a [`Player`] decodes them in order on two VP9
//! decoders, starting them again to loop.

use std::io::Cursor;

use crate::vp9;

/// The largest file read (Telegram's stickers are 256 KB at most).
pub const MOST_BYTES: usize = 4 << 20;

/// A WebM video, its frames demuxed.
pub struct Webm {
    pub width: u32,
    pub height: u32,
    /// Each frame: when it shows (seconds), its VP9 sample, and its alpha
    /// stream's (none: opaque).
    frames: Vec<(f64, Vec<u8>, Option<Vec<u8>>)>,
    /// How long it plays, once through.
    pub duration: f64,
}

impl Webm {
    pub fn read(bytes: &[u8]) -> Result<Webm, String> {
        if bytes.len() > MOST_BYTES {
            return Err("WebM: too large".into());
        }
        let mut mk = matroska_demuxer::MatroskaFile::open(Cursor::new(bytes)).map_err(|e| format!("WebM: {e}"))?;
        let scale = mk.info().timestamp_scale().get() as f64 / 1e9;
        let t = mk.tracks().iter().find(|t| t.video().is_some() && t.codec_id() == "V_VP9").ok_or("WebM: no VP9 picture")?;
        let track = t.track_number().get();
        let v = t.video().ok_or("WebM: no picture")?;
        let (width, height) = (v.pixel_width().get() as u32, v.pixel_height().get() as u32);
        let step = t.default_duration().map(|d| d.get() as f64 / 1e9);
        let mut frames = Vec::new();
        let mut f = matroska_demuxer::Frame::default();
        while mk.next_frame(&mut f).map_err(|e| format!("WebM: {e}"))? {
            if f.track == track {
                frames.push((f.timestamp as f64 * scale, std::mem::take(&mut f.data), f.additional.take()));
            }
        }
        if frames.is_empty() {
            return Err("WebM: no frames".into());
        }
        frames.sort_by(|a, b| a.0.total_cmp(&b.0));
        // Played from its first frame (a file's first block may be late).
        let start = frames[0].0;
        for f in &mut frames {
            f.0 -= start;
        }
        let last = frames.last().map_or(0.0, |f| f.0);
        let gap = step.unwrap_or_else(|| if frames.len() > 1 { last / (frames.len() - 1) as f64 } else { 1.0 / 30.0 });
        let duration = mk.info().duration().map(|d| d * scale).filter(|d| *d > last).unwrap_or(last + gap);
        Ok(Webm { width, height, frames, duration })
    }

    pub fn open(path: &std::path::Path) -> Result<Webm, String> {
        let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
        Webm::read(&bytes)
    }

    pub fn len(&self) -> usize {
        self.frames.len()
    }

    pub fn is_empty(&self) -> bool {
        self.frames.is_empty()
    }

    /// Whether it has an alpha stream.
    pub fn alpha(&self) -> bool {
        self.frames.iter().any(|f| f.2.is_some())
    }

    /// The frame showing `t` seconds in (looping).
    pub fn frame_at(&self, t: f64) -> usize {
        let t = if self.duration > 0.0 { t.rem_euclid(self.duration) } else { 0.0 };
        self.frames.partition_point(|f| f.0 <= t).saturating_sub(1)
    }

    /// When frame `i` shows, seconds in.
    pub fn time_of(&self, i: usize) -> f64 {
        self.frames.get(i).map_or(self.duration, |f| f.0)
    }
}

/// Decodes a [`Webm`]'s frames in order.
pub struct Player {
    colour: vp9::Decoder,
    alpha: vp9::Decoder,
    /// The next frame the decoders take.
    next: usize,
    out: Vec<(i64, vp9::Frame)>,
}

impl Default for Player {
    fn default() -> Self {
        Player::new()
    }
}

impl Player {
    pub fn new() -> Player {
        Player { colour: vp9::Decoder::new(1), alpha: vp9::Decoder::new(1), next: 0, out: Vec::new() }
    }

    /// Frame `i` of `video`: straight RGBA rows, `video.width` wide.
    /// Decodes on from the last frame asked (from the first again for an
    /// earlier one).
    pub fn frame(&mut self, video: &Webm, i: usize) -> Result<Vec<u8>, String> {
        let i = i.min(video.frames.len().saturating_sub(1));
        if i < self.next {
            self.colour.reset();
            self.alpha.reset();
            self.next = 0;
        }
        let (mut colour, mut alpha) = (None, None);
        while self.next <= i {
            let (_, sample, extra) = &video.frames[self.next];
            self.out.clear();
            self.colour.decode(sample, self.next as i64, &mut self.out)?;
            if let Some((_, f)) = self.out.pop() {
                colour = Some(f);
            }
            if let Some(extra) = extra {
                self.out.clear();
                self.alpha.decode(extra, self.next as i64, &mut self.out)?;
                if let Some((_, f)) = self.out.pop() {
                    alpha = Some(f);
                }
            }
            self.next += 1;
        }
        let colour = colour.ok_or("WebM: no picture for that frame")?;
        let mut rgba = colour.to_rgba();
        match alpha {
            Some(a) => a.luma_into_alpha(&mut rgba, colour.width),
            // (A frame's alpha stream may be missing: opaque, as a
            // decoder without alpha shows it.)
            None if video.alpha() => {}
            None => {}
        }
        Ok(rgba)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A VP9 video with alpha, as Chromium's tests have (`bear-vp9a.webm`,
    /// in `OLO_SAMPLES`): every frame decodes, and its alpha comes
    /// through as the alpha stream's luma itself (that file's is flat, 68
    /// throughout: each of its frames 18 to 43 bytes).
    #[test]
    #[ignore]
    fn plays_vp9_with_alpha() {
        let dir = std::env::var("OLO_SAMPLES").expect("OLO_SAMPLES");
        let v = Webm::open(&std::path::Path::new(&dir).join("bear-vp9a.webm")).unwrap();
        assert!(v.alpha() && !v.is_empty(), "{} frames", v.len());
        let mut p = Player::new();
        let t = std::time::Instant::now();
        let mut alphas = std::collections::BTreeSet::new();
        for i in 0..v.len() {
            let px = p.frame(&v, i).unwrap();
            assert_eq!(px.len(), (v.width * v.height * 4) as usize);
            alphas.extend(px.as_chunks::<4>().0.iter().map(|q| q[3]));
        }
        let each = t.elapsed().as_secs_f64() * 1000.0 / v.len() as f64;
        eprintln!("{}×{}, {} frames, {:.2} s, {each:.2} ms a frame, alphas {alphas:?}", v.width, v.height, v.len(), v.duration);
        assert_eq!(alphas.into_iter().collect::<Vec<u8>>(), [68]);
        // Looping: back to the first.
        assert_eq!(p.frame(&v, 0).unwrap().len(), (v.width * v.height * 4) as usize);
        assert_eq!(v.frame_at(v.duration + v.time_of(1) / 2.0 + 1e-6), v.frame_at(v.time_of(1) / 2.0 + 1e-6));
    }

    #[test]
    fn not_webm() {
        assert!(Webm::read(b"not a video").is_err());
    }
}
