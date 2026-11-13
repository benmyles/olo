//! The paragraphs' heights added up, and the widest of them: a tree over
//! them (each node the sum of the heights below it, and the widest), so
//! one paragraph's height changing costs log n, not a pass over all of
//! them, and so do a paragraph's top and the paragraph at a place down
//! the text. The paragraphs wider than a width are found by going down
//! only where something is (a wrap width changing touches only those
//! that can wrap differently).
//!
//! Sums are kept in f64: a long text's tops are exact far down, where
//! adding up thousands of f32s drifts by whole points.

pub(super) struct Heights {
    /// Leaves from `cap` (a power of two): each paragraph's height, then
    /// none; each node above them the sum of its two.
    sum: Vec<f64>,
    /// The same for the widest natural width.
    wide: Vec<f32>,
    cap: usize,
    len: usize,
}

impl Heights {
    pub(super) fn new() -> Heights {
        Heights { sum: vec![0.0; 2], wide: vec![0.0; 2], cap: 1, len: 0 }
    }

    /// Made again for `items` (each paragraph's height and natural width).
    pub(super) fn build(&mut self, items: impl ExactSizeIterator<Item = (f32, f32)>) {
        let n = items.len();
        let cap = n.max(1).next_power_of_two();
        self.cap = cap;
        self.len = n;
        self.sum.clear();
        self.sum.resize(2 * cap, 0.0);
        self.wide.clear();
        self.wide.resize(2 * cap, 0.0);
        for (k, (h, w)) in items.enumerate() {
            self.sum[cap + k] = h as f64;
            self.wide[cap + k] = w;
        }
        for k in (1..cap).rev() {
            self.sum[k] = self.sum[2 * k] + self.sum[2 * k + 1];
            self.wide[k] = self.wide[2 * k].max(self.wide[2 * k + 1]);
        }
    }

    /// How many paragraphs it was made for.
    pub(super) fn len(&self) -> usize {
        self.len
    }

    /// Paragraph `i` is `h` tall and `w` wide unwrapped.
    pub(super) fn set(&mut self, i: usize, h: f32, w: f32) {
        let mut k = self.cap + i;
        if self.sum[k] == h as f64 && self.wide[k] == w {
            return;
        }
        self.sum[k] = h as f64;
        self.wide[k] = w;
        while k > 1 {
            k /= 2;
            self.sum[k] = self.sum[2 * k] + self.sum[2 * k + 1];
            self.wide[k] = self.wide[2 * k].max(self.wide[2 * k + 1]);
        }
    }

    /// Paragraph `i`'s top: those before it added up (all of them for
    /// `i` past the last).
    pub(super) fn top(&self, i: usize) -> f64 {
        if i >= self.len {
            return self.total();
        }
        let mut k = self.cap + i;
        let mut above = 0.0;
        while k > 1 {
            // A right child: all of its left sibling is before it.
            if k & 1 == 1 {
                above += self.sum[k - 1];
            }
            k /= 2;
        }
        above
    }

    pub(super) fn total(&self) -> f64 {
        self.sum[1]
    }

    /// The paragraph at `y` down the text: the first whose bottom is below
    /// it (the first for `y` above the text, the last for `y` past it).
    pub(super) fn at(&self, y: f64) -> usize {
        let (mut k, mut y) = (1, y);
        while k < self.cap {
            let left = self.sum[2 * k];
            if left <= y {
                y -= left;
                k = 2 * k + 1;
            } else {
                k *= 2;
            }
        }
        (k - self.cap).min(self.len.saturating_sub(1))
    }

    /// The widest paragraph's natural width.
    pub(super) fn widest(&self) -> f32 {
        self.wide[1]
    }

    /// The paragraphs wider than `w`, in order, into `out`.
    pub(super) fn wider(&self, w: f32, out: &mut Vec<usize>) {
        let mut stack = vec![1usize];
        while let Some(k) = stack.pop() {
            if self.wide[k] <= w {
                continue;
            }
            if k >= self.cap {
                if k - self.cap < self.len {
                    out.push(k - self.cap);
                }
                continue;
            }
            stack.push(2 * k + 1);
            stack.push(2 * k);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Heights;

    /// What a pass over them gave: each top, the total, the paragraph at
    /// a place (as `partition_point` over the bottoms did).
    fn tops(h: &[f32]) -> Vec<f64> {
        let mut out = vec![0.0];
        for &x in h {
            out.push(out.last().unwrap() + x as f64);
        }
        out
    }

    fn at(tops: &[f64], y: f64) -> usize {
        tops[1..].partition_point(|&t| t <= y).min(tops.len() - 2)
    }

    #[test]
    fn as_a_pass_over_them_adds_them_up() {
        for n in [1, 2, 3, 7, 8, 9, 100, 1000] {
            let hs: Vec<f32> = (0..n).map(|i| [17.5, 0.0, 35.0, 18.25, 52.5][i % 5]).collect();
            let ws: Vec<f32> = (0..n).map(|i| (i * 37 % 101) as f32).collect();
            let mut t = Heights::new();
            t.build(hs.iter().copied().zip(ws.iter().copied()));
            let want = tops(&hs);
            for i in 0..=n {
                assert_eq!(t.top(i), want[i], "top of {i} of {n}");
            }
            assert_eq!(t.total(), want[n]);
            for y in [-5.0, 0.0, 17.4, 17.5, 17.6, 52.5, 70.75, want[n] - 0.1, want[n], want[n] + 40.0] {
                assert_eq!(t.at(y), at(&want, y), "at {y} of {n}");
            }
            assert_eq!(t.widest(), ws.iter().copied().fold(0.0, f32::max));
            let mut wide = Vec::new();
            t.wider(60.0, &mut wide);
            assert_eq!(wide, (0..n).filter(|&i| ws[i] > 60.0).collect::<Vec<_>>());
        }
    }

    #[test]
    fn one_changing_changes_what_follows() {
        let mut hs = vec![20.0f32; 1000];
        let mut t = Heights::new();
        t.build(hs.iter().map(|&h| (h, 10.0)));
        for (i, h) in [(0, 40.0), (999, 0.0), (500, 61.5), (3, 20.0)] {
            hs[i] = h;
            t.set(i, h, 10.0);
            let want = tops(&hs);
            for k in [0, 1, 3, 4, 499, 500, 501, 998, 999, 1000] {
                assert_eq!(t.top(k), want[k]);
            }
            assert_eq!(t.at(want[501] - 0.5), 500);
        }
        t.set(250, 20.0, 400.0);
        assert_eq!(t.widest(), 400.0);
        let mut wide = Vec::new();
        t.wider(10.0, &mut wide);
        assert_eq!(wide, vec![250]);
        t.set(250, 20.0, 5.0);
        assert_eq!(t.widest(), 10.0);
    }
}
