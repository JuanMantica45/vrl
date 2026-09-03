use std::{collections::BTreeMap, fmt, ops::Deref};

use crate::value::depth::{depth_exceeds, MAX_VALUE_DEPTH};
use crate::value::{KeyString, Value};
use crate::{
    compiler::{
        expression::{Expr, Resolved},
        state::{TypeInfo, TypeState},
        Context, Expression, TypeDef,
    },
    value::Kind,
};

#[derive(Debug, Clone, PartialEq)]
pub struct Object {
    inner: BTreeMap<KeyString, Expr>,
}

impl Object {
    #[must_use]
    pub fn new(inner: BTreeMap<KeyString, Expr>) -> Self {
        Self { inner }
    }
}

impl Deref for Object {
    type Target = BTreeMap<KeyString, Expr>;

    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

// OBE-10732: `v = { "a": v }` in a loop grows nesting one level per iteration. Same tradeoff as
// the array literal cap in `array.rs`: an over-limit field value is dropped and logged.
fn cap_depth(fields: BTreeMap<KeyString, Value>) -> BTreeMap<KeyString, Value> {
    fields
        .into_iter()
        .map(|(key, value)| {
            if depth_exceeds(&value, MAX_VALUE_DEPTH - 1) {
                tracing::warn!(
                    max_depth = MAX_VALUE_DEPTH,
                    "object literal field exceeds max value depth, replaced with null"
                );
                (key, Value::Null)
            } else {
                (key, value)
            }
        })
        .collect()
}

impl Expression for Object {
    fn resolve(&self, ctx: &mut Context) -> Resolved {
        let fields: BTreeMap<_, _> = self
            .inner
            .iter()
            .map(|(key, expr)| expr.resolve(ctx).map(|v| (key.clone(), v)))
            .collect::<Result<BTreeMap<_, _>, _>>()?;

        Ok(Value::Object(cap_depth(fields)))
    }

    fn resolve_constant(&self, state: &TypeState) -> Option<Value> {
        self.inner
            .iter()
            .map(|(key, expr)| expr.resolve_constant(state).map(|v| (key.clone(), v)))
            .collect::<Option<BTreeMap<_, _>>>()
            .map(Value::Object)
    }

    fn type_info(&self, state: &TypeState) -> TypeInfo {
        let mut state = state.clone();
        let mut fallible = false;
        let mut returns = Kind::never();

        let mut type_defs = BTreeMap::new();
        for (k, expr) in &self.inner {
            let type_def = expr.apply_type_info(&mut state).upgrade_undefined();
            returns.merge_keep(type_def.returns().clone(), false);

            // If any expression is fallible, the entire object is fallible.
            fallible |= type_def.is_fallible();

            // If any expression aborts, the entire object aborts
            if type_def.is_never() {
                return TypeInfo::new(
                    state,
                    TypeDef::never()
                        .maybe_fallible(fallible)
                        .with_returns(returns),
                );
            }
            type_defs.insert(k.clone(), type_def);
        }

        let collection = type_defs
            .into_iter()
            .map(|(field, type_def)| (field.into(), type_def.into()))
            .collect::<BTreeMap<_, _>>();

        let result = TypeDef::object(collection)
            .maybe_fallible(fallible)
            .with_returns(returns);
        TypeInfo::new(state, result)
    }
}

impl fmt::Display for Object {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let exprs = self
            .inner
            .iter()
            .map(|(k, v)| format!(r#""{k}": {v}"#))
            .collect::<Vec<_>>()
            .join(", ");

        write!(f, "{{ {exprs} }}")
    }
}

impl From<BTreeMap<KeyString, Expr>> for Object {
    fn from(inner: BTreeMap<KeyString, Expr>) -> Self {
        Self { inner }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A `Value` nested exactly `depth` levels: `nested(1)` is a scalar, `nested(2)` is `[scalar]`.
    fn nested(depth: usize) -> Value {
        let mut v = Value::Null;
        for _ in 1..depth {
            v = Value::Array(vec![v]);
        }
        v
    }

    // OBE-10732: an over-limit field is dropped, the boundary and ordinary fields are untouched.
    #[test]
    fn cap_depth_drops_only_the_over_limit_field() {
        let at_boundary = nested(MAX_VALUE_DEPTH - 1);
        let fields = BTreeMap::from([
            (KeyString::from("a"), Value::Integer(1)),
            (KeyString::from("b"), at_boundary.clone()),
            (KeyString::from("c"), nested(MAX_VALUE_DEPTH)),
        ]);
        let capped = cap_depth(fields);
        assert_eq!(capped[&KeyString::from("a")], Value::Integer(1));
        assert_eq!(capped[&KeyString::from("b")], at_boundary);
        assert_eq!(capped[&KeyString::from("c")], Value::Null);
    }
}
