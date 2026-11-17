//! How the test's figures are said.

/// A rate: "6.39 Mbit/s", "63.9 Mbit/s", "639 Mbit/s", "1.23 Gbit/s".
pub fn rate(mbps: f64) -> String {
    match mbps {
        m if m >= 995.0 => format!("{:.2} Gbit/s", m / 1000.0),
        m if m >= 99.5 => format!("{m:.0} Mbit/s"),
        m if m >= 9.95 => format!("{m:.1} Mbit/s"),
        m if m >= 0.995 => format!("{m:.2} Mbit/s"),
        m => format!("{:.0} kbit/s", m * 1000.0),
    }
}

/// A time: "4.8 ms", "46 ms", "1.20 s".
pub fn ms(ms: f64) -> String {
    match ms {
        t if t >= 995.0 => format!("{:.2} s", t / 1000.0),
        t if t >= 9.95 => format!("{t:.0} ms"),
        t => format!("{t:.1} ms"),
    }
}

/// A count with its thousands apart: "271,293".
pub fn count(n: u64) -> String {
    let digits = n.to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// An amount of data: "639 MB", "1.28 GB".
pub fn bytes(n: u64) -> String {
    match n as f64 {
        b if b >= 0.995e9 => format!("{:.2} GB", b / 1e9),
        b if b >= 0.995e6 => format!("{:.0} MB", b / 1e6),
        b if b >= 0.995e3 => format!("{:.0} kB", b / 1e3),
        b => format!("{b:.0} bytes"),
    }
}

/// A share: "0.01%", "2.5%", "12%"; "none" for nothing at all.
pub fn percent(p: f64) -> String {
    match p {
        p if p <= 0.0 => "none".into(),
        p if p < 0.005 => "under 0.01%".into(),
        p if p < 0.995 => format!("{p:.2}%"),
        p if p < 9.95 => format!("{p:.1}%"),
        p => format!("{p:.0}%"),
    }
}

/// How responsive the line is while busy, in a word (round trips a
/// minute, as Apple's networkQuality grades them).
pub fn responsiveness(rpm: f64) -> &'static str {
    match rpm {
        r if r >= 800.0 => "high",
        r if r >= 200.0 => "medium",
        _ => "low",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn figures_are_said_to_three_digits() {
        assert_eq!([rate(0.512), rate(6.394), rate(63.94), rate(639.4), rate(999.9), rate(1234.0)], ["512 kbit/s", "6.39 Mbit/s", "63.9 Mbit/s", "639 Mbit/s", "1.00 Gbit/s", "1.23 Gbit/s"]);
        assert_eq!([ms(4.84), ms(9.96), ms(46.3), ms(1203.0)], ["4.8 ms", "10 ms", "46 ms", "1.20 s"]);
        assert_eq!([count(0), count(17), count(1000), count(271_293), count(1_234_567)], ["0", "17", "1,000", "271,293", "1,234,567"]);
        assert_eq!([bytes(900), bytes(64_000), bytes(638_525_947), bytes(1_280_000_000)], ["900 bytes", "64 kB", "639 MB", "1.28 GB"]);
        assert_eq!([percent(0.0), percent(0.001), percent(0.0063), percent(2.51), percent(11.9)], ["none", "under 0.01%", "0.01%", "2.5%", "12%"]);
        assert_eq!([responsiveness(1200.0), responsiveness(700.0), responsiveness(90.0)], ["high", "medium", "low"]);
    }
}
