use crate::compiler::prelude::*;
use std::io::Cursor;

fn parse_csv(csv_string: Value, delimiter: Value) -> Resolved {
    let csv_string = csv_string.try_bytes()?;
    let delimiter = delimiter.try_bytes()?;
    if delimiter.len() != 1 {
        return Err("delimiter must be a single character".into());
    }
    let delimiter = delimiter[0];

    let mut reader = csv::ReaderBuilder::new()
        .has_headers(false)
        .delimiter(delimiter)
        .flexible(true)
        .from_reader(Cursor::new(&*csv_string));

    let mut record = csv::ByteRecord::new();
    match reader.read_byte_record(&mut record) {
        Ok(true) => Ok(record
            .iter()
            .map(|field| Bytes::copy_from_slice(field).into())
            .collect::<Vec<Value>>()
            .into()),
        Ok(false) => Ok(Vec::<Value>::new().into()),
        Err(err) => Err(format!("invalid csv record: {err}").into()),
    }
}

#[derive(Clone, Copy, Debug)]
pub struct ParseCsv;

impl Function for ParseCsv {
    fn identifier(&self) -> &'static str {
        "parse_csv"
    }

    fn examples(&self) -> &'static [Example] {
        &[Example {
            title: "parse a single CSV formatted row",
            source: r#"parse_csv!(s'foo,bar,"foo "", bar"')"#,
            result: Ok(r#"["foo", "bar", "foo \", bar"]"#),
        }]
    }

    fn compile(
        &self,
        _state: &state::TypeState,
        _ctx: &mut FunctionCompileContext,
        arguments: ArgumentList,
    ) -> Compiled {
        let value = arguments.required("value");
        let delimiter = arguments.optional("delimiter").unwrap_or(expr!(","));
        Ok(ParseCsvFn { value, delimiter }.as_expr())
    }

    fn parameters(&self) -> &'static [Parameter] {
        &[
            Parameter {
                keyword: "value",
                kind: kind::BYTES,
                required: true,
            },
            Parameter {
                keyword: "delimiter",
                kind: kind::BYTES,
                required: false,
            },
        ]
    }
}

#[derive(Debug, Clone)]
struct ParseCsvFn {
    value: Box<dyn Expression>,
    delimiter: Box<dyn Expression>,
}

impl FunctionExpression for ParseCsvFn {
    fn resolve(&self, ctx: &mut Context) -> Resolved {
        let csv_string = self.value.resolve(ctx)?;
        let delimiter = self.delimiter.resolve(ctx)?;

        parse_csv(csv_string, delimiter)
    }

    fn type_def(&self, _: &state::TypeState) -> TypeDef {
        TypeDef::array(inner_kind()).fallible()
    }
}

