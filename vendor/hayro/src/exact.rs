//! Olo: an upright image drawn smaller than it is, resampled once onto
//! the very device pixels it covers (vendor/README.md).
//!
//! As published, such an image is first resized to its size on the
//! device rounded *up* to whole pixels, with a Hermite filter, and that
//! picture is then drawn through the last fraction of the scale with
//! nearest-pixel sampling (an image that doesn't ask for interpolation,
//! as most don't). Two faults came of it, both seen in a page that is one
//! 3840 × 2160 picture shown at a third of its size: a column and a row
//! of the picture dropped somewhere across it wherever the size wasn't
//! whole (2200.3 pixels: resized to 2201, then squeezed), a break through
//! every line of text it crossed; and the picture softer than other
//! viewers make it (Hermite is close to bilinear).
//!
//! Here each device pixel the image touches is worked out from the
//! source directly: its centre taken back to the image through the
//! transform, and a Lanczos filter of three lobes laid there, as wide as
//! the device pixel is in the image. One pass, no second sampling, so
//! nothing is dropped and nothing is filtered twice; what comes out is
//! drawn a pixel to a pixel.

use kurbo::Affine;

/// Where an upright image lands: along each axis, device = `a` × image +
/// `e` (in pixels; a negative `a` is the image mirrored).
#[derive(Clone, Copy, Debug)]
pub(crate) struct Upright {
    pub(crate) ax: f64,
    pub(crate) ex: f64,
    pub(crate) ay: f64,
    pub(crate) ey: f64,
}

impl Upright {
    /// `transform` (image pixels to device pixels), if it neither turns
    /// nor shears the image.
    pub(crate) fn of(transform: &Affine) -> Option<Self> {
        let [a, b, c, d, e, f] = transform.as_coeffs();
        let tiny = 1e-9 * (a.abs() + d.abs());
        let upright = b.abs() <= tiny && c.abs() <= tiny && a != 0.0 && d != 0.0;
        (upright && [a, d, e, f].iter().all(|v| v.is_finite())).then_some(Self {
            ax: a,
            ex: e,
            ay: d,
            ey: f,
        })
    }
}

/// Weights are whole numbers out of this (14 bits).
const ONE: i32 = 1 << 14;
/// What the first pass keeps of each value beside its 8 bits.
const KEPT: u32 = 6;

fn lanczos3(x: f64) -> f64 {
    let x = x.abs();
    if x < 1e-9 {
        1.0
    } else if x >= 3.0 {
        0.0
    } else {
        let px = std::f64::consts::PI * x;
        3.0 * px.sin() * (px / 3.0).sin() / (px * px)
    }
}

/// One axis's weights: for each device pixel, the first source pixel it
/// is made of and `len` weights from there on (summing to `ONE`).
struct Taps {
    first: Vec<usize>,
    len: usize,
    weights: Vec<i32>,
}

/// The weights for the device pixels `out0 .. out0 + out` along an axis
/// of `src` source pixels, where device = `a` × source + `e`.
fn taps(src: usize, a: f64, e: f64, out0: i64, out: usize) -> Taps {
    // (How many source pixels a device pixel spans, when it's more than
    // one: the filter is that much wider. Along an axis that isn't
    // shrunk it's the plain interpolating filter.)
    let span = (1.0 / a.abs()).max(1.0);
    let radius = 3.0 * span;
    let len = (((2.0 * radius).ceil() as usize) + 1).min(src.max(1));
    let mut first = Vec::with_capacity(out);
    let mut weights = vec![0i32; out * len];
    let mut exact = vec![0f64; len];
    for i in 0..out {
        // The device pixel's centre, in the image (source pixel k is
        // [k, k + 1), its centre k + 0.5).
        let centre = ((out0 + i as i64) as f64 + 0.5 - e) / a;
        let start = (centre - radius).floor() as i64;
        let at = start.clamp(0, src as i64 - len as i64);
        exact.fill(0.0);
        let mut sum = 0.0;
        for k in start..=(centre + radius).ceil() as i64 {
            let w = lanczos3((k as f64 + 0.5 - centre) / span);
            // (Past an edge the edge's own pixel stands in.)
            let j = (k.clamp(0, src as i64 - 1) - at) as usize;
            exact[j.min(len - 1)] += w;
            sum += w;
        }
        // To whole numbers that sum to exactly ONE (the rounding's
        // remainder goes to the heaviest), so a flat colour stays flat.
        let row = &mut weights[i * len..(i + 1) * len];
        let mut total = 0;
        let mut heaviest = 0;
        for (j, w) in exact.iter().enumerate() {
            row[j] = (w / sum * ONE as f64).round() as i32;
            total += row[j];
            if row[j] > row[heaviest] {
                heaviest = j;
            }
        }
        row[heaviest] += ONE - total;
        first.push(at as usize);
    }
    Taps {
        first,
        len,
        weights,
    }
}

