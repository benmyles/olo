//! Sound played faster or slower with its pitch kept (a video watched at
//! 1.5×, a lecture at 0.75×): WSOLA, as SoundTouch's `TDStretch` does it.
//! The sound is cut into overlapping pieces some tens of milliseconds
//! long, taken from the input `rate` times as far apart as they're laid
//! down in the output. Each piece is moved, within a short window, to
//! where it best continues the one before (the most alike: their
//! correlation, weighted toward the window's middle) and cross-faded into
//! it, so the waveform's periods line up and the pitch stays as recorded.
//! Interleaved 16-bit samples, one or two channels; a piece costs a few
//! hundred thousand multiplications (a coarse search, then a fine one
//! around the best), well under a millisecond a second of sound.

use std::collections::VecDeque;

pub struct Stretch {
    channels: usize,
    rate: f64,
    /// A piece's length, the window searched for where it fits best and
    /// the cross-fade between pieces, in frames.
    seq: usize,
    seek: usize,
    overlap: usize,
    /// Steps the coarse search takes.
    step: usize,
    /// Interleaved input not yet used: the next search starts at its
    /// first frame.
    input: Vec<f32>,
    /// The end of the last piece, faded from (interleaved, `overlap`
    /// frames).
    mid: Vec<f32>,
    /// `mid` mixed to one channel and weighted toward its middle: what
    /// each place is compared with.
    reference: Vec<f32>,
    /// The input's span searched, mixed to one channel, and its running
    /// sums of squares (each place's loudness, to compare fairly).
    mono: Vec<f32>,
    squares: Vec<f64>,
    /// What the input skips beyond whole frames, so it doesn't drift.
    skip: f64,
    started: bool,
    ended: bool,
    /// Frames pushed and made, so the end comes out as long as the rate
    /// says.
    pushed: u64,
    made: u64,
    out: VecDeque<i16>,
}

impl Stretch {
    /// Sound at `sample_rate` with `channels` interleaved, played at
    /// `rate` (2: twice as fast).
    pub fn new(sample_rate: u32, channels: usize, rate: f64) -> Stretch {
        let rate = rate.clamp(0.1, 4.0);
        // SoundTouch's: longer pieces and a wider search slowed down,
        // shorter ones sped up (90 ms at half speed to 40 at double).
        let t = rate.clamp(0.5, 2.0);
        let seq_ms = (90.0 + (t - 0.5) * (40.0 - 90.0) / 1.5).clamp(40.0, 90.0);
        let seek_ms = (20.0 + (t - 0.5) * (15.0 - 20.0) / 1.5).clamp(15.0, 20.0);
        let frames = |ms: f64| ((f64::from(sample_rate) * ms / 1000.0).round() as usize).max(8);
        let overlap = frames(8.0);
        Stretch {
            channels: channels.clamp(1, 2),
            rate,
            seq: frames(seq_ms).max(2 * overlap + 8),
            seek: frames(seek_ms),
            overlap,
            step: (sample_rate as usize / 12_000).max(1),
            input: Vec::new(),
            mid: Vec::new(),
            reference: Vec::new(),
            mono: Vec::new(),
            squares: Vec::new(),
            skip: 0.0,
            started: false,
            ended: false,
            pushed: 0,
            made: 0,
            out: VecDeque::new(),
        }
    }

    pub fn rate(&self) -> f64 {
        self.rate
    }

    /// More of the input.
    pub fn push(&mut self, samples: &[i16]) {
        self.input.extend(samples.iter().map(|&s| f32::from(s)));
        self.pushed += (samples.len() / self.channels) as u64;
        self.process();
    }

