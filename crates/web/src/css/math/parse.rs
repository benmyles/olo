use super::*;
use crate::css::Kind;
#[derive(Clone, Copy)]
struct Term {
    ty: NumericType,
    constant: Option<f64>,
    start: usize,
}
struct Parser<'a> {
    code: SmallVec<[Instruction; 16]>,
    options: Options,
    limits: Limits,
    work: usize,
    cancelled: Option<&'a AtomicBool>,
}
pub(super) fn parse(mut values: Components<'_>, options: Options, limits: Limits, cancelled: Option<&AtomicBool>) -> Result<Value> {
    let mut parser = Parser { code: SmallVec::new(), options, limits, work: 0, cancelled };
    parser.skip(&mut values)?;
    let component = parser.take(&mut values)?;
    let term = parser.value(component, 0, false)?;
    parser.end(values)?;
    parser.finish(term)
}
pub(super) fn compile(component: Component<'_>, options: Options, limits: Limits, cancelled: Option<&AtomicBool>) -> Result<Value> {
    compile_accounted(component, options, limits, cancelled).map(|(value, _)| value)
}
pub(super) fn compile_accounted(component: Component<'_>, options: Options, limits: Limits, cancelled: Option<&AtomicBool>) -> Result<(Value, usize)> {
    let mut parser = Parser { code: SmallVec::new(), options, limits, work: 0, cancelled };
    parser.tick((component.span().end - component.span().start) as usize + 1)?;
    let term = parser.value(component, 0, false)?;
    let work = parser.work;
    parser.finish(term).map(|value| (value, work))
}
impl Parser<'_> {
    fn tick(&mut self, n: usize) -> Result<()> {
        check(self.cancelled)?;
        self.work = self.work.checked_add(n).ok_or(Error::Limit)?;
        if self.work > self.limits.work { Err(Error::Limit) } else { Ok(()) }
    }
    fn take<'a>(&mut self, input: &mut Components<'a>) -> Result<Component<'a>> {
        let c = input.next().ok_or(Error::Invalid)?;
        let span = c.span();
        self.tick((span.end - span.start) as usize + 1)?;
        Ok(c)
    }
    fn skip(&mut self, input: &mut Components<'_>) -> Result<bool> {
        let mut skipped = false;
        while input.clone().next().is_some_and(|c| c.kind() == Kind::Whitespace) {
            self.take(input)?;
            skipped = true;
        }
        Ok(skipped)
    }
    fn end(&mut self, mut input: Components<'_>) -> Result<()> {
        self.skip(&mut input)?;
        if input.next().is_some() { Err(Error::Invalid) } else { Ok(()) }
    }
    fn comma(&mut self, input: &mut Components<'_>) -> Result<()> {
        self.skip(input)?;
        if self.take(input)?.kind() != Kind::Comma {
            return Err(Error::Invalid);
        }
        self.skip(input)?;
        Ok(())
    }
    fn push(&mut self, instruction: Instruction) -> Result<()> {
        self.tick(1)?;
        if self.code.len() >= self.limits.instructions {
            return Err(Error::Limit);
        }
        self.code.push(instruction);
        Ok(())
    }
    fn literal(&mut self, value: f64, ty: NumericType) -> Result<Term> {
        let start = self.code.len();
        self.push(Instruction::new(Op::Constant, value, 0))?;
        Ok(Term { ty, constant: Some(value), start })
    }
    fn relative(&mut self, value: f64, unit: RelativeUnit, ty: NumericType) -> Result<Term> {
        let start = self.code.len();
        self.push(Instruction { value, op: Op::Relative, arg: 0, unit })?;
        Ok(Term { ty, constant: None, start })
    }
    fn unary(&mut self, term: Term, op: Op, ty: NumericType, factor: f64) -> Result<Term> {
        let instruction = Instruction::new(op, factor, 0);
        if let Some(value) = term.constant
            && !cfg!(feature = "baseline-math-folding")
        {
            self.code.truncate(term.start);
            return self.literal(ops::unary(instruction, value), ty);
        }
        self.push(instruction)?;
        Ok(Term { ty, constant: None, ..term })
    }
    fn binary(&mut self, left: Term, right: Term, op: Op, ty: NumericType, arg: u32) -> Result<Term> {
        let instruction = Instruction::new(op, 0., arg);
        if let (Some(a), Some(b)) = (left.constant, right.constant)
            && !cfg!(feature = "baseline-math-folding")
        {
            self.code.truncate(left.start);
            return self.literal(ops::binary(instruction, a, b), ty);
        }
        self.push(instruction)?;
        Ok(Term { ty, constant: None, ..left })
    }
    fn sum(&mut self, input: &mut Components<'_>, depth: usize) -> Result<Term> {
        if cfg!(feature = "baseline-math-folding") || cfg!(feature = "baseline-math-sums") { self.sum_ordered(input, depth) } else { self.sum_combined(input, depth) }
    }
    /// CSS Values 4 sum simplification combines numeric children in the
    /// same canonical unit. Retain zero and all contextual dependencies.
    /// The constant anchor avoids moving instructions or a second arena.
    fn sum_combined(&mut self, input: &mut Components<'_>, depth: usize) -> Result<Term> {
        let first = self.product(input, depth)?;
        let mut ty = first.ty;
        let anchor = first.constant.is_some();
        let mut constant = first.constant;
        let mut dynamic = if anchor { None } else { Some(first) };
        loop {
            let before = self.skip(input)?;
            let Some(op) = input.clone().next().and_then(Component::character).filter(|&c| c == '+' || c == '-') else {
                break;
            };
            if !before {
                return Err(Error::Invalid);
            }
            self.take(input)?;
            if !self.skip(input)? {
                return Err(Error::Invalid);
            }
            let right = self.product(input, depth)?;
            ty = ty.add(right.ty)?;
            if let Some(value) = right.constant {
                self.code.truncate(right.start);
                self.tick(1)?;
                let signed = if op == '-' { -value } else { value };
                constant = Some(match constant {
                    Some(left) => left + signed,
                    None => signed,
                });
                if anchor {
                    self.code[first.start].value = constant.unwrap();
                }
            } else {
                dynamic = Some(match dynamic {
                    Some(left) => {
                        let inner_ty = left.ty.add(right.ty)?;
                        self.binary(left, right, if op == '-' { Op::Sub } else { Op::Add }, inner_ty, 0)?
                    }
                    None => {
                        if op == '-' {
                            self.unary(right, Op::Negate, right.ty, 1.)?
                        } else {
                            right
                        }
                    }
                });
            }
        }
        match (dynamic, constant) {
            (None, Some(value)) => Ok(Term { ty, constant: Some(value), ..first }),
            (Some(dynamic), Some(value)) => {
                if anchor {
                    self.binary(Term { ty, constant: Some(value), ..first }, dynamic, Op::Add, ty, 0)
                } else {
                    let numeric = self.literal(value, ty)?;
                    self.binary(dynamic, numeric, Op::Add, ty, 0)
                }
            }
            (Some(dynamic), None) => Ok(Term { ty, ..dynamic }),
            _ => unreachable!(),
        }
    }
    fn sum_ordered(&mut self, input: &mut Components<'_>, depth: usize) -> Result<Term> {
        let mut left = self.product(input, depth)?;
        loop {
            let before = self.skip(input)?;
            let Some(op) = input.clone().next().and_then(Component::character).filter(|&c| c == '+' || c == '-') else {
                return Ok(left);
            };
            if !before {
                return Err(Error::Invalid);
            }
            self.take(input)?;
            if !self.skip(input)? {
                return Err(Error::Invalid);
            }
            let right = self.product(input, depth)?;
            let ty = left.ty.add(right.ty)?;
            left = self.binary(left, right, if op == '+' { Op::Add } else { Op::Sub }, ty, 0)?;
        }
    }
    fn product(&mut self, input: &mut Components<'_>, depth: usize) -> Result<Term> {
        self.skip(input)?;
        let c = self.take(input)?;
        let mut left = self.value(c, depth, true)?;
        loop {
            let mut lookahead = input.clone();
            self.skip(&mut lookahead)?;
            let Some(op) = lookahead.clone().next().and_then(Component::character).filter(|&c| c == '*' || c == '/') else {
                return Ok(left);
            };
            *input = lookahead;
            self.take(input)?;
            self.skip(input)?;
            let c = self.take(input)?;
            let right = self.value(c, depth, true)?;
            let ty = left.ty.product(right.ty, op == '/')?;
            left = self.binary(left, right, if op == '*' { Op::Mul } else { Op::Div }, ty, 0)?;
        }
    }
    fn value(&mut self, c: Component<'_>, depth: usize, in_calculation: bool) -> Result<Term> {
        self.tick(1)?;
        if depth > self.limits.depth.min(128) {
            return Err(Error::DepthLimit);
        }
        match c.kind() {
            Kind::Integer | Kind::Number => self.literal(c.number().unwrap().value, NumericType::dimension(Dimension::Number)),
            Kind::Percentage | Kind::IntegerPercentage => {
                let ty = NumericType::percentage(self.options)?;
                let value = c.number().unwrap().value;
                if self.options.percentage_hint.is_some() { self.relative(value, RelativeUnit::Percentage, ty) } else { self.literal(value, ty) }
            }
            Kind::Dimension | Kind::IntegerDimension => {
                let name = c.unit().unwrap();
                self.tick(name.len())?;
                let value = c.number().unwrap().value;
                match units::unit(name).ok_or(Error::Invalid)? {
                    units::Unit::Absolute(d, factor) => self.literal(value * factor, NumericType::dimension(d)),
                    units::Unit::Relative(u) => {
                        // Physical unit factors are finite/nonnegative. A
                        // zero coefficient needs neither host data nor a
                        // dependency; preserve its IEEE sign inside math.
                        if value == 0. && !cfg!(feature = "baseline-math-folding") {
                            self.literal(value, NumericType::dimension(Dimension::Length))
                        } else {
                            self.relative(value, u, NumericType::dimension(Dimension::Length))
                        }
                    }
                }
            }
            Kind::Ident if in_calculation => {
                let name = c.text().unwrap();
                self.tick(name.len())?;
                let value = if name.eq_ignore_ascii_case("pi") {
                    std::f64::consts::PI
                } else if name.eq_ignore_ascii_case("e") {
                    std::f64::consts::E
                } else if name.eq_ignore_ascii_case("infinity") {
                    f64::INFINITY
                } else if name.eq_ignore_ascii_case("-infinity") {
                    f64::NEG_INFINITY
                } else if name.eq_ignore_ascii_case("nan") {
                    f64::NAN
                } else {
                    return Err(Error::Invalid);
                };
                self.literal(value, NumericType::dimension(Dimension::Number))
            }
            Kind::Round if in_calculation => {
                let mut children = c.children();
                let term = self.sum(&mut children, depth + 1)?;
                self.end(children)?;
                Ok(term)
            }
            Kind::Function => self.function(c, depth + 1),
            _ => Err(Error::Invalid),
        }
    }
    fn function(&mut self, c: Component<'_>, depth: usize) -> Result<Term> {
        if depth > self.limits.depth.min(128) {
            return Err(Error::DepthLimit);
        }
        let name = c.text().unwrap();
        self.tick(name.len())?;
        if ["var", "env", "attr"].iter().any(|s| name.eq_ignore_ascii_case(s)) {
            return Err(Error::RequiresSubstitution);
        }
        if ["media-progress", "container-progress", "calc-size"].iter().any(|s| name.eq_ignore_ascii_case(s)) {
            return Err(Error::Unsupported("contextual math functions"));
        }
        let is = |s: &str| name.eq_ignore_ascii_case(s);
        let mut args = c.children();
        self.skip(&mut args)?;
        if is("sibling-index") || is("sibling-count") {
            self.end(args)?;
            let start = self.code.len();
            self.push(Instruction::new(Op::TreeCount, 0., u32::from(is("sibling-count"))))?;
            return Ok(Term { ty: NumericType::dimension(Dimension::Number), constant: None, start });
        }
        if is("progress") {
            return self.progress(args, depth);
        }
        if is("calc") {
            let term = self.sum(&mut args, depth)?;
            self.end(args)?;
            return Ok(term);
        }
        if is("clamp") {
            return self.clamp(args, depth);
        }
        if is("round") {
            return self.round(args, depth);
        }
        let op = if is("min") {
            Op::Min
        } else if is("max") {
            Op::Max
        } else if is("hypot") {
            Op::Hypot
        } else if is("abs") {
            Op::Abs
        } else if is("sign") {
            Op::Sign
        } else if is("sin") {
            Op::Sin
        } else if is("cos") {
            Op::Cos
        } else if is("tan") {
            Op::Tan
        } else if is("asin") {
            Op::Asin
        } else if is("acos") {
            Op::Acos
        } else if is("atan") {
            Op::Atan
        } else if is("atan2") {
            Op::Atan2
        } else if is("pow") {
            Op::Pow
        } else if is("sqrt") {
            Op::Sqrt
        } else if is("log") {
            Op::Log
        } else if is("exp") {
            Op::Exp
        } else if is("mod") {
            Op::Mod
        } else if is("rem") {
            Op::Rem
        } else {
            return Err(Error::Invalid);
        };
        let mut first = self.sum(&mut args, depth)?;
        if first.ty.kind().is_none() {
            return Err(Error::Invalid);
        }
        match op {
            Op::Min | Op::Max | Op::Hypot => {
                if matches!(op, Op::Hypot) {
                    first = self.unary(first, Op::Abs, first.ty, 1.)?;
                }
                while args.clone().next().is_some() {
                    self.comma(&mut args)?;
                    let next = self.sum(&mut args, depth)?;
                    let ty = first.ty.add(next.ty)?;
                    first = self.binary(first, next, op, ty, 0)?;
                }
                Ok(first)
            }
            Op::Pow | Op::Atan2 | Op::Mod | Op::Rem | Op::Log => {
                let second = if matches!(op, Op::Log) && args.clone().next().is_none() {
                    self.literal(std::f64::consts::E, NumericType::dimension(Dimension::Number))?
                } else {
                    self.comma(&mut args)?;
                    self.sum(&mut args, depth)?
                };
                self.end(args)?;
                if matches!(op, Op::Pow | Op::Log) && (first.ty.kind() != Some(Dimension::Number) || second.ty.kind() != Some(Dimension::Number)) {
                    return Err(Error::Invalid);
                }
                let mut ty = first.ty.add(second.ty)?;
                if matches!(op, Op::Atan2) {
                    ty = ty.consistent_result(Dimension::Angle);
                }
                self.binary(first, second, op, ty, 0)
            }
            _ => {
                self.end(args)?;
                let kind = first.ty.kind().ok_or(Error::Invalid)?;
                let mut ty = first.ty;
                let mut factor = 1.;
                match op {
                    Op::Sin | Op::Cos | Op::Tan => {
                        if !matches!(kind, Dimension::Number | Dimension::Angle) {
                            return Err(Error::Invalid);
                        }
                        if kind == Dimension::Angle {
                            factor = std::f64::consts::PI / 180.;
                        }
                        ty = ty.consistent_result(Dimension::Number);
                    }
                    Op::Asin | Op::Acos | Op::Atan => {
                        if kind != Dimension::Number {
                            return Err(Error::Invalid);
                        }
                        ty = ty.consistent_result(Dimension::Angle);
                    }
                    Op::Sqrt | Op::Exp => {
                        if kind != Dimension::Number {
                            return Err(Error::Invalid);
                        }
                    }
                    Op::Sign => ty = ty.consistent_result(Dimension::Number),
                    _ => {}
                }
                self.unary(first, op, ty, factor)
            }
        }
    }
    fn none(&mut self, input: &mut Components<'_>) -> Result<bool> {
        self.skip(input)?;
        if input.clone().next().is_some_and(|c| c.kind() == Kind::Ident && c.text().unwrap().eq_ignore_ascii_case("none")) {
            self.take(input)?;
            Ok(true)
        } else {
            Ok(false)
        }
    }
    fn clamp(&mut self, mut args: Components<'_>, depth: usize) -> Result<Term> {
        let lower = if self.none(&mut args)? { None } else { Some(self.sum(&mut args, depth)?) };
        self.comma(&mut args)?;
        let mut result = self.sum(&mut args, depth)?;
        if result.ty.kind().is_none() {
            return Err(Error::Invalid);
        }
        self.comma(&mut args)?;
        if !self.none(&mut args)? {
            let upper = self.sum(&mut args, depth)?;
            let ty = result.ty.add(upper.ty)?;
            result = self.binary(result, upper, Op::Min, ty, 0)?;
        }
        self.end(args)?;
        if let Some(lower) = lower {
            let ty = lower.ty.add(result.ty)?;
            result = self.binary(lower, result, Op::Max, ty, 0)?;
        }
        Ok(result)
    }
    fn round(&mut self, mut args: Components<'_>, depth: usize) -> Result<Term> {
        let mut strategy = 0;
        if let Some(c) = args.clone().next().filter(|c| c.kind() == Kind::Ident) {
            let name = c.text().unwrap();
            if name.eq_ignore_ascii_case("line-width") {
                return Err(Error::Unsupported("device-pixel line-width rounding"));
            }
            let found = ["nearest", "up", "down", "to-zero"].iter().position(|s| name.eq_ignore_ascii_case(s));
            if let Some(found) = found {
                strategy = found as u32;
                self.take(&mut args)?;
                self.comma(&mut args)?;
            }
        }
        let first = self.sum(&mut args, depth)?;
        let second = if args.clone().next().is_none() {
            if first.ty.kind() != Some(Dimension::Number) {
                return Err(Error::Invalid);
            }
            self.literal(1., NumericType::dimension(Dimension::Number))?
        } else {
            self.comma(&mut args)?;
            self.sum(&mut args, depth)?
        };
        self.end(args)?;
        let ty = first.ty.add(second.ty)?;
        if ty.kind().is_none() {
            return Err(Error::Invalid);
        }
        self.binary(first, second, Op::Round, ty, strategy)
    }
    fn progress(&mut self, mut args: Components<'_>, depth: usize) -> Result<Term> {
        let clamp = if args.clone().next().is_some_and(|c| c.kind() == Kind::Ident && c.text().unwrap().eq_ignore_ascii_case("no-clamp")) {
            self.take(&mut args)?;
            self.skip(&mut args)?;
            false
        } else {
            true
        };
        let value = self.sum(&mut args, depth)?;
        self.comma(&mut args)?;
        let start = self.sum(&mut args, depth)?;
        self.comma(&mut args)?;
        let end = self.sum(&mut args, depth)?;
        self.end(args)?;
        let ty = value.ty.add(start.ty)?.add(end.ty)?;
        if ty.kind().is_none() {
            return Err(Error::Invalid);
        }
        let ty = ty.consistent_result(Dimension::Number);
        if let (Some(v), Some(s), Some(e)) = (value.constant, start.constant, end.constant)
            && !cfg!(feature = "baseline-math-folding")
        {
            self.code.truncate(value.start);
            return self.literal(ops::progress(v, s, e, clamp), ty);
        }
        self.push(Instruction::new(Op::Progress, 0., u32::from(clamp)))?;
        Ok(Term { ty, constant: None, ..value })
    }
    fn finish(self, term: Term) -> Result<Value> {
        if term.ty.kind().is_none() {
            return Err(Error::Invalid);
        }
        if let Some(value) = term.constant {
            return Ok(Value::Constant(Quantity { value, ty: term.ty }));
        }
        let mut dependencies = Dependencies::default();
        let mut stack = 0usize;
        let mut max_stack = 0;
        for instruction in &self.code {
            stack = stack.checked_sub(instruction.arity()).ok_or(Error::Invalid)? + 1;
            max_stack = max_stack.max(stack);
            if matches!(instruction.op, Op::Relative) {
                dependencies.0 |= 1 << instruction.unit as u8;
            } else if matches!(instruction.op, Op::TreeCount) {
                dependencies.0 |= 1 << (48 + instruction.arg);
            }
        }
        if max_stack > self.limits.stack {
            return Err(Error::Limit);
        }
        if stack != 1 {
            return Err(Error::Invalid);
        }
        Ok(Value::Expression(Arc::new(Program { code: self.code.into_vec(), ty: term.ty, dependencies, stack: max_stack })))
    }
}
