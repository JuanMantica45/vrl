//! OBE-10732 spike: measure which `Value` traversal overflows first, and at what depth.
//!
//! Deep values are built iteratively (O(1) stack per level) so that construction itself never
//! recurses — this isolates the traversal under test. Values we are not measuring are leaked with
//! `mem::forget` so a stray recursive drop cannot be mistaken for the mode's own overflow.
//!
//! Usage: depth_probe <mode> <depth> <stack_bytes>
//! Modes: build | drop | clone | display | serialize | partial_eq
//!
//! Exits 0 and prints OK when the traversal survives. A stack overflow aborts the process
//! (SIGSEGV/SIGABRT), which is the signal the caller measures.

use std::mem;
use vrl::value::Value;

fn build(depth: usize) -> Value {
    let mut v = Value::Null;
    for _ in 0..depth {
        v = Value::Array(vec![v]);
    }
    v
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() != 4 {
        eprintln!("usage: depth_probe <mode> <depth> <stack_bytes>");
        std::process::exit(2);
    }
    let mode = args[1].clone();
    let depth: usize = args[2].parse().expect("depth");
    let stack: usize = args[3].parse().expect("stack_bytes");

    let handle = std::thread::Builder::new()
        .stack_size(stack)
        .spawn(move || {
            let v = build(depth);

            match mode.as_str() {
                // Control: construction only. Should never overflow.
                "build" => {
                    mem::forget(v);
                }
                // Recursive drop glue on Vec<Value>.
                "drop" => {
                    drop(v);
                }
                // Derived Clone. The clone is measured; both values leak so drop can't confound.
                "clone" => {
                    let c = v.clone();
                    mem::forget(c);
                    mem::forget(v);
                }
                // Hand-written recursive Display::fmt.
                "display" => {
                    let s = v.to_string();
                    mem::forget(v);
                    mem::forget(s);
                }
                // Serialize -> serde_json (write side has no recursion limit).
                "serialize" => {
                    let s = serde_json::to_string(&v).expect("serialize");
                    mem::forget(v);
                    mem::forget(s);
                }
                // Derived PartialEq.
                "partial_eq" => {
                    let c = v.clone();
                    let eq = v == c;
                    mem::forget(c);
                    mem::forget(v);
                    if !eq {
                        eprintln!("unexpected inequality");
                        std::process::exit(3);
                    }
                }
                other => {
                    eprintln!("unknown mode: {other}");
                    std::process::exit(2);
                }
            }
            println!("OK");
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
