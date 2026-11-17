//! How loud the chime plays: at a quarter of the output's full volume,
//! whatever the person's volume is set to, and not at all when the sound
//! is muted (Ben, 2026-11-16: "the olo startup sound should not play if
//! the sound is muted, and if not muted it should play at the same
//! volume regardless of what the user volume is set at, and that should
//! be 25% of the max volume"). It was played as any sound, at the
//! person's volume (as the computer started, at the card's own: full).
//!
//! The person's volume is left as it is: the chime is made louder or
//! softer by as much as their volume is below or above a quarter. In a
//! session that is the chime's own stream's volume (`stream_level`: the
//! sound server multiplies it with the output's); as the computer starts
//! it is the samples themselves (`card_gain`, `scaled`). The one case
//! where that can't be done cleanly, an output whose volume is the
//! card's own set below about a fifth, has its volume lent to the chime
//! (`lend`: set to a quarter while it plays, then put back).
//!
//! Levels are on the volume's own scale, what a slider shows (the sound
//! server's cubic one: a level's gain is its cube, so a quarter is
//! −36 dB); gains multiply samples.

/// The chime's loudness: a quarter of full volume.
pub const LEVEL: f32 = 0.25;

/// How far the chime's samples can be made louder before they clip: it
/// peaks at −6 dBFS (`render`).
pub const HEADROOM: f32 = 2.0;

/// An output at less than this plays nothing to speak of: as muted.
const SILENT: f32 = 0.0005;

/// A level's gain.
pub fn gain(level: f32) -> f32 {
    level.max(0.0).powi(3)
}

/// A gain's level.
pub fn level(gain: f32) -> f32 {
    gain.max(0.0).cbrt()
}

/// A gain in decibels.
pub fn db(gain: f32) -> f32 {
    20.0 * gain.max(f32::MIN_POSITIVE).log10()
}

/// The level to play the chime's stream at, the output being at `output`
/// (1.0 its full volume) and muted or not: none when it's muted or at
/// nothing (no chime). Times the output's, it comes to `LEVEL`.
///
/// `hardware`: the output's volume is the card's own (a mixer control,
/// headphones' own volume), turned down after the sound has left the
/// sound server as whole numbers; a stream made louder than its room
/// (`HEADROOM`) would clip on the way, so it's made no louder than that
/// (such an output set lower is lent to the chime first: `lend`). An
/// output whose volume the sound server applies itself takes any level:
/// nothing is rounded until both have been applied.
pub fn stream_level(output: f32, muted: bool, hardware: bool) -> Option<f32> {
    if muted || !(output > SILENT) {
        return None;
    }
    let wanted = LEVEL / output;
    Some(if hardware { wanted.min(level(HEADROOM)) } else { wanted })
}

/// What an output's volume is set to while the chime plays (and put
/// back after), if it must be: `LEVEL`, where the volume is the card's
/// own (`hardware`) and so low that the chime's stream would have to be
/// louder than its samples have room for. None otherwise: nothing of
/// the person's is touched.
pub fn lend(output: f32, muted: bool, hardware: bool) -> Option<f32> {
    (hardware && !muted && output > SILENT && LEVEL / output > level(HEADROOM) + 1e-4).then_some(LEVEL)
}

/// The gain for the chime's samples played straight to a card (as the
/// computer starts), whose own mixer is turned down by `card_db`
/// decibels from its loudest (0 where the card has no volume of its own
/// for its speakers: the sound server's is the only one). No more than
/// the samples' room.
pub fn card_gain(card_db: f32) -> f32 {
    10f32.powf((db(gain(LEVEL)) - card_db.min(0.0)) / 20.0).min(HEADROOM)
}

/// `samples` times `gain` as 32-bit ones (a card's own depth or near it:
/// turned far down, 16 bits would leave the chime ten), clipped at full
/// scale.
pub fn scaled(samples: &[i16], gain: f32) -> Vec<i32> {
    let by = f64::from(gain) * 65536.0;
    samples.iter().map(|&s| (f64::from(s) * by).round().clamp(f64::from(i32::MIN), f64::from(i32::MAX)) as i32).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Whatever the person's volume, the chime comes out at a quarter;
    /// muted or at nothing, not at all.
    #[test]
    fn a_quarter_whatever_the_volume() {
        for output in [1.0, 0.75, 0.4375, 0.25, 0.1, 0.02] {
            let stream = stream_level(output, false, false).unwrap();
            assert!((stream * output - LEVEL).abs() < 1e-6, "at {output}");
        }
        assert_eq!(stream_level(0.5, true, false), None, "muted");
        assert_eq!(stream_level(0.0, false, false), None, "at nothing");
        assert_eq!(stream_level(f32::NAN, false, false), None);
        // The card's own volume: exact down to where the samples' room
        // ends, then as loud as they cleanly go.
        assert!((stream_level(0.5, false, true).unwrap() - 0.5).abs() < 1e-6);
        assert!((stream_level(0.2, false, true).unwrap() * 0.2 - LEVEL).abs() < 1e-3);
        let most = stream_level(0.05, false, true).unwrap();
        assert!((gain(most) - HEADROOM).abs() < 1e-4, "{most}");
    }

    /// Only a card's own volume, set too low to make up, is lent.
    #[test]
    fn lent_only_when_it_must_be() {
        assert_eq!(lend(0.05, false, true), Some(LEVEL));
        assert_eq!(lend(0.163, false, true), Some(LEVEL));
        assert_eq!(lend(0.2, false, true), None, "made up by the stream");
        assert_eq!(lend(0.6, false, true), None);
        assert_eq!(lend(0.05, false, false), None, "the sound server's volume: the stream makes it up");
        assert_eq!(lend(0.05, true, true), None, "muted: nothing plays");
        assert_eq!(lend(0.0, false, true), None, "at nothing: nothing plays");
        // Lent, the stream plays as it is.
        assert_eq!(stream_level(LEVEL, false, true), Some(1.0));
    }

    /// Straight to a card: −36 dB where the card has no volume of its
    /// own, less by what its mixer already takes off, never past the
    /// samples' room.
    #[test]
    fn straight_to_a_card() {
        assert!((db(card_gain(0.0)) + 36.12).abs() < 0.01);
        assert!((db(card_gain(-20.0)) + 16.12).abs() < 0.01);
        assert!((db(card_gain(-36.12))).abs() < 0.01);
        assert!((card_gain(-60.0) - HEADROOM).abs() < 1e-6);
        // (A mixer said to be above its loudest: as at its loudest.)
        assert_eq!(card_gain(3.0), card_gain(0.0));
        let s = scaled(&[16384, -16384, 1, 0], card_gain(0.0));
        assert_eq!(s[0], -s[1]);
        assert!((f64::from(s[0]) / 2f64.powi(31) - 0.5 / 64.0).abs() < 1e-6, "{}", s[0]);
        assert_eq!(s[2], 1024, "a sample's least step kept");
        assert_eq!(scaled(&[i16::MAX], 4.0), [i32::MAX], "clipped, not wrapped");
    }
}
