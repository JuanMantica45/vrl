#![deny(clippy::arithmetic_side_effects)]

use crate::compiler::{
    value::{Kind, VrlValueConvert},
    ExpressionError,
};
use crate::value::{ObjectMap, Value};
use bytes::{BufMut, Bytes, BytesMut};

use super::ValueError;

/// Maximum byte length of a string produced by the `*` (repeat) operator.
/// Prevents OOM when an attacker supplies a large integer multiplier (OBE-10736).
///
/// `pub(crate)` so `compiler::expression::op` can compare a literal multiplier
/// against it at compile time (a literal count above this can never succeed,
/// regardless of operand length, so it's always marked fallible).
pub(crate) const MAX_REPEAT_BYTES: usize = 64 * 1024 * 1024; // 64 MiB

pub trait VrlValueArithmetic: Sized {
    /// Similar to [`std::ops::Mul`], but fallible (e.g. `TryMul`).
    fn try_mul(self, rhs: Self) -> Result<Self, ValueError>;

    /// Similar to [`std::ops::Div`], but fallible (e.g. `TryDiv`).
    fn try_div(self, rhs: Self) -> Result<Self, ValueError>;

    /// Similar to [`std::ops::Add`], but fallible (e.g. `TryAdd`).
    fn try_add(self, rhs: Self) -> Result<Self, ValueError>;

    /// Similar to [`std::ops::Sub`], but fallible (e.g. `TrySub`).
    fn try_sub(self, rhs: Self) -> Result<Self, ValueError>;

    /// Try to "OR" (`||`) two values types.
    ///
    /// If the lhs value is `null` or `false`, the rhs is evaluated and
    /// returned. The rhs is a closure that can return an error, and thus this
    /// method can return an error as well.
    fn try_or(self, rhs: impl FnMut() -> Result<Self, ExpressionError>)
        -> Result<Self, ValueError>;

    /// Try to "AND" (`&&`) two values types.
    ///
    /// A lhs or rhs value of `Null` returns `false`.
    fn try_and(self, rhs: Self) -> Result<Self, ValueError>;

    /// Similar to [`std::ops::Rem`], but fallible (e.g. `TryRem`).
    fn try_rem(self, rhs: Self) -> Result<Self, ValueError>;

    /// Similar to [`std::cmp::Ord`], but fallible (e.g. `TryOrd`).
    fn try_gt(self, rhs: Self) -> Result<Self, ValueError>;

    /// Similar to [`std::cmp::Ord`], but fallible (e.g. `TryOrd`).
    fn try_ge(self, rhs: Self) -> Result<Self, ValueError>;

    /// Similar to [`std::cmp::Ord`], but fallible (e.g. `TryOrd`).
    fn try_lt(self, rhs: Self) -> Result<Self, ValueError>;

    /// Similar to [`std::cmp::Ord`], but fallible (e.g. `TryOrd`).
    fn try_le(self, rhs: Self) -> Result<Self, ValueError>;

    fn try_merge(self, rhs: Self) -> Result<Self, ValueError>;

    /// Similar to [`std::cmp::Eq`], but does a lossless comparison for integers
    /// and floats.
    fn eq_lossy(&self, rhs: &Self) -> bool;
}

fn safe_sub(lhv: f64, rhv: f64) -> Option<Value> {
    let result = lhv - rhv;
    if result.is_nan() {
        None
    } else {
        Some(Value::from_f64_or_zero(result))
    }
}

fn safe_add(lhv: f64, rhv: f64) -> Option<Value> {
    let result = lhv + rhv;
    if result.is_nan() {
        None
    } else {
        Some(Value::from_f64_or_zero(result))
    }
}

fn safe_mul(lhv: f64, rhv: f64) -> Option<Value> {
    let result = lhv * rhv;
    if result.is_nan() {
        None
    } else {
        Some(Value::from_f64_or_zero(result))
    }
}

