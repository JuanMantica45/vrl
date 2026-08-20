use crate::compiler::prelude::*;
use crate::parsing::xml::{parse_xml, ParseOptions};

#[derive(Clone, Copy, Debug)]
pub struct ParseXml;

impl Function for ParseXml {
    fn identifier(&self) -> &'static str {
        "parse_xml"
    }

    fn examples(&self) -> &'static [Example] {
        &[Example {
            title: "parse XML",
            source: indoc! {r#"
				value = s'<book category="CHILDREN"><title lang="en">Harry Potter</title><author>J K. Rowling</author><year>2005</year></book>';

				parse_xml!(value, text_key: "value", parse_number: false)
            "#},
            result: Ok(
                r#"{ "book": { "@category": "CHILDREN", "author": "J K. Rowling", "title": { "@lang": "en", "value": "Harry Potter" }, "year": "2005" } }"#,
            ),
        }]
    }

    fn compile(
        &self,
        _state: &state::TypeState,
        _ctx: &mut FunctionCompileContext,
        arguments: ArgumentList,
    ) -> Compiled {
        let value = arguments.required("value");

        let trim = arguments.optional("trim");
        let include_attr = arguments.optional("include_attr");
        let attr_prefix = arguments.optional("attr_prefix");
        let text_key = arguments.optional("text_key");
        let always_use_text_key = arguments.optional("always_use_text_key");
        let parse_bool = arguments.optional("parse_bool");
        let parse_null = arguments.optional("parse_null");
        let parse_number = arguments.optional("parse_number");

        Ok(ParseXmlFn {
            value,
            trim,
            include_attr,
            attr_prefix,
            text_key,
            always_use_text_key,
            parse_bool,
            parse_null,
            parse_number,
        }
        .as_expr())
    }

    fn parameters(&self) -> &'static [Parameter] {
        &[
            Parameter {
                keyword: "value",
                kind: kind::BYTES,
                required: true,
            },
            Parameter {
                keyword: "trim",
                kind: kind::BOOLEAN,
                required: false,
            },
            Parameter {
                keyword: "include_attr",
                kind: kind::BOOLEAN,
                required: false,
            },
            Parameter {
                keyword: "attr_prefix",
                kind: kind::BYTES,
                required: false,
            },
            Parameter {
                keyword: "text_key",
                kind: kind::BYTES,
                required: false,
            },
            Parameter {
                keyword: "always_use_text_key",
                kind: kind::BOOLEAN,
                required: false,
            },
            Parameter {
                keyword: "parse_bool",
                kind: kind::BOOLEAN,
                required: false,
            },
            Parameter {
                keyword: "parse_null",
                kind: kind::BOOLEAN,
                required: false,
            },
            Parameter {
                keyword: "parse_number",
                kind: kind::BOOLEAN,
                required: false,
            },
        ]
    }
}

#[derive(Debug, Clone)]
struct ParseXmlFn {
    value: Box<dyn Expression>,

    trim: Option<Box<dyn Expression>>,
    include_attr: Option<Box<dyn Expression>>,
    attr_prefix: Option<Box<dyn Expression>>,
    text_key: Option<Box<dyn Expression>>,
    always_use_text_key: Option<Box<dyn Expression>>,
    parse_bool: Option<Box<dyn Expression>>,
    parse_null: Option<Box<dyn Expression>>,
    parse_number: Option<Box<dyn Expression>>,
}

impl FunctionExpression for ParseXmlFn {
    fn resolve(&self, ctx: &mut Context) -> Resolved {
        let value = self.value.resolve(ctx)?;

        let options = ParseOptions {
            trim: self
                .trim
                .as_ref()
                .map(|expr| expr.resolve(ctx))
                .transpose()?,

            include_attr: self
                .include_attr
                .as_ref()
                .map(|expr| expr.resolve(ctx))
                .transpose()?,

            attr_prefix: self
                .attr_prefix
                .as_ref()
                .map(|expr| expr.resolve(ctx))
                .transpose()?,

            text_key: self
                .text_key
                .as_ref()
                .map(|expr| expr.resolve(ctx))
                .transpose()?,

            always_use_text_key: self
                .always_use_text_key
                .as_ref()
                .map(|expr| expr.resolve(ctx))
                .transpose()?,

            parse_bool: self
                .parse_bool
                .as_ref()
                .map(|expr| expr.resolve(ctx))
                .transpose()?,

            parse_null: self
                .parse_null
                .as_ref()
                .map(|expr| expr.resolve(ctx))
                .transpose()?,

            parse_number: self
                .parse_number
                .as_ref()
                .map(|expr| expr.resolve(ctx))
                .transpose()?,
        };

        parse_xml(value, options)
    }

