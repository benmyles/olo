//! The ThinkPad's palm model (`thinkpad_model`): `thinkpad_baseline`
//! trained on Ben's hands on the pad (`data/thinkpad-2026-11-05*.json`:
//! a flat palm, a heel, a flat thumb, palms resting as when typing;
//! fingertips pointing to every edge and corner, tapping, clicking,
//! dragging and scrolling, a thumb resting low and clicking, a heel
//! resting while a finger points), by `make_thinkpad_model` (don't
//! edit: run it again). 85 contacts (25 palms, 60 fingers): it catches 100.0% of
//! the palms, each by its report 5 (the firmware's word, at once or
//! nearly), and takes 0.0% of the fingers
//! for palms (on the 17 kept back from training: 100.0% and 0.0%; the
//! hand-written one there: 100.0% and 0.0%).

pub(crate) const CONTACTS: u32 = 85;
pub(crate) const PALMS_CAUGHT: f32 = 1.0;
pub(crate) const FINGERS_LOST: f32 = 0.0;
pub(crate) const THRESHOLD: f32 = 0.001;
pub(crate) const B2: f32 = -0.6975657;
pub(crate) const MEAN: &[f32] = &[0.16907495, 0.21579936, 0.18829979, 0.19990343, 0.056417547, 0.009430392, 0.014999023, 0.11791858];
pub(crate) const STD: &[f32] = &[0.37481505, 0.4113653, 0.3754473, 0.39993352, 0.10849706, 0.01955154, 0.02244225, 0.073684484];
pub(crate) const LO: &[f32] = &[0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0];
pub(crate) const HI: &[f32] = &[1.0, 1.0, 1.0, 1.0, 0.6055013, 0.174, 0.174, 0.249];
pub(crate) const B1: &[f32] = &[-0.10539757, -0.59243834, 0.56337124, 0.23331907, 0.6222709, -0.021110874];
pub(crate) const W2: &[f32] = &[2.222103, 1.2493241, -1.4701262, -1.2466551, -1.5211364, -0.0013694862];
pub(crate) const W1: &[f32] = &[
    0.03449564,
    1.2594457,
    0.4508752,
    0.11888821,
    -0.008678351,
    -0.07315464,
    -0.040543325,
    0.05258815,
    0.61811817,
    0.12763104,
    0.04510629,
    0.70632887,
    -0.0148034915,
    -0.024357688,
    -0.0003420507,
    0.03144487,
    -0.17789419,
    -0.93861216,
    -0.19817512,
    -0.19750138,
    0.810374,
    0.29028735,
    0.26139584,
    0.26030764,
    -0.15024902,
    -0.9071922,
    -0.16784894,
    -0.16725889,
    -0.04871709,
    0.2462752,
    0.58086395,
    0.2211149,
    -0.17942363,
    -1.1225933,
    -0.44975108,
    -0.19878782,
    -0.058596976,
    0.2994599,
    0.26900926,
    0.26581553,
    -8.4096304e-5,
    -0.00026876156,
    -0.00014445948,
    -0.00015739157,
    -3.7382717e-5,
    -0.097790934,
    -3.7914488e-5,
    0.055274844,
];
