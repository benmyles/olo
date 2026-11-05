//! CSS Syntax An+B microsyntax over already-tokenized components.
use super::{Error, Result};
use crate::css::{Component, Components, Kind};
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AnB {
    pub a: i64,
    pub b: i64,
}
impl AnB {
    /// Coefficients outside i64 are explicit resource non-passes, never
    /// silently saturated into a different formula.
    pub fn parse(mut input: Components<'_>) -> Result<Self> {
        skip(&mut input);
        let first = input.next().ok_or(Error::Invalid)?;
        if first.kind() == Kind::Integer {
            let b = integer(first)?;
            end(input)?;
            return Ok(Self { a: 0, b });
        }
        let (a, unit) = match first.kind() {
            Kind::IntegerDimension => (number(first.number().unwrap().representation)?, first.unit().unwrap()),
            Kind::Ident => {
                let s = first.text().unwrap();
                if s.eq_ignore_ascii_case("odd") || s.eq_ignore_ascii_case("even") {
                    end(input)?;
                    return Ok(Self { a: 2, b: i64::from(s.eq_ignore_ascii_case("odd")) });
                }
                if let Some(s) = s.strip_prefix('-') { (-1, s) } else { (1, s) }
            }
            Kind::Delim if first.character() == Some('+') => {
                let n = input.next().ok_or(Error::Invalid)?;
                if n.kind() != Kind::Ident {
                    return Err(Error::Invalid);
                }
                (1, n.text().unwrap())
            }
            _ => return Err(Error::Invalid),
        };
        if !unit.as_bytes().first().is_some_and(|c| c.eq_ignore_ascii_case(&b'n')) {
            return Err(Error::Invalid);
        }
        let suffix = &unit[1..];
        let b = if suffix.is_empty() {
            skip(&mut input);
            match input.next() {
                None => return Ok(Self { a, b: 0 }),
                Some(c) if c.kind() == Kind::Integer => {
                    let r = c.number().unwrap().representation;
                    if !r.starts_with(['+', '-']) {
                        return Err(Error::Invalid);
                    }
                    number(r)?
                }
                Some(c) if c.kind() == Kind::Delim && matches!(c.character(), Some('+' | '-')) => {
                    skip(&mut input);
                    signed_unsigned(input.next().ok_or(Error::Invalid)?, c.character() == Some('-'))?
                }
                _ => return Err(Error::Invalid),
            }
        } else if let Some(digits) = suffix.strip_prefix('-') {
            if digits.is_empty() {
                skip(&mut input);
                signed_unsigned(input.next().ok_or(Error::Invalid)?, true)?
            } else {
                if !digits.bytes().all(|b| b.is_ascii_digit()) {
                    return Err(Error::Invalid);
                }
                negative(digits)?
            }
        } else {
            return Err(Error::Invalid);
        };
        end(input)?;
        Ok(Self { a, b })
    }
    pub fn matches(self, index: u64) -> bool {
        if index == 0 {
            return false;
        }
        let difference = index as i128 - self.b as i128;
        if self.a == 0 { difference == 0 } else { difference % self.a as i128 == 0 && difference / self.a as i128 >= 0 }
    }
}
fn signed_unsigned(c: Component<'_>, negative_sign: bool) -> Result<i64> {
    if c.kind() != Kind::Integer {
        return Err(Error::Invalid);
    }
    let r = c.number().unwrap().representation;
    if !r.as_bytes()[0].is_ascii_digit() {
        return Err(Error::Invalid);
    }
    if negative_sign { negative(r) } else { number(r) }
}
fn negative(s: &str) -> Result<i64> {
    let n = s.parse::<i128>().map_err(|_| Error::NumberLimit)?;
    i64::try_from(n.checked_neg().ok_or(Error::NumberLimit)?).map_err(|_| Error::NumberLimit)
}
fn number(s: &str) -> Result<i64> {
    s.parse().map_err(|_| Error::NumberLimit)
}
fn integer(c: Component<'_>) -> Result<i64> {
    if c.kind() != Kind::Integer {
        return Err(Error::Invalid);
    }
    let r = c.number().unwrap().representation;
    number(r)
}
fn skip(input: &mut Components<'_>) {
    while input.clone().next().is_some_and(|c| c.kind() == Kind::Whitespace) {
        input.next();
    }
}
fn end(mut input: Components<'_>) -> Result<()> {
    skip(&mut input);
    if input.next().is_none() { Ok(()) } else { Err(Error::Invalid) }
}
