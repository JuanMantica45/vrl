use std::{collections::BTreeMap, fmt, ops::Deref};

use crate::value::depth::{depth_exceeds, MAX_VALUE_DEPTH};
use crate::value::Value;
use crate::{
    compiler::{
        expression::{Expr, Resolved},
        state::{TypeInfo, TypeState},
        Context, Expression, TypeDef,
    },
    value::Kind,
};

#[derive(Debug, Clone, PartialEq)]
pub struct Array {
    inner: Vec<Expr>,
}

impl Array {
    pub(crate) fn new(inner: Vec<Expr>) -> Self {
        Self { inner }
    }
}

impl Deref for Array {
    type Target = Vec<Expr>;

    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

// OBE-10732: `v = [v]` in a loop grows nesting one level per iteration, same shape `push` closed.
// Literal syntax can't be made fallible without breaking every array literal in existence, so —
// as with the array-index cap in `crud/mod.rs` — an over-limit item is dropped and logged instead.
fn cap_depth(items: Vec<Value>) -> Vec<Value> {
    items
        .into_iter()
        .map(|item| {
            if depth_exceeds(&item, MAX_VALUE_DEPTH - 1) {
                tracing::warn!(
                    max_depth = MAX_VALUE_DEPTH,
                    "array literal element exceeds max value depth, replaced with null"
                );
                Value::Null
            } else {
                item
            }
        })
        .collect()
}

impl Expression for Array {
    fn resolve(&self, ctx: &mut Context) -> Resolved {
        let items = self
            .inner
            .iter()
            .map(|expr| expr.resolve(ctx))
            .collect::<Result<Vec<_>, _>>()?;

        Ok(Value::Array(cap_depth(items)))
    }

    fn resolve_constant(&self, state: &TypeState) -> Option<Value> {
        self.inner
            .iter()
            .map(|x| x.resolve_constant(state))
            .collect::<Option<Vec<_>>>()
            .map(Value::Array)
    }

    fn type_info(&self, state: &TypeState) -> TypeInfo {
        let mut state = state.clone();

        let mut type_defs = vec![];
        let mut fallible = false;

        for expr in &self.inner {
            let type_def = expr.apply_type_info(&mut state).upgrade_undefined();

            // If any expression is fallible, the entire array is fallible.
            fallible |= type_def.is_fallible();

            // If any expression aborts, the entire array aborts
            if type_def.is_never() {
                return TypeInfo::new(state, TypeDef::never().maybe_fallible(fallible));
            }
            type_defs.push(type_def);
        }

        let returns = type_defs.iter().fold(Kind::never(), |returns, type_def| {
            returns.union(type_def.returns().clone())
        });

        let collection = type_defs
            .into_iter()
            .enumerate()
            .map(|(index, type_def)| (index.into(), type_def.into()))
            .collect::<BTreeMap<_, _>>();

        TypeInfo::new(
            state,
            TypeDef::array(collection)
                .maybe_fallible(fallible)
                .with_returns(returns),
        )
    }
}

impl fmt::Display for Array {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let exprs = self
            .inner
            .iter()
            .map(Expr::to_string)
            .collect::<Vec<_>>()
            .join(", ");

        write!(f, "[{exprs}]")
    }
}

impl From<Vec<Expr>> for Array {
    fn from(inner: Vec<Expr>) -> Self {
        Self { inner }
    }
}

#[cfg(test)]
mod tests {
    use crate::value::kind::Collection;
    use crate::{expr, test_type_def, value::Kind};

    use super::*;

    test_type_def![
        empty_array {
            expr: |_| expr!([]),
            want: TypeDef::array(Collection::empty()),
        }

        scalar_array {
            expr: |_| expr!([1, "foo", true]),
            want: TypeDef::array(BTreeMap::from([
                (0.into(), Kind::integer()),
                (1.into(), Kind::bytes()),
                (2.into(), Kind::boolean()),
            ])),
        }

        mixed_array {
            expr: |_| expr!([1, [true, "foo"], { "bar": null }]),
            want: TypeDef::array(BTreeMap::from([
                (0.into(), Kind::integer()),
                (1.into(), Kind::array(BTreeMap::from([
                    (0.into(), Kind::boolean()),
                    (1.into(), Kind::bytes()),
                ]))),
                (2.into(), Kind::object(BTreeMap::from([
                    ("bar".into(), Kind::null())
                ]))),
            ])),
        }
    ];

    /// A `Value` nested exactly `depth` levels: `nested(1)` is a scalar, `nested(2)` is `[scalar]`.
    fn nested(depth: usize) -> Value {
        let mut v = Value::Null;
        for _ in 1..depth {
            v = Value::Array(vec![v]);
        }
        v
    }

    // OBE-10732: an over-limit item is dropped, the boundary and ordinary items are untouched.
    #[test]
    fn cap_depth_drops_only_the_over_limit_item() {
        let at_boundary = nested(MAX_VALUE_DEPTH - 1);
        let items = vec![
            Value::Integer(1),
            at_boundary.clone(),
            nested(MAX_VALUE_DEPTH),
        ];
        assert_eq!(
            cap_depth(items),
            vec![Value::Integer(1), at_boundary, Value::Null]
        );
    }
}
