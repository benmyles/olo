//! Faces found and recognised by two small models run in process with
//! tract: YuNet (OpenCV Zoo's face detector, MIT) finds each face and its
//! five landmarks (the eyes, the nose's tip, the mouth's corners), and
//! SFace (OpenCV Zoo's recogniser, Apache 2.0) turns the face, aligned to
//! its template by those landmarks, into 128 numbers: two of the same
//! person point the same way (cosine similarity). Infrared frames are
//! grey; the models take three channels, so the grey goes in each.

use std::path::Path;

use tract_onnx::prelude::*;

type Plan = std::sync::Arc<TypedSimplePlan>;

/// The detector's input, a square this many pixels a side.
const DETECT: usize = 640;
/// The recogniser's: an aligned face this many a side.
const FACE: usize = 112;
/// Faces the detector is sure of at least this much.
pub const DETECT_SCORE: f32 = 0.7;
/// OpenCV's threshold for SFace's cosine similarity: the same person
/// above it.
pub const SAME_PERSON: f32 = 0.363;

/// SFace's template: where the five landmarks go in its 112×112 face.
const TEMPLATE: [[f32; 2]; 5] = [[38.2946, 51.6963], [73.5318, 51.5014], [56.0252, 71.7366], [41.5493, 92.3655], [70.7299, 92.2041]];

/// A grey picture (an infrared frame, a photograph made grey).
#[derive(Clone, Debug)]
pub struct Gray {
    pub w: usize,
    pub h: usize,
    pub data: Vec<u8>,
}

impl Gray {
    pub fn from_image(img: &image::DynamicImage) -> Gray {
        let g = img.to_luma8();
        Gray { w: g.width() as usize, h: g.height() as usize, data: g.into_raw() }
    }

    /// Bilinear sample at (x, y), 0 outside.
    fn sample(&self, x: f32, y: f32) -> f32 {
        if x < 0.0 || y < 0.0 || x > (self.w - 1) as f32 || y > (self.h - 1) as f32 {
            return 0.0;
        }
        let (x0, y0) = (x.floor() as usize, y.floor() as usize);
        let (x1, y1) = ((x0 + 1).min(self.w - 1), (y0 + 1).min(self.h - 1));
        let (fx, fy) = (x - x0 as f32, y - y0 as f32);
        let p = |x: usize, y: usize| self.data[y * self.w + x] as f32;
        let top = p(x0, y0) * (1.0 - fx) + p(x1, y0) * fx;
        let bottom = p(x0, y1) * (1.0 - fx) + p(x1, y1) * fx;
        top * (1.0 - fy) + bottom * fy
    }

    /// The mean brightness inside `r` (x, y, w, h), clipped to the frame.
    pub fn mean_in(&self, r: [f32; 4]) -> f32 {
        let (x0, y0) = (r[0].max(0.0) as usize, r[1].max(0.0) as usize);
        let (x1, y1) = (((r[0] + r[2]) as usize).min(self.w), ((r[1] + r[3]) as usize).min(self.h));
        if x1 <= x0 || y1 <= y0 {
            return 0.0;
        }
        let mut sum = 0u64;
        for y in y0..y1 {
            sum += self.data[y * self.w + x0..y * self.w + x1].iter().map(|v| *v as u64).sum::<u64>();
        }
        sum as f32 / ((x1 - x0) * (y1 - y0)) as f32
    }
}

/// A face found: how sure, its box (x, y, w, h) and five landmarks, in
/// the picture's pixels.
#[derive(Clone, Debug, PartialEq)]
pub struct Face {
    pub score: f32,
    pub rect: [f32; 4],
    pub landmarks: [[f32; 2]; 5],
}

/// What a face is, as the recogniser sees it (unit length).
pub type Embedding = [f32; 128];

pub struct Models {
    detect: Plan,
    embed: Plan,
}

impl Models {
    /// The two models from `dir` (`yunet.onnx`, `sface.onnx`).
    pub fn load(dir: &Path) -> Result<Models, String> {
        let load =
            |name: &str, shape: [usize; 4]| -> TractResult<Plan> { tract_onnx::onnx().model_for_path(dir.join(name))?.with_input_fact(0, f32::fact(shape).into())?.into_optimized()?.into_runnable() };
        let detect = load("yunet.onnx", [1, 3, DETECT, DETECT]).map_err(|e| format!("the face detector: {e}"))?;
        let embed = load("sface.onnx", [1, 3, FACE, FACE]).map_err(|e| format!("the face recogniser: {e}"))?;
        Ok(Models { detect, embed })
    }

