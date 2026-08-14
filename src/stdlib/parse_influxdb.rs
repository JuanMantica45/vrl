use std::collections::BTreeMap;

use chrono::DateTime;
use influxdb_line_protocol::{FieldValue, ParsedLine};

use crate::compiler::prelude::*;
use crate::{btreemap, value};

/// Limits that bound how much memory `parse_influxdb` can be made to allocate
/// (OBE-10729).
///
/// Every field in a line becomes its own metric object, and each of those
/// carries a full deep copy of that line's tag set, so the output grows as
/// `tags × fields`. Measured before this cap, ~11 KiB of input (800 tags and
/// 800 fields) expanded to ~106 MiB resident — roughly 8,700x — and the ticket's
/// larger payload OOM-kills a 16 GiB host.
const MAX_TAGS_PER_SERIES: usize = 256;
const MAX_FIELDS_PER_LINE: usize = 1024;

/// Budget on tag copies for the whole call, not just one line.
///
/// The per-line caps above are on their own insufficient: one call parses many
/// lines and flattens them together, so an input made of many lines that each
/// sit just under those caps would still multiply out to the same blow-up.
const MAX_TOTAL_TAG_ENTRIES: usize = 262_144;

fn influxdb_line_to_metrics(
    line: ParsedLine,
    tag_entry_budget: &mut usize,
) -> Result<Vec<ObjectMap>, ExpressionError> {
    let ParsedLine {
        series,
        field_set,
        timestamp,
    } = line;

    let timestamp = timestamp.map(DateTime::from_timestamp_nanos);

    // Reject before building anything, so an over-limit line costs nothing.
    let tag_count = match series.tag_set.as_ref() {
        Some(tags) => tags.len(),
        None => 0,
    };
    if tag_count > MAX_TAGS_PER_SERIES {
        return Err(Error::TooManyTags.into());
    }
    if field_set.len() > MAX_FIELDS_PER_LINE {
        return Err(Error::TooManyFields.into());
    }

    // Charge this line's tag copies (one per field) against the call budget.
    let tag_entries = tag_count.saturating_mul(field_set.len());
    if tag_entries > *tag_entry_budget {
        return Err(Error::TagEntryBudgetExceeded.into());
    }
    *tag_entry_budget -= tag_entries;

    let tags: Option<ObjectMap> = series.tag_set.as_ref().map(|tags| {
        tags.iter()
            .map(|t| (t.0.to_string().into(), t.1.to_string().into()))
            .collect()
    });

    field_set
        .into_iter()
        .map(|f| {
            let mut metric = ObjectMap::new();
            let measurement = &series.measurement;
            let field_key = f.0.to_string();
            let field_value = match f.1 {
                FieldValue::I64(v) => v as f64,
                FieldValue::U64(v) => v as f64,
                FieldValue::F64(v) => v,
                FieldValue::Boolean(v) => {
                    if v {
                        1.0
                    } else {
                        0.0
                    }
                }
                FieldValue::String(_) => {
                    return Err(Error::StringFieldSetValuesNotSupported.into());
                }
            };

            // `influxdb_line_protocol` crate seems to not allow NaN float values while parsing
            // field values and this case should not happen, but just in case, we should
            // handle it.
            let Ok(field_value) = NotNan::new(field_value) else {
                return Err(Error::NaNFieldSetValuesNotSupported.into());
            };

            let metric_name = format!("{measurement}_{field_key}");
            metric.insert("name".into(), metric_name.into());

            if let Some(tags) = tags.as_ref() {
                metric.insert("tags".into(), tags.clone().into());
            }

            if let Some(timestamp) = timestamp {
                metric.insert("timestamp".into(), timestamp.into());
            }

            metric.insert("kind".into(), "absolute".into());

            let gauge_object = value!({
                value: field_value
            });
            metric.insert("gauge".into(), gauge_object);

            Ok(metric)
        })
        .collect()
}

