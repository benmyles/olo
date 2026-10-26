//! DeskCalc's arithmetic: a small recursive-descent evaluator.
//!
//! `+ - * / % ^ !`, × and ÷, parentheses, implicit multiplication (`2pi`,
//! `3(4+1)`), exponents (`1.5E3`), the constants `pi` (π) and `e`, and the
//! functions on Haiku's scientific keypad. A function takes the next term,
//! so `sin 30` and `sin(30)` agree, and `sqrt 2^2` is `sqrt(2^2)` as on a
//! calculator. Unbalanced closing parentheses at the end are forgiven.

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Angle {
    Radians,
    Degrees,
}

#[derive(Debug, Clone, PartialEq)]
enum Tok {
    Num(f64),
    Ident(String),
    Op(char),
}

fn lex(s: &str) -> Result<Vec<Tok>, String> {
    let chars: Vec<char> = s.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c.is_whitespace() {
            i += 1;
        } else if c.is_ascii_digit() || c == '.' {
            let start = i;
            while i < chars.len() && (chars[i].is_ascii_digit() || chars[i] == '.') {
                i += 1;
            }
            // An exponent: E or e, an optional sign, then digits.
            if i < chars.len() && (chars[i] == 'e' || chars[i] == 'E') {
                let mut j = i + 1;
                if j < chars.len() && (chars[j] == '+' || chars[j] == '-') {
                    j += 1;
                }
                if j < chars.len() && chars[j].is_ascii_digit() {
                    while j < chars.len() && chars[j].is_ascii_digit() {
                        j += 1;
                    }
                    i = j;
                }
            }
            let text: String = chars[start..i].iter().collect();
            let n = text.parse::<f64>().map_err(|_| format!("“{text}” isn't a number"))?;
            out.push(Tok::Num(n));
        } else if c.is_alphabetic() || c == 'π' {
            if c == 'π' {
                out.push(Tok::Ident("pi".into()));
                i += 1;
                continue;
            }
            let start = i;
            while i < chars.len() && chars[i].is_alphanumeric() && chars[i] != 'π' {
                i += 1;
            }
            out.push(Tok::Ident(chars[start..i].iter().collect::<String>().to_lowercase()));
        } else {
            let op = match c {
                '×' | '*' => '*',
                '÷' | '/' => '/',
                '−' | '-' => '-',
                '+' | '%' | '^' | '!' | '(' | ')' => c,
                ',' => return Err("use . for decimals".into()),
                _ => return Err(format!("“{c}” isn't understood")),
            };
            out.push(Tok::Op(op));
            i += 1;
        }
    }
    Ok(out)
}

const FUNCTIONS: &[&str] = &[
    "sin", "cos", "tan", "asin", "acos", "atan", "sinh", "cosh", "tanh", "ln", "log", "exp", "sqrt", "cbrt", "ceil",
    "floor", "abs", "round",
];

struct Parser<'a> {
    toks: &'a [Tok],
    at: usize,
    angle: Angle,
}