    /// The faces in `img`, the surest first.
    pub fn detect(&self, img: &Gray) -> Vec<Face> {
        // Scaled to fit the square, at its top left (the rest black).
        let k = DETECT as f32 / img.w.max(img.h) as f32;
        let mut view = tract_ndarray::Array4::<f32>::zeros((1, 3, DETECT, DETECT));
        {
            let (w, h) = (((img.w as f32 * k) as usize).min(DETECT), ((img.h as f32 * k) as usize).min(DETECT));
            for y in 0..h {
                for x in 0..w {
                    let v = img.sample((x as f32 + 0.5) / k - 0.5, (y as f32 + 0.5) / k - 0.5);
                    for c in 0..3 {
                        view[[0, c, y, x]] = v;
                    }
                }
            }
        }
        let input: Tensor = view.into();
        let out = match self.detect.run(tvec!(input.into())) {
            Ok(o) => o,
            Err(e) => {
                tracing::warn!("face detector: {e}");
                return Vec::new();
            }
        };
        let view = |i: usize| out[i].to_plain_array_view::<f32>().map(|a| a.iter().copied().collect::<Vec<f32>>()).unwrap_or_default();
        let mut faces = Vec::new();
        for (s, stride) in [8usize, 16, 32].into_iter().enumerate() {
            let (cls, obj, bbox, kps) = (view(s), view(3 + s), view(6 + s), view(9 + s));
            let cols = DETECT / stride;
            for (i, (c, o)) in cls.iter().zip(&obj).enumerate() {
                let score = (c.clamp(0.0, 1.0) * o.clamp(0.0, 1.0)).sqrt();
                if score < DETECT_SCORE || bbox.len() < i * 4 + 4 || kps.len() < i * 10 + 10 {
                    continue;
                }
                let (col, row) = ((i % cols) as f32, (i / cols) as f32);
                let st = stride as f32;
                let (cx, cy) = ((col + bbox[i * 4]) * st, (row + bbox[i * 4 + 1]) * st);
                let (w, h) = (bbox[i * 4 + 2].exp() * st, bbox[i * 4 + 3].exp() * st);
                let mut landmarks = [[0.0; 2]; 5];
                for (n, l) in landmarks.iter_mut().enumerate() {
                    *l = [(kps[i * 10 + 2 * n] + col) * st / k, (kps[i * 10 + 2 * n + 1] + row) * st / k];
                }
                faces.push(Face { score, rect: [(cx - w / 2.0) / k, (cy - h / 2.0) / k, w / k, h / k], landmarks });
            }
        }
        faces.sort_by(|a, b| b.score.total_cmp(&a.score));
        // One box a face: those overlapping a surer one go.
        let mut kept: Vec<Face> = Vec::new();
        for f in faces {
            if kept.iter().all(|k| iou(&k.rect, &f.rect) < 0.3) {
                kept.push(f);
            }
        }
        kept
    }

    /// What `face` (found in `img`) is.
    pub fn embed(&self, img: &Gray, face: &Face) -> Option<Embedding> {
        let t = align(&face.landmarks);
        let mut view = tract_ndarray::Array4::<f32>::zeros((1, 3, FACE, FACE));
        {
            for v in 0..FACE {
                for u in 0..FACE {
                    // From the face's square back into the picture.
                    let (x, y) = inverse(&t, u as f32, v as f32);
                    let p = img.sample(x, y);
                    for c in 0..3 {
                        view[[0, c, v, u]] = p;
                    }
                }
            }
        }
        let input: Tensor = view.into();
        let out = self.embed.run(tvec!(input.into())).ok()?;
        let raw: Vec<f32> = out[0].to_plain_array_view::<f32>().ok()?.iter().copied().collect();
        if raw.len() != 128 {
            return None;
        }
        let norm = raw.iter().map(|v| v * v).sum::<f32>().sqrt().max(1e-6);
        let mut e = [0.0; 128];
        for (o, v) in e.iter_mut().zip(raw) {
            *o = v / norm;
        }
        Some(e)
    }
}

/// How alike two faces are: their cosine similarity (1 the same).
pub fn similarity(a: &Embedding, b: &Embedding) -> f32 {
    a.iter().zip(b).map(|(x, y)| x * y).sum()
}

