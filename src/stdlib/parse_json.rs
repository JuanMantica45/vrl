use std::collections::HashMap;

use serde_json::{
    value::{RawValue, Value as JsonValue},
    Error, Map,
};

use crate::compiler::prelude::*;
use crate::stdlib::json_utils::json_type_def::json_type_def;

// In slurp mode: 0 values is an error, 1 unwraps to itself, N collapses to an array.
fn collapse_slurp(values: Vec<Value>) -> Resolved {
    match values.len() {
        0 => Err("unable to parse json: input contains no JSON values".into()),
        1 => Ok(values.into_iter().next().unwrap()),
        _ => Ok(Value::from(values)),
    }
}

fn parse_json(value: Value, lossy: Option<Value>, slurp: Option<Value>) -> Resolved {
    let lossy = lossy.map(Value::try_boolean).transpose()?.unwrap_or(true);
    let slurp = slurp.map(Value::try_boolean).transpose()?.unwrap_or(false);
    let bytes = if lossy {
        value.try_bytes_utf8_lossy()?.into_owned().into()
    } else {
        value.try_bytes()?
    };

    if slurp {
        let mut values = Vec::new();
        for v in serde_json::Deserializer::from_slice(&bytes).into_iter::<Value>() {
            values.push(v.map_err(|e| format!("unable to parse json: {e}"))?);
        }
        return collapse_slurp(values);
    }

    let value = serde_json::from_slice::<'_, Value>(&bytes)
        .map_err(|e| format!("unable to parse json: {e}"))?;
    Ok(value)
}

// parse_json_with_depth method recursively traverses the value and returns raw JSON-formatted bytes
// after reaching provided depth.
fn parse_json_with_depth(
    value: Value,
    max_depth: Value,
    lossy: Option<Value>,
    slurp: Option<Value>,
) -> Resolved {
    let parsed_depth = validate_depth(max_depth)?;
    let lossy = lossy.map(Value::try_boolean).transpose()?.unwrap_or(true);
    let slurp = slurp.map(Value::try_boolean).transpose()?.unwrap_or(false);
    let bytes = if lossy {
        value.try_bytes_utf8_lossy()?.into_owned().into()
    } else {
        value.try_bytes()?
    };

    if slurp {
        let mut values = Vec::new();
        for v in serde_json::Deserializer::from_slice(&bytes).into_iter::<Box<RawValue>>() {
            let raw = v.map_err(|e| format!("unable to read json: {e}"))?;
            let parsed = parse_layer(&raw, parsed_depth)
                .map_err(|e| format!("unable to parse json with max depth: {e}"))?;
            values.push(Value::from(parsed));
        }
        return collapse_slurp(values);
    }

    let raw_value = serde_json::from_slice::<'_, &RawValue>(&bytes)
        .map_err(|e| format!("unable to read json: {e}"))?;

    let res = parse_layer(raw_value, parsed_depth)
        .map_err(|e| format!("unable to parse json with max depth: {e}"))?;

    Ok(Value::from(res))
}

fn parse_layer(value: &RawValue, remaining_depth: u8) -> std::result::Result<JsonValue, Error> {
    let raw_value = value.get();

    // RawValue is a JSON object.
    if raw_value.starts_with('{') {
        if remaining_depth == 0 {
            // If max_depth is reached, return the raw representation of the JSON object,
            // e.g., "{\"key\":\"value\"}"
            serde_json::value::to_value(raw_value)
        } else {
            // Parse each value of the object as a raw JSON value recursively with the same method.
            let map: HashMap<String, &RawValue> = serde_json::from_str(raw_value)?;

            let mut res_map: Map<String, JsonValue> = Map::with_capacity(map.len());
            for (k, v) in map {
                res_map.insert(k, parse_layer(v, remaining_depth - 1)?);
            }
            Ok(serde_json::Value::from(res_map))
        }
    // RawValue is a JSON array.
    } else if raw_value.starts_with('[') {
        if remaining_depth == 0 {
            // If max_depth is reached, return the raw representation of the JSON array,
            // e.g., "[\"one\",\"two\",\"three\"]"
            serde_json::value::to_value(raw_value)
        } else {
            // Parse all values of the array as a raw JSON value recursively with the same method.
            let arr: Vec<&RawValue> = serde_json::from_str(raw_value)?;

            let mut res_arr: Vec<JsonValue> = Vec::with_capacity(arr.len());
            for v in arr {
                res_arr.push(parse_layer(v, remaining_depth - 1)?)
            }
            Ok(serde_json::Value::from(res_arr))
        }
    // RawValue is not an object or array, do not need to traverse the doc further.
    // Parse and return the value.
    } else {
        serde_json::from_str(raw_value)
    }
}

