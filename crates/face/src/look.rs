//! A look: frames from the camera until a face enrolled is seen (or the
//! time's up), and enrolling one. Only infrared frames count, so a face
//! on a screen isn't there at all (displays give off no infrared). A face
//! must be near enough and lit by the emitter's flood, and its relief
//! must be like the one enrolled: the middle of a real face (the nose,
//! the brow) catches the flood more than its sides, which turn away,
//! while a print is lit evenly (the ratio, measured at enrolment, is
//! the person's own). Two frames must match, not one.

use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::{
    camera::Frames,
    model::{Embedding, Face, Gray, Models, similarity},
};

/// A face this many pixels wide at least (about an arm's length from
/// the ThinkPad's 640-pixel camera).
const NEAR: f32 = 80.0;
/// The similarity a look needs to an enrolled face: above SFace's own
/// threshold (0.363), as infrared and a lock want a margin.
pub const MATCH: f32 = 0.42;
/// Looks enrolled: from straight on and turned a little.
pub const ENROL: usize = 5;

/// What's kept of a person's face (root's alone: `olo-faced`).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Enrolled {
    /// Each look's embedding.
    pub faces: Vec<Vec<f32>>,
    /// The least relief a look of theirs had, a margin taken off.
    pub relief: f32,
}

/// How a look went.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum Outcome {
    /// The person enrolled (the best similarity).
    Match(f32),
    /// A face, but not theirs (the best similarity).
    NoMatch(f32),
    /// No face near enough, lit, all along.
    NoFace,
    /// Theirs, but flat: a picture.
    Flat,
}

/// How much brighter a face's middle is than its sides, in the flood.
pub fn relief(img: &Gray, f: &Face) -> f32 {
    let [x, y, w, h] = f.rect;
    let middle = img.mean_in([x + w * 0.35, y + h * 0.3, w * 0.3, h * 0.4]);
    let left = img.mean_in([x, y + h * 0.3, w * 0.15, h * 0.4]);
    let right = img.mean_in([x + w * 0.85, y + h * 0.3, w * 0.15, h * 0.4]);
    middle / ((left + right) / 2.0).max(1.0)
}

/// The face to go by in a frame: the biggest near enough and lit.
fn the_face(models: &Models, img: &Gray) -> Option<Face> {
    let mut faces: Vec<Face> = models.detect(img).into_iter().filter(|f| f.rect[2] >= NEAR).collect();
    faces.sort_by(|a, b| (b.rect[2] * b.rect[3]).total_cmp(&(a.rect[2] * a.rect[3])));
    let f = faces.into_iter().next()?;
    // Lit: the flood makes a near face bright (a dark one is too far or
    // the emitter's off).
    (img.mean_in(f.rect) > 30.0).then_some(f)
}

/// Look for `enrolled` in the frames until `limit`.
pub fn verify(frames: &mut dyn Frames, models: &Models, enrolled: &Enrolled, limit: Duration) -> Outcome {
    let start = Instant::now();
    let faces: Vec<Embedding> = enrolled.faces.iter().filter_map(|f| f.as_slice().try_into().ok()).collect();
    let (mut matched, mut best, mut flat, mut seen) = (0, -1.0f32, false, false);
    while start.elapsed() < limit {
        // No one there at all: not worth the wait (the password's next).
        if !seen && start.elapsed() > Duration::from_secs(2) {
            break;
        }
        let Ok(img) = frames.frame(Duration::from_millis(500)) else { continue };
        let Some(face) = the_face(models, &img) else { continue };
        seen = true;
        let Some(e) = models.embed(&img, &face) else { continue };
        let s = faces.iter().map(|f| similarity(f, &e)).fold(-1.0, f32::max);
        best = best.max(s);
        if s < MATCH {
            continue;
        }
        if relief(&img, &face) < enrolled.relief {
            flat = true;
            continue;
        }
        matched += 1;
        if matched >= 2 {
            return Outcome::Match(best);
        }
    }
    match (seen, flat) {
        (_, true) => Outcome::Flat,
        (true, _) => Outcome::NoMatch(best),
        _ => Outcome::NoFace,
    }
}

/// Enrol a face from the frames: `ENROL` looks, each a little different
/// from those before (the landmarks moved), within `limit`. `progress`
/// hears how many so far.
pub fn enrol(frames: &mut dyn Frames, models: &Models, limit: Duration, progress: &dyn Fn(usize)) -> Option<Enrolled> {
    let start = Instant::now();
    let mut looks: Vec<(Embedding, [[f32; 2]; 5], f32)> = Vec::new();
    while looks.len() < ENROL && start.elapsed() < limit {
        let Ok(img) = frames.frame(Duration::from_millis(500)) else { continue };
        let Some(face) = the_face(models, &img) else { continue };
        // A new look: its landmarks a few percent of the face away from
        // every one kept (turned, nearer, further).
        let moved = |l: &[[f32; 2]; 5]| l.iter().zip(&face.landmarks).map(|(a, b)| (a[0] - b[0]).hypot(a[1] - b[1])).sum::<f32>() / 5.0;
        if looks.iter().any(|(_, l, _)| moved(l) < face.rect[2] * 0.04) {
            continue;
        }
        let Some(e) = models.embed(&img, &face) else { continue };
        // The same person throughout.
        if looks.iter().any(|(k, _, _)| similarity(k, &e) < MATCH) {
            continue;
        }
        looks.push((e, face.landmarks, relief(&img, &face)));
        progress(looks.len());
    }
    if looks.len() < ENROL {
        return None;
    }
    let least = looks.iter().map(|l| l.2).fold(f32::MAX, f32::min);
    // A margin under the least: a little more turned than at enrolment
    // still passes; a print (about 1) doesn't when theirs is well over.
    let relief = 1.0 + (least - 1.0).max(0.0) * 0.5;
    Some(Enrolled { faces: looks.into_iter().map(|l| l.0.to_vec()).collect(), relief })
}