#[derive(Debug, Clone, thiserror::Error)]
enum Error {
    #[error("field set values of type string are not supported")]
    StringFieldSetValuesNotSupported,
    #[error("NaN field set values are not supported")]
    NaNFieldSetValuesNotSupported,
    #[error("too many tags in a single line (limit {MAX_TAGS_PER_SERIES})")]
    TooManyTags,
    #[error("too many fields in a single line (limit {MAX_FIELDS_PER_LINE})")]
    TooManyFields,
    #[error("input expands to too many tag entries (limit {MAX_TOTAL_TAG_ENTRIES})")]
    TagEntryBudgetExceeded,
}

impl From<Error> for ExpressionError {
    fn from(error: Error) -> Self {
        Self::Error {
            message: format!("Error while converting InfluxDB line protocol metric to Vector's metric model: {error}"),
            labels: vec![],
            notes: vec![],
        }
    }
}

fn parse_influxdb(bytes: Value) -> Resolved {
    let bytes = bytes.try_bytes()?;
    let line = String::from_utf8_lossy(&bytes);
    let parsed_line = influxdb_line_protocol::parse_lines(&line);

    // One budget shared by every line in this call (OBE-10729).
    let mut tag_entry_budget = MAX_TOTAL_TAG_ENTRIES;

    let metrics = parsed_line
        .into_iter()
        .map(|line_result| line_result.map_err(ExpressionError::from))
        .map(|line_result| {
            line_result.and_then(|line| influxdb_line_to_metrics(line, &mut tag_entry_budget))
        })
        .collect::<Result<Vec<_>, _>>()?
        .into_iter()
        .flatten()
        .map(Value::from)
        .collect();

    Ok(Value::Array(metrics))
}

impl From<influxdb_line_protocol::Error> for ExpressionError {
    fn from(error: influxdb_line_protocol::Error) -> Self {
        Self::Error {
            message: format!("InfluxDB line protocol parsing error: {error}"),
            labels: vec![],
            notes: vec![],
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct ParseInfluxDB;

impl Function for ParseInfluxDB {
    fn identifier(&self) -> &'static str {
        "parse_influxdb"
    }

    fn summary(&self) -> &'static str {
        "parse an InfluxDB line protocol string into a list of vector-compatible metrics"
    }

    fn parameters(&self) -> &'static [Parameter] {
        &[Parameter {
            keyword: "value",
            kind: kind::BYTES,
            required: true,
        }]
    }

    fn examples(&self) -> &'static [Example] {
        &[Example {
            title: "parse influxdb line protocol",
            source: r#"parse_influxdb!("cpu,host=A,region=us-west usage_system=64i,usage_user=10u,temperature=50.5,on=true,sleep=false 1590488773254420000")"#,
            result: Ok(indoc! {r#"
                [
                    {
                        "name": "cpu_usage_system",
                        "tags": {
                            "host": "A",
                            "region": "us-west"
                        },
                        "timestamp": "2020-05-26T10:26:13.254420Z",
                        "kind": "absolute",
                        "gauge": {
                            "value": 64.0
                        }
                    },
                    {
                        "name": "cpu_usage_user",
                        "tags": {
                            "host": "A",
                            "region": "us-west"
                        },
                        "timestamp": "2020-05-26T10:26:13.254420Z",
                        "kind": "absolute",
                        "gauge": {
                            "value": 10.0
                        }
                    },
                    {
                        "name": "cpu_temperature",
                        "tags": {
                            "host": "A",
                            "region": "us-west"
                        },
                        "timestamp": "2020-05-26T10:26:13.254420Z",
                        "kind": "absolute",
                        "gauge": {
                            "value": 50.5
                        }
                    },
                    {
                        "name": "cpu_on",
                        "tags": {
                            "host": "A",
                            "region": "us-west"
                        },
                        "timestamp": "2020-05-26T10:26:13.254420Z",
                        "kind": "absolute",
                        "gauge": {
                            "value": 1.0
                        }
                    },
                    {
                        "name": "cpu_sleep",
                        "tags": {
                            "host": "A",
                            "region": "us-west"
                        },
                        "timestamp": "2020-05-26T10:26:13.254420Z",
                        "kind": "absolute",
                        "gauge": {
                            "value": 0.0
                        }
                    }
                ]
            "#}),
        }]
    }

    fn compile(
        &self,
        _state: &state::TypeState,
        _ctx: &mut FunctionCompileContext,
        arguments: ArgumentList,
    ) -> Compiled {
        let value = arguments.required("value");

        Ok(ParseInfluxDBFn { value }.as_expr())
    }
}

#[derive(Clone, Debug)]
struct ParseInfluxDBFn {
    value: Box<dyn Expression>,
}

impl FunctionExpression for ParseInfluxDBFn {
    fn resolve(&self, ctx: &mut Context) -> Resolved {
        let value = self.value.resolve(ctx)?;

        parse_influxdb(value)
    }