    fn type_def(&self, _: &state::TypeState) -> TypeDef {
        type_def()
    }
}

fn type_def() -> TypeDef {
    TypeDef::bytes()
        .or_object(Collection::from_unknown(inner_kind()))
        .fallible()
}

fn inner_kind() -> Kind {
    Kind::object(Collection::any())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::value;

    test_function![
        parse_xml => ParseXml;

        simple_text {
            args: func_args![ value: "<a>test</a>" ],
            want: Ok(value!({ "a": "test" })),
            tdef: type_def(),
        }

        include_attr {
            args: func_args![ value: r#"<a href="https://vector.dev">test</a>"# ],
            want: Ok(value!({ "a": { "@href": "https://vector.dev", "text": "test" } })),
            tdef: type_def(),
        }

        exclude_attr {
            args: func_args![ value: r#"<a href="https://vector.dev">test</a>"#, include_attr: false ],
            want: Ok(value!({ "a": "test" })),
            tdef: type_def(),
        }

        custom_text_key {
            args: func_args![ value: "<b>test</b>", text_key: "node", always_use_text_key: true ],
            want: Ok(value!({ "b": { "node": "test" } })),
            tdef: type_def(),
        }

        // https://github.com/vectordotdev/vector/issues/11901
        include_attributes_if_single_node {
            args: func_args![ value: r#"<root><node attr="value"><message>foo</message></node></root>"# ],
            want: Ok(value!({ "root": { "node": { "@attr": "value", "message": "foo" } } })),
            tdef: type_def(),
        }

        // https://github.com/vectordotdev/vector/issues/11901
        include_attributes_multiple_children {
            args: func_args![ value: r#"<root><node attr="value"><message>bar</message></node><node attr="value"><message>baz</message></node></root>"#],
            want: Ok(value!({"root":{ "node":[ { "@attr": "value", "message": "bar" }, { "@attr": "value", "message": "baz" } ] } })),
            tdef: type_def(),
        }

        nested_object {
            args: func_args![ value: r#"<a attr="value"><b>one</b><c>two</c></a>"# ],
            want: Ok(value!({ "a": { "@attr": "value", "b": "one", "c": "two" } })),
            tdef: type_def(),
        }

        nested_object_array {
            args: func_args![ value: "<a><b>one</b><b>two</b></a>" ],
            want: Ok(value!({ "a": { "b": ["one", "two"] } })),
            tdef: type_def(),
        }

        header_and_comments {
            args: func_args![ value: indoc!{r#"
                <?xml version="1.0" encoding="ISO-8859-1"?>
                <!-- Example found somewhere in the deep depths of the web -->
                <note>
                    <to>Tove</to>
                    <!-- Randomly inserted inner comment -->
                    <from>Jani</from>
                    <heading>Reminder</heading>
                    <body>Don't forget me this weekend!</body>
                </note>

                <!-- Could literally be placed anywhere -->
            "#}],
            want: Ok(value!(
                {
                    "note": {
                        "to": "Tove",
                        "from": "Jani",
                        "heading": "Reminder",
                        "body": "Don't forget me this weekend!"
                    }
                }
            )),
            tdef: type_def(),
        }

        // The sole real child is text ("text123"); the leading PI is not a
        // real child, so this flattens the same as `<p>text123</p>` would.
        header_inside_element {
            args: func_args![ value: "<p><?xml?>text123</p>" ],
            want: Ok(value!({ "p": "text123" })),
            tdef: type_def(),
        }

        mixed_types {
            args: func_args![ value: indoc!{r#"
                <?xml version="1.0" encoding="ISO-8859-1"?>
                <!-- Mixed types -->
                <data>
                    <!-- Booleans -->
                    <item>true</item>
                    <item>false</item>
                    <!-- String -->
                    <item>string!</item>
                    <!-- Empty object -->
                    <item />
                    <!-- Literal value "null" -->
                    <item>null</item>
                    <!-- Integer -->
                    <item>1</item>
                    <!-- Float -->
                    <item>1.0</item>
                </data>
            "#}],
            want: Ok(value!(
                {
                    "data": {
                        "item": [
                            true,
                            false,
                            "string!",
                            {},
                            null,
                            1,
                            1.0
                        ]
                    }
                }
            )),
            tdef: type_def(),
        }

        just_strings {
            args: func_args![ value: indoc!{r#"
                <?xml version="1.0" encoding="ISO-8859-1"?>
                <!-- All scalar types are just strings -->
                <data>
                    <item>true</item>
                    <item>false</item>
                    <item>string!</item>
                    <!-- Still an empty object -->
                    <item />
                    <item>null</item>
                    <item>1</item>
                    <item>1.0</item>
                </data>
            "#}, parse_null: false, parse_bool: false, parse_number: false],
            want: Ok(value!(
                {
                    "data": {
                        "item": [
                            "true",
                            "false",
                            "string!",
                            {},
                            "null",
                            "1",
                            "1.0"
                        ]
                    }
                }
            )),
            tdef: type_def(),
        }

        untrimmed {
            args: func_args![ value: "<root>  <a>test</a>  </root>", trim: false ],
            want: Ok(value!(
                {
                    "root": {
                        "a": "test",
                        "text": ["  ", "  "],
                    }
                }
            )),
            tdef: type_def(),
        }

        invalid_token {
            args: func_args![ value: "true" ],
            want: Err("unable to parse xml: unknown token at 1:1"),
            tdef: type_def(),
        }

        flat_parent_property {
            args: func_args![ value: indoc!{r#"
                <?xml version="1.0" encoding="UTF-8"?>
                <MY_XML>
                  <property1>
                    <property1_a>a</property1_a>
                    <property1_b>b</property1_b>
                    <property1_c>c</property1_c>
                  </property1>
                  <property2>
                    <property2_object>
                      <property2a_a>a</property2a_a>
                      <property2a_b>b</property2a_b>
                      <property2a_c>c</property2a_c>
                    </property2_object>
                  </property2>
                </MY_XML>
            "#}],
            want: Ok(value!(
                {
                  "MY_XML": {
                    "property1": {
                      "property1_a": "a",
                      "property1_b": "b",
                      "property1_c": "c"
                    },
                    "property2": {
                      "property2_object": {
                        "property2a_a": "a",
                        "property2a_b": "b",
                        "property2a_c": "c"
                      }
                    }
                  }
                }
            )),
            tdef: type_def(),
        }

        nested_parent_property {
            args: func_args![ value: indoc!{r#"
                <?xml version="1.0" encoding="UTF-8"?>
                <MY_XML>
                  <property1>
                    <property1_a>a</property1_a>
                    <property1_b>b</property1_b>
                    <property1_c>c</property1_c>
                  </property1>
                  <property2>
                    <property2_object>
                      <property2a_a>a</property2a_a>
                      <property2a_b>b</property2a_b>
                      <property2a_c>c</property2a_c>
                    </property2_object>
                    <property2_object>
                      <property2a_a>a</property2a_a>
                      <property2a_b>b</property2a_b>
                      <property2a_c>c</property2a_c>
                    </property2_object>
                  </property2>
                </MY_XML>
            "#}],
            want: Ok(value!(
                {
                  "MY_XML": {
                    "property1": {
                      "property1_a": "a",
                      "property1_b": "b",
                      "property1_c": "c"
                    },
                    "property2": {
                      "property2_object": [
                        {
                          "property2a_a": "a",
                          "property2a_b": "b",
                          "property2a_c": "c"
                        },
                        {
                          "property2a_a": "a",
                          "property2a_b": "b",
                          "property2a_c": "c"
                        }
                      ]
                    }
                  }
                }
            )),
            tdef: type_def(),
        }

        if_no_sibling {
            args: func_args![ value: "<root><a>test</a></root>"],
            want: Ok(value!({ "root": { "a": "test" } })),
            tdef: type_def(),
        }

        if_no_sibling2 {
            args: func_args![ value: "<root><a><a1>test</a1></a><b>test2</b></root>"],
            want: Ok(value!({ "root": { "a": { "a1": "test" }, "b" : "test2" } })),
            tdef: type_def(),
        }

        // OBE-10731: element whose sole child is a Comment must not panic
        comment_only_child {
            args: func_args![value: "<a><!--x--></a>"],
            want: Ok(value!({ "a": {} })),
            tdef: type_def(),
        }

        // OBE-10731: an element whose only child is a comment or processing instruction took
        // the single-child "flatten" path, handing a non-element/non-text node to
        // `process_node`, which hits `unreachable!("shouldn't be other XML nodes")`.
        only_child_is_a_comment {
            args: func_args![ value: "<a><!-- comment --></a>"],
            want: Ok(value!({ "a": {} })),
            tdef: type_def(),
        }

        // OBE-10731: element whose sole child is a PI must not panic
        pi_only_child {
            args: func_args![value: "<a><?pi?></a>"],
            want: Ok(value!({ "a": {} })),
            tdef: type_def(),
        }

        only_child_is_a_processing_instruction {
            args: func_args![ value: "<a><?target data?></a>"],
            want: Ok(value!({ "a": {} })),
            tdef: type_def(),
        }

        // OBE-10731: nested element whose sole child is a comment must not panic
        nested_comment_only_child {
            args: func_args![value: "<r><inner><!--boom--></inner></r>"],
            want: Ok(value!({ "r": { "inner": {} } })),
            tdef: type_def(),
        }

        // A comment sibling next to the sole real (text) child must not change
        // the flatten decision — the count that picks the single-child fast
        // path must be over real children only, not raw XML nodes.
        flatten_text_with_comment_sibling {
            args: func_args![value: "<a>5<!-- note --></a>"],
            want: Ok(value!({ "a": 5 })),
            tdef: type_def(),
        }

        // Same as above, for a sole real (element) child.
        flatten_element_with_comment_sibling {
            args: func_args![value: "<a><!-- note --><b/></a>"],
            want: Ok(value!({ "a": { "b": {} } })),
            tdef: type_def(),
        }
    ];

    // OBE-10742: deeply-nested XML must return an error, not overflow the stack.
    #[test]
    fn deeply_nested_xml_returns_error() {
        // Build <a><a><a>...</a></a></a> with 200 levels — exceeds MAX_XML_DEPTH (128).
        let open: String = "<a>".repeat(200);
        let close: String = "</a>".repeat(200);
        let xml = format!("{}{}", open, close);
        let result = parse_xml(
            Value::Bytes(xml.into()),
            ParseOptions {
                trim: None,
                include_attr: None,
                attr_prefix: None,
                text_key: None,
                always_use_text_key: None,
                parse_bool: None,
                parse_null: None,
                parse_number: None,
            },
        );
        assert!(result.is_err(), "expected error for XML exceeding depth limit");
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("nesting limit"), "error should mention nesting limit; got: {msg}");
    }

    #[test]
    fn test_kind() {
        let state = state::TypeState::default();

        let func = ParseXmlFn {
            value: value!(true).into_expression(),
            trim: None,
            include_attr: None,
            attr_prefix: None,
            text_key: None,
            always_use_text_key: None,
            parse_bool: None,
            parse_null: None,
            parse_number: None,
        };

        let type_def = func.type_def(&state);

        assert!(type_def.is_fallible());
        assert!(!type_def.is_exact());
        assert!(type_def.contains_bytes());
        assert!(type_def.contains_object());

        let object1 = type_def.as_object().unwrap();

        assert!(object1.known().is_empty());
        assert!(object1.unknown_kind().contains_object());

        let object2 = object1.unknown_kind().as_object().cloned().unwrap();

        assert!(object2.known().is_empty());
        assert!(object2.unknown_kind().is_any());
    }

    #[test]
    fn test_xml_depth_limit_obe10742() {
        // OBE-10742: deeply nested XML from attacker-controlled input must be rejected.
        // Without the fix, process_node recurses for every element level and can stack overflow.
        let depth = 200usize;
        let open: String = (0..depth).map(|i| format!("<a{i}>")).collect();
        let close: String = (0..depth).rev().map(|i| format!("</a{i}>")).collect();
        let xml = format!("{open}hi{close}");
        let value = Value::Bytes(xml.into());
        let options = crate::parsing::xml::ParseOptions::default();
        let result = crate::parsing::xml::parse_xml(value, options);
        assert!(result.is_err(), "XML with {depth} nesting levels must be rejected (OBE-10742)");
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("nesting limit"), "error must mention nesting limit: {msg}");
    }
}