fn safe_rem(lhv: f64, rhv: f64) -> Option<Value> {
    let result = lhv % rhv;
    if result.is_nan() {
        None
    } else {
        Some(Value::from_f64_or_zero(result))
    }
}

impl VrlValueArithmetic for Value {
    /// Similar to [`std::ops::Mul`], but fallible (e.g. `TryMul`).
    fn try_mul(self, rhs: Self) -> Result<Self, ValueError> {
        let err = || ValueError::Mul(self.kind(), rhs.kind());

        // When multiplying a string by an integer, if the number is negative we set it to zero to
        // return an empty string.
        let as_usize = |num: i64| if num < 0 { 0 } else { num as usize };

        let value = match self {
            Value::Integer(lhv) if rhs.is_bytes() => {
                // `try_bytes` consumes `rhs`, so the `err` closure above (which borrows
                // it) cannot be used past this point. Both operand kinds are known
                // exactly in this arm, so build the error from them directly rather than
                // deriving `Kind` from the values — `Kind::from(&Value)` deep-walks
                // containers, and doing that eagerly would cost every multiplication.
                let repeat_err = || ValueError::Mul(Kind::integer(), Kind::bytes());
                let bytes = rhs.try_bytes()?;
                let n = as_usize(lhv);
                let out_len = bytes.len().checked_mul(n).ok_or_else(repeat_err)?;
                if out_len > MAX_REPEAT_BYTES {
                    return Err(repeat_err());
                }
                Bytes::from(bytes.repeat(n)).into()
            }
            Value::Integer(lhv) if rhs.is_float() => {
                Value::from_f64_or_zero(lhv as f64 * rhs.try_float()?)
            }
            Value::Integer(lhv) => {
                let rhv = rhs.try_into_i64().map_err(|_| err())?;
                i64::wrapping_mul(lhv, rhv).into()
            }
            Value::Float(lhv) => {
                let rhs = rhs.try_into_f64().map_err(|_| err())?;
                safe_mul(*lhv, rhs).ok_or_else(err)?
            }
            Value::Bytes(lhv) if rhs.is_integer() => {
                // See the note in the `Integer * Bytes` arm above.
                let repeat_err = || ValueError::Mul(Kind::bytes(), Kind::integer());
                let n = as_usize(rhs.try_integer()?);
                let out_len = lhv.len().checked_mul(n).ok_or_else(repeat_err)?;
                if out_len > MAX_REPEAT_BYTES {
                    return Err(repeat_err());
                }
                Bytes::from(lhv.repeat(n)).into()
            }
            _ => return Err(err()),
        };

        Ok(value)
    }

    /// Similar to [`std::ops::Div`], but fallible (e.g. `TryDiv`).
    fn try_div(self, rhs: Self) -> Result<Self, ValueError> {
        let err = || ValueError::Div(self.kind(), rhs.kind());

        let rhv = rhs.try_into_f64().map_err(|_| err())?;

        if rhv == 0.0 {
            return Err(ValueError::DivideByZero);
        }

        let value = match self {
            Value::Integer(lhv) => Value::from_f64_or_zero(lhv as f64 / rhv),
            Value::Float(lhv) => Value::from_f64_or_zero(lhv.into_inner() / rhv),
            _ => return Err(err()),
        };

        Ok(value)
    }

