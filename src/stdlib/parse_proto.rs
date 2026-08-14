use crate::compiler::prelude::*;
use crate::protobuf::get_message_descriptor;
use crate::protobuf::parse_proto;
use crate::stdlib::json_utils::json_type_def::json_type_def;
use once_cell::sync::Lazy;
use prost_reflect::MessageDescriptor;
use std::env;
use std::ffi::OsString;
use std::path::Path;
use std::path::PathBuf;

#[derive(Clone, Copy, Debug)]
pub struct ParseProto;

// This needs to be static because parse_proto needs to read a file
// and the file path needs to be a literal.
static EXAMPLE_PARSE_PROTO_EXPR: Lazy<&str> = Lazy::new(|| {
    let path = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").unwrap())
        .join("tests/data/protobuf/test_protobuf.desc")
        .display()
        .to_string();

    Box::leak(
        format!(
            r#"parse_proto!(decode_base64!("Cgdzb21lb25lIggKBjEyMzQ1Ng=="), "{path}", "test_protobuf.Person")"#
        )
        .into_boxed_str(),
    )
});

static EXAMPLES: Lazy<Vec<Example>> = Lazy::new(|| {
    vec![Example {
        title: "message",
        source: &EXAMPLE_PARSE_PROTO_EXPR,
        result: Ok(r#"{ "name": "someone", "phones": [{"number": "123456"}] }"#),
    }]
});

impl Function for ParseProto {
    fn identifier(&self) -> &'static str {
        "parse_proto"
    }

    fn summary(&self) -> &'static str {
        "parse a string to a protobuf based type"
    }

    fn usage(&self) -> &'static str {
        indoc! {"
            Parses the provided `value` as protocol buffer.
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
                keyword: "desc_file",
                kind: kind::BYTES,
                required: true,
            },
            Parameter {
                keyword: "message_type",
                kind: kind::BYTES,
                required: true,
            },
        ]
    }

    fn examples(&self) -> &'static [Example] {
        EXAMPLES.as_slice()
    }

    fn compile(
        &self,
        state: &state::TypeState,
        _ctx: &mut FunctionCompileContext,
        arguments: ArgumentList,
    ) -> Compiled {
        let value = arguments.required("value");
        let desc_file = arguments.required_literal("desc_file", state)?;
        // OBE-10728: every failure below must become a compile diagnostic. These
        // were `expect`s, so any unreadable/undecodable `desc_file` — a path the
        // program author fully controls — aborted the whole host process during
        // compilation rather than failing this one program.
        let desc_file_str = desc_file
            .try_bytes_utf8_lossy()
            .map_err(|e| Box::new(e) as Box<dyn DiagnosticMessage>)?;
        let message_type = arguments.required_literal("message_type", state)?;
        let message_type_str = message_type
            .try_bytes_utf8_lossy()
            .map_err(|e| Box::new(e) as Box<dyn DiagnosticMessage>)?;
        let os_string: OsString = desc_file_str.into_owned().into();
        let path_buf = PathBuf::from(os_string);
        let path = Path::new(&path_buf);
        let descriptor = get_message_descriptor(path, &message_type_str)
            .map_err(|e| Box::new(ExpressionError::from(e)) as Box<dyn DiagnosticMessage>)?;

        Ok(ParseProtoFn { descriptor, value }.as_expr())
    }
}

#[derive(Debug, Clone)]
struct ParseProtoFn {
    descriptor: MessageDescriptor,
    value: Box<dyn Expression>,
}

impl FunctionExpression for ParseProtoFn {
    fn resolve(&self, ctx: &mut Context) -> Resolved {
        let value = self.value.resolve(ctx)?;
        parse_proto(&self.descriptor, value)
    }

    fn type_def(&self, _: &state::TypeState) -> TypeDef {
        json_type_def()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::value;
    use std::fs;

    fn test_data_dir() -> PathBuf {
        PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").unwrap()).join("tests/data/protobuf")
    }

    fn read_pb_file(protobuf_bin_message_path: &str) -> String {
        fs::read_to_string(test_data_dir().join(protobuf_bin_message_path)).unwrap()
    }

    test_function![
        parse_proto => ParseProto;

        parses {
            args: func_args![ value: read_pb_file("person_someone.pb"),
                desc_file: test_data_dir().join("test_protobuf.desc").to_str().unwrap().to_owned(),
                message_type: "test_protobuf.Person"],
            want: Ok(value!({ name: "someone", phones: [{number: "123456"}] })),
            tdef: json_type_def(),
        }

        parses_proto3 {
            args: func_args![ value: read_pb_file("person_someone3.pb"),
                desc_file: test_data_dir().join("test_protobuf3.desc").to_str().unwrap().to_owned(),
                message_type: "test_protobuf3.Person"],
            want: Ok(value!({ data: {data_phone: "HOME"}, name: "someone", phones: [{number: "1234", type: "MOBILE"}] })),
            tdef: json_type_def(),
        }
    ];

    // OBE-10728: an unreadable/undecodable `desc_file` must fail compilation with
    // a diagnostic. This used to be `.expect(..)`, which aborted the entire host
    // process during compilation — and `desc_file` is fully controlled by the
    // program author, so one short program took down the worker.
    fn compile_with_desc_file(desc_file: &str) -> Result<(), String> {
        let state = state::TypeState::default();
        let mut ctx = FunctionCompileContext::new(
            crate::diagnostic::Span::new(0, 0),
            crate::compiler::CompileConfig::default(),
        );
        let args = func_args![
            value: "",
            desc_file: desc_file.to_owned(),
            message_type: "X"
        ];
        ParseProto
            .compile(&state, &mut ctx, args.into())
            .map(|_| ())
            .map_err(|e| e.message())
    }

    #[test]
    fn missing_desc_file_fails_to_compile_without_panicking() {
        let err = compile_with_desc_file("/nonexistent-descriptor-set")
            .expect_err("a missing desc_file must fail to compile");
        assert!(
            err.contains("Failed to open protobuf desc file"),
            "unexpected diagnostic: {err}"
        );
    }

    // Previously this read forever instead of failing.
    #[cfg(unix)]
    #[test]
    fn character_device_desc_file_fails_to_compile_without_hanging() {
        let err = compile_with_desc_file("/dev/zero")
            .expect_err("/dev/zero must fail to compile");
        assert!(
            err.contains("is not a regular file"),
            "unexpected diagnostic: {err}"
        );
    }
}
