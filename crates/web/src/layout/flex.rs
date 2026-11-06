//! Original single-line flex size distribution. Inputs are used content-box
//! bases/constraints and non-content extents, in CSS pixels. This worker stage
//! does not construct items, determine intrinsic/auto bases, wrap or align them.
use super::*;

#[derive(Clone, Copy, Debug)]
pub struct Item {
    pub base: f64,
    pub minimum: f64,
    pub maximum: f64,
    pub grow: f64,
    pub shrink: f64,
    /// Padding, borders and non-auto margins along the main axis.
    pub outer: f64,
}
impl Default for Item {
    fn default() -> Self {
        Self { base: 0., minimum: 0., maximum: f64::INFINITY, grow: 0., shrink: 1., outer: 0. }
    }
}
#[derive(Clone, Copy, Debug, Default)]
pub struct Progress {
    pub work: usize,
    pub rounds: usize,
    pub frozen: usize,
    pub finished: bool,
}
pub struct Line {
    sizes: Box<[f64]>,
    remaining: f64,
}
impl Line {
    pub fn sizes(&self) -> &[f64] {
        &self.sizes
    }
    /// Space still available for alignment/auto margins; negative is overflow.
    pub fn remaining(&self) -> f64 {
        self.remaining
    }
}
#[derive(Clone, Copy, PartialEq, Eq)]
enum Phase {
    Setup,
    Sum,
    Distribute,
    Freeze,
    Export,
}
const ACTIVE: u8 = 0;
const MINIMUM: u8 = 1;
const MAXIMUM: u8 = 2;
const FROZEN: u8 = 3;
/// Consumes its scalar item scratch and publishes one size per item. Each
/// budget unit visits at most one item; repeated constraint freezes share the
/// global work quota. No layout, shaping, timer or UI operation happens here.
pub struct Job<'a> {
    items: Vec<Item>,
    targets: Vec<f64>,
    prefixes: Vec<f64>,
    states: Vec<u8>,
    unit: f64,
    available: f64,
    gaps: f64,
    used: f64,
    factor: f64,
    weight: f64,
    free: f64,
    initial_free: f64,
    violation: f64,
    grow: bool,
    initial: bool,
    newly_frozen: usize,
    phase: Phase,
    at: usize,
    limits: Limits,
    cancel: Option<&'a AtomicBool>,
    progress: Progress,
    failed: bool,
}
fn units(value: f64, unit: f64) -> Result<f64> {
    let value = value * unit;
    if !value.is_finite() {
        return Err(Error::Invalid);
    }
    // Keep every layout quantum exactly representable through subtraction.
    if value.abs() > (1u64 << 52) as f64 {
        return Err(Error::Limit);
    }
    Ok(value.round())
}
impl<'a> Job<'a> {
    pub fn new(items: Vec<Item>, available: f64, gap: f64, scale: f64, limits: Limits, cancel: Option<&'a AtomicBool>) -> Result<Self> {
        check(cancel)?;
        if items.len() > limits.boxes {
            return Err(Error::Limit);
        }
        if !scale.is_finite() || scale <= 0. || !(scale * 64.).is_finite() || scale * 64. == 0. || available < 0. || gap < 0. {
            return Err(Error::Invalid);
        }
        let unit = scale * 64.;
        let available = units(available, unit)?;
        let gaps = units(gap, unit)? * items.len().saturating_sub(1) as f64;
        if !gaps.is_finite() {
            return Err(Error::Invalid);
        }
        if gaps > (1u64 << 52) as f64 {
            return Err(Error::Limit);
        }
        let finished = items.is_empty();
        Ok(Self {
            items,
            targets: Vec::new(),
            prefixes: Vec::new(),
            states: Vec::new(),
            unit,
            available,
            gaps,
            used: gaps,
            factor: 0.,
            weight: 0.,
            free: 0.,
            initial_free: 0.,
            violation: 0.,
            grow: false,
            initial: true,
            newly_frozen: 0,
            phase: Phase::Setup,
            at: 0,
            limits,
            cancel,
            progress: Progress { finished, ..Default::default() },
            failed: false,
        })
    }
    pub fn progress(&self) -> Progress {
        self.progress
    }
    /// Scalar item/scratch payload; excludes capacity, headers and allocator
    /// overhead. Only eight bytes per item survive in the published sizes.
    pub fn working_payload_bytes(&self) -> usize {
        self.items.len() * size_of::<Item>() + (self.targets.len() + self.prefixes.len()) * size_of::<f64>() + self.states.len()
    }
    fn valid_accumulators(&self) -> Result<()> {
        // Factors are finite f32 and bases bounded by 2^52; even usize::MAX
        // such terms cannot overflow f64. Preserve exact grid arithmetic in
        // the signed lengths, including negative non-auto margins.
        debug_assert!([self.used, self.factor, self.weight, self.free, self.violation].into_iter().all(f64::is_finite));
        if [self.used, self.free, self.violation].into_iter().any(|v| v.abs() > (1u64 << 52) as f64) {
            return Err(Error::Limit);
        }
        Ok(())
    }
    pub fn step(&mut self, budget: usize) -> Result<Progress> {
        if self.failed {
            return Err(Error::Failed);
        }
        let result = self.advance(budget);
        if result.is_err() {
            self.failed = true;
        }
        result
    }
    fn hypothetical(item: Item) -> f64 {
        item.base.max(item.minimum).min(item.maximum.max(item.minimum))
    }
    fn factor(&self, item: Item) -> f64 {
        if self.grow { item.grow } else { item.shrink }
    }
    fn transition(&mut self) -> Result<()> {
        self.at = 0;
        match self.phase {
            Phase::Setup => {
                self.grow = self.used < self.available;
                self.used = self.gaps;
                self.phase = Phase::Sum;
            }
            Phase::Sum => {
                self.free = self.available - self.used;
                if self.initial {
                    self.initial_free = self.free;
                    self.initial = false;
                }
                if self.progress.frozen == self.items.len() || (self.grow && self.free <= 0.) || (!self.grow && self.free >= 0.) {
                    self.used = self.gaps;
                    self.phase = Phase::Export;
                } else {
                    if self.factor < 1. {
                        let partial = (self.initial_free * self.factor).trunc();
                        if partial.abs() < self.free.abs() {
                            self.free = partial;
                        }
                    }
                    self.violation = 0.;
                    self.at = self.items.len();
                    self.progress.rounds += 1;
                    self.phase = Phase::Distribute;
                }
            }
            Phase::Distribute => {
                if self.violation == 0. {
                    // All targets are final, including opposing adjustments
                    // whose sum is zero. Avoid two redundant full-item passes.
                    self.progress.frozen = self.items.len();
                    self.used = self.gaps;
                    self.phase = Phase::Export;
                } else {
                    self.newly_frozen = 0;
                    self.phase = Phase::Freeze;
                }
            }
            Phase::Freeze => {
                if self.newly_frozen == 0 {
                    return Err(Error::Invalid);
                }
                self.used = self.gaps;
                self.factor = 0.;
                self.weight = 0.;
                self.phase = Phase::Sum;
            }
            Phase::Export => self.progress.finished = true,
        }
        self.valid_accumulators()
    }
    fn advance(&mut self, budget: usize) -> Result<Progress> {
        check(self.cancel)?;
        for _ in 0..budget {
            if self.progress.finished {
                break;
            }
            check(self.cancel)?;
            self.progress.work = self.progress.work.checked_add(1).ok_or(Error::Limit)?;
            if self.progress.work > self.limits.work {
                return Err(Error::Limit);
            }
            if (self.phase == Phase::Distribute && self.at == 0) || (self.phase != Phase::Distribute && self.at == self.items.len()) {
                self.transition()?;
                continue;
            }
            let at = if self.phase == Phase::Distribute {
                self.at -= 1;
                self.at
            } else {
                let at = self.at;
                self.at += 1;
                at
            };
            match self.phase {
                Phase::Setup => {
                    let item = &mut self.items[at];
                    if item.base < 0. || item.minimum < 0. || item.maximum < 0. || !item.grow.is_finite() || item.grow < 0. || !item.shrink.is_finite() || item.shrink < 0. {
                        return Err(Error::Invalid);
                    }
                    item.base = units(item.base, self.unit)?;
                    item.minimum = units(item.minimum, self.unit)?;
                    if item.maximum != f64::INFINITY {
                        item.maximum = units(item.maximum, self.unit)?;
                    }
                    item.outer = units(item.outer, self.unit)?;
                    item.grow = f64::from(item.grow as f32);
                    item.shrink = f64::from(item.shrink as f32);
                    if !item.grow.is_finite() || !item.shrink.is_finite() {
                        return Err(Error::Limit);
                    }
                    let hypothetical = Self::hypothetical(*item);
                    self.targets.push(hypothetical);
                    self.prefixes.push(0.);
                    self.states.push(ACTIVE);
                    self.used += hypothetical + item.outer;
                }
                Phase::Sum => {
                    let item = self.items[at];
                    let hypothetical = Self::hypothetical(item);
                    let factor = self.factor(item);
                    if self.progress.rounds == 0 && self.states[at] != FROZEN && (factor == 0. || if self.grow { item.base > hypothetical } else { item.base < hypothetical }) {
                        self.states[at] = FROZEN;
                        self.progress.frozen += 1;
                    }
                    if self.states[at] == FROZEN {
                        self.used += self.targets[at] + item.outer;
                    } else {
                        self.targets[at] = hypothetical;
                        self.states[at] = ACTIVE;
                        self.factor += factor;
                        self.weight += if self.grow { factor } else { factor * item.base };
                        self.prefixes[at] = self.weight;
                        self.used += item.base + item.outer;
                    }
                }
                Phase::Distribute => {
                    if self.states[at] == FROZEN {
                        continue;
                    }
                    let item = self.items[at];
                    let weight = if self.grow { item.grow } else { item.shrink * item.base };
                    let extra = if self.prefixes[at] == 0. { 0. } else { (self.free * (weight / self.prefixes[at])).round() };
                    self.free -= extra;
                    let target = item.base + extra;
                    let clamped = target.max(item.minimum).min(item.maximum.max(item.minimum));
                    let violation = clamped - target;
                    self.targets[at] = clamped;
                    self.states[at] = if violation > 0. {
                        MINIMUM
                    } else if violation < 0. {
                        MAXIMUM
                    } else {
                        ACTIVE
                    };
                    self.violation += violation;
                }
                Phase::Freeze => {
                    if self.states[at] != FROZEN && (self.violation == 0. || self.states[at] == if self.violation > 0. { MINIMUM } else { MAXIMUM }) {
                        self.states[at] = FROZEN;
                        self.newly_frozen += 1;
                        self.progress.frozen += 1;
                    }
                }
                Phase::Export => {
                    self.used += self.targets[at] + self.items[at].outer;
                    self.targets[at] /= self.unit;
                }
            }
            self.valid_accumulators()?;
        }
        Ok(self.progress)
    }
    pub fn finish(self) -> Result<Line> {
        check(self.cancel)?;
        if self.failed {
            Err(Error::Failed)
        } else if !self.progress.finished {
            Err(Error::Incomplete)
        } else {
            Ok(Line { sizes: self.targets.into_boxed_slice(), remaining: (self.available - self.used) / self.unit })
        }
    }
}