    /// Similar to [`std::ops::Add`], but fallible (e.g. `TryAdd`).
    fn try_add(self, rhs: Self) -> Result<Self, ValueError> {
        let value = match (self, rhs) {
            (Value::Integer(lhs), Value::Float(rhs)) => Value::from_f64_or_zero(lhs as f64 + *rhs),
            (Value::Integer(lhs), rhs) => {
                let rhv = rhs
                    .try_into_i64()
                    .map_err(|_| ValueError::Add(Kind::integer(), rhs.kind()))?;
                i64::wrapping_add(lhs, rhv).into()
            }
            (Value::Float(lhs), rhs) => {
                let rhs = rhs
                    .try_into_f64()
                    .map_err(|_| ValueError::Add(Kind::float(), rhs.kind()))?;
                safe_add(*lhs, rhs).ok_or(ValueError::Add(Kind::float(), Kind::float()))?
            }
            (lhs @ Value::Bytes(_), Value::Null) => lhs,
            (Value::Bytes(lhs), Value::Bytes(rhs)) => {
                #[allow(clippy::arithmetic_side_effects)]
                let mut value = BytesMut::with_capacity(lhs.len() + rhs.len());
                value.put(lhs);
                value.put(rhs);
                value.freeze().into()
            }
            (Value::Null, rhs @ Value::Bytes(_)) => rhs,
            (lhs, rhs) => return Err(ValueError::Add(lhs.kind(), rhs.kind())),
        };

        Ok(value)
    }

    /// Similar to [`std::ops::Sub`], but fallible (e.g. `TrySub`).
    fn try_sub(self, rhs: Self) -> Result<Self, ValueError> {
        let err = || ValueError::Sub(self.kind(), rhs.kind());

        let value = match self {
            Value::Integer(lhv) if rhs.is_float() => {
                Value::from_f64_or_zero(lhv as f64 - rhs.try_float()?)
            }
            Value::Integer(lhv) => {
                let rhv = rhs.try_into_i64().map_err(|_| err())?;
                i64::wrapping_sub(lhv, rhv).into()
            }
            Value::Float(lhs) => {
                let rhs = rhs.try_into_f64().map_err(|_| err())?;
                safe_sub(*lhs, rhs).ok_or_else(err)?
            }
            _ => return Err(err()),
        };

        Ok(value)
    }

    /// Try to "OR" (`||`) two values types.
    ///
    /// If the lhs value is `null` or `false`, the rhs is evaluated and
    /// returned. The rhs is a closure that can return an error, and thus this
    /// method can return an error as well.
    fn try_or(
        self,
        mut rhs: impl FnMut() -> Result<Self, ExpressionError>,
    ) -> Result<Self, ValueError> {
        let err = ValueError::Or;

        match self {
            Value::Null => rhs().map_err(err),
            Value::Boolean(false) => rhs().map_err(err),
            value => Ok(value),
        }
    }

    /// Try to "AND" (`&&`) two values types.
    ///
    /// A lhs or rhs value of `Null` returns `false`.
    fn try_and(self, rhs: Self) -> Result<Self, ValueError> {
        let err = || ValueError::And(self.kind(), rhs.kind());

        let value = match self {
            Value::Null => false.into(),
            Value::Boolean(lhv) => match rhs {
                Value::Null => false.into(),
                Value::Boolean(rhv) => (lhv && rhv).into(),
                _ => return Err(err()),
            },
            _ => return Err(err()),
        };

        Ok(value)
    }

    /// Similar to [`std::ops::Rem`], but fallible (e.g. `TryRem`).
    fn try_rem(self, rhs: Self) -> Result<Self, ValueError> {
        let err = || ValueError::Rem(self.kind(), rhs.kind());

        let rhv = rhs.try_into_f64().map_err(|_| err())?;

        if rhv == 0.0 {
            return Err(ValueError::DivideByZero);
        }

        let value = match self {
            Value::Integer(lhv) if rhs.is_float() => {
                Value::from_f64_or_zero(lhv as f64 % rhs.try_float()?)
            }
            Value::Integer(lhv) => {
                let rhv = rhs.try_into_i64().map_err(|_| err())?;
                i64::wrapping_rem(lhv, rhv).into()
            }
            Value::Float(lhv) => {
                let rhv = rhs.try_into_f64().map_err(|_| err())?;
                safe_rem(*lhv, rhv).ok_or_else(err)?
            }
            _ => return Err(err()),
        };

        Ok(value)
    }

