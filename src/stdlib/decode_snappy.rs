use crate::compiler::prelude::*;
use crate::stdlib::util::{DECOMPRESS_LIMIT_ERROR, DEFAULT_DECOMPRESS_LIMIT};
use snap::raw::{decompress_len, Decoder};

fn decode_snappy(value: Value) -> Resolved {
    let value = value.try_bytes()?;

    // A snappy frame declares its uncompressed length up front, and
    // `decompress_vec` sizes its output buffer from that value before doing any
    // work. A few bytes of input can therefore claim gigabytes, so reject an
    // over-limit claim before allocating anything (OBE-10737).
    let claimed_len =
        decompress_len(&value).map_err(|_| "unable to decode value with Snappy decoder")?;
    if claimed_len as u64 > DEFAULT_DECOMPRESS_LIMIT {
        return Err(DECOMPRESS_LIMIT_ERROR.into());
    }

    let mut decoder = Decoder::new();
    match decoder.decompress_vec(&value) {
        Ok(buf) => Ok(Value::Bytes(buf.into())),
        Err(_) => Err("unable to decode value with Snappy decoder".into()),
    }
}

#[derive(Clone, Copy, Debug)]
pub struct DecodeSnappy;

impl Function for DecodeSnappy {
    fn identifier(&self) -> &'static str {
        "decode_snappy"
    }

    fn examples(&self) -> &'static [Example] {
        &[Example {
            title: "demo string",
            source: r#"decode_snappy!(decode_base64!("LKxUaGUgcXVpY2sgYnJvd24gZm94IGp1bXBzIG92ZXIgMTMgbGF6eSBkb2dzLg=="))"#,
            result: Ok("The quick brown fox jumps over 13 lazy dogs."),
        }]
    }

    fn compile(
        &self,
        _state: &state::TypeState,
        _ctx: &mut FunctionCompileContext,
        arguments: ArgumentList,
    ) -> Compiled {
        let value = arguments.required("value");

        Ok(DecodeSnappyFn { value }.as_expr())
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
struct DecodeSnappyFn {
    value: Box<dyn Expression>,
}

impl FunctionExpression for DecodeSnappyFn {
    fn resolve(&self, ctx: &mut Context) -> Resolved {
        let value = self.value.resolve(ctx)?;

        decode_snappy(value)
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
    use base64::Engine;
    use nom::AsBytes;

    fn decode_base64(text: &str) -> Vec<u8> {
        let engine = base64::engine::GeneralPurpose::new(
            &base64::alphabet::STANDARD,
            base64::engine::general_purpose::GeneralPurposeConfig::new(),
        );

        engine.decode(text).expect("Cannot decode from Base64")
    }

    test_function![
        decode_snappy => DecodeSnappy;

        right_snappy {
            args: func_args![value: value!(decode_base64("LKxUaGUgcXVpY2sgYnJvd24gZm94IGp1bXBzIG92ZXIgMTMgbGF6eSBkb2dzLg==").as_bytes())],
            want: Ok(value!(b"The quick brown fox jumps over 13 lazy dogs.")),
            tdef: TypeDef::bytes().fallible(),
        }

        wrong_snappy {
            args: func_args![value: value!("some_bytes")],
            want: Err("unable to decode value with Snappy decoder"),
            tdef: TypeDef::bytes().fallible(),
        }
    ];

    // OBE-10737: a snappy frame that *claims* a huge uncompressed length must be
    // rejected before the output buffer is allocated. Unfixed, `decompress_vec`
    // eagerly allocates the claimed size from this handful of input bytes.
    //
    // The claim is 1 GiB: comfortably above DEFAULT_DECOMPRESS_LIMIT but below
    // snap's own `u32::MAX` ceiling, so snap itself will not reject it — our
    // check is the only thing standing between this input and a 1 GiB alloc.
    #[test]
    fn snappy_oversized_claimed_length_rejected() {
        // A snappy stream begins with the uncompressed length as a varint.
        let mut payload = Vec::new();
        let mut claim: u64 = 1024 * 1024 * 1024;
        while claim >= 0x80 {
            payload.push((claim as u8) | 0x80);
            claim >>= 7;
        }
        payload.push(claim as u8);
        // Body is deliberately truncated — we must fail on the size claim, not
        // by decoding to completion.
        payload.extend_from_slice(&[0x00, 0x00, 0x00]);

        let result = decode_snappy(Value::Bytes(payload.into()));
        assert!(result.is_err(), "oversized claimed length must be rejected");
        assert_eq!(
            result.unwrap_err().to_string(),
            DECOMPRESS_LIMIT_ERROR,
            "must be rejected for exceeding the size limit, not as a decode error"
        );
    }

    // A real snappy payload well under the limit must still round-trip.
    #[test]
    fn snappy_under_limit_still_decodes() {
        let original = vec![b'a'; 1024 * 1024];
        let compressed = snap::raw::Encoder::new()
            .compress_vec(&original)
            .expect("snappy encode failed");

        let result = decode_snappy(Value::Bytes(compressed.into())).expect("must decode");
        assert_eq!(result, Value::Bytes(original.into()));
    }
}