    fn type_def(&self, _: &state::TypeState) -> TypeDef {
        type_def()
    }
}

fn tags_kind() -> Kind {
    Kind::object(Collection::from_unknown(Kind::bytes())) | Kind::null()
}

fn gauge_kind() -> Kind {
    Kind::object(btreemap! {
        "value" => Kind::float(),
    })
}

fn metric_kind() -> BTreeMap<Field, Kind> {
    btreemap! {
        "name" => Kind::bytes(),
        "tags" => tags_kind(),
        "timestamp" => Kind::timestamp() | Kind::null(),
        "kind" => Kind::bytes(),
        "gauge" => gauge_kind(),
    }
}

fn inner_kind() -> Kind {
    Kind::object(metric_kind())
}

fn type_def() -> TypeDef {
    TypeDef::array(Collection::from_unknown(inner_kind())).fallible()
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::btreemap;

    test_function![
        parse_influxdb => ParseInfluxDB;

        influxdb_valid {
            args: func_args![ value: "cpu,host=A,region=us-west usage_system=64i,usage_user=10u,temperature=50.5,on=true,sleep=false 1590488773254420000" ],
            want: Ok(Value::from(vec![
                Value::from(btreemap! {
                    "name" => "cpu_usage_system",
                    "tags" => btreemap! {
                        "host" => "A",
                        "region" => "us-west",
                    },
                    "timestamp" => DateTime::from_timestamp_nanos(1_590_488_773_254_420_000),
                    "kind" => "absolute",
                    "gauge" => btreemap! {
                        "value" => 64.0,
                    },
                }),
                Value::from(btreemap! {
                    "name" => "cpu_usage_user",
                    "tags" => btreemap! {
                        "host" => "A",
                        "region" => "us-west",
                    },
                    "timestamp" => DateTime::from_timestamp_nanos(1_590_488_773_254_420_000),
                    "kind" => "absolute",
                    "gauge" => btreemap! {
                        "value" => 10.0,
                    },
                }),
                Value::from(btreemap! {
                    "name" => "cpu_temperature",
                    "tags" => btreemap! {
                        "host" => "A",
                        "region" => "us-west",
                    },
                    "timestamp" => DateTime::from_timestamp_nanos(1_590_488_773_254_420_000),
                    "kind" => "absolute",
                    "gauge" => btreemap! {
                        "value" => 50.5,
                    },
                }),
                Value::from(btreemap! {
                    "name" => "cpu_on",
                    "tags" => btreemap! {
                        "host" => "A",
                        "region" => "us-west",
                    },
                    "timestamp" => DateTime::from_timestamp_nanos(1_590_488_773_254_420_000),
                    "kind" => "absolute",
                    "gauge" => btreemap! {
                        "value" => 1.0,
                    },
                }),
                Value::from(btreemap! {
                    "name" => "cpu_sleep",
                    "tags" => btreemap! {
                        "host" => "A",
                        "region" => "us-west",
                    },
                    "timestamp" => DateTime::from_timestamp_nanos(1_590_488_773_254_420_000),
                    "kind" => "absolute",
                    "gauge" => btreemap! {
                        "value" => 0.0,
                    },
                }),
            ])),
            tdef: type_def(),
        }


        influxdb_valid_no_timestamp {
            args: func_args![ value: "cpu,host=A,region=us-west usage_system=64i,usage_user=10i" ],
            want: Ok(Value::from(vec![
                Value::from(btreemap! {
                    "name" => "cpu_usage_system",
                    "tags" => btreemap! {
                        "host" => "A",
                        "region" => "us-west",
                    },
                    "kind" => "absolute",
                    "gauge" => btreemap! {
                        "value" => 64.0,
                    },
                }),
                Value::from(btreemap! {
                    "name" => "cpu_usage_user",
                    "tags" => btreemap! {
                        "host" => "A",
                        "region" => "us-west",
                    },
                    "kind" => "absolute",
                    "gauge" => btreemap! {
                        "value" => 10.0,
                    },
                }),
            ])),
            tdef: type_def(),
        }

        influxdb_valid_no_tags {
            args: func_args![ value: "cpu usage_system=64i,usage_user=10i 1590488773254420000" ],
            want: Ok(Value::from(vec![
                Value::from(btreemap! {
                    "name" => "cpu_usage_system",
                    "timestamp" => DateTime::from_timestamp_nanos(1_590_488_773_254_420_000),
                    "kind" => "absolute",
                    "gauge" => btreemap! {
                        "value" => 64.0,
                    },
                }),
                Value::from(btreemap! {
                    "name" => "cpu_usage_user",
                    "timestamp" => DateTime::from_timestamp_nanos(1_590_488_773_254_420_000),
                    "kind" => "absolute",
                    "gauge" => btreemap! {
                        "value" => 10.0,
                    },
                }),
            ])),
            tdef: type_def(),
        }

        influxdb_valid_no_tags_no_timestamp {
            args: func_args![ value: "cpu usage_system=64i,usage_user=10i" ],
            want: Ok(Value::from(vec![
                Value::from(btreemap! {
                    "name" => "cpu_usage_system",
                    "kind" => "absolute",
                    "gauge" => btreemap! {
                        "value" => 64.0,
                    },
                }),
                Value::from(btreemap! {
                    "name" => "cpu_usage_user",
                    "kind" => "absolute",
                    "gauge" => btreemap! {
                        "value" => 10.0,
                    },
                }),
            ])),
            tdef: type_def(),
        }

        influxdb_invalid_string_field_set_value {
            args: func_args![ value: r#"valid foo="bar""# ],
            want: Err("Error while converting InfluxDB line protocol metric to Vector's metric model: field set values of type string are not supported"),
            tdef: type_def(),
        }

        influxdb_invalid_no_fields{
            args: func_args![ value: "cpu " ],
            want: Err("InfluxDB line protocol parsing error: No fields were provided"),
            tdef: type_def(),
        }

        // OBE-10729: every field in a line gets a full copy of that line's tag
        // set, so `tags x fields` output growth turned ~11 KiB of input into
        // ~106 MiB resident. These three cases cover the shapes that exploit it.
        influxdb_too_many_tags_rejected {
            args: func_args![ value: influx_line(MAX_TAGS_PER_SERIES + 1, 8) ],
            want: Err("Error while converting InfluxDB line protocol metric to Vector's metric model: too many tags in a single line (limit 256)"),
            tdef: type_def(),
        }

        influxdb_too_many_fields_rejected {
            args: func_args![ value: influx_line(2, MAX_FIELDS_PER_LINE + 1) ],
            want: Err("Error while converting InfluxDB line protocol metric to Vector's metric model: too many fields in a single line (limit 1024)"),
            tdef: type_def(),
        }

        // Each line here is individually within both per-line caps, so only the
        // call-wide budget stops it. Without that budget this input would
        // materialize 40 x 250 x 1000 = 10M tag entries.
        influxdb_many_lines_under_per_line_caps_rejected {
            args: func_args![ value: (0..40)
                .map(|i| format!("m{i},{} {}",
                    (0..250).map(|t| format!("a{t}=0")).collect::<Vec<_>>().join(","),
                    (0..1000).map(|f| format!("f{f}=0")).collect::<Vec<_>>().join(",")))
                .collect::<Vec<_>>()
                .join("\n") ],
            want: Err("Error while converting InfluxDB line protocol metric to Vector's metric model: input expands to too many tag entries (limit 262144)"),
            tdef: type_def(),
        }

    ];

    /// Build an InfluxDB line with `tags` tag pairs and `fields` field pairs.
    fn influx_line(tags: usize, fields: usize) -> String {
        let tag_set = (0..tags)
            .map(|i| format!("a{i}=0"))
            .collect::<Vec<_>>()
            .join(",");
        let field_set = (0..fields)
            .map(|i| format!("f{i}=0"))
            .collect::<Vec<_>>()
            .join(",");
        if tag_set.is_empty() {
            format!("m {field_set}")
        } else {
            format!("m,{tag_set} {field_set}")
        }
    }

    // OBE-10729: the limits must hold the output bounded rather than merely
    // rejecting one hand-picked payload. Drive the worst legal shape and assert
    // the number of materialized tag entries stays within the budget.
    #[test]
    fn tag_entry_output_is_bounded_by_the_budget() {
        // 40 lines x 250 tags x 1000 fields — each line legal, total over budget.
        let input = (0..40)
            .map(|i| format!("m{i},{} {}",
                (0..250).map(|t| format!("a{t}=0")).collect::<Vec<_>>().join(","),
                (0..1000).map(|f| format!("f{f}=0")).collect::<Vec<_>>().join(",")))
            .collect::<Vec<_>>()
            .join("\n");

        let err = parse_influxdb(Value::from(input))
            .expect_err("input exceeding the tag-entry budget must be rejected");
        assert!(
            err.to_string().contains("too many tag entries"),
            "expected the call-wide budget to reject this, got: {err}"
        );

        // And a shape that fits the budget must still succeed, proving the cap
        // rejects on total cost rather than on line count alone.
        let ok_input = (0..40)
            .map(|i| format!("m{i},a0=0,a1=0 f0=0,f1=0"))
            .collect::<Vec<_>>()
            .join("\n");
        let parsed = parse_influxdb(Value::from(ok_input)).expect("within budget must parse");
        let Value::Array(metrics) = parsed else {
            panic!("expected an array of metrics")
        };
        assert_eq!(metrics.len(), 80, "40 lines x 2 fields");
    }

    // A line sitting exactly on both per-line limits is legal and must still
    // parse — the caps must not be off by one.
    #[test]
    fn line_at_per_line_limits_is_accepted() {
        let at_tag_limit = parse_influxdb(Value::from(influx_line(MAX_TAGS_PER_SERIES, 1)))
            .expect("a line at exactly MAX_TAGS_PER_SERIES must parse");
        let Value::Array(metrics) = at_tag_limit else {
            panic!("expected an array")
        };
        assert_eq!(metrics.len(), 1, "one field yields one metric");
        let Some(Value::Object(tags)) = metrics[0].as_object().and_then(|m| m.get("tags")).cloned()
        else {
            panic!("expected a tags object")
        };
        assert_eq!(tags.len(), MAX_TAGS_PER_SERIES, "all tags preserved");

        parse_influxdb(Value::from(influx_line(1, MAX_FIELDS_PER_LINE)))
            .expect("a line at exactly MAX_FIELDS_PER_LINE must parse");
    }
}
