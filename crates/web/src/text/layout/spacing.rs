use super::*;

/// Logical inline-box advances at a prepared UTF-8 boundary. Start belongs
/// to following text, end to preceding text. Empty boxes after text use End.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Side {
    Start,
    End,
}
#[derive(Clone, Copy, Debug)]
pub struct Spacing {
    pub at: u32,
    pub advance: f64,
    pub side: Side,
}
struct Row {
    at: u32,
    start: f64,
    end: f64,
    starts: u32,
    ends: u32,
}
#[derive(Default)]
pub(super) struct Spaces {
    rows: Vec<Row>,
}
impl Spaces {
    pub fn new(mut input: Vec<Spacing>, source: &str, limit: usize, cancel: Option<&AtomicBool>) -> Result<Self> {
        check(cancel)?;
        if input.len() > limit || input.len() > u32::MAX as usize {
            return Err(Error::Limit);
        }
        if input.iter().any(|s| s.at as usize > source.len() || !source.is_char_boundary(s.at as usize) || !s.advance.is_finite()) {
            return Err(Error::Invalid);
        }
        input.sort_by_key(|s| s.at);
        let mut result = Self::default();
        for s in input {
            check(cancel)?;
            if let Some(last) = result.rows.last_mut().filter(|r| r.at == s.at) {
                if s.side == Side::Start {
                    last.start += s.advance;
                    last.starts += 1;
                } else {
                    last.end += s.advance;
                    last.ends += 1;
                }
                if !last.start.is_finite() || !last.end.is_finite() {
                    return Err(Error::Invalid);
                }
            } else {
                let start = result.rows.last().map_or(0., |r| r.start) + if s.side == Side::Start { s.advance } else { 0. };
                let end = result.rows.last().map_or(0., |r| r.end) + if s.side == Side::End { s.advance } else { 0. };
                let starts = result.rows.last().map_or(0, |r| r.starts) + u32::from(s.side == Side::Start);
                let ends = result.rows.last().map_or(0, |r| r.ends) + u32::from(s.side == Side::End);
                if !start.is_finite() || !end.is_finite() {
                    return Err(Error::Invalid);
                }
                result.rows.push(Row { at: s.at, start, end, starts, ends });
            }
        }
        Ok(result)
    }
    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }
    fn prefix(&self, i: usize, start: bool) -> f64 {
        i.checked_sub(1).map_or(0., |i| if start { self.rows[i].start } else { self.rows[i].end })
    }
    pub fn contains(&self, open_from: u32, close_from: u32, end: u32) -> bool {
        let hi = self.rows.partition_point(|r| r.at <= end);
        let lo_start = self.rows.partition_point(|r| r.at < open_from);
        let lo_end = self.rows.partition_point(|r| r.at <= close_from);
        let count = |i: usize, start: bool| i.checked_sub(1).map_or(0, |i| if start { self.rows[i].starts } else { self.rows[i].ends });
        count(hi, true) > count(lo_start, true) || count(hi, false) > count(lo_end, false)
    }
    /// Opening edges within trimmed trailing whitespace move to the following
    /// line. Closing and empty-box edges stay before the break. The two source
    /// starts differ when a previous line consumed whitespace after an opening.
    pub fn width(&self, open_from: u32, close_from: u32, text_end: u32, consumed_end: u32, final_end: bool) -> f64 {
        if self.rows.is_empty() {
            return 0.;
        }
        let open_lo = self.rows.partition_point(|r| r.at < open_from);
        let open_hi = if final_end { self.rows.partition_point(|r| r.at <= consumed_end) } else { self.rows.partition_point(|r| r.at < text_end) };
        let close_lo = self.rows.partition_point(|r| r.at <= close_from);
        let close_hi = self.rows.partition_point(|r| r.at <= consumed_end);
        self.prefix(open_hi, true) - self.prefix(open_lo, true) + self.prefix(close_hi, false) - self.prefix(close_lo, false)
    }
    pub fn before(&self, open_from: u32, close_from: u32, at: u32) -> f64 {
        self.width(open_from, close_from, at, at, true)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn edges_belong_to_the_correct_side_of_a_line_break() {
        let e = |at, advance, side| Spacing { at, advance, side };
        let s = Spaces::new(vec![e(0, 2., Side::Start), e(2, 3., Side::Start), e(2, 5., Side::End), e(4, 7., Side::Start), e(4, 11., Side::End)], "abcd", 8, None).unwrap();
        assert_eq!(s.width(0, 0, 2, 2, false), 7.);
        assert_eq!(s.width(2, 2, 4, 4, false), 14.);
        assert_eq!(s.width(2, 2, 4, 4, true), 21.);
        assert_eq!(s.before(0, 0, 2), 10.);
        assert_eq!(s.width(4, 4, 4, 4, true), 7.);
        let closed = Spaces::new(vec![e(1, 5., Side::End)], "a ", 8, None).unwrap();
        assert!(!closed.contains(1, 2, 2));
        assert!(Spaces::new(vec![e(1, 1., Side::Start)], "é", 8, None).is_err());
        assert!(Spaces::new(vec![e(0, f64::MAX, Side::Start), e(0, f64::MAX, Side::Start)], "", 8, None).is_err());
        assert!(Spaces::new(Vec::new(), "", 0, Some(&AtomicBool::new(true))).is_err());
    }
}