/// `src` (`w` × `h` pixels of `n` bytes each) as the device pixels
/// `x0 .. x0 + dw` by `y0 .. y0 + dh` show it under `place`: `dw` × `dh`
/// pixels of `n` bytes.
pub(crate) fn resample(
    src: &[u8],
    w: usize,
    h: usize,
    n: usize,
    place: &Upright,
    (x0, y0): (i64, i64),
    (dw, dh): (usize, usize),
) -> Vec<u8> {
    debug_assert_eq!(src.len(), w * h * n);
    if w == 0 || h == 0 || dw == 0 || dh == 0 {
        return vec![0; dw * dh * n];
    }
    let (across, down) = (
        taps(w, place.ax, place.ex, x0, dw),
        taps(h, place.ay, place.ey, y0, dh),
    );
    // Down the image first (each device row from the source rows it's
    // made of, a whole row at a time: the loop the compiler makes fast),
    // then across the rows that are left. Each row of either pass is on
    // its own, so a large picture's rows are shared among threads (a
    // full page of 3840 × 2160 to 0.6 of its size took 181 ms on one
    // thread and takes 34 ms on eight).
    let stride = w * n;
    let mut rows = vec![0i16; dh * stride];
    in_parts(&mut rows, stride, w * h * n, |j0, part| {
        let mut sums = vec![0i32; stride];
        for (j, row) in part.chunks_exact_mut(stride).enumerate() {
            let j = j0 + j;
            sums.fill(0);
            let weights = &down.weights[j * down.len..(j + 1) * down.len];
            for (k, weight) in weights.iter().enumerate() {
                if *weight == 0 {
                    continue;
                }
                let from = &src[(down.first[j] + k) * stride..][..stride];
                for (sum, v) in sums.iter_mut().zip(from) {
                    *sum += weight * *v as i32;
                }
            }
            for (kept, sum) in row.iter_mut().zip(&sums) {
                *kept = (sum >> (14 - KEPT)) as i16;
            }
        }
    });
    let mut out = vec![0u8; dw * dh * n];
    let half = 1 << (13 + KEPT);
    in_parts(&mut out, dw * n, dh * stride, |j0, part| {
        for (j, to) in part.chunks_exact_mut(dw * n).enumerate() {
            let row = &rows[(j0 + j) * stride..][..stride];
            for (i, pixel) in to.chunks_exact_mut(n).enumerate() {
                let weights = &across.weights[i * across.len..(i + 1) * across.len];
                let from = &row[across.first[i] * n..][..across.len * n];
                let mut sums = [0i32; 4];
                for (weight, p) in weights.iter().zip(from.chunks_exact(n)) {
                    for (sum, v) in sums.iter_mut().zip(p) {
                        *sum += weight * *v as i32;
                    }
                }
                for (byte, sum) in pixel.iter_mut().zip(sums) {
                    *byte = ((sum + half) >> (14 + KEPT)).clamp(0, 255) as u8;
                }
            }
        }
    });
    out
}