impl Parser<'_> {
    fn peek(&self) -> Option<&Tok> {
        self.toks.get(self.at)
    }

    fn eat(&mut self, op: char) -> bool {
        if self.peek() == Some(&Tok::Op(op)) {
            self.at += 1;
            true
        } else {
            false
        }
    }

    fn expr(&mut self) -> Result<f64, String> {
        let mut v = self.term()?;
        loop {
            if self.eat('+') {
                v += self.term()?;
            } else if self.eat('-') {
                v -= self.term()?;
            } else {
                return Ok(v);
            }
        }
    }

    /// Does the next token start a factor (for implicit multiplication)?
    /// Not a bare number: "1 2" is a typo, not 2.
    fn starts_factor(&self) -> bool {
        matches!(self.peek(), Some(Tok::Ident(_) | Tok::Op('(')))
    }

    fn term(&mut self) -> Result<f64, String> {
        let mut v = self.unary()?;
        loop {
            if self.eat('*') {
                v *= self.unary()?;
            } else if self.eat('/') {
                let d = self.unary()?;
                if d == 0.0 {
                    return Err("Division by zero".into());
                }
                v /= d;
            } else if self.eat('%') {
                let d = self.unary()?;
                if d == 0.0 {
                    return Err("Division by zero".into());
                }
                v %= d;
            } else if self.starts_factor() {
                v *= self.power()?;
            } else {
                return Ok(v);
            }
        }
    }

    fn unary(&mut self) -> Result<f64, String> {
        if self.eat('-') {
            Ok(-self.unary()?)
        } else if self.eat('+') {
            self.unary()
        } else {
            self.power()
        }
    }

    fn power(&mut self) -> Result<f64, String> {
        let base = self.postfix()?;
        if self.eat('^') {
            // Right-associative, and binds tighter than a leading minus
            // on its left (-2^2 = -4) but takes one on its right (2^-1).
            let exp = self.unary()?;
            return Ok(base.powf(exp));
        }
        Ok(base)
    }

    fn postfix(&mut self) -> Result<f64, String> {
        let mut v = self.primary()?;
        while self.eat('!') {
            v = factorial(v)?;
        }
        Ok(v)
    }

    fn primary(&mut self) -> Result<f64, String> {
        match self.peek().cloned() {
            Some(Tok::Num(n)) => {
                self.at += 1;
                Ok(n)
            }
            Some(Tok::Op('(')) => {
                self.at += 1;
                let v = self.expr()?;
                // A missing ) at the very end is fine: "sin(30" works.
                if !self.eat(')') && self.peek().is_some() {
                    return Err("Missing )".into());
                }
                Ok(v)
            }
            Some(Tok::Ident(name)) => {
                self.at += 1;
                match name.as_str() {
                    "pi" => Ok(std::f64::consts::PI),
                    "e" => Ok(std::f64::consts::E),
                    f if FUNCTIONS.contains(&f) => {
                        if self.peek().is_none() {
                            return Err(format!("{f} of what?"));
                        }
                        let x = self.power()?;
                        self.apply(f, x)
                    }
                    other => Err(format!("“{other}” isn't known")),
                }
            }
            Some(Tok::Op(c)) => Err(format!("Unexpected “{c}”")),
            None => Err("Incomplete expression".into()),
        }
    }

    fn apply(&self, f: &str, x: f64) -> Result<f64, String> {
        let (to_rad, from_rad) = match self.angle {
            Angle::Radians => (1.0, 1.0),
            Angle::Degrees => (std::f64::consts::PI / 180.0, 180.0 / std::f64::consts::PI),
        };
        let v = match f {
            "sin" => exact_trig((x * to_rad).sin()),
            "cos" => exact_trig((x * to_rad).cos()),
            "tan" => {
                let c = exact_trig((x * to_rad).cos());
                if c == 0.0 {
                    return Err("tan is undefined there".into());
                }
                exact_trig((x * to_rad).sin()) / c
            }
            "asin" => x.asin() * from_rad,
            "acos" => x.acos() * from_rad,
            "atan" => x.atan() * from_rad,
            "sinh" => x.sinh(),
            "cosh" => x.cosh(),
            "tanh" => x.tanh(),
            "ln" => x.ln(),
            "log" => x.log10(),
            "exp" => x.exp(),
            "sqrt" => x.sqrt(),
            "cbrt" => x.cbrt(),
            "ceil" => x.ceil(),
            "floor" => x.floor(),
            "abs" => x.abs(),
            "round" => x.round(),
            _ => unreachable!(),
        };
        if v.is_nan() {
            return Err(format!("{f} is undefined there"));
        }
        Ok(v)
    }
}

/// sin(180°) is 0, not 1.2e-16.
fn exact_trig(v: f64) -> f64 {
    if v.abs() < 1e-15 { 0.0 } else { v }
}

fn factorial(v: f64) -> Result<f64, String> {
    if v < 0.0 || v.fract() != 0.0 {
        return Err("! needs a whole number".into());
    }
    if v > 170.0 {
        return Ok(f64::INFINITY);
    }
    Ok((1..=v as u32).fold(1.0, |a, k| a * k as f64))
}

/// Evaluate `text`.
pub fn eval(text: &str, angle: Angle) -> Result<f64, String> {
    let toks = lex(text)?;
    if toks.is_empty() {
        return Err("Nothing to calculate".into());
    }
    let mut p = Parser { toks: &toks, at: 0, angle };
    let v = p.expr()?;
    // Stray closing parentheses at the end: forgive them.
    while p.eat(')') {}
    if p.at < toks.len() {
        return Err("Unexpected input".into());
    }
    if v.is_nan() {
        return Err("Not a number".into());
    }
    Ok(v)
}

