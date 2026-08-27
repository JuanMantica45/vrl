//! OBE-10732 spike, part 2: what depth can a real VRL program actually reach?
//!
//! Runs the ticket's own exploit shape — `v = push([], v)` inside `for_each`, which grows nesting
//! one level per iteration — and optionally applies a sink afterwards. Answers the reachability
//! question that decides whether the unguardable traversals (Clone/PartialEq/Drop) need a
//! construction cap at all.
//!
//! Usage: vrl_depth_probe <sink> <iterations> <stack_bytes>
//! Sinks: none | eq | display | encode_json

use std::collections::BTreeMap;
use vrl::compiler::{state::RuntimeState, Context, TargetValue, TimeZone};
use vrl::value::{Secrets, Value};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() != 4 {
        eprintln!("usage: vrl_depth_probe <sink> <iterations> <stack_bytes>");
        std::process::exit(2);
    }
    let sink = args[1].clone();
    let iters: usize = args[2].parse().expect("iterations");
    let stack: usize = args[3].parse().expect("stack_bytes");

    let sink_src = match sink.as_str() {
        "none" => "",
        "eq" => "if v == v { .hit = true }",
        "display" => ".hit = to_string!(v)",
        "encode_json" => ".hit = encode_json(v)",
        other => {
            eprintln!("unknown sink: {other}");
            std::process::exit(2);
        }
    };

    // `v = push([], v)` wraps the accumulator once per iteration: depth grows to `iters`.
    let src = format!(
        r#"
v = []
for_each(array!(.items)) -> |_i, _x| {{ v = push([], v) }}
{sink_src}
.depth_built = length(v)
"#
    );

    let handle = std::thread::Builder::new()
        .stack_size(stack)
        .spawn(move || {
            let fns = vrl::stdlib::all();
            let result = match vrl::compiler::compile(&src, &fns) {
                Ok(r) => r,
                Err(e) => {
                    println!("COMPILE_ERROR: {e:?}");
                    return;
                }
            };

            let items = Value::Array(vec![Value::Integer(0); iters]);
            let mut target = TargetValue {
                value: Value::Object(BTreeMap::from([("items".into(), items)])),
                metadata: Value::Object(BTreeMap::new()),
                secrets: Secrets::default(),
            };
            let mut state = RuntimeState::default();
            let timezone = TimeZone::default();
            let mut ctx = Context::new(&mut target, &mut state, &timezone);

            match result.program.resolve(&mut ctx) {
                Ok(_) => println!("OK"),
                Err(e) => println!("RUNTIME_ERROR: {e}"),
            }
            // Falling out of scope here drops the runtime state, including the deep `v`.
        })
        .expect("spawn");

    match handle.join() {
        Ok(()) => std::process::exit(0),
        Err(_) => {
            eprintln!("PANIC");
            std::process::exit(1)
        }
    }
}