fn iou(a: &[f32; 4], b: &[f32; 4]) -> f32 {
    let (x0, y0) = (a[0].max(b[0]), a[1].max(b[1]));
    let (x1, y1) = ((a[0] + a[2]).min(b[0] + b[2]), (a[1] + a[3]).min(b[1] + b[3]));
    let inter = (x1 - x0).max(0.0) * (y1 - y0).max(0.0);
    let union = a[2] * a[3] + b[2] * b[3] - inter;
    if union <= 0.0 { 0.0 } else { inter / union }
}

/// The similarity (rotation, one scale, a shift: [a, b, tx, ty], as
/// x' = a·x − b·y + tx, y' = b·x + a·y + ty) taking `landmarks` nearest
/// to the template (least squares: Procrustes without reflection).
fn align(landmarks: &[[f32; 2]; 5]) -> [f32; 4] {
    let mean = |p: &[[f32; 2]; 5]| {
        let s = p.iter().fold([0.0, 0.0], |acc, q| [acc[0] + q[0], acc[1] + q[1]]);
        [s[0] / 5.0, s[1] / 5.0]
    };
    let (ms, md) = (mean(landmarks), mean(&TEMPLATE));
    let (mut num_a, mut num_b, mut den) = (0.0, 0.0, 0.0);
    for (s, d) in landmarks.iter().zip(&TEMPLATE) {
        let (x, y) = (s[0] - ms[0], s[1] - ms[1]);
        let (dx, dy) = (d[0] - md[0], d[1] - md[1]);
        num_a += x * dx + y * dy;
        num_b += x * dy - y * dx;
        den += x * x + y * y;
    }
    let den = den.max(1e-6);
    let (a, b) = (num_a / den, num_b / den);
    [a, b, md[0] - (a * ms[0] - b * ms[1]), md[1] - (b * ms[0] + a * ms[1])]
}

/// The picture's point that `t` takes to (u, v).
fn inverse(t: &[f32; 4], u: f32, v: f32) -> (f32, f32) {
    let [a, b, tx, ty] = *t;
    let det = (a * a + b * b).max(1e-9);
    let (x, y) = (u - tx, v - ty);
    ((a * x + b * y) / det, (-b * x + a * y) / det)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_template_aligns_to_itself() {
        let t = align(&TEMPLATE);
        assert!((t[0] - 1.0).abs() < 1e-4 && t[1].abs() < 1e-4 && t[2].abs() < 1e-3 && t[3].abs() < 1e-3, "{t:?}");
        // Twice the size, shifted: halved and shifted back.
        let big = TEMPLATE.map(|p| [p[0] * 2.0 + 30.0, p[1] * 2.0 + 10.0]);
        let t = align(&big);
        let (x, y) = inverse(&t, TEMPLATE[2][0], TEMPLATE[2][1]);
        assert!((x - big[2][0]).abs() < 1e-2 && (y - big[2][1]).abs() < 1e-2, "{x} {y}");
    }

    /// The models on public-domain NASA portraits (`OLO_FACE_SAMPLES`:
    /// a folder with the models and armstrong-1.jpg, armstrong-2.jpg,
    /// aldrin.jpg and collins.jpg): two photographs of one man alike,
    /// of two men not, in grey as infrared is.
    #[test]
    #[ignore]
    fn recognises_a_face_in_grey() {
        let dir = std::path::PathBuf::from(std::env::var("OLO_FACE_SAMPLES").expect("OLO_FACE_SAMPLES"));
        let models = Models::load(&dir).unwrap();
        let embed = |name: &str| {
            let img = Gray::from_image(&image::open(dir.join(name)).unwrap());
            let t = std::time::Instant::now();
            let faces = models.detect(&img);
            let found = t.elapsed();
            let face = faces.first().unwrap_or_else(|| panic!("no face in {name}"));
            let t = std::time::Instant::now();
            let e = models.embed(&img, face).unwrap();
            println!(
                "{name}: {} faces, the first {:.2} sure at {:?}; found in {:.0} ms, recognised in {:.0} ms",
                faces.len(),
                face.score,
                face.rect,
                found.as_secs_f64() * 1e3,
                t.elapsed().as_secs_f64() * 1e3
            );
            e
        };
        let (a1, a2, al, co) = (embed("armstrong-1.jpg"), embed("armstrong-2.jpg"), embed("aldrin.jpg"), embed("collins.jpg"));
        let same = similarity(&a1, &a2);
        let others = [similarity(&a1, &al), similarity(&a1, &co), similarity(&a2, &al), similarity(&al, &co)];
        println!("Armstrong with himself {same:.3}; with the others {others:.3?}");
        assert!(same > SAME_PERSON, "{same}");
        assert!(others.iter().all(|o| *o < SAME_PERSON), "{others:?}");
    }
}
