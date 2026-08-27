use crate::compiler::prelude::*;
use crate::value::depth::{depth_exceeds, MAX_VALUE_DEPTH};

fn push(list: Value, item: Value) -> Resolved {
    // OBE-10732: `v = push([], v)` inside a loop grows nesting one level per iteration, which is
    // how a VRL program builds a `Value` deep enough to overflow the stack in `Clone`, `PartialEq`
    // or `Display`. None of those can return an error, so the only place to stop it is before the
    // value is built. The item lands one level below the resulting array, so it may be at most
    // `MAX_VALUE_DEPTH - 1` deep.
    if depth_exceeds(&item, MAX_VALUE_DEPTH - 1) {
        return Err(format!(
            "cannot push: the result would nest deeper than the limit of {MAX_VALUE_DEPTH}"
        )
        .into());
    }

    let mut list = list.try_array()?;
    list.push(item);
    Ok(list.into())
}

#[derive(Clone, Copy, Debug)]
pub struct Push;

impl Function for Push {
    fn identifier(&self) -> &'static str {
        "push"
    }

    fn parameters(&self) -> &'static [Parameter] {
        &[
            Parameter {
                keyword: "value",
                kind: kind::ARRAY,
                required: true,
            },
            Parameter {
                keyword: "item",
                kind: kind::ANY,
                required: true,
            },
        ]
    }

    fn examples(&self) -> &'static [Example] {
        &[
            Example {
                title: "push item",
                source: r#"push(["foo"], "bar")"#,
                result: Ok(r#"["foo", "bar"]"#),
            },
            Example {
                title: "empty array",
                source: r#"push([], "bar")"#,
                result: Ok(r#"["bar"]"#),
            },
        ]
    }

    fn compile(
        &self,
        _state: &state::TypeState,
        _ctx: &mut FunctionCompileContext,
        arguments: ArgumentList,
    ) -> Compiled {
        let value = arguments.required("value");
        let item = arguments.required("item");

        Ok(PushFn { value, item }.as_expr())
    }
}

#[derive(Debug, Clone)]
struct PushFn {
    value: Box<dyn Expression>,
    item: Box<dyn Expression>,
}

impl FunctionExpression for PushFn {
    fn resolve(&self, ctx: &mut Context) -> Resolved {
        let list = self.value.resolve(ctx)?;
        let item = self.item.resolve(ctx)?;

        push(list, item)
    }

    fn type_def(&self, state: &state::TypeState) -> TypeDef {
        let item = self.item.type_def(state).kind().clone().upgrade_undefined();
        let mut typedef = self.value.type_def(state).restrict_array();

        let array = typedef.as_array_mut().expect("must be an array");

        if let Some(exact_len) = array.exact_length() {
            // The exact array length is known, so just add the item to the correct index.
            array.known_mut().insert(exact_len.into(), item);
        } else {
            // We don't know where the item will be inserted, so just add it to the unknown.
            array.set_unknown(array.unknown_kind().union(item));
        }

        typedef.infallible()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::btreemap;
    use crate::value;

    test_function![
        push => Push;

        empty_array {
            args: func_args![value: value!([]), item: value!("foo")],
            want: Ok(value!(["foo"])),
            tdef: TypeDef::array(btreemap! {
                Index::from(0) => Kind::bytes(),
            }),
        }

        new_item {
            args: func_args![value: value!([11, false, 42.5]), item: value!("foo")],
            want: Ok(value!([11, false, 42.5, "foo"])),
            tdef: TypeDef::array(btreemap! {
                Index::from(0) => Kind::integer(),
                Index::from(1) => Kind::boolean(),
                Index::from(2) => Kind::float(),
                Index::from(3) => Kind::bytes(),
            }),
        }

        already_exists_item {
            args: func_args![value: value!([11, false, 42.5]), item: value!(42.5)],
            want: Ok(value!([11, false, 42.5, 42.5])),
            tdef: TypeDef::array(btreemap! {
                Index::from(0) => Kind::integer(),
                Index::from(1) => Kind::boolean(),
                Index::from(2) => Kind::float(),
                Index::from(3) => Kind::float(),
            }),
        }
    ];
}

#[cfg(test)]
mod depth_tests {
    use super::*;
    use crate::value::depth::MAX_VALUE_DEPTH;

    /// Builds a `Value` nested `depth` levels. Iterative, so building it costs no stack —
    /// which is the whole reason a deep `Value` is reachable from VRL in the first place.
    /// A `Value` nested exactly `depth` levels: `nested(1)` is a scalar, `nested(2)` is `[scalar]`.
    fn nested(depth: usize) -> Value {
        let mut v = Value::Null;
        for _ in 1..depth {
            v = Value::Array(vec![v]);
        }
        v
    }

    // OBE-10732: `v = push([], v)` in a loop grows nesting one level per iteration, with no cap.
    // Past a few thousand levels the resulting `Value` crashes the process in `PartialEq`, `Clone`
    // or `Display` — none of which can report an error — so the only place to stop it is here.
    #[test]
    fn push_rejects_an_item_that_would_exceed_the_depth_cap() {
        let item = nested(MAX_VALUE_DEPTH);
        assert!(
            push(Value::Array(vec![]), item).is_err(),
            "expected an error once the result would exceed MAX_VALUE_DEPTH"
        );
    }

    #[test]
    fn push_accepts_an_item_at_the_boundary() {
        let item = nested(MAX_VALUE_DEPTH - 1);
        assert!(
            push(Value::Array(vec![]), item).is_ok(),
            "expected a value landing exactly at MAX_VALUE_DEPTH to be accepted"
        );
    }

    #[test]
    fn push_leaves_ordinary_values_alone() {
        assert!(push(Value::Array(vec![]), Value::Integer(1)).is_ok());
        assert!(push(Value::Array(vec![]), nested(8)).is_ok());
    }
}
