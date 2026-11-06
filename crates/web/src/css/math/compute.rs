use super::*;

#[derive(Clone, Copy)]
struct Term {
    start: usize,
    constant: Option<f64>,
}
impl Value {
    pub(crate) fn same(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Constant(a), Self::Constant(b)) => a.ty == b.ty && a.value.to_bits() == b.value.to_bits(),
            (Self::Expression(a), Self::Expression(b)) => Arc::ptr_eq(a, b),
            _ => false,
        }
    }
    /// Freeze this element's unit/tree context at computed-value time. When
    /// `defer_percentage` is true, layout-dependent percentages stay in the
    /// resulting program. No inner IEEE value is clamped or materialized.
    /// Unchanged programs share their allocation; changed programs own only
    /// the remaining calculation, never the host or source expression.
    pub fn computed(&self, context: &impl Context, defer_percentage: bool, limits: Limits, cancelled: Option<&AtomicBool>) -> Result<Self> {
        check(cancelled)?;
        let Self::Expression(program) = self else { return Ok(self.clone()) };
        if program.code.len() > limits.instructions || program.stack > limits.stack {
            return Err(Error::Limit);
        }
        let retained = if defer_percentage { 1u64 << RelativeUnit::Percentage as u8 } else { 0 };
        if program.dependencies.0 != 0 && program.dependencies.0 & !retained == 0 {
            return Ok(self.clone());
        }
        let mut code = SmallVec::<[Instruction; 16]>::new();
        let mut terms = SmallVec::<[Term; 16]>::new();
        let mut work = 0usize;
        for &original in &program.code {
            check(cancelled)?;
            work = work.checked_add(1).ok_or(Error::Limit)?;
            if work > limits.work {
                return Err(Error::Limit);
            }
            let arity = original.arity();
            let operands = terms.len().checked_sub(arity).ok_or(Error::Invalid)?;
            let start = if arity == 0 { code.len() } else { terms[operands].start };
            let constant = match original.op {
                Op::Constant => Some(original.value),
                Op::Relative if defer_percentage && original.unit == RelativeUnit::Percentage => None,
                Op::Relative => {
                    let factor = context.factor(original.unit)?;
                    if !factor.is_finite() || original.unit != RelativeUnit::Percentage && factor.is_sign_negative() {
                        return Err(Error::ContextUnavailable);
                    }
                    Some(original.value * factor)
                }
                Op::TreeCount => Some(context.tree_count(if original.arg == 0 { TreeCount::SiblingIndex } else { TreeCount::SiblingCount })? as f64),
                _ => {
                    let values = &terms[operands..];
                    if values.iter().all(|v| v.constant.is_some()) {
                        Some(match arity {
                            1 => ops::unary(original, values[0].constant.unwrap()),
                            2 => ops::binary(original, values[0].constant.unwrap(), values[1].constant.unwrap()),
                            3 => ops::progress(values[0].constant.unwrap(), values[1].constant.unwrap(), values[2].constant.unwrap(), original.arg != 0),
                            _ => return Err(Error::Invalid),
                        })
                    } else {
                        None
                    }
                }
            };
            terms.truncate(operands);
            if let Some(value) = constant {
                code.truncate(start);
                code.push(Instruction::new(Op::Constant, value, 0));
            } else {
                code.push(original);
            }
            terms.push(Term { start, constant });
        }
        if terms.len() != 1 {
            return Err(Error::Invalid);
        }
        if let Some(value) = terms[0].constant {
            return Ok(Self::Constant(Quantity { value, ty: program.ty }));
        }
        let mut dependencies = Dependencies::default();
        let (mut stack, mut maximum) = (0usize, 0usize);
        for instruction in &code {
            check(cancelled)?;
            work = work.checked_add(1).ok_or(Error::Limit)?;
            if work > limits.work {
                return Err(Error::Limit);
            }
            stack = stack.checked_sub(instruction.arity()).ok_or(Error::Invalid)? + 1;
            maximum = maximum.max(stack);
            if matches!(instruction.op, Op::Relative) {
                dependencies.0 |= 1 << instruction.unit as u8;
            }
        }
        Ok(Self::Expression(Arc::new(Program { code: code.into_vec(), ty: program.ty, dependencies, stack: maximum })))
    }
}
