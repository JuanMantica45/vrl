use itertools::Itertools;

use crate::compiler::prelude::*;

static DEFAULT_SEPARATOR: &str = ".";

const MAX_UNFLATTEN_DEPTH: usize = 128;

fn unflatten(value: Value, separator: Value, recursive: Value) -> Resolved {
    let separator = separator.try_bytes_utf8_lossy()?.into_owned();
    let recursive = recursive.try_boolean()?;
    let map = value.try_object()?;
    Ok(do_unflatten(map.into(), &separator, recursive, 0))
}

fn do_unflatten(value: Value, separator: &str, recursive: bool, depth: usize) -> Value {
    match value {
        Value::Object(map) => do_unflatten_entries(map, separator, recursive, depth).into(),
        // Note that objects inside arrays are not unflattened
        _ => value,
    }
}

fn do_unflatten_entries<I>(entries: I, separator: &str, recursive: bool, depth: usize) -> ObjectMap
where
    I: IntoIterator<Item = (KeyString, Value)>,
{
    if depth >= MAX_UNFLATTEN_DEPTH {
        // Stop splitting keys at the depth limit; collect remainder as literal keys.
        return entries.into_iter().collect();
    }

    let grouped = entries
        .into_iter()
        .map(|(key, value)| {
            let (head, rest) = match key.split_once(separator) {
                Some((key, rest)) => (key.to_string().into(), Some(rest.to_string().into())),
                None => (key.clone(), None),
            };
            (head, rest, value)
        })
        .into_group_map_by(|(head, _, _)| head.clone());

    grouped
        .into_iter()
        .map(|(key, mut values)| {
            if values.len() == 1 {
                match values.pop().expect("exactly one element") {
                    (_, None, value) => {
                        let value = if recursive {
                            do_unflatten(value, separator, recursive, depth + 1)
                        } else {
                            value
                        };
                        return (key, value);
                    }
                    (_, Some(rest), value) => {
                        let result =
                            do_unflatten_entry((rest, value), separator, recursive, depth + 1);
                        return (key, result);
                    }
                }
            }

            let new_entries = values
                .into_iter()
                .filter_map(|(_, rest, value)| {
                    // In this case, there is more than one value prefixed with the same key
                    // and therefore there must be nested values, so we can't set a single top-level value
                    // and we must filter it out.
                    // Example input of this case:
                    // {
                    //    "a.b": 1,
                    //    "a": 2
                    // }
                    // Here, we will have two items grouped by "a",
                    // one will have `"b"` as rest and the other will have `None`.
                    // We have to filter the second, as we can't set the second value
                    // as the value of the entry `"a"` (considered the top-level key at this level)
                    rest.map(|rest| (rest, value))
                })
                .collect::<Vec<_>>();
            let result = do_unflatten_entries(new_entries, separator, recursive, depth + 1);
            (key, result.into())
        })
        .collect()
}

// Optimization in the case we have to flatten objects like
// { "a.b.c.d": 1 }
// and avoid doing recursive calls to `do_unflatten_entries` with a single entry every time
fn do_unflatten_entry(
    entry: (KeyString, Value),
    separator: &str,
    recursive: bool,
    depth: usize,
) -> Value {
    let (key, value) = entry;
    // splitn caps the segment count at the depth budget remaining after `depth`; the final
    // piece retains any remaining separator characters as a literal key (OBE-10744).
    let remaining = MAX_UNFLATTEN_DEPTH.saturating_sub(depth);
    let keys: Vec<KeyString> = key
        .splitn(remaining + 1, separator)
        .map(Into::into)
        .collect();
    let mut result = if recursive {
        do_unflatten(value, separator, recursive, depth + keys.len())
    } else {
        value
    };
    for key in keys.into_iter().rev() {
        result = Value::Object(ObjectMap::from_iter([(key, result)]));
    }
    result
}

#[derive(Clone, Copy, Debug)]
pub struct Unflatten;