    /// The input has ended: what's left of it is made too (followed by
    /// silence for the pieces it needs), the whole as long as the rate
    /// says.
    pub fn finish(&mut self) {
        if self.ended {
            return;
        }
        self.ended = true;
        if !self.started {
            self.out.extend(self.input.drain(..).map(to_i16));
            return;
        }
        let target = (self.pushed as f64 / self.rate).round() as u64;
        // (Faded out over its last 5 ms, so pieces reaching past it into
        // the silence don't click.)
        let ch = self.channels;
        let fade = (self.overlap * 5 / 8).min(self.input.len() / ch).max(1);
        let from = self.input.len() / ch - fade.min(self.input.len() / ch);
        for (k, frame) in self.input[from * ch..].chunks_mut(ch).enumerate() {
            let g = 1.0 - (k + 1) as f32 / fade as f32;
            frame.iter_mut().for_each(|x| *x *= g);
        }
        let pad = (self.rate * self.seq as f64).ceil() as usize + self.seq + self.seek;
        while self.made < target {
            let before = self.made;
            self.input.resize(self.input.len() + pad * self.channels, 0.0);
            self.process();
            if self.made == before {
                break;
            }
        }
        let extra = (self.made.saturating_sub(target) as usize * self.channels).min(self.out.len());
        self.out.truncate(self.out.len() - extra);
        self.input.clear();
    }

    /// It has ended and all it made was taken.
    pub fn done(&self) -> bool {
        self.ended && self.out.is_empty()
    }

    /// Frames made and not yet taken.
    pub fn available(&self) -> usize {
        self.out.len() / self.channels
    }

    /// Up to `frames` frames of what it made, added to `out`.
    pub fn take(&mut self, frames: usize, out: &mut Vec<i16>) {
        let n = (frames * self.channels).min(self.out.len());
        out.extend(self.out.drain(..n));
    }

    fn process(&mut self) {
        let ch = self.channels;
        let (seq, seek, ov) = (self.seq, self.seek, self.overlap);
        if !self.started {
            if self.input.len() < ov * ch {
                return;
            }
            // (The first piece fades from the input's own start: into
            // itself, unchanged.)
            self.mid = self.input[..ov * ch].to_vec();
            self.set_reference();
            self.started = true;
        }
        let nominal = self.rate * (seq - ov) as f64;
        loop {
            let need = ((nominal + self.skip).ceil() as usize + ov).max(seq) + seek;
            if self.input.len() < need * ch {
                return;
            }
            let offset = self.best_offset();
            // Faded from the last piece's end into this one's start…
            for i in 0..ov {
                let into = i as f32 / ov as f32;
                for c in 0..ch {
                    let (a, b) = (self.mid[i * ch + c], self.input[(offset + i) * ch + c]);
                    self.out.push_back(to_i16(a + (b - a) * into));
                }
            }
            self.made += (seq - ov) as u64;
            // …then its middle as it is, its end kept for the next fade.
            self.out.extend(self.input[(offset + ov) * ch..(offset + seq - ov) * ch].iter().map(|&s| to_i16(s)));
            self.mid.copy_from_slice(&self.input[(offset + seq - ov) * ch..(offset + seq) * ch]);
            self.set_reference();
            self.skip += nominal;
            let n = self.skip.floor() as usize;
            self.skip -= n as f64;
            self.input.drain(..n * ch);
        }
    }

    fn set_reference(&mut self) {
        let (ch, ov) = (self.channels, self.overlap);
        self.reference.clear();
        self.reference.extend((0..ov).map(|i| {
            let m: f32 = self.mid[i * ch..(i + 1) * ch].iter().sum();
            m * (i * (ov - i)) as f32
        }));
    }