/// A result as DeskCalc shows it: up to 15 significant digits, no
/// trailing zeros, exponent form when very large or small.
pub fn format(v: f64) -> String {
    if v.is_infinite() {
        return if v > 0.0 { "inf".into() } else { "-inf".into() };
    }
    if v == 0.0 {
        return "0".into();
    }
    let mag = v.abs().log10().floor() as i32;
    if !(-6..15).contains(&mag) {
        let s = format!("{:.14e}", v);
        let (m, e) = s.split_once('e').unwrap_or((&s, "0"));
        let m = m.trim_end_matches('0').trim_end_matches('.');
        let e: i32 = e.parse().unwrap_or(0);
        return format!("{m}E{e}");
    }
    let decimals = (14 - mag).clamp(0, 20) as usize;
    let s = format!("{:.*}", decimals, v);
    let s = if s.contains('.') { s.trim_end_matches('0').trim_end_matches('.').to_owned() } else { s };
    if s == "-0" { "0".into() } else { s }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ev(s: &str) -> String {
        match eval(s, Angle::Radians) {
            Ok(v) => format(v),
            Err(e) => format!("error: {e}"),
        }
    }

    fn deg(s: &str) -> String {
        format(eval(s, Angle::Degrees).unwrap())
    }

    #[test]
    fn arithmetic() {
        assert_eq!(ev("1+2*3"), "7");
        assert_eq!(ev("(1+2)*3"), "9");
        assert_eq!(ev("10/4"), "2.5");
        assert_eq!(ev("7%3"), "1");
        assert_eq!(ev("2^3^2"), "512");
        assert_eq!(ev("-2^2"), "-4");
        assert_eq!(ev("2^-1"), "0.5");
        assert_eq!(ev("5!"), "120");
        assert_eq!(ev("3!!"), "720");
        assert_eq!(ev("0.1+0.2"), "0.3");
        assert_eq!(ev("6 × 7 ÷ 2"), "21");
        assert_eq!(ev("1.5E3"), "1500");
        assert_eq!(ev("2e-3"), "0.002");
        assert_eq!(ev("--3"), "3");
    }

    #[test]
    fn implicit_and_constants() {
        assert_eq!(ev("2pi"), format(2.0 * std::f64::consts::PI));
        assert_eq!(ev("3(4+1)"), "15");
        assert_eq!(ev("(1+1)(2+2)"), "8");
        assert_eq!(ev("2e"), format(2.0 * std::f64::consts::E));
        assert_eq!(ev("π"), format(std::f64::consts::PI));
    }

    #[test]
    fn functions() {
        assert_eq!(ev("sqrt 16"), "4");
        assert_eq!(ev("sqrt(16)+1"), "5");
        assert_eq!(ev("sqrt 2^2"), "2");
        assert_eq!(ev("ln e"), "1");
        assert_eq!(ev("log 1000"), "3");
        assert_eq!(ev("cbrt 27"), "3");
        assert_eq!(ev("floor 2.7 + ceil 2.1"), "5");
        assert_eq!(deg("sin 30"), "0.5");
        assert_eq!(deg("sin 180"), "0");
        assert_eq!(deg("cos 60"), "0.5");
        assert_eq!(deg("atan 1"), "45");
        assert_eq!(ev("sin(pi/2"), "1");
        assert_eq!(ev("cos 0)"), "1");
    }

    #[test]
    fn errors() {
        assert!(ev("1/0").starts_with("error"));
        assert!(ev("sqrt(-1)").starts_with("error"));
        assert!(ev("2+").starts_with("error"));
        assert!(ev("foo").starts_with("error"));
        assert!(ev("1,5").starts_with("error"));
        assert!(ev("").starts_with("error"));
        assert!(ev("(1+2))3").starts_with("error"));
        assert!(ev("2.5!").starts_with("error"));
        assert!(ev("1 2 +").starts_with("error"));
    }

    #[test]
    fn formatting() {
        assert_eq!(format(1e20), "1E20");
        assert_eq!(format(1.5e-9), "1.5E-9");
        assert_eq!(format(123456789012345.0), "123456789012345");
        assert_eq!(format(1234567890123456.0), "1.23456789012346E15");
        assert_eq!(format(12345678901234.0), "12345678901234");
        assert_eq!(format(-0.000001), "-0.000001");
        assert_eq!(format(1.0 / 3.0), "0.333333333333333");
        assert_eq!(format(2.0 / 3.0), "0.666666666666667");
        assert_eq!(format(-0.0), "0");
        assert_eq!(format(f64::INFINITY), "inf");
        assert_eq!(ev("171!"), "inf");
    }
}