impl Function for Unflatten {
    fn identifier(&self) -> &'static str {
        "unflatten"
    }

    fn parameters(&self) -> &'static [Parameter] {
        &[
            Parameter {
                keyword: "value",
                kind: kind::OBJECT,
                required: true,
            },
            Parameter {
                keyword: "separator",
                kind: kind::BYTES,
                required: false,
            },
            Parameter {
                keyword: "recursive",
                kind: kind::BOOLEAN,
                required: false,
            },
        ]
    }

    fn examples(&self) -> &'static [Example] {
        &[
            Example {
                title: "simple",
                source: r#"unflatten({ "foo.bar.baz": true, "foo.bar.qux": false, "foo.quux": 42 })"#,
                result: Ok(r#"{ "foo": { "bar": { "baz": true, "qux": false }, "quux": 42 } }"#),
            },
            Example {
                title: "inner flattened recursive",
                source: r#"unflatten({ "flattened.parent": { "foo.bar": true, "foo.baz": false } })"#,
                result: Ok(
                    r#"{ "flattened": { "parent": { "foo": { "bar": true, "baz": false } } } }"#,
                ),
            },
            Example {
                title: "inner flattened not recursive",
                source: r#"unflatten({ "flattened.parent": { "foo.bar": true, "foo.baz": false } }, recursive: false)"#,
                result: Ok(
                    r#"{ "flattened": { "parent": { "foo.bar": true, "foo.baz": false } } }"#,
                ),
            },
            Example {
                title: "with custom separator",
                source: r#"unflatten({ "foo_bar": true }, "_")"#,
                result: Ok(r#"{"foo": { "bar": true }}"#),
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
        let separator = arguments
            .optional("separator")
            .unwrap_or_else(|| expr!(DEFAULT_SEPARATOR));
        let recursive = arguments
            .optional("recursive")
            .unwrap_or_else(|| expr!(true));

        Ok(UnflattenFn {
            value,
            separator,
            recursive,
        }
        .as_expr())
    }
}

#[derive(Debug, Clone)]
struct UnflattenFn {
    value: Box<dyn Expression>,
    separator: Box<dyn Expression>,
    recursive: Box<dyn Expression>,
}

impl FunctionExpression for UnflattenFn {
    fn resolve(&self, ctx: &mut Context) -> Resolved {
        let value = self.value.resolve(ctx)?;
        let separator = self.separator.resolve(ctx)?;
        let recursive = self.recursive.resolve(ctx)?;

        unflatten(value, separator, recursive)
    }

    fn type_def(&self, _: &TypeState) -> TypeDef {
        TypeDef::object(Collection::any())
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::value;

    test_function![
        unflatten => Unflatten;

        map {
            args: func_args![value: value!({parent: "child"})],
            want: Ok(value!({parent: "child"})),
            tdef: TypeDef::object(Collection::any()),
        }

        nested_map {
            args: func_args![value: value!({"parent.child1": 1, "parent.child2": 2, key: "val"})],
            want: Ok(value!({parent: {child1: 1, child2: 2}, key: "val"})),
            tdef: TypeDef::object(Collection::any()),
        }

        nested_map_with_separator {
            args: func_args![value: value!({"parent_child1": 1, "parent_child2": 2, key: "val"}), separator: "_"],
            want: Ok(value!({parent: {child1: 1, child2: 2}, key: "val"})),
            tdef: TypeDef::object(Collection::any()),
        }

        double_nested_map {
            args: func_args![value: value!({
                "parent.child1": 1,
                "parent.child2.grandchild1": 1,
                "parent.child2.grandchild2": 2,
                key: "val",
            })],
            want: Ok(value!({
                parent: {
                    child1: 1,
                    child2: { grandchild1: 1, grandchild2: 2 },
                },
                key: "val",
            })),
            tdef: TypeDef::object(Collection::any()),
        }

        // Not only keys at first level are unflattened
        double_inner_nested_map_not_recursive {
            args: func_args![value: value!({
                "parent.children": {"child1":1, "child2.grandchild1": 1, "child2.grandchild2": 2 },
                key: "val",
            }), recursive: false],
            want: Ok(value!({
                parent: {
                    children: {child1: 1, "child2.grandchild1": 1, "child2.grandchild2": 2 }
                },
                key: "val",
            })),
            tdef: TypeDef::object(Collection::any()),
        }

        // Not only keys at first level are unflattened
        double_inner_nested_map_recursive {
            args: func_args![value: value!({
                "parent.children": {child1:1, "child2.grandchild1": 1, "child2.grandchild2": 2 },
                key: "val",
            })],
            want: Ok(value!({
                parent: {
                    children: {
                        child1: 1,
                        child2: { grandchild1: 1, grandchild2: 2 },
                    },
                },
                key: "val",
            })),
            tdef: TypeDef::object(Collection::any()),
        }

        map_and_array {
            args: func_args![value: value!({
                "parent.child1": [1, [2, 3]],
                "parent.child2.grandchild1": 1,
                "parent.child2.grandchild2": [1, [2, 3], 4],
                key: "val",
            })],
            want: Ok(value!({
                parent: {
                    child1: [1, [2, 3]],
                    child2: {grandchild1: 1, grandchild2: [1, [2, 3], 4]},
                },
                key: "val",
            })),
            tdef: TypeDef::object(Collection::any()),
        }

        map_and_array_with_separator {
            args: func_args![value: value!({
                "parent_child1": [1, [2, 3]],
                "parent_child2_grandchild1": 1,
                "parent_child2_grandchild2": [1, [2, 3], 4],
                key: "val",
            }), separator: "_"],
            want: Ok(value!({
                parent: {
                    child1: [1, [2, 3]],
                    child2: {grandchild1: 1, grandchild2: [1, [2, 3], 4]},
                },
                key: "val",
            })),
            tdef: TypeDef::object(Collection::any()),
        }

        // Objects inside arrays are not unflattened
        objects_inside_arrays {
            args: func_args![value: value!({
                "parent": [{"child1":1},{"child2.grandchild1": 1, "child2.grandchild2": 2 }],
                key: "val",
            })],
            want: Ok(value!({
                "parent": [{"child1":1},{"child2.grandchild1": 1, "child2.grandchild2": 2 }],
                key: "val",
            })),
            tdef: TypeDef::object(Collection::any()),
        }

        triple_nested_map {
            args: func_args![value: value!({
                "parent1.child1.grandchild1": 1,
                "parent1.child2.grandchild2": 2,
                "parent1.child2.grandchild3": 3,
                parent2: 4,
            })],
            want: Ok(value!({
                parent1: {
                    child1: { grandchild1: 1 },
                    child2: { grandchild2: 2, grandchild3: 3 },
                },
                parent2: 4,
            })),
            tdef: TypeDef::object(Collection::any()),
        }

        single_very_nested_map{
            args: func_args![value: value!({
                "a.b.c.d.e.f.g": 1,
            })],
            want: Ok(value!({
                a: {
                    b: {
                        c: {
                            d: {
                                e: {
                                    f: {
                                        g: 1,
                                    },
                                },
                            },
                        },
                    },
                },
            })),
            tdef: TypeDef::object(Collection::any()),
        }

        consecutive_separators {
            args: func_args![value: value!({
                "a..b": 1,
                "a...c": 2,
            })],
            want: Ok(value!({
                a: {
                    "": {
                        b: 1,
                        "": {
                            c: 2,
                        },
                    },
                },
            })),
            tdef: TypeDef::object(Collection::any()),
        }

        traling_separator{
            args: func_args![value: value!({
                "a.": 1,
            })],
            want: Ok(value!({
                a: {
                    "": 1,
                },
            })),
            tdef: TypeDef::object(Collection::any()),
        }

        consecutive_trailing_separator{
            args: func_args![value: value!({
                "a..": 1,
            })],
            want: Ok(value!({
                a: {
                    "": {
                        "": 1,
                    }
                },
            })),
            tdef: TypeDef::object(Collection::any()),
        }

        filter_out_top_level_value_when_multiple_values {
            args: func_args![value: value!({
                "a.b": 1,
                "a": 2,
            })],
            want: Ok(value!({
                a: { b: 1 },
            })),
            tdef: TypeDef::object(Collection::any()),
        }
    ];

    #[test]
    fn test_depth_limit_obe10744() {
        // OBE-10744: unflatten with deeply grouped entries must terminate at MAX_UNFLATTEN_DEPTH.
        // Build two entries sharing a 200-level common prefix ("k0.k1...k199.x" and "...y").
        // Without the fix, do_unflatten_entries recurses 200 times and can stack overflow.
        let prefix: Vec<String> = (0..200).map(|i| format!("k{i}")).collect();
        let key1: KeyString = [prefix.join("."), "x".into()].join(".").into();
        let key2: KeyString = [prefix.join("."), "y".into()].join(".").into();
        let entries: Vec<(KeyString, Value)> = vec![(key1, Value::Integer(1)), (key2, Value::Integer(2))];
        // Must return without panicking and depth must be bounded.
        let result = do_unflatten_entries(entries, ".", false, 0);
        // Walk result to verify nesting depth is bounded.
        let mut depth = 0usize;
        let mut cur = Value::Object(result);
        loop {
            match cur {
                Value::Object(ref m) if m.len() == 1 => {
                    cur = m.values().next().unwrap().clone();
                    depth += 1;
                    assert!(depth <= MAX_UNFLATTEN_DEPTH + 1, "nesting depth {depth} exceeds cap (OBE-10744)");
                }
                _ => break,
            }
        }
    }

    #[test]
    fn test_unflatten_recursive_forwards_depth_obe10744() {
        // OBE-10744 (reset-to-0 regression): `recursive: true` re-enters `do_unflatten` on every
        // pre-existing nested `Value::Object`, not just on split dotted keys. If that re-entry
        // resets depth to 0 instead of forwarding the accumulated depth, the cap never engages
        // for plain nested objects, and a deeply pre-nested value still drives unbounded
        // native-stack recursion.
        //
        // Build MAX_UNFLATTEN_DEPTH + 10 levels of single-key nested objects, with a dotted leaf
        // key at the bottom. With depth correctly forwarded, recursion must stop once the cap is
        // reached, leaving the leaf-most dotted key un-split.
        let total_levels = MAX_UNFLATTEN_DEPTH + 10;
        let mut value = value!({ "leaf.key": 1 });
        for i in (0..total_levels).rev() {
            value = Value::Object(ObjectMap::from_iter([(format!("l{i}").into(), value)]));
        }

        let result = do_unflatten(value, ".", true, 0);

        let mut cur = &result;
        for i in 0..total_levels {
            let Value::Object(map) = cur else {
                panic!("expected object at level {i}, got {cur:?}")
            };
            cur = map.values().next().expect("expected exactly one entry");
        }
        let Value::Object(leaf_map) = cur else {
            panic!("expected leaf object, got {cur:?}")
        };
        assert!(
            leaf_map.contains_key("leaf.key"),
            "past the depth cap, dotted keys must remain un-split — recursion should have \
             stopped instead of continuing to the leaf: {leaf_map:?}"
        );
    }
}