fn validate_depth(value: Value) -> ExpressionResult<u8> {
    let res = value.try_integer()?;

    // The lower cap is 1 because it is pointless to use anything lower,
    // because 'data = parse_json!(.message, max_depth: 0)' equals to 'data = .message'.
    //
    // The upper cap is 128 because serde_json has the same recursion limit by default.
    // https://github.com/serde-rs/json/blob/4d57ebeea8d791b8a51c229552d2d480415d00e6/json/src/de.rs#L111
    if (1..=128).contains(&res) {
        Ok(res as u8)
    } else {
        Err(ExpressionError::from(format!(
            "max_depth value should be greater than 0 and less than 128, got {res}"
        )))
    }
}

#[derive(Clone, Copy, Debug)]
pub struct ParseJson;

impl Function for ParseJson {
    fn identifier(&self) -> &'static str {
        "parse_json"
    }

    fn summary(&self) -> &'static str {
        "parse a string to a JSON type"
    }

    fn usage(&self) -> &'static str {
        indoc! {"
            Parses the provided `value` as JSON.

            Only JSON types are returned. If you need to convert a `string` into a `timestamp`,
            consider the `parse_timestamp` function.
        "}
    }

    fn parameters(&self) -> &'static [Parameter] {
        &[
            Parameter {
                keyword: "value",
                kind: kind::BYTES,
                required: true,
            },
            Parameter {
                keyword: "max_depth",
                kind: kind::INTEGER,
                required: false,
            },
            Parameter {
                keyword: "lossy",
                kind: kind::BOOLEAN,
                required: false,
            },
            Parameter {
                keyword: "slurp",
                kind: kind::BOOLEAN,
                required: false,
            },
        ]
    }

    fn examples(&self) -> &'static [Example] {
        &[
            Example {
                title: "object",
                source: r#"parse_json!(s'{ "field": "value" }')"#,
                result: Ok(r#"{ "field": "value" }"#),
            },
            Example {
                title: "array",
                source: r#"parse_json!("[true, 0]")"#,
                result: Ok("[true, 0]"),
            },
            Example {
                title: "string",
                source: r#"parse_json!(s'"hello"')"#,
                result: Ok("hello"),
            },
            Example {
                title: "integer",
                source: r#"parse_json!("42")"#,
                result: Ok("42"),
            },
            Example {
                title: "float",
                source: r#"parse_json!("42.13")"#,
                result: Ok("42.13"),
            },
            Example {
                title: "boolean",
                source: r#"parse_json!("false")"#,
                result: Ok("false"),
            },
            Example {
                title: "invalid value",
                source: r#"parse_json!("{ INVALID }")"#,
                result: Err(
                    r#"function call error for "parse_json" at (0:26): unable to parse json: key must be a string at line 1 column 3"#,
                ),
            },
            Example {
                title: "max_depth",
                source: r#"parse_json!(s'{"first_level":{"second_level":"finish"}}', max_depth: 1)"#,
                result: Ok(r#"{"first_level":"{\"second_level\":\"finish\"}"}"#),
            },
            Example {
                title: "slurp (multiple values)",
                source: r#"parse_json!(s'{"a":1}{"b":2}', slurp: true)"#,
                result: Ok(r#"[{ "a": 1 }, { "b": 2 }]"#),
            },
            Example {
                title: "slurp (single value passes through)",
                source: r#"parse_json!(s'{"a":1}', slurp: true)"#,
                result: Ok(r#"{ "a": 1 }"#),
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
        let max_depth = arguments.optional("max_depth");
        let lossy = arguments.optional("lossy");
        let slurp = arguments.optional("slurp");

        match max_depth {
            Some(max_depth) => Ok(ParseJsonMaxDepthFn {
                value,
                max_depth,
                lossy,
                slurp,
            }
            .as_expr()),
            None => Ok(ParseJsonFn {
                value,
                lossy,
                slurp,
            }
            .as_expr()),
        }
    }
}

#[derive(Debug, Clone)]
struct ParseJsonFn {
    value: Box<dyn Expression>,
    lossy: Option<Box<dyn Expression>>,
    slurp: Option<Box<dyn Expression>>,
}

impl FunctionExpression for ParseJsonFn {
    fn resolve(&self, ctx: &mut Context) -> Resolved {
        let value = self.value.resolve(ctx)?;
        let lossy = self.lossy.as_ref().map(|e| e.resolve(ctx)).transpose()?;
        let slurp = self.slurp.as_ref().map(|e| e.resolve(ctx)).transpose()?;
        parse_json(value, lossy, slurp)
    }

    fn type_def(&self, _: &state::TypeState) -> TypeDef {
        json_type_def()
    }
}

#[derive(Debug, Clone)]
struct ParseJsonMaxDepthFn {
    value: Box<dyn Expression>,
    max_depth: Box<dyn Expression>,
    lossy: Option<Box<dyn Expression>>,
    slurp: Option<Box<dyn Expression>>,
}

impl FunctionExpression for ParseJsonMaxDepthFn {
    fn resolve(&self, ctx: &mut Context) -> Resolved {
        let value = self.value.resolve(ctx)?;
        let max_depth = self.max_depth.resolve(ctx)?;
        let lossy = self.lossy.as_ref().map(|e| e.resolve(ctx)).transpose()?;
        let slurp = self.slurp.as_ref().map(|e| e.resolve(ctx)).transpose()?;
        parse_json_with_depth(value, max_depth, lossy, slurp)
    }

    fn type_def(&self, _: &state::TypeState) -> TypeDef {
        json_type_def()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::value;

    test_function![
        parse_json => ParseJson;

        parses {
            args: func_args![ value: r#"{"field": "value"}"# ],
            want: Ok(value!({ field: "value" })),
            tdef: json_type_def(),
        }

        complex_json {
            args: func_args![ value: r#"{"object": {"string":"value","number":42,"array":["hello","world"],"boolean":false}}"# ],
            want: Ok(value!({ object: {string: "value", number: 42, array: ["hello", "world"], boolean: false} })),
            tdef: json_type_def(),
        }

        invalid_json_errors {
            args: func_args![ value: r#"{"field": "value"# ],
            want: Err("unable to parse json: EOF while parsing a string at line 1 column 16"),
            tdef: json_type_def(),
        }

        max_depth {
            args: func_args![ value: r#"{"top_layer": {"layer_one": "finish", "layer_two": 2}}"#, max_depth: 1],
            want: Ok(value!({ top_layer: r#"{"layer_one": "finish", "layer_two": 2}"# })),
            tdef: json_type_def(),
        }

        max_depth_array {
            args: func_args![ value: r#"[{"top_layer": {"next_layer": ["finish"]}}]"#, max_depth: 2],
            want: Ok(value!([{ top_layer: r#"{"next_layer": ["finish"]}"# }])),
            tdef: json_type_def(),
        }

        max_depth_exceeds_layers {
            args: func_args![ value: r#"{"top_layer": {"layer_one": "finish", "layer_two": 2}}"#, max_depth: 10],
            want: Ok(value!({ top_layer: {layer_one: "finish", layer_two: 2} })),
            tdef: json_type_def(),
        }

        invalid_json_with_max_depth {
            args: func_args![ value: r#"{"field": "value"#, max_depth: 3 ],
            want: Err("unable to read json: EOF while parsing a string at line 1 column 16"),
            tdef: json_type_def(),
        }

        invalid_input_max_depth {
            args: func_args![ value: r#"{"top_layer": "finish"}"#, max_depth: 129],
            want: Err("max_depth value should be greater than 0 and less than 128, got 129"),
            tdef: json_type_def(),
        }

        // // TODO: provide a function version of the `test_function!` macro.
        max_int {
            args: func_args![ value: format!("{{\"num\": {}}}", i64::MAX - 1)],
            want: Ok(value!({"num": 9_223_372_036_854_775_806_i64})),
            tdef: json_type_def(),
        }

        lossy_float_conversion {
            args: func_args![ value: r#"{"num": 9223372036854775808}"#],
            want: Ok(value!({"num": 9.223_372_036_854_776e18})),
            tdef: json_type_def(),
        }

        // Checks that the parsing uses the default lossy argument value
        parse_invalid_utf8_default_lossy_arg {
            // 0x22 is a quote character
            // 0xf5 is out of the range of valid UTF-8 bytes
            args: func_args![ value: Bytes::from_static(&[0x22,0xf5,0x22])],
            want: Ok(value!(std::char::REPLACEMENT_CHARACTER.to_string())),
            tdef: json_type_def(),
        }

        parse_invalid_utf8_lossy_arg_true {
            // 0xf5 is out of the range of valid UTF-8 bytes
            args: func_args![ value: Bytes::from_static(&[0x22,0xf5,0x22]), lossy: true],
            // U+FFFD is the replacement character for invalid UTF-8
            want: Ok(value!(std::char::REPLACEMENT_CHARACTER.to_string())),
            tdef: json_type_def(),
        }

        invalid_utf8_json_lossy_arg_false {
            args: func_args![ value: Bytes::from_static(&[0x22,0xf5,0x22]), lossy: false],
            want: Err("unable to parse json: invalid unicode code point at line 1 column 3"),
            tdef: json_type_def(),
        }
    ];

    #[cfg(not(feature = "float_roundtrip"))]
    test_function![
        parse_json => ParseJson;

        no_roundtrip_float_conversion {
            args: func_args![ value: r#"{"num": 1626175065.5934923}"#],
            want: Ok(value!({"num": 1_626_175_065.593_492_5})),
            tdef: json_type_def(),
        }
    ];

    #[cfg(feature = "float_roundtrip")]
    test_function![
        parse_json => ParseJson;

        roundtrip_float_conversion {
            args: func_args![ value: r#"{"num": 1626175065.5934923}"#],
            want: Ok(value!({"num": 1_626_175_065.593_492_3})),
            tdef: json_type_def(),
        }
    ];

    // Multi-line, concatenated, NDJSON, and slurp-mode behavior in one table.
    // For errors, the value is a substring the message must contain.
    #[test]
    fn parse_json_cases() {
        struct Case {
            name: &'static str,
            input: &'static str,
            slurp: bool,
            max_depth: Option<i64>,
            expected: Result<Value, &'static str>,
        }

        let cases: Vec<Case> = vec![
            // ----- strict mode: documents that parse cleanly -----
            Case { name: "multiline_object", slurp: false, max_depth: None,
                input: "{\n  \"field\": \"value\",\n  \"num\": 42\n}",
                expected: Ok(value!({ field: "value", num: 42 })) },
            Case { name: "multiline_array", slurp: false, max_depth: None,
                input: "[\n  1,\n  2,\n  3\n]",
                expected: Ok(value!([1, 2, 3])) },
            Case { name: "multiline_nested", slurp: false, max_depth: None,
                input: "{\n  \"arr\": [1, 2],\n  \"obj\": { \"x\": true }\n}",
                expected: Ok(value!({ arr: [1, 2], obj: { x: true } })) },
            Case { name: "leading_trailing_whitespace", slurp: false, max_depth: None,
                input: "  \n\t  {\"a\": 1}  \n  ",
                expected: Ok(value!({ a: 1 })) },
            Case { name: "escaped_newline_in_string", slurp: false, max_depth: None,
                input: r#"{"text":"line1\nline2"}"#,
                expected: Ok(value!({ text: "line1\nline2" })) },

            // ----- strict mode: rejected (multiple values or invalid) -----
            Case { name: "concatenated_no_separator", slurp: false, max_depth: None,
                input: r#"{"a":1}{"b":2}"#,
                expected: Err("trailing characters at line 1 column 8") },
            Case { name: "ndjson_default", slurp: false, max_depth: None,
                input: "{\"a\":1}\n{\"b\":2}\n{\"c\":3}\n",
                expected: Err("trailing characters at line 2 column 1") },
            Case { name: "literal_newline_in_string", slurp: false, max_depth: None,
                input: "{\"text\":\"line1\nline2\"}",
                expected: Err("control character (\\u0000-\\u001F)") },
            Case { name: "empty_input", slurp: false, max_depth: None,
                input: "",
                expected: Err("EOF while parsing a value at line 1 column 0") },
            Case { name: "whitespace_only", slurp: false, max_depth: None,
                input: "  \n  \t  ",
                expected: Err("EOF while parsing a value at line 2 column 5") },

            // ----- slurp: multi-value inputs collapsed to an array -----
            Case { name: "slurp_concatenated", slurp: true, max_depth: None,
                input: r#"{"a":1}{"b":2}"#,
                expected: Ok(value!([{ a: 1 }, { b: 2 }])) },
            Case { name: "slurp_ndjson", slurp: true, max_depth: None,
                input: "{\"a\":1}\n{\"b\":2}\n{\"c\":3}\n",
                expected: Ok(value!([{ a: 1 }, { b: 2 }, { c: 3 }])) },
            Case { name: "slurp_mixed_values", slurp: true, max_depth: None,
                input: r#"{"a":1}[1,2]"hello"42"#,
                expected: Ok(value!([{ a: 1 }, [1, 2], "hello", 42])) },

            // ----- slurp: single value passes through unwrapped (incl. arrays) -----
            Case { name: "slurp_single_object_unwrapped", slurp: true, max_depth: None,
                input: r#"{"a":1}"#,
                expected: Ok(value!({ a: 1 })) },
            Case { name: "slurp_single_array_unwrapped", slurp: true, max_depth: None,
                input: r#"[1,2,3]"#,
                expected: Ok(value!([1, 2, 3])) },

            // ----- slurp: errors -----
            Case { name: "slurp_trailing_garbage", slurp: true, max_depth: None,
                input: r#"{"a":1} not_json"#,
                expected: Err("expected ident at line 1 column 10") },
            Case { name: "slurp_empty", slurp: true, max_depth: None,
                input: "",
                expected: Err("input contains no JSON values") },
            Case { name: "slurp_whitespace_only", slurp: true, max_depth: None,
                input: "  \n\t  ",
                expected: Err("input contains no JSON values") },
            Case { name: "slurp_false_is_strict", slurp: false, max_depth: None,
                input: r#"{"a":1}{"b":2}"#,
                expected: Err("trailing characters at line 1 column 8") },

            // ----- slurp + max_depth (each value depth-limited individually) -----
            Case { name: "slurp_with_max_depth", slurp: true, max_depth: Some(1),
                input: r#"{"top":{"inner":1}}{"top":{"inner":2}}"#,
                expected: Ok(value!([
                    { top: r#"{"inner":1}"# },
                    { top: r#"{"inner":2}"# },
                ])) },
            Case { name: "slurp_with_max_depth_single", slurp: true, max_depth: Some(1),
                input: r#"{"top":{"inner":1}}"#,
                expected: Ok(value!({ top: r#"{"inner":1}"# })) },
        ];

        for c in cases {
            let value = Value::from(c.input);
            let slurp = c.slurp.then(|| Value::from(true));
            let result = match c.max_depth {
                Some(d) => parse_json_with_depth(value, Value::from(d), None, slurp),
                None => parse_json(value, None, slurp),
            };
            match (c.expected, result) {
                (Ok(want), Ok(got)) => assert_eq!(
                    got, want,
                    "case `{}`: value mismatch", c.name,
                ),
                (Err(want_substr), Err(got)) => {
                    let got_msg = got.to_string();
                    assert!(
                        got_msg.contains(want_substr),
                        "case `{}`: error mismatch\n  want substring: {:?}\n  got: {:?}",
                        c.name, want_substr, got_msg,
                    );
                }
                (Ok(want), Err(got)) => panic!(
                    "case `{}`: expected Ok({want:?}), got Err({got})", c.name,
                ),
                (Err(want), Ok(got)) => panic!(
                    "case `{}`: expected Err containing {want:?}, got Ok({got:?})", c.name,
                ),
            }
        }
    }
}
