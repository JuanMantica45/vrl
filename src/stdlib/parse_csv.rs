use crate::compiler::prelude::*;
use csv_core::{ReadRecordResult, Reader as CsvReader, ReaderBuilder};
use std::cell::RefCell;

thread_local! {
    // Building a csv_core DFA dominates the cost of parsing one short row, so we
    // cache one reader per delimiter and reuse it (reset) across calls.
    static READERS: RefCell<Vec<(u8, CsvReader)>> = const { RefCell::new(Vec::new()) };
}

/// Parse the first CSV record of `input` into its byte fields.
///
/// We drive `csv_core` directly so the DFA can be reused across calls and the
/// input slice fed without a `BufReader` copy. The read loop below mirrors the
/// `csv` crate's `Reader::read_byte_record_impl` (same `read_record` cursor /
/// `OutputFull` / `OutputEndsFull` accumulator pattern) — the reference to
/// check this against:
///   - csv crate loop: <https://github.com/BurntSushi/rust-csv/blob/1.3.1/src/reader.rs#L1619> (`read_byte_record_impl`)
///   - csv_core primitive: <https://docs.rs/csv-core/0.1.11/csv_core/struct.Reader.html#method.read_record>
///
/// `csv_core` never errors — it always prefers *a* parse over none.
fn parse_first_record(input: &[u8], delimiter: u8) -> Vec<Value> {
    READERS.with_borrow_mut(|readers| {
        let reader = match readers.iter().position(|(d, _)| *d == delimiter) {
            Some(i) => &mut readers[i].1,
            None => {
                readers.push((delimiter, ReaderBuilder::new().delimiter(delimiter).build()));
                &mut readers.last_mut().unwrap().1
            }
        };
        reader.reset();

        // Unescaping never lengthens a field, so the record fits in `input.len()`.
        let mut output = vec![0u8; input.len()];
        let mut ends = vec![0usize; 64];
        let (mut nin, mut nout, mut nend) = (0usize, 0usize, 0usize);
        loop {
            let (result, in_read, out_written, ends_written) =
                reader.read_record(&input[nin..], &mut output[nout..], &mut ends[nend..]);
            nin += in_read;
            nout += out_written;
            nend += ends_written;
            match result {
                ReadRecordResult::Record | ReadRecordResult::End => break,
                // Final field had no trailing terminator; flush it with empty input.
                ReadRecordResult::InputEmpty => {}
                ReadRecordResult::OutputFull => output.resize(output.len() * 2, 0),
                ReadRecordResult::OutputEndsFull => ends.resize(ends.len() * 2, 0),
            }
        }

        let mut fields = Vec::with_capacity(nend);
        let mut start = 0;
        for &end in &ends[..nend] {
            fields.push(Bytes::copy_from_slice(&output[start..end]).into());
            start = end;
        }
        fields
    })
}

fn parse_csv(csv_string: Value, delimiter: Value) -> Resolved {
    let csv_string = csv_string.try_bytes()?;
    let delimiter = delimiter.try_bytes()?;
    if delimiter.len() != 1 {
        return Err("delimiter must be a single character".into());
    }
    let delimiter = delimiter[0];

    Ok(parse_first_record(&csv_string, delimiter).into())
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

    /// Parse the first record exactly as the previous `csv`-crate implementation
    /// did, to use as a differential oracle against our `csv_core` parser.
    fn csv_crate_oracle(input: &[u8], delimiter: u8) -> Vec<Value> {
        let mut reader = csv::ReaderBuilder::new()
            .has_headers(false)
            .delimiter(delimiter)
            .flexible(true)
            .from_reader(std::io::Cursor::new(input));
        let mut record = csv::ByteRecord::new();
        match reader.read_byte_record(&mut record) {
            Ok(true) => record
                .iter()
                .map(|f| Bytes::copy_from_slice(f).into())
                .collect(),
            _ => Vec::new(),
        }
    }

    #[test]
    fn matches_csv_crate_on_edge_cases() {
        let cases: &[(&[u8], u8)] = &[
            (b"", b','),
            (b"foo", b','),
            (b"foo,bar", b','),
            (b"foo,bar\n", b','),
            (b"foo,bar\r\n", b','),
            (b"a,b\nc,d", b','),                  // only first record
            (b"\n", b','),                        // empty line
            (b"\r\n", b','),
            (b",,,", b','),                       // empty fields
            (b"a,b,", b','),                      // trailing empty field
            (b"\"\"", b','),                      // empty quoted field
            (b"\"\"\"\"", b','),                  // escaped quote only
            (b"\"a,b\",c", b','),                 // quoted comma
            (b"\"a\"\"b\",c", b','),              // escaped quote inside field
            (b"a\"b,c", b','),                    // lenient: quote mid-field
            (b"\"unterminated,c", b','),          // unterminated quote
            (b"\xEF\xBB\xBFfoo,bar", b','),       // leading UTF-8 BOM
            (b"foo,b\xFFar", b','),               // invalid UTF-8 bytes
            (b" a , b ", b','),                   // surrounding whitespace
            (b"a,\"b\nc\",d", b','),              // newline inside quotes
            (b"a\rb", b','),                      // bare carriage return
            (b"x\ty\tz", b'\t'),                  // tab delimiter
            (b"x|y|z", b'|'),                     // pipe delimiter
            (b"\"a\tb\",c", b'\t'),               // quoted tab with tab delimiter
        ];
        // Driven through the shared thread-local reader cache (including
        // delimiter switches), so any stale state carried across calls by
        // `reset()` or the per-delimiter cache would be caught here.
        for &(input, delimiter) in cases {
            let ours = parse_first_record(input, delimiter);
            let oracle = csv_crate_oracle(input, delimiter);
            assert_eq!(
                ours,
                oracle,
                "mismatch for input {:?} (delimiter {:?})",
                String::from_utf8_lossy(input),
                delimiter as char,
            );
        }
    }

    // The loop's only exit is `Record | End`. These cases never take that arm on
    // the first `read_record`, exercising the `continue` arms instead, and prove
    // the loop still terminates correctly.

    #[test]
    fn unterminated_record_exits_via_input_empty_then_record() {
        // No trailing terminator: first read returns `InputEmpty` (input
        // exhausted mid-record); the next iteration feeds the now-empty slice,
        // which csv_core finalizes to `Record`.
        for input in [&b"a,b,c"[..], b"single", b"\"quoted\"", b"trailing,"] {
            assert_eq!(parse_first_record(input, b','), csv_crate_oracle(input, b','));
        }
    }

    #[test]
    fn many_fields_exit_via_ends_growth_then_record() {
        // `ends` starts at 64, so >64 fields force the `OutputEndsFull` arm: the
        // loop grows `ends` and continues, still leaving only via `Record`.
        for n in [1usize, 63, 64, 65, 200, 1000] {
            let input = (0..n)
                .map(|i| i.to_string())
                .collect::<Vec<_>>()
                .join(",")
                .into_bytes();
            let ours = parse_first_record(&input, b',');
            assert_eq!(ours.len(), n, "field count for n={n}");
            assert_eq!(ours, csv_crate_oracle(&input, b','), "mismatch for n={n}");
        }
    }

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
