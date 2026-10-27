//! Arithmetic typed into the palette ("12*(3+4)", "2^10", "15% of 80"):
//! its answer is the first result, and choosing it copies the number.

/// The value of `text` if it is arithmetic (and not just a number).
pub fn eval(text: &str) -> Option<f64> {
    let text = text.trim().replace(" of ", "*").replace('×', "*").replace('÷', "/").replace(',', "");
    if !text.chars().any(|c| "+-*/^%()".contains(c)) || !text.chars().any(|c| c.is_ascii_digit()) {
        return None;
    }
    let mut p = Parser { s: text.as_bytes(), i: 0 };
    let v = p.sum()?;
    p.space();
    (p.i == p.s.len() && v.is_finite()).then_some(v)
}

/// A value as shown: up to 10 decimals, trailing zeros dropped.
pub fn show(v: f64) -> String {
    if v.fract() == 0.0 && v.abs() < 1e15 {
        return format!("{}", v as i64);
    }
    let s = format!("{v:.10}");
    s.trim_end_matches('0').trim_end_matches('.').to_owned()
}

struct Parser<'a> {
    s: &'a [u8],
    i: usize,
}

impl Parser<'_> {
    fn space(&mut self) {
        while self.s.get(self.i).is_some_and(|c| c.is_ascii_whitespace()) {
            self.i += 1;
        }
    }

    fn eat(&mut self, c: u8) -> bool {
        self.space();
        if self.s.get(self.i) == Some(&c) {
            self.i += 1;
            true
        } else {
            false
        }
    }

    fn sum(&mut self) -> Option<f64> {
        let mut v = self.product()?;
        loop {
            if self.eat(b'+') {
                v += self.product()?;
            } else if self.eat(b'-') {
                v -= self.product()?;
            } else {
                return Some(v);
            }
        }
    }

    fn product(&mut self) -> Option<f64> {
        let mut v = self.power()?;
        loop {
            if self.eat(b'*') {
                v *= self.power()?;
            } else if self.eat(b'/') {
                v /= self.power()?;
            } else {
                return Some(v);
            }
        }
    }

    fn power(&mut self) -> Option<f64> {
        let base = self.unary()?;
        if self.eat(b'^') {
            // Right-associative.
            return Some(base.powf(self.power()?));
        }
        Some(base)
    }

    fn unary(&mut self) -> Option<f64> {
        if self.eat(b'-') {
            return Some(-self.unary()?);
        }
        if self.eat(b'+') {
            return self.unary();
        }
        let mut v = self.atom()?;
        // "15%" is 0.15.
        while self.eat(b'%') {
            v /= 100.0;
        }
        Some(v)
    }

    fn atom(&mut self) -> Option<f64> {
        if self.eat(b'(') {
            let v = self.sum()?;
            return self.eat(b')').then_some(v);
        }
        self.space();
        let start = self.i;
        while self.s.get(self.i).is_some_and(|c| c.is_ascii_digit() || *c == b'.') {
            self.i += 1;
        }
        std::str::from_utf8(&self.s[start..self.i]).ok()?.parse().ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sums() {
        assert_eq!(eval("12*(3+4)"), Some(84.0));
        assert_eq!(eval("2^3^2"), Some(512.0));
        assert_eq!(eval("15% of 80"), Some(12.0));
        assert_eq!(eval("-3 + 1.5"), Some(-1.5));
        assert_eq!(eval("1,000 / 8"), Some(125.0));
        assert_eq!(eval("42"), None);
        assert_eq!(eval("tile left"), None);
        assert_eq!(eval("1/0"), None);
        assert_eq!(eval("(1+2"), None);
        assert_eq!(show(0.1 + 0.2), "0.3");
        assert_eq!(show(84.0), "84");
    }
}
