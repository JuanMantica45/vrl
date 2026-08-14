use crate::compiler::prelude::*;
use regex::bytes::RegexSet;

fn match_any(value: Value, pattern: &RegexSet) -> Resolved {
    let bytes = value.try_bytes()?;
    Ok(pattern.is_match(&bytes).into())
}

#[derive(Clone, Copy, Debug)]
pub struct MatchAny;

impl Function for MatchAny {
    fn identifier(&self) -> &'static str {
        "match_any"
    }

    fn parameters(&self) -> &'static [Parameter] {
        &[
            Parameter {
                keyword: "value",
                kind: kind::BYTES,
                required: true,
            },
            Parameter {
                keyword: "patterns",
                kind: kind::ARRAY,
                required: true,
            },
        ]
    }

    fn examples(&self) -> &'static [Example] {
        &[
            Example {
                title: "match",
                source: r#"match_any("foo bar baz", patterns: [r'foo', r'123'])"#,
                result: Ok("true"),
            },
            Example {
                title: "no_match",
                source: r#"match_any("My name is John Doe", patterns: [r'\d+', r'Jane'])"#,
                result: Ok("false"),
            },
        ]
    }

    fn compile(
        &self,
        state: &state::TypeState,
        _ctx: &mut FunctionCompileContext,
        arguments: ArgumentList,
    ) -> Compiled {
        let value = arguments.required("value");
        let patterns = arguments.required_array("patterns")?;

        let mut re_strings = Vec::with_capacity(patterns.len());
        for expr in patterns {
            let value =
                expr.resolve_constant(state)
                    .ok_or(function::Error::ExpectedStaticExpression {
                        keyword: "patterns",
                        expr,
                    })?;

            let re = value
                .try_regex()
                .map_err(|e| Box::new(e) as Box<dyn DiagnosticMessage>)?;
            re_strings.push(re.to_string());
        }

        let regex_set = RegexSet::new(re_strings).map_err(|e| {
            Box::new(ExpressionError::from(format!("could not compile regex set: {e}")))
                as Box<dyn DiagnosticMessage>
        })?;

        Ok(MatchAnyFn { value, regex_set }.as_expr())
    }
}

#[derive(Clone, Debug)]
struct MatchAnyFn {
    value: Box<dyn Expression>,
    regex_set: RegexSet,
}

impl FunctionExpression for MatchAnyFn {
    fn resolve(&self, ctx: &mut Context) -> Resolved {
        let value = self.value.resolve(ctx)?;
        match_any(value, &self.regex_set)
    }

    fn type_def(&self, _: &state::TypeState) -> TypeDef {
        TypeDef::boolean().infallible()
    }
}

#[cfg(test)]
#[allow(clippy::trivial_regex)]
mod tests {
    use super::*;
    use crate::value;
    use regex::Regex;

    test_function![
        r#match_any => MatchAny;

        yes {
            args: func_args![value: "foobar",
                             patterns: Value::Array(vec![
                                 Value::Regex(Regex::new("foo").unwrap().into()),
                                 Value::Regex(Regex::new("bar").unwrap().into()),
                                 Value::Regex(Regex::new("baz").unwrap().into()),
                             ])],
            want: Ok(value!(true)),
            tdef: TypeDef::boolean().infallible(),
        }

        no {
            args: func_args![value: "foo 2 bar",
                             patterns: Value::Array(vec![
                                 Value::Regex(Regex::new("baz|quux").unwrap().into()),
                                 Value::Regex(Regex::new("foobar").unwrap().into()),
                             ])],
            want: Ok(value!(false)),
            tdef: TypeDef::boolean().infallible(),
        }
    ];

    // OBE-10725: `RegexSet::new` fails when the *combined* compiled size of the
    // patterns exceeds the regex crate's size limit. That used to be an
    // `expect`, aborting the process at compile time; it must now surface as a
    // recoverable compilation error.
    //
    // Each pattern below compiles fine on its own — only the union is over the
    // limit — so this exercises exactly the path that no other test reaches.
    #[test]
    fn oversized_regex_set_fails_to_compile_without_panicking() {
        let pattern_a = "x0(?:[a-z]{100000})";
        let pattern_b = "x1(?:[a-z]{100000})";

        // Precondition: individually valid. If a future regex version changes its
        // limits this asserts loudly rather than silently voiding the test.
        let regex_a = Regex::new(pattern_a).expect("pattern A must compile alone");
        let regex_b = Regex::new(pattern_b).expect("pattern B must compile alone");
        assert!(
            regex::bytes::RegexSet::new([pattern_a, pattern_b]).is_err(),
            "precondition: the combined set must exceed the regex size limit"
        );

        let state = state::TypeState::default();
        let mut ctx = FunctionCompileContext::new(
            crate::diagnostic::Span::new(0, 0),
            crate::compiler::CompileConfig::default(),
        );
        let args = func_args![
            value: "irrelevant",
            patterns: Value::Array(vec![
                Value::Regex(regex_a.into()),
                Value::Regex(regex_b.into()),
            ])
        ];

        let error = MatchAny
            .compile(&state, &mut ctx, args.into())
            .err()
            .expect("an oversized regex set must fail to compile");

        let message = error.message();
        assert!(
            message.contains("could not compile regex set"),
            "expected a regex-set compilation error, got: {message}"
        );
    }
}
