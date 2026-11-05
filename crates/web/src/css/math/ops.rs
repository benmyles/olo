use super::*;
pub(super) fn progress(value: f64, start: f64, end: f64, clamp: bool) -> f64 {
    if value.is_nan() || start.is_nan() || end.is_nan() {
        return f64::NAN;
    }
    if start == end {
        if clamp || value == start {
            return 0.;
        }
        return if value < start { f64::NEG_INFINITY } else { f64::INFINITY };
    }
    let result = (value - start) / (end - start);
    if clamp { result.clamp(0., 1.) } else { result }
}
pub(super) fn unary(i: Instruction, a: f64) -> f64 {
    if a.is_nan() {
        return f64::NAN;
    }
    match i.op {
        Op::Abs => a.abs(),
        Op::Negate => -a,
        Op::Sign => {
            if a == 0. {
                a
            } else {
                a.signum()
            }
        }
        Op::Sin => (a * i.value).sin(),
        Op::Cos => (a * i.value).cos(),
        Op::Tan => (a * i.value).tan(),
        Op::Asin => a.asin().to_degrees(),
        Op::Acos => a.acos().to_degrees(),
        Op::Atan => a.atan().to_degrees(),
        Op::Sqrt => a.sqrt(),
        Op::Exp => a.exp(),
        _ => unreachable!(),
    }
}
pub(super) fn binary(i: Instruction, a: f64, b: f64) -> f64 {
    if a.is_nan() || b.is_nan() {
        return f64::NAN;
    }
    match i.op {
        Op::Add => a + b,
        Op::Sub => a - b,
        Op::Mul => a * b,
        Op::Div => a / b,
        Op::Min => min(a, b),
        Op::Max => max(a, b),
        Op::Atan2 => a.atan2(b).to_degrees(),
        Op::Pow => {
            if a.abs() == 1. && b.is_infinite() {
                f64::NAN
            } else {
                a.powf(b)
            }
        }
        Op::Hypot => a.hypot(b),
        Op::Log => log(a, b),
        Op::Round => round(a, b, i.arg),
        Op::Mod => modulo(a, b),
        Op::Rem => a % b,
        _ => unreachable!(),
    }
}
fn log(a: f64, b: f64) -> f64 {
    if a < 0. || b <= 0. || b == 1. {
        f64::NAN
    } else if a == 0. {
        f64::NEG_INFINITY
    } else if a == 1. {
        0.
    } else if a.is_infinite() {
        f64::INFINITY
    } else {
        a.ln() / b.ln()
    }
}
fn min(a: f64, b: f64) -> f64 {
    if a == b && a == 0. { if a.is_sign_negative() || b.is_sign_negative() { -0. } else { 0. } } else { a.min(b) }
}
fn max(a: f64, b: f64) -> f64 {
    if a == b && a == 0. { if a.is_sign_negative() && b.is_sign_negative() { -0. } else { 0. } } else { a.max(b) }
}
fn modulo(a: f64, b: f64) -> f64 {
    if a.is_infinite() || b == 0. {
        return f64::NAN;
    }
    if b.is_infinite() {
        return if a.is_sign_negative() != b.is_sign_negative() { f64::NAN } else { a };
    }
    let r = a % b;
    if r == 0. {
        0f64.copysign(b)
    } else if r.is_sign_negative() != b.is_sign_negative() {
        r + b
    } else {
        r
    }
}
fn round(a: f64, b: f64, strategy: u32) -> f64 {
    let b = b.abs();
    if b == 0. || (a.is_infinite() && b.is_infinite()) {
        return f64::NAN;
    }
    if a.is_infinite() || a == 0. {
        return a;
    }
    if b.is_infinite() {
        return match strategy {
            1 if a > 0. => f64::INFINITY,
            2 if a < 0. => f64::NEG_INFINITY,
            _ => 0f64.copysign(a),
        };
    }
    let rem = a % b;
    if rem == 0. {
        return a;
    }
    let truncated = a - rem;
    let (lower, upper) = if a < 0. { (truncated - b, if truncated == 0. { -0. } else { truncated }) } else { (truncated, truncated + b) };
    match strategy {
        1 => upper,
        2 => lower,
        3 => {
            if a < 0. {
                upper
            } else {
                lower
            }
        }
        _ => {
            if a < 0. {
                if -rem <= b + rem { upper } else { lower }
            } else if rem < b - rem {
                lower
            } else {
                upper
            }
        }
    }
}