    /// Similar to [`std::cmp::Ord`], but fallible (e.g. `TryOrd`).
    fn try_gt(self, rhs: Self) -> Result<Self, ValueError> {
        let err = || ValueError::Rem(self.kind(), rhs.kind());

        let value = match self {
            Value::Integer(lhv) if rhs.is_float() => (lhv as f64 > rhs.try_float()?).into(),
            Value::Integer(lhv) => (lhv > rhs.try_into_i64().map_err(|_| err())?).into(),
            Value::Float(lhv) => (lhv.into_inner() > rhs.try_into_f64().map_err(|_| err())?).into(),
            Value::Bytes(lhv) => (lhv > rhs.try_bytes()?).into(),
            Value::Timestamp(lhv) => (lhv > rhs.try_timestamp()?).into(),
            _ => return Err(err()),
        };

        Ok(value)
    }

    /// Similar to [`std::cmp::Ord`], but fallible (e.g. `TryOrd`).
    fn try_ge(self, rhs: Self) -> Result<Self, ValueError> {
        let err = || ValueError::Ge(self.kind(), rhs.kind());

        let value = match self {
            Value::Integer(lhv) if rhs.is_float() => (lhv as f64 >= rhs.try_float()?).into(),
            Value::Integer(lhv) => (lhv >= rhs.try_into_i64().map_err(|_| err())?).into(),
            Value::Float(lhv) => {
                (lhv.into_inner() >= rhs.try_into_f64().map_err(|_| err())?).into()
            }
            Value::Bytes(lhv) => (lhv >= rhs.try_bytes()?).into(),
            Value::Timestamp(lhv) => (lhv >= rhs.try_timestamp()?).into(),
            _ => return Err(err()),
        };

        Ok(value)
    }

    /// Similar to [`std::cmp::Ord`], but fallible (e.g. `TryOrd`).
    fn try_lt(self, rhs: Self) -> Result<Self, ValueError> {
        let err = || ValueError::Ge(self.kind(), rhs.kind());

        let value = match self {
            Value::Integer(lhv) if rhs.is_float() => ((lhv as f64) < rhs.try_float()?).into(),
            Value::Integer(lhv) => (lhv < rhs.try_into_i64().map_err(|_| err())?).into(),
            Value::Float(lhv) => (lhv.into_inner() < rhs.try_into_f64().map_err(|_| err())?).into(),
            Value::Bytes(lhv) => (lhv < rhs.try_bytes()?).into(),
            Value::Timestamp(lhv) => (lhv < rhs.try_timestamp()?).into(),
            _ => return Err(err()),
        };

        Ok(value)
    }

    /// Similar to [`std::cmp::Ord`], but fallible (e.g. `TryOrd`).
    fn try_le(self, rhs: Self) -> Result<Self, ValueError> {
        let err = || ValueError::Ge(self.kind(), rhs.kind());

        let value = match self {
            Value::Integer(lhv) if rhs.is_float() => (lhv as f64 <= rhs.try_float()?).into(),
            Value::Integer(lhv) => (lhv <= rhs.try_into_i64().map_err(|_| err())?).into(),
            Value::Float(lhv) => {
                (lhv.into_inner() <= rhs.try_into_f64().map_err(|_| err())?).into()
            }
            Value::Bytes(lhv) => (lhv <= rhs.try_bytes()?).into(),
            Value::Timestamp(lhv) => (lhv <= rhs.try_timestamp()?).into(),
            _ => return Err(err()),
        };

        Ok(value)
    }

    fn try_merge(self, rhs: Self) -> Result<Self, ValueError> {
        let err = || ValueError::Merge(self.kind(), rhs.kind());

        let value = match (&self, &rhs) {
            (Value::Object(lhv), Value::Object(rhv)) => lhv
                .iter()
                .chain(rhv.iter())
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect::<ObjectMap>()
                .into(),
            _ => return Err(err()),
        };

        Ok(value)
    }

