//! Original typed CSS numeric calculations. Compile once; resolve relative
//! units against an explicit host context. No cascade, CSSOM or layout.
//! Run long operations on workers; retain programs and reuse Scratch.
mod compute;
mod ops;
mod parse;
mod units;
use crate::css::{Component, Components};
use smallvec::SmallVec;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
pub use units::RelativeUnit;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Dimension {
    Number,
    Length,
    Angle,
    Time,
    Frequency,
    Resolution,
    Flex,
    Percentage,
}
impl Dimension {
    fn axis(self) -> Option<usize> {
        if self == Self::Number { None } else { Some(self as usize - 1) }
    }
}
/// Seven dimension exponents and the calculation's percentage hint.
/// Exponents outside [-127,127] are an explicit implementation limit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NumericType {
    powers: [i8; 7],
    hint: Option<Dimension>,
}
impl NumericType {
    pub fn dimension(dimension: Dimension) -> Self {
        let mut result = Self { powers: [0; 7], hint: None };
        if let Some(axis) = dimension.axis() {
            result.powers[axis] = 1;
        }
        result
    }
    pub fn kind(self) -> Option<Dimension> {
        let Some(axis) = self.powers.iter().position(|&p| p != 0) else {
            return Some(Dimension::Number);
        };
        if self.powers[axis] != 1 || self.powers.iter().enumerate().any(|(i, &p)| i != axis && p != 0) {
            return None;
        }
        Some([Dimension::Length, Dimension::Angle, Dimension::Time, Dimension::Frequency, Dimension::Resolution, Dimension::Flex, Dimension::Percentage][axis])
    }
    pub fn percentage_hint(self) -> Option<Dimension> {
        self.hint
    }
    fn percentage(options: Options) -> Result<Self> {
        if !options.allow_percentage {
            return Err(Error::Invalid);
        }
        let dimension = options.percentage_hint.unwrap_or(Dimension::Percentage);
        if dimension == Dimension::Number {
            return Err(Error::Invalid);
        }
        let mut result = Self::dimension(dimension);
        result.hint = Some(dimension);
        Ok(result)
    }
    fn apply_hint(&mut self, hint: Dimension) -> Result<()> {
        if hint != Dimension::Percentage {
            let axis = hint.axis().ok_or(Error::Invalid)?;
            self.powers[axis] = self.powers[axis].checked_add(self.powers[6]).ok_or(Error::TypeLimit)?;
            self.powers[6] = 0;
        }
        self.hint = Some(hint);
        Ok(())
    }
    fn align(mut self, mut rhs: Self) -> Result<(Self, Self)> {
        match (self.hint, rhs.hint) {
            (Some(a), Some(b)) if a != b => return Err(Error::Invalid),
            (Some(h), None) => rhs.apply_hint(h)?,
            (None, Some(h)) => self.apply_hint(h)?,
            _ => {}
        }
        Ok((self, rhs))
    }
    fn add(self, rhs: Self) -> Result<Self> {
        let (left, right) = self.align(rhs)?;
        if left.powers != right.powers {
            return Err(Error::Invalid);
        }
        Ok(left)
    }
    fn product(self, rhs: Self, divide: bool) -> Result<Self> {
        let (mut left, right) = self.align(rhs)?;
        for i in 0..7 {
            left.powers[i] = if divide { left.powers[i].checked_sub(right.powers[i]) } else { left.powers[i].checked_add(right.powers[i]) }.filter(|&n| n != i8::MIN).ok_or(Error::TypeLimit)?;
        }
        Ok(left)
    }
    fn consistent_result(self, dimension: Dimension) -> Self {
        let mut result = Self::dimension(dimension);
        result.hint = self.hint;
        result
    }
}
#[derive(Clone, Copy, Debug)]
pub struct Options {
    pub allow_percentage: bool,
    /// Percentages relative to numbers stay percentage-typed. Only dimension
    /// hints are allowed; the host supplies their basis when resolving.
    pub percentage_hint: Option<Dimension>,
}
impl Default for Options {
    fn default() -> Self {
        Self { allow_percentage: true, percentage_hint: None }
    }
}
#[derive(Clone, Copy, Debug)]
pub struct Limits {
    /// Compile-time emitter storage, including temporary instructions.
    pub instructions: usize,
    /// Compile-time source/token work, or executed resolution instructions.
    pub work: usize,
    /// Compile-time nesting, with an unconditional hard maximum of 128.
    pub depth: usize,
    /// Maximum resolution value-stack slots, checked while compiling too.
    pub stack: usize,
}
impl Default for Limits {
    fn default() -> Self {
        Self { instructions: 65536, work: 1_000_000, depth: 64, stack: 4096 }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    Invalid,
    Unsupported(&'static str),
    RequiresSubstitution,
    ContextUnavailable,
    Limit,
    TypeLimit,
    DepthLimit,
    Cancelled,
}
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "CSS math {self:?}")
    }
}
impl std::error::Error for Error {}
pub type Result<T> = std::result::Result<T, Error>;
/// Canonical px/deg/s/Hz/dppx/fr coordinates, or percentage points.
/// IEEE special values remain raw until the property applies its bounds.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Quantity {
    pub value: f64,
    pub ty: NumericType,
}
impl Quantity {
    /// Censor IEEE specials only at a top-level calculation/property boundary.
    /// Inner calculations must retain NaN, infinities and signed zero.
    pub fn materialize(self, min: f64, max: f64) -> Self {
        let value = if self.value.is_nan() || self.value == 0. { 0. } else { self.value };
        Self { value: value.clamp(min, max), ty: self.ty }
    }
}
/// Unit-level dependency bits, suitable for host style invalidation.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Dependencies(u64);
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum TreeCount {
    SiblingIndex,
    SiblingCount,
}
impl Dependencies {
    pub fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }
    pub fn contains(self, unit: RelativeUnit) -> bool {
        self.0 & (1 << unit as u8) != 0
    }
    pub fn is_empty(self) -> bool {
        self.0 == 0
    }
    pub fn tree_count(self, kind: TreeCount) -> bool {
        self.0 & (1 << (48 + kind as u8)) != 0
    }
}
pub trait Context {
    /// Canonical px per unit. Percentage uses the dimension's basis / 100;
    /// its basis can be negative (e.g. background-position). Other length
    /// factors must be finite and nonnegative, with unsigned zero. No
    /// environment is guessed. Supply a stable, cheap snapshot for a resolve.
    fn factor(&self, _unit: RelativeUnit) -> Result<f64> {
        Err(Error::ContextUnavailable)
    }
    /// Host-resolved, tree-scope-checked CSS tree-counting value. The host
    /// must enforce stylesheet/element shadow boundaries; never count an
    /// arbitrary visible subtree here. Zero can represent a failed scoped
    /// reference, as defined by CSS Values 5.
    fn tree_count(&self, _kind: TreeCount) -> Result<u32> {
        Err(Error::ContextUnavailable)
    }
}
pub struct NoContext;
impl Context for NoContext {}
#[derive(Clone, Copy, Debug)]
#[repr(u8)]
enum Op {
    Constant,
    Relative,
    TreeCount,
    Progress,
    Add,
    Sub,
    Mul,
    Div,
    Min,
    Max,
    Abs,
    Negate,
    Sign,
    Sin,
    Cos,
    Tan,
    Asin,
    Acos,
    Atan,
    Atan2,
    Pow,
    Sqrt,
    Hypot,
    Log,
    Exp,
    Round,
    Mod,
    Rem,
}
#[derive(Clone, Copy)]
struct Instruction {
    value: f64,
    arg: u32,
    op: Op,
    unit: RelativeUnit,
}
impl Instruction {
    fn new(op: Op, value: f64, arg: u32) -> Self {
        Self { op, value, arg, unit: RelativeUnit::Percentage }
    }
    fn arity(self) -> usize {
        match self.op {
            Op::Constant | Op::Relative | Op::TreeCount => 0,
            Op::Progress => 3,
            Op::Abs | Op::Negate | Op::Sign | Op::Sin | Op::Cos | Op::Tan | Op::Asin | Op::Acos | Op::Atan | Op::Sqrt | Op::Exp => 1,
            _ => 2,
        }
    }
}
pub struct Program {
    code: Vec<Instruction>,
    ty: NumericType,
    dependencies: Dependencies,
    stack: usize,
}
#[derive(Clone)]
pub enum Value {
    Constant(Quantity),
    Expression(Arc<Program>),
}
impl std::fmt::Debug for Value {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MathValue").field("memory", &self.memory()).finish_non_exhaustive()
    }
}
#[derive(Clone, Copy, Debug)]
pub struct Memory {
    pub instructions: usize,
    pub instruction_size: usize,
    pub accounted_reserved_bytes: usize,
    pub stack: usize,
}
/// Reusable resolution storage, not part of a retained computed value.
#[derive(Default)]
pub struct Scratch {
    values: SmallVec<[f64; 16]>,
}
impl Value {
    pub fn parse(values: Components<'_>, options: Options, limits: Limits, cancelled: Option<&AtomicBool>) -> Result<Self> {
        parse::parse(values, options, limits, cancelled)
    }
    pub fn compile(component: Component<'_>, options: Options, limits: Limits, cancelled: Option<&AtomicBool>) -> Result<Self> {
        parse::compile(component, options, limits, cancelled)
    }
    pub(crate) fn compile_accounted(component: Component<'_>, options: Options, limits: Limits, cancelled: Option<&AtomicBool>) -> Result<(Self, usize)> {
        parse::compile_accounted(component, options, limits, cancelled)
    }
    pub fn ty(&self) -> NumericType {
        match self {
            Self::Constant(q) => q.ty,
            Self::Expression(e) => e.ty,
        }
    }
    pub fn constant(&self) -> Option<Quantity> {
        match self {
            Self::Constant(q) => Some(*q),
            _ => None,
        }
    }
    pub fn dependencies(&self) -> Dependencies {
        match self {
            Self::Constant(_) => Dependencies::default(),
            Self::Expression(e) => e.dependencies,
        }
    }
    pub fn memory(&self) -> Memory {
        match self {
            Self::Constant(_) => Memory { instructions: 0, instruction_size: size_of::<Instruction>(), accounted_reserved_bytes: 0, stack: 0 },
            Self::Expression(e) => Memory {
                instructions: e.code.len(),
                instruction_size: size_of::<Instruction>(),
                accounted_reserved_bytes: e.code.capacity() * size_of::<Instruction>() + size_of::<Program>() + 2 * size_of::<usize>(),
                stack: e.stack,
            },
        }
    }
    pub fn resolve_raw(&self, context: &impl Context, limits: Limits, cancelled: Option<&AtomicBool>) -> Result<Quantity> {
        self.resolve_with(context, &mut Scratch::default(), limits, cancelled)
    }
    pub fn resolve_with(&self, context: &impl Context, scratch: &mut Scratch, limits: Limits, cancelled: Option<&AtomicBool>) -> Result<Quantity> {
        check(cancelled)?;
        let Self::Expression(e) = self else {
            return Ok(self.constant().unwrap());
        };
        if e.code.len() > limits.work || e.stack > limits.stack {
            return Err(Error::Limit);
        }
        scratch.values.clear();
        for instruction in &e.code {
            check(cancelled)?;
            let n = instruction.arity();
            let value = match n {
                0 => match instruction.op {
                    Op::Constant => instruction.value,
                    Op::Relative => {
                        let factor = context.factor(instruction.unit)?;
                        if !factor.is_finite() || (instruction.unit != RelativeUnit::Percentage && factor.is_sign_negative()) {
                            return Err(Error::ContextUnavailable);
                        }
                        instruction.value * factor
                    }
                    Op::TreeCount => context.tree_count(if instruction.arg == 0 { TreeCount::SiblingIndex } else { TreeCount::SiblingCount })? as f64,
                    _ => unreachable!(),
                },
                1 => ops::unary(*instruction, scratch.values.pop().unwrap()),
                2 => {
                    let rhs = scratch.values.pop().unwrap();
                    let lhs = scratch.values.pop().unwrap();
                    ops::binary(*instruction, lhs, rhs)
                }
                3 => {
                    let end = scratch.values.pop().unwrap();
                    let start = scratch.values.pop().unwrap();
                    let value = scratch.values.pop().unwrap();
                    ops::progress(value, start, end, instruction.arg != 0)
                }
                _ => unreachable!(),
            };
            scratch.values.push(value);
        }
        Ok(Quantity { value: scratch.values[0], ty: e.ty })
    }
}
fn check(cancelled: Option<&AtomicBool>) -> Result<()> {
    if cancelled.is_some_and(|c| c.load(Ordering::Relaxed)) { Err(Error::Cancelled) } else { Ok(()) }
}
