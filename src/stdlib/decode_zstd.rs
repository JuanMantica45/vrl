use crate::compiler::prelude::*;
use crate::stdlib::util::{DECOMPRESS_LIMIT_ERROR, DEFAULT_DECOMPRESS_LIMIT};
use nom::AsBytes;
use std::io::Read;


fn decode_zstd(value: Value) -> Resolved {
    let value = value.try_bytes()?;
    let mut buf = Vec::new();
    zstd::Decoder::new(std::io::Cursor::new(value.as_bytes()))
        .map_err(|_| "unable to decode value with Zstd decoder")?
        .take(DEFAULT_DECOMPRESS_LIMIT + 1)
        .read_to_end(&mut buf)
        .map_err(|_| "unable to decode value with Zstd decoder")?;
    if buf.len() as u64 > DEFAULT_DECOMPRESS_LIMIT {
        return Err(DECOMPRESS_LIMIT_ERROR.into());
    }
    Ok(Value::Bytes(buf.into()))
}

#[derive(Clone, Copy, Debug)]
pub struct DecodeZstd;

impl Function for DecodeZstd {
    fn identifier(&self) -> &'static str {
        "decode_zstd"
    }

    fn examples(&self) -> &'static [Example] {
        &[Example {
            title: "demo string",
            source: r#"decode_zstd!(decode_base64!("KLUv/QBY/QEAYsQOFKClbQBedqXsb96EWDax/f/F/z+gNU4ZTInaUeAj82KqPFjUzKqhcfDqAIsLvAsnY1bI/N2mHzDixRQA"))"#,
            result: Ok("you_have_successfully_decoded_me.congratulations.you_are_breathtaking."),
        }]
    }

    fn compile(
        &self,
        _state: &state::TypeState,
        _ctx: &mut FunctionCompileContext,
        arguments: ArgumentList,
    ) -> Compiled {
        let value = arguments.required("value");

        Ok(DecodeZstdFn { value }.as_expr())
    }

    fn parameters(&self) -> &'static [Parameter] {
        &[Parameter {
            keyword: "value",
            kind: kind::BYTES,
            required: true,
        }]
    }
}

#[derive(Clone, Debug)]
struct DecodeZstdFn {
    value: Box<dyn Expression>,
}

impl FunctionExpression for DecodeZstdFn {
    fn resolve(&self, ctx: &mut Context) -> Resolved {
        let value = self.value.resolve(ctx)?;

        decode_zstd(value)
    }

    fn type_def(&self, _: &state::TypeState) -> TypeDef {
        // Always fallible due to the possibility of decoding errors that VRL can't detect
        TypeDef::bytes().fallible()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::value;
    use nom::AsBytes;

    fn get_encoded_bytes(text: &str) -> Vec<u8> {
        let result =
            zstd::encode_all(text.as_bytes(), 0).expect("Cannot encode bytes with Zstd encoder");

        result
    }

    test_function![
        decode_zstd => DecodeZstd;

        right_zstd {
            args: func_args![value: value!(get_encoded_bytes("sample").as_bytes())],
            want: Ok(value!(b"sample")),
            tdef: TypeDef::bytes().fallible(),
        }

        wrong_zstd {
            args: func_args![value: value!("some_bytes")],
            want: Err("unable to decode value with Zstd decoder"),
            tdef: TypeDef::bytes().fallible(),
        }
    ];

    // OBE-10737: a zstd bomb that decompresses to >64 MiB must return an error.
    #[test]
    fn zstd_bomb_exceeds_limit() {
        let zeros = vec![0u8; 65 * 1024 * 1024];
        let compressed = zstd::encode_all(zeros.as_slice(), 22).expect("zstd encode failed");

        let result = decode_zstd(Value::Bytes(compressed.into()));
        assert!(result.is_err(), "expected error for zstd bomb exceeding size limit");
        assert!(
            result.unwrap_err().to_string().contains("exceeds size limit"),
            "error should mention size limit"
        );
    }
}
