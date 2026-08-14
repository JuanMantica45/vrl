use crate::compiler::prelude::*;
use rand::{thread_rng, Rng};
use std::ops::Range;

const NON_FINITE_ERR: &str = "min and max must be finite";
const INVALID_RANGE_ERR: &str = "max must be greater than min";
const RANGE_OVERFLOW_ERR: &str = "range max - min overflows";

fn random_float(min: Value, max: Value) -> Resolved {
    let range = get_range(min.try_float()?, max.try_float()?)?;

    let f: f64 = thread_rng().gen_range(range);

    Ok(Value::from_f64_or_zero(f))
}

/// The single place the `random_float` bounds are validated.
///
/// `gen_range` panics on a non-finite or empty range, and on a `max - min`
/// width that overflows to infinity, so every caller — compile time and
/// runtime — must go through here.
fn get_range(min: f64, max: f64) -> std::result::Result<Range<f64>, &'static str> {
    if !min.is_finite() || !max.is_finite() {
        return Err(NON_FINITE_ERR);
    }
    if max <= min {
        return Err(INVALID_RANGE_ERR);
    }
    if !(max - min).is_finite() {
        return Err(RANGE_OVERFLOW_ERR);
    }

    Ok(min..max)
}

#[derive(Clone, Copy, Debug)]
pub struct RandomFloat;

impl Function for RandomFloat {
    fn identifier(&self) -> &'static str {
        "random_float"
    }

    fn parameters(&self) -> &'static [Parameter] {
        &[
            Parameter {
                keyword: "min",
                kind: kind::FLOAT,
                required: true,
            },
            Parameter {
                keyword: "max",
                kind: kind::FLOAT,
                required: true,
            },
        ]
    }

    fn examples(&self) -> &'static [Example] {
        &[Example {
            title: "generate a random float from 0.0 to 10.0",
            source: "
				f = random_float(0.0, 10.0)
				f >= 0 && f < 10
                ",
            result: Ok("true"),
        }]
    }

    fn compile(
        &self,
        state: &state::TypeState,
        _ctx: &mut FunctionCompileContext,
        arguments: ArgumentList,
    ) -> Compiled {
        let min = arguments.required("min");
        let max = arguments.required("max");

        if let (Some(min), Some(max)) = (min.resolve_constant(state), max.resolve_constant(state)) {
            // check if range is valid. Non-float constants are rejected by
            // argument-kind validation, so skipping them here loses nothing.
            if let (Ok(min), Ok(max_f)) = (min.try_float(), max.clone().try_float()) {
                get_range(min, max_f).map_err(|err| function::Error::InvalidArgument {
                    keyword: "max",
                    value: max,
                    error: err,
                })?;
            }
        }

        Ok(RandomFloatFn { min, max }.as_expr())
    }
}

#[derive(Debug, Clone)]
struct RandomFloatFn {
    min: Box<dyn Expression>,
    max: Box<dyn Expression>,
}

impl FunctionExpression for RandomFloatFn {
    fn resolve(&self, ctx: &mut Context) -> Resolved {
        let min = self.min.resolve(ctx)?;
        let max = self.max.resolve(ctx)?;

        random_float(min, max)
    }

    fn type_def(&self, state: &state::TypeState) -> TypeDef {
        match (
            self.min.resolve_constant(state),
            self.max.resolve_constant(state),
        ) {
            (Some(min), Some(max)) => match (min.try_float(), max.try_float()) {
                (Ok(min), Ok(max)) if get_range(min, max).is_ok() => TypeDef::float().infallible(),
                _ => TypeDef::float().fallible(),
            },
            _ => TypeDef::float().fallible(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::value;
    // positive tests are handled by examples

    test_function![
        random_float => RandomFloat;

        bad_range {
            args: func_args![min: value!(1.0), max: value!(1.0)],
            want: Err("invalid argument"),
            tdef: TypeDef::float().fallible(),
        }

        // OBE-10730: constant infinite bounds must now be caught at compile time (type_def fallible).
        infinite_max_compile_time {
            args: func_args![min: value!(0.0), max: Value::Float(NotNan::new(f64::INFINITY).unwrap())],
            want: Err("invalid argument"),
            tdef: TypeDef::float().fallible(),
        }

        overflow_range_compile_time {
            args: func_args![min: Value::Float(NotNan::new(-1.0e308).unwrap()), max: Value::Float(NotNan::new(1.0e308).unwrap())],
            want: Err("invalid argument"),
            tdef: TypeDef::float().fallible(),
        }
    ];

    // Positive: valid finite bounds succeed and produce a value in [min, max).
    #[test]
    fn valid_finite_range_returns_ok_in_range() {
        let min = Value::Float(NotNan::new(0.0).unwrap());
        let max = Value::Float(NotNan::new(10.0).unwrap());
        let result = random_float(min, max).expect("should succeed");
        let f = match result {
            Value::Float(v) => *v,
            _ => panic!("expected float"),
        };
        assert!(f >= 0.0 && f < 10.0, "result {f} out of [0, 10)");
    }

    // OBE-10730: non-finite bounds and range-overflow must return errors, not panic.
    #[test]
    fn non_finite_min_returns_error() {
        let min = Value::Float(NotNan::new(f64::INFINITY).unwrap());
        let max = Value::Float(NotNan::new(1.0).unwrap());
        assert!(random_float(min, max).is_err());
    }

    #[test]
    fn non_finite_max_returns_error() {
        let min = Value::Float(NotNan::new(0.0).unwrap());
        let max = Value::Float(NotNan::new(f64::INFINITY).unwrap());
        assert!(random_float(min, max).is_err());
    }

    #[test]
    fn negative_infinity_returns_error() {
        let min = Value::Float(NotNan::new(f64::NEG_INFINITY).unwrap());
        let max = Value::Float(NotNan::new(0.0).unwrap());
        assert!(random_float(min, max).is_err());
    }

    #[test]
    fn range_overflow_returns_error() {
        let min = Value::Float(NotNan::new(-1.0e308).unwrap());
        let max = Value::Float(NotNan::new(1.0e308).unwrap());
        assert!(random_float(min, max).is_err());
    }
}