#[inline]
fn inner_kind() -> Collection<Index> {
    let mut v = Collection::any();
    v.set_unknown(Kind::bytes());
    v
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::value;

    test_function![
        parse_csv => ParseCsv;

        valid {
            args: func_args![value: value!("foo,bar,\"foo \"\", bar\"")],
            want: Ok(value!(["foo", "bar", "foo \", bar"])),
            tdef: TypeDef::array(inner_kind()).fallible(),
        }

        invalid_utf8 {
            args: func_args![value: value!(Bytes::copy_from_slice(&b"foo,b\xFFar"[..]))],
            want: Ok(value!(vec!["foo".into(), value!(Bytes::copy_from_slice(&b"b\xFFar"[..]))])),
            tdef: TypeDef::array(inner_kind()).fallible(),
        }

        custom_delimiter {
            args: func_args![value: value!("foo bar"), delimiter: value!(" ")],
            want: Ok(value!(["foo", "bar"])),
            tdef: TypeDef::array(inner_kind()).fallible(),
        }

        invalid_delimiter {
            args: func_args![value: value!("foo bar"), delimiter: value!(",,")],
            want: Err("delimiter must be a single character"),
            tdef: TypeDef::array(inner_kind()).fallible(),
        }

        single_value {
            args: func_args![value: value!("foo")],
            want: Ok(value!(["foo"])),
            tdef: TypeDef::array(inner_kind()).fallible(),
        }

        empty_string {
            args: func_args![value: value!("")],
            want: Ok(value!([])),
            tdef: TypeDef::array(inner_kind()).fallible(),
        }

        multiple_lines {
            args: func_args![value: value!("first,line\nsecond,line,with,more,fields")],
            want: Ok(value!(["first", "line"])),
            tdef: TypeDef::array(inner_kind()).fallible(),
        }

        quoted_fields_with_commas {
           args: func_args![value: value!("\"field,with,commas\",normal,\"another,quoted\"")],
           want: Ok(value!(["field,with,commas", "normal", "another,quoted"])),
           tdef: TypeDef::array(inner_kind()).fallible(),
       }

       quoted_fields_with_quotes {
           args: func_args![value: value!("\"field with \"\"quotes\"\"\",normal")],
           want: Ok(value!(["field with \"quotes\"", "normal"])),
           tdef: TypeDef::array(inner_kind()).fallible(),
       }

       mixed_quoted_unquoted {
           args: func_args![value: value!("unquoted,\"quoted field\",another_unquoted")],
           want: Ok(value!(["unquoted", "quoted field", "another_unquoted"])),
           tdef: TypeDef::array(inner_kind()).fallible(),
       }

       empty_fields {
           args: func_args![value: value!("field1,,field3,")],
           want: Ok(value!(["field1", "", "field3", ""])),
           tdef: TypeDef::array(inner_kind()).fallible(),
       }

       quoted_empty_fields {
           args: func_args![value: value!("field1,\"\",field3")],
           want: Ok(value!(["field1", "", "field3"])),
           tdef: TypeDef::array(inner_kind()).fallible(),
       }

       whitespace_handling {
           args: func_args![value: value!(" field1 , field2 ,field3 ")],
           want: Ok(value!([" field1 ", " field2 ", "field3 "])),
           tdef: TypeDef::array(inner_kind()).fallible(),
       }

       quoted_whitespace {
           args: func_args![value: value!("\" field1 \",\"field2\",\" field3 \"")],
           want: Ok(value!([" field1 ", "field2", " field3 "])),
           tdef: TypeDef::array(inner_kind()).fallible(),
       }

       newlines_in_quoted_fields {
           args: func_args![value: value!("\"field\nwith\nnewlines\",normal")],
           want: Ok(value!(["field\nwith\nnewlines", "normal"])),
           tdef: TypeDef::array(inner_kind()).fallible(),
       }

       tab_delimiter {
           args: func_args![value: value!("field1\tfield2\tfield3"), delimiter: value!("\t")],
           want: Ok(value!(["field1", "field2", "field3"])),
           tdef: TypeDef::array(inner_kind()).fallible(),
       }

       pipe_delimiter {
           args: func_args![value: value!("field1|field2|field3"), delimiter: value!("|")],
           want: Ok(value!(["field1", "field2", "field3"])),
           tdef: TypeDef::array(inner_kind()).fallible(),
       }

       semicolon_delimiter {
           args: func_args![value: value!("field1;field2;field3"), delimiter: value!(";")],
           want: Ok(value!(["field1", "field2", "field3"])),
           tdef: TypeDef::array(inner_kind()).fallible(),
       }

       single_quote_field {
           args: func_args![value: value!("field1,'field2',field3")],
           want: Ok(value!(["field1", "'field2'", "field3"])),
           tdef: TypeDef::array(inner_kind()).fallible(),
       }

       numeric_looking_fields {
           args: func_args![value: value!("123,45.67,\"789\",0")],
           want: Ok(value!(["123", "45.67", "789", "0"])),
           tdef: TypeDef::array(inner_kind()).fallible(),
       }

       boolean_looking_fields {
           args: func_args![value: value!("true,false,TRUE,FALSE")],
           want: Ok(value!(["true", "false", "TRUE", "FALSE"])),
           tdef: TypeDef::array(inner_kind()).fallible(),
       }

       special_characters {
           args: func_args![value: value!("field@#$%,\"field^&*()\",field!~`")],
           want: Ok(value!(["field@#$%", "field^&*()", "field!~`"])),
           tdef: TypeDef::array(inner_kind()).fallible(),
       }

       unicode_characters {
           args: func_args![value: value!("café,naïve,\"résumé\",München")],
           want: Ok(value!(["café", "naïve", "résumé", "München"])),
           tdef: TypeDef::array(inner_kind()).fallible(),
       }


       malformed_quotes_unclosed {
           args: func_args![value: value!("field1,\"unclosed quote,field3")],
           want: Ok(value!(["field1", "unclosed quote,field3"])),
           tdef: TypeDef::array(inner_kind()).fallible(),
       }

       malformed_quotes_embedded {
           args: func_args![value: value!("field1,fie\"ld2,field3")],
           want: Ok(value!(["field1", "fie\"ld2", "field3"])),
           tdef: TypeDef::array(inner_kind()).fallible(),
       }

       empty_delimiter {
           args: func_args![value: value!("foo,bar"), delimiter: value!("")],
           want: Err("delimiter must be a single character"),
           tdef: TypeDef::array(inner_kind()).fallible(),
       }

       multi_byte_delimiter_attempt {
           args: func_args![value: value!("foo,bar"), delimiter: value!("🎵")],
           want: Err("delimiter must be a single character"),
           tdef: TypeDef::array(inner_kind()).fallible(),
       }

       carriage_return_handling {
           args: func_args![value: value!("field1,field2\r\nfield3,field4")],
           want: Ok(value!(["field1", "field2"])),
           tdef: TypeDef::array(inner_kind()).fallible(),
       }

       only_commas {
           args: func_args![value: value!(",,,")],
           want: Ok(value!(["", "", "", ""])),
           tdef: TypeDef::array(inner_kind()).fallible(),
       }

       only_quotes {
           args: func_args![value: value!("\"\"")],
           want: Ok(value!([""])),
           tdef: TypeDef::array(inner_kind()).fallible(),
       }

    ];
}