/// `rows` (rows of `row` values) worked on by `work`, which is given a
/// run of whole rows and the index of its first: on this thread alone
/// where the job is small (`size`: the values it reads), else shared
/// among as many threads as there are processors, eight at most.
fn in_parts<T: Send>(rows: &mut [T], row: usize, size: usize, work: impl Fn(usize, &mut [T]) + Sync) {
    let count = rows.len() / row.max(1);
    let threads = if size < 1 << 20 {
        1
    } else {
        std::thread::available_parallelism().map_or(1, |n| n.get()).min(8).min(count.max(1))
    };
    if threads <= 1 {
        return work(0, rows);
    }
    let each = count.div_ceil(threads);
    std::thread::scope(|scope| {
        for (part, run) in rows.chunks_mut(each * row).enumerate() {
            let work = &work;
            scope.spawn(move || work(part * each, run));
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn place(scale: f64, offset: f64) -> Upright {
        Upright {
            ax: scale,
            ex: offset,
            ay: scale,
            ey: offset,
        }
    }

    /// A flat colour comes out flat, edges included, at any scale and
    /// offset (the weights sum to one, and an edge's pixel stands in past
    /// it).
    #[test]
    fn a_flat_colour_stays_flat() {
        let src = vec![137u8; 97 * 61 * 3];
        for (scale, offset) in [(0.5731f64, 0.0f64), (0.3, 0.4), (0.9997, 0.25), (0.05, 3.7)] {
            let (dw, dh) = (
                (97.0 * scale + offset).ceil() as usize,
                (61.0 * scale + offset).ceil() as usize,
            );
            let out = resample(&src, 97, 61, 3, &place(scale, offset), (0, 0), (dw, dh));
            assert!(out.iter().all(|v| *v == 137), "{scale} {offset}");
        }
    }

    /// A ramp across the image stays a ramp, each device pixel the
    /// ramp's value at its own centre: nothing dropped, nothing doubled,
    /// whatever fraction of a pixel the size ends on. (As published, a
    /// width of 2200.3 was made 2201 and squeezed: a column went missing
    /// part of the way across.)
    #[test]
    fn a_ramp_keeps_every_step_in_its_place() {
        let (w, h) = (3840usize, 4usize);
        let mut src = Vec::with_capacity(w * h);
        for _ in 0..h {
            // (Each source pixel the ramp's nearest value.)
            src.extend((0..w).map(|x| ((x * 255 + (w - 1) / 2) / (w - 1)) as u8));
        }
        for scale in [0.573, 0.5730001, 0.45000002, 0.51875, 0.3, 0.123456] {
            let dw = (w as f64 * scale).ceil() as usize;
            let out = resample(&src, w, h, 1, &place(scale, 0.0), (0, 0), (dw, 2));
            for (i, v) in out[..dw].iter().enumerate().skip(4).take(dw - 8) {
                let centre = (i as f64 + 0.5) / scale;
                let want = (centre - 0.5) * 255.0 / (w - 1) as f64;
                assert!(
                    (*v as f64 - want).abs() <= 1.01,
                    "at {scale}: device pixel {i} is {v}, the ramp there is {want:.2}"
                );
            }
        }
    }

    /// Fine detail keeps its contrast: black and white stripes three
    /// source pixels wide each, shown at 0.6, still swing nearly the
    /// whole range (a bilinear or Hermite filter leaves them grey).
    #[test]
    fn fine_stripes_keep_their_contrast() {
        let (w, h) = (600usize, 8usize);
        let row: Vec<u8> = (0..w).map(|x| if (x / 3) % 2 == 0 { 0 } else { 255 }).collect();
        let src: Vec<u8> = (0..h).flat_map(|_| row.clone()).collect();
        let out = resample(&src, w, h, 1, &place(0.6, 0.0), (0, 0), (360, 4));
        let (low, high) = out[20..340].iter().fold((255u8, 0u8), |(l, h), v| (l.min(*v), h.max(*v)));
        assert!(low <= 25 && high >= 230, "{low}..{high}");
    }

    /// A part of the device (a surface smaller than the image's place on
    /// it) is the same pixels as the whole's, and a mirrored image is the
    /// mirror image.
    #[test]
    fn a_part_is_the_wholes_and_a_mirror_mirrors() {
        let (w, h) = (200usize, 150usize);
        let src: Vec<u8> = (0..w * h).map(|i| ((i * 31 + (i / w) * 17) % 251) as u8).collect();
        let p = place(0.437, 2.3);
        let (dw, dh) = (90usize, 68usize);
        let whole = resample(&src, w, h, 1, &p, (0, 0), (dw, dh));
        let part = resample(&src, w, h, 1, &p, (30, 20), (40, 30));
        for y in 0..30 {
            assert_eq!(part[y * 40..(y + 1) * 40], whole[(y + 20) * dw + 30..(y + 20) * dw + 70]);
        }
        // Mirrored left to right: device = -a × image + (a × w + e).
        let mirrored = Upright {
            ax: -p.ax,
            ex: p.ax * w as f64 + p.ex,
            ..p
        };
        let flipped_src: Vec<u8> = src.chunks_exact(w).flat_map(|r| r.iter().rev().copied()).collect();
        let a = resample(&src, w, h, 1, &mirrored, (0, 0), (dw, dh));
        let b = resample(&flipped_src, w, h, 1, &p, (0, 0), (dw, dh));
        assert_eq!(a, b);
    }

    /// How long a full-page picture takes (`--ignored --nocapture`):
    /// 3840 × 2160 RGB to 0.6 and to 0.3 of its size.
    #[test]
    #[ignore]
    fn how_long_a_page_takes() {
        let (w, h) = (3840usize, 2160usize);
        let src: Vec<u8> = (0..w * h * 3).map(|i| (i * 7 % 253) as u8).collect();
        for scale in [0.6, 0.3] {
            let (dw, dh) = ((w as f64 * scale) as usize, (h as f64 * scale) as usize);
            let began = std::time::Instant::now();
            let out = resample(&src, w, h, 3, &place(scale, 0.0), (0, 0), (dw, dh));
            println!("{w}x{h} to {dw}x{dh}: {:.1} ms ({} bytes)", began.elapsed().as_secs_f64() * 1e3, out.len());
        }
    }

    #[test]
    fn only_an_upright_transform_is_upright() {
        assert!(Upright::of(&Affine::new([0.5, 0.0, 0.0, 0.5, 3.2, 1.1])).is_some());
        assert!(Upright::of(&Affine::new([0.5, 0.0, 0.0, -0.5, 3.2, 100.0])).is_some());
        assert!(Upright::of(&Affine::rotate(0.3)).is_none());
        assert!(Upright::of(&Affine::new([0.5, 0.0, 0.1, 0.5, 0.0, 0.0])).is_none());
        assert!(Upright::of(&Affine::new([0.0, 0.0, 0.0, 0.5, 0.0, 0.0])).is_none());
    }
}
