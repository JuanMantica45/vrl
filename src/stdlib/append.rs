use crate::compiler::prelude::*;
use crate::value::depth::{depth_exceeds, MAX_VALUE_DEPTH};

fn append(value: Value, items: Value) -> Resolved {
    let mut value = value.try_array()?;
    let mut items = items.try_array()?;

    // OBE-10732: same reasoning as `push` — every element of both arrays becomes a direct child
    // of the result, so each is checked.
    if value
        .iter()
        .chain(items.iter())
        .any(|item| depth_exceeds(item, MAX_VALUE_DEPTH - 1))
    {
        return Err(format!(
            "cannot append: the result would nest deeper than the limit of {MAX_VALUE_DEPTH}"
        )
        .into());
    }

    value.append(&mut items);
    Ok(value.into())
}

#[derive(Clone, Copy, Debug)]
pub struct Append;

impl Function for Append {
    fn identifier(&self) -> &'static str {
        "append"
    }

    fn parameters(&self) -> &'static [Parameter] {
        &[
            Parameter {
                keyword: "value",
                kind: kind::ARRAY,
                required: true,
            },
            Parameter {
                keyword: "items",
                kind: kind::ARRAY,
                required: true,
            },
        ]
    }

    fn examples(&self) -> &'static [Example] {
        &[Example {
            title: "append array",
            source: "append([0, 1], [2, 3])",
            result: Ok("[0, 1, 2, 3]"),
        }]
    }

    fn compile(
        &self,
        _state: &state::TypeState,
        _ctx: &mut FunctionCompileContext,
        arguments: ArgumentList,
    ) -> Compiled {
        let value = arguments.required("value");
        let items = arguments.required("items");

        Ok(AppendFn { value, items }.as_expr())
    }
}

#[derive(Debug, Clone)]
struct AppendFn {
    value: Box<dyn Expression>,
    items: Box<dyn Expression>,
}

impl FunctionExpression for AppendFn {
    fn resolve(&self, ctx: &mut Context) -> Resolved {
        let value = self.value.resolve(ctx)?;
        let items = self.items.resolve(ctx)?;

        append(value, items)
    }

    fn type_def(&self, state: &state::TypeState) -> TypeDef {
        let mut self_value = self.value.type_def(state).restrict_array();
        let items = self.items.type_def(state).restrict_array();

        let self_array = self_value.as_array_mut().expect("must be an array");
        let items_array = items.as_array().expect("must be an array");

        if let Some(exact_len) = self_array.exact_length() {
            // The exact array length is known.
            for (i, i_kind) in items_array.known() {
                self_array
                    .known_mut()
                    .insert((i.to_usize() + exact_len).into(), i_kind.clone());
            }

            // "value" can't have an unknown, so they new unknown is just that of "items".
            self_array.set_unknown(items_array.unknown_kind());
        } else {
            // We don't know where the items will be inserted, so the union of all items will be added to the unknown.
            self_array.set_unknown(self_array.unknown_kind().union(items_array.reduced_kind()));
        }

        self_value.infallible()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{btreemap, value};

    test_function![
        append => Append;

        both_arrays_empty {
            args: func_args![value: value!([]), items: value!([])],
            want: Ok(value!([])),
            tdef: TypeDef::array(Collection::empty()),
        }

        one_array_empty {
            args: func_args![value: value!([]), items: value!([1, 2, 3])],
            want: Ok(value!([1, 2, 3])),
            tdef: TypeDef::array(btreemap! {
                Index::from(0) => Kind::integer(),
                Index::from(1) => Kind::integer(),
                Index::from(2) => Kind::integer(),
            }),
        }

        neither_array_empty {
            args: func_args![value: value!([1, 2, 3]), items: value!([4, 5, 6])],
            want: Ok(value!([1, 2, 3, 4, 5, 6])),
            tdef: TypeDef::array(btreemap! {
                Index::from(0) => Kind::integer(),
                Index::from(1) => Kind::integer(),
                Index::from(2) => Kind::integer(),
                Index::from(3) => Kind::integer(),
                Index::from(4) => Kind::integer(),
                Index::from(5) => Kind::integer(),
            }),
        }

        mixed_array_types {
            args: func_args![value: value!([1, 2, 3]), items: value!([true, 5.0, "bar"])],
            want: Ok(value!([1, 2, 3, true, 5.0, "bar"])),
            tdef: TypeDef::array(btreemap! {
                Index::from(0) => Kind::integer(),
                Index::from(1) => Kind::integer(),
                Index::from(2) => Kind::integer(),
                Index::from(3) => Kind::boolean(),
                Index::from(4) => Kind::float(),
                Index::from(5) => Kind::bytes(),
            }),
        }
    ];
}

#[cfg(test)]
mod depth_tests {
    use super::*;

    /// A `Value` nested exactly `depth` levels: `nested(1)` is a scalar, `nested(2)` is `[scalar]`.
    fn nested(depth: usize) -> Value {
        let mut v = Value::Null;
        for _ in 1..depth {
            v = Value::Array(vec![v]);
        }
        v
    }

    #[test]
    fn append_rejects_only_past_the_depth_cap() {
        let boundary = Value::Array(vec![nested(MAX_VALUE_DEPTH - 1)]);
        let over = Value::Array(vec![nested(MAX_VALUE_DEPTH)]);
        assert!(append(Value::Array(vec![]), boundary).is_ok());
        assert!(append(Value::Array(vec![]), over).is_err());
    }

    // The type error is more fundamental, so it must win when both are wrong.
    #[test]
    fn append_reports_the_type_error_before_the_depth_error() {
        let too_deep_items = Value::Array(vec![nested(MAX_VALUE_DEPTH)]);
        let err = append(Value::Integer(1), too_deep_items)
            .expect_err("expected an error")
            .to_string();
        assert!(
            !err.contains("nest deeper"),
            "expected the try_array type error, got the depth error instead: {err}"
        );
    }
}