    /// Similar to [`std::cmp::Eq`], but does a lossless comparison for integers
    /// and floats.
    fn eq_lossy(&self, rhs: &Self) -> bool {
        use Value::{Float, Integer};

        match self {
            Integer(lhv) => rhs
                .try_into_f64()
                .map(|rhv| *lhv as f64 == rhv)
                .unwrap_or(false),

            Float(lhv) => rhs
                .try_into_f64()
                .map(|rhv| lhv.into_inner() == rhv)
                .unwrap_or(false),

            _ => self == rhs,
        }
    }
}

#[cfg(test)]
mod tests {
    use ordered_float::NotNan;

    use super::*;

    // OBE-10736: string * int must not OOM or capacity-overflow abort.

    #[test]
    fn try_mul_bytes_large_count_returns_error() {
        let s = Value::Bytes(bytes::Bytes::from("a"));
        let n = Value::Integer(64 * 1024 * 1024 + 1);
        assert!(s.try_mul(n).is_err(), "expected error for repeat count exceeding MAX_REPEAT_BYTES");
    }

    #[test]
    fn try_mul_bytes_overflow_count_returns_error() {
        let s = Value::Bytes(bytes::Bytes::from_static(b"aaaa")); // len = 4
        let n = Value::Integer(i64::MAX); // 4 * i64::MAX overflows usize on 64-bit
        assert!(s.try_mul(n).is_err(), "expected error on checked_mul overflow");
    }

    #[test]
    fn try_mul_int_bytes_large_count_returns_error() {
        let n = Value::Integer(64 * 1024 * 1024 + 1);
        let s = Value::Bytes(bytes::Bytes::from("b"));
        assert!(n.try_mul(s).is_err(), "expected error for int * bytes exceeding MAX_REPEAT_BYTES");
    }

    #[test]
    fn try_mul_bytes_small_count_succeeds() {
        let s = Value::Bytes(bytes::Bytes::from("ab"));
        let n = Value::Integer(3);
        let result = s.try_mul(n).expect("expected success for small repeat");
        assert_eq!(result, Value::Bytes(bytes::Bytes::from("ababab")));
    }

    #[test]
    fn try_mul_bytes_zero_count_returns_empty() {
        let s = Value::Bytes(bytes::Bytes::from("hello"));
        let n = Value::Integer(0);
        let result = s.try_mul(n).expect("expected success for zero repeat");
        assert_eq!(result, Value::Bytes(bytes::Bytes::new()));
    }

    // `NotNan` permits infinities, so a float operand can legitimately be ±∞ (e.g. produced by
    // an overflowing multiplication, or by `parse_json` on an out-of-range literal). Operations
    // whose result is NaN used to reach `NotNan`'s `Add`/`Mul`/`Rem` impls, which panic.
    // See OBE-10727.
    fn float(v: f64) -> Value {
        Value::Float(NotNan::new(v).expect("test operand is not NaN"))
    }

    #[test]
    fn multiplying_infinity_by_zero_returns_an_error_instead_of_panicking() {
        assert!(float(f64::INFINITY).try_mul(float(0.0)).is_err());
        assert!(float(0.0).try_mul(float(f64::NEG_INFINITY)).is_err());
    }

    #[test]
    fn adding_opposite_infinities_returns_an_error_instead_of_panicking() {
        assert!(float(f64::INFINITY)
            .try_add(float(f64::NEG_INFINITY))
            .is_err());
    }

    #[test]
    fn taking_the_remainder_of_infinity_returns_an_error_instead_of_panicking() {
        assert!(float(f64::INFINITY).try_rem(float(f64::INFINITY)).is_err());
        assert!(float(f64::INFINITY).try_rem(float(2.0)).is_err());
    }

    #[test]
    fn an_infinite_result_is_still_a_valid_value() {
        // Only NaN results are rejected — overflow to ±∞ must keep working.
        assert_eq!(
            float(f64::MAX).try_mul(float(10.0)),
            Ok(float(f64::INFINITY))
        );
        assert_eq!(
            float(f64::INFINITY).try_add(float(1.0)),
            Ok(float(f64::INFINITY))
        );
    }
}
