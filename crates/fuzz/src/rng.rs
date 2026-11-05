//! A small seeded random source (SplitMix64): a run is reproducible
//! from its seed as long as the session starts the same way.

pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Rng {
        Rng(seed ^ 0x9e37_79b9_7f4a_7c15)
    }

    pub fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }

    /// In [0, 1).
    pub fn random(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64
    }

    /// In [lo, hi].
    pub fn uniform(&mut self, lo: f64, hi: f64) -> f64 {
        lo + (hi - lo) * self.random()
    }

    /// In [0, n).
    pub fn below(&mut self, n: usize) -> usize {
        if n == 0 { 0 } else { (self.next_u64() % n as u64) as usize }
    }

    /// In [lo, hi], both included.
    pub fn int(&mut self, lo: i64, hi: i64) -> i64 {
        lo + self.below((hi - lo + 1).max(1) as usize) as i64
    }

    pub fn choice<'a, T>(&mut self, items: &'a [T]) -> &'a T {
        &items[self.below(items.len())]
    }

    /// One of `ids`, sorted first (so the order they came in doesn't
    /// matter), or None.
    pub fn pick(&mut self, ids: impl IntoIterator<Item = u64>) -> Option<u64> {
        let mut v: Vec<u64> = ids.into_iter().collect();
        if v.is_empty() {
            return None;
        }
        v.sort_unstable();
        v.dedup();
        Some(v[self.below(v.len())])
    }

    /// `k` of `items`, in a random order.
    pub fn sample<T: Clone>(&mut self, items: &[T], k: usize) -> Vec<T> {
        let mut v: Vec<T> = items.to_vec();
        for i in 0..v.len() {
            let j = i + self.below(v.len() - i);
            v.swap(i, j);
        }
        v.truncate(k);
        v
    }

    /// An entry of `table` with a chance its weight's share.
    pub fn weighted<T: Copy>(&mut self, table: &[(u32, T)]) -> T {
        let total: u32 = table.iter().map(|(w, _)| w).sum();
        let mut n = self.uniform(0.0, total as f64);
        for &(w, t) in table {
            n -= w as f64;
            if n <= 0.0 {
                return t;
            }
        }
        table[table.len() - 1].1
    }
}
