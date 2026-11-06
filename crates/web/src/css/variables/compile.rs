use super::*;
use crate::css::Component;
pub(super) struct Arguments<'a> {
    pub first: Components<'a>,
    pub fallback: Option<Components<'a>>,
}
pub(super) fn arguments(c: Component<'_>) -> Result<Arguments<'_>> {
    let all = c.children();
    let mut input = all.clone();
    let first = input.find(|c| c.kind() != Kind::Whitespace).ok_or(Error::Invalid)?;
    let (first, fallback) = if first.kind() == Kind::Curly {
        let next = input.find(|c| c.kind() != Kind::Whitespace);
        let fallback = match next {
            Some(comma) if comma.kind() == Kind::Comma => Some(all.between(comma.after(), c.after())),
            None => None,
            _ => return Err(Error::Invalid),
        };
        (first.children(), fallback)
    } else {
        let mut end = c.after();
        let mut fallback = None;
        if first.kind() == Kind::Comma {
            return Err(Error::Invalid);
        }
        for v in input {
            if v.kind() == Kind::Curly {
                return Err(Error::Invalid);
            }
            if v.kind() == Kind::Comma {
                end = v.at;
                fallback = Some(all.between(v.after(), c.after()));
                break;
            }
        }
        (all.between(first.at, end), fallback)
    };
    if first.clone().all(|c| c.kind() == Kind::Whitespace) {
        return Err(Error::Invalid);
    }
    let fallback = fallback
        .map(|f| {
            let mut input = f.clone();
            if let Some(c) = input.find(|c| c.kind() != Kind::Whitespace)
                && c.kind() == Kind::Curly
            {
                if input.any(|c| c.kind() != Kind::Whitespace) {
                    return Err(Error::Invalid);
                }
                return Ok(c.children());
            }
            Ok(f)
        })
        .transpose()?;
    Ok(Arguments { first, fallback })
}
struct Compiler<'a> {
    limits: Limits,
    work: usize,
    cancelled: Option<&'a AtomicBool>,
}
impl Compiler<'_> {
    fn scan(&mut self, values: Components<'_>, top: bool, depth: usize) -> Result<bool> {
        check(self.cancelled)?;
        if depth > self.limits.depth.min(256) {
            return Err(Error::DepthLimit);
        }
        let mut pending = false;
        for c in values {
            check(self.cancelled)?;
            self.work = self.work.checked_add(1 + (c.span().end - c.span().start) as usize).ok_or(Error::Limit)?;
            if self.work > self.limits.work {
                return Err(Error::Limit);
            }
            if matches!(c.kind(), Kind::BadString | Kind::BadUrl | Kind::Close) || top && (c.kind() == Kind::Semicolon || c.character() == Some('!')) {
                return Err(Error::Invalid);
            }
            if c.is_container() {
                if c.kind() == Kind::Function && c.text().unwrap().eq_ignore_ascii_case("var") {
                    let args = arguments(c)?;
                    self.scan(args.first, true, depth + 1)?;
                    if let Some(f) = args.fallback {
                        self.scan(f, true, depth + 1)?;
                    }
                    pending = true;
                } else {
                    pending |= self.scan(c.children(), false, depth + 1)?;
                    pending |= c.kind() == Kind::Function && substitution(c.text().unwrap());
                }
            }
        }
        Ok(pending)
    }
}
pub(super) fn one(values: Components<'_>, limits: Limits, cancelled: Option<&AtomicBool>) -> Result<Value> {
    let pending = Compiler { limits, work: 0, cancelled }.scan(values.clone(), true, 0)?;
    let mut assembled = Assembler::new(limits.syntax());
    assembled.copy(values, cancelled).map_err(syntax_error)?;
    let end = assembled.len() | if pending { PENDING } else { 0 };
    Ok(Value { syntax: Arc::new(assembled.finish()), start: 0, end })
}
pub(super) fn batch<'a>(values: impl IntoIterator<Item = Components<'a>>, limits: Limits, cancelled: Option<&AtomicBool>) -> Result<Vec<Value>> {
    let mut compiler = Compiler { limits, work: 0, cancelled };
    let mut assembled = Assembler::new(limits.syntax());
    let mut ranges = Vec::new();
    for values in values {
        check(cancelled)?;
        if ranges.len() >= limits.properties {
            return Err(Error::Limit);
        }
        let pending = compiler.scan(values.clone(), true, 0)?;
        let start = assembled.len();
        assembled.copy(values, cancelled).map_err(syntax_error)?;
        ranges.push(start..assembled.len() | if pending { PENDING } else { 0 });
    }
    let syntax = Arc::new(assembled.finish());
    Ok(ranges.into_iter().map(|range| Value { syntax: syntax.clone(), start: range.start, end: range.end }).collect())
}

pub(crate) fn inspect(values: Components<'_>, limits: Limits, cancelled: Option<&AtomicBool>) -> Result<bool> {
    Compiler { limits, work: 0, cancelled }.scan(values, true, 0)
}