    /// Where in the search window the next piece best continues the
    /// last (frames from the input's start).
    fn best_offset(&mut self) -> usize {
        let (ch, ov, seek) = (self.channels, self.overlap, self.seek);
        let span = seek + ov;
        self.mono.clear();
        self.mono.extend((0..span).map(|i| self.input[i * ch..(i + 1) * ch].iter().sum::<f32>()));
        self.squares.clear();
        self.squares.push(0.0);
        let mut sum = 0.0f64;
        for &x in &self.mono {
            sum += f64::from(x) * f64::from(x);
            self.squares.push(sum);
        }
        let score = |o: usize| -> f64 {
            let x = &self.mono[o..o + ov];
            let corr: f32 = self.reference.iter().zip(x).map(|(r, x)| r * x).sum();
            let norm = (self.squares[o + ov] - self.squares[o]).max(1e-9).sqrt();
            // (Nearer the middle a little preferred: SoundTouch's.)
            let t = (2.0 * o as f64 - seek as f64) / seek as f64;
            (f64::from(corr) / norm + 0.1) * (1.0 - 0.25 * t * t)
        };
        let best = |range: &mut dyn Iterator<Item = usize>| range.map(|o| (o, score(o))).max_by(|a, b| a.1.total_cmp(&b.1)).map_or(0, |(o, _)| o);
        let coarse = best(&mut (0..seek).step_by(self.step));
        best(&mut (coarse.saturating_sub(self.step)..(coarse + self.step + 1).min(seek)))
    }
}

fn to_i16(s: f32) -> i16 {
    s.round().clamp(-32768.0, 32767.0) as i16
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A sine of `hz` for `secs`, stereo at 48 kHz.
    fn sine(hz: f64, secs: f64) -> Vec<i16> {
        (0..(48_000.0 * secs) as usize)
            .flat_map(|i| {
                let s = (10_000.0 * (2.0 * std::f64::consts::PI * hz * i as f64 / 48_000.0).sin()) as i16;
                [s, s]
            })
            .collect()
    }

    fn stretched(input: &[i16], rate: f64) -> Vec<i16> {
        let mut s = Stretch::new(48_000, 2, rate);
        let mut out = Vec::new();
        for chunk in input.chunks(4096) {
            s.push(chunk);
            s.take(usize::MAX / 4, &mut out);
        }
        s.finish();
        s.take(usize::MAX / 4, &mut out);
        assert!(s.done());
        out
    }

    /// What it costs: a second of 48 kHz stereo at each speed
    /// (`cargo test --release -p olo-preview stretch_cost -- --ignored
    /// --nocapture`).
    #[test]
    #[ignore]
    fn stretch_cost() {
        let input = sine(440.0, 20.0);
        for rate in [0.25, 0.5, 1.5, 2.0] {
            let t = std::time::Instant::now();
            let out = stretched(&input, rate);
            let took = t.elapsed().as_secs_f64();
            println!("{rate}×: {:.2} ms a second of sound in, {:.2} ms a second out", took * 1000.0 / 20.0, took * 1000.0 / (out.len() as f64 / 96_000.0));
        }
    }

    /// As long as the rate says, at the pitch recorded, with no clicks
    /// where pieces meet: 0.25× to 2×.
    #[test]
    fn rate_without_pitch() {
        let input = sine(440.0, 2.0);
        for rate in [0.25, 0.5, 0.75, 1.25, 1.5, 2.0] {
            let out = stretched(&input, rate);
            let secs = out.len() as f64 / 2.0 / 48_000.0;
            assert!((secs - 2.0 / rate).abs() < 0.1, "{rate}×: {secs:.3} s");
            // Its pitch: crossings of zero, upward, a second (but for its
            // last half second, the input's end).
            let left: Vec<i16> = out.iter().step_by(2).copied().collect();
            let left = &left[..left.len() - 24_000];
            let ups = left.windows(2).filter(|w| w[0] < 0 && w[1] >= 0).count() as f64;
            let hz = ups / (left.len() as f64 / 48_000.0);
            assert!((hz - 440.0).abs() < 440.0 * 0.02, "{rate}×: {hz:.1} Hz");
            // No sample further from the one before than the sine's
            // steepest step allows (with a margin): no clicks.
            let steepest = 10_000.0 * 2.0 * std::f64::consts::PI * 440.0 / 48_000.0;
            let worst = left.windows(2).map(|w| (f64::from(w[1]) - f64::from(w[0])).abs()).fold(0.0, f64::max);
            assert!(worst < steepest * 1.3, "{rate}×: a step of {worst:.0}, the sine's at most {steepest:.0}");
        }
    }
}
