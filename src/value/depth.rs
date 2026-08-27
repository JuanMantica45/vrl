//! Bounds how deeply a [`Value`] may be nested.
//!
//! `Value`'s `Clone`, `PartialEq`, `Hash` and drop glue are all structurally recursive and none of
//! them can report an error — their signatures return `Self`, `bool`, a hash and nothing. So a
//! deeply-nested `Value` cannot be handled safely once it exists; it has to not exist. This is the
//! same defence `serde_json` uses (a depth limit in its *parser*, `de.rs`) and, contrary to
//! OBE-10732's description, `serde_json` has no `impl Drop for Value` to copy.

use super::Value;

/// Largest nesting depth a VRL program may construct.
///
/// Derived from measurement rather than chosen. The cheapest traversal to overflow is
/// `Display::fmt` at ~625 bytes of stack per level, so 512 levels costs ~320 KiB — 6.4x headroom
/// inside the 2 MiB stack tokio gives Vector's workers (Vector never calls `thread_stack_size`,
/// so the tokio default applies). Measured limits on a 2 MiB thread, for reference:
///
/// | traversal    | overflows at | bytes/level |
/// |--------------|--------------|-------------|
/// | `Display`    | 3,294        | ~625        |
/// | `PartialEq`  | 9,415        | ~223        |
/// | `Clone`      | 10,983       | ~190        |
/// | `Serialize`  | 32,951       | ~64         |
/// | drop glue    | 43,932       | ~48         |
///
/// 512 also sits well above every other cap in this crate (128) and above `serde_json`'s parser
/// limit (128), so it cannot plausibly reject legitimate data.
pub const MAX_VALUE_DEPTH: usize = 512;

/// Returns `true` if `value` nests deeper than `limit`.
///
/// Iterative: it walks an explicit heap worklist instead of recursing, so the check itself can
/// never overflow the stack it exists to protect. It stops as soon as the limit is passed, so for
/// the shape this guards against — an accumulator wrapped one level per loop iteration — the cost
/// is O(limit) rather than O(size of value).
pub fn depth_exceeds(value: &Value, limit: usize) -> bool {
    // Depth-first with an explicit stack of (node, depth-of-node).
    let mut stack: Vec<(&Value, usize)> = vec![(value, 1)];

    while let Some((node, depth)) = stack.pop() {
        if depth > limit {
            return true;
        }
        match node {
            Value::Array(array) => stack.extend(array.iter().map(|child| (child, depth + 1))),
            Value::Object(map) => stack.extend(map.values().map(|child| (child, depth + 1))),
            _ => {}
        }
    }

    false
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A `Value` nested exactly `depth` levels: `nested(1)` is a scalar, `nested(2)` is `[scalar]`.
    fn nested(depth: usize) -> Value {
        let mut v = Value::Null;
        for _ in 1..depth {
            v = Value::Array(vec![v]);
        }
        v
    }

    #[test]
    fn scalars_have_depth_one() {
        assert!(!depth_exceeds(&Value::Integer(1), 1));
        assert!(depth_exceeds(&Value::Integer(1), 0));
    }

    #[test]
    fn reports_exactly_at_the_boundary() {
        assert!(!depth_exceeds(&nested(9), 10));
        assert!(!depth_exceeds(&nested(10), 10));
        assert!(depth_exceeds(&nested(11), 10));
    }

    #[test]
    fn finds_depth_nested_in_an_object() {
        let mut v = Value::Null;
        for _ in 0..20 {
            let mut map = crate::value::ObjectMap::new();
            map.insert("a".into(), v);
            v = Value::Object(map);
        }
        assert!(depth_exceeds(&v, 10));
        assert!(!depth_exceeds(&v, 30));
    }

    // The check must not be defeated by putting the deep branch behind a wide shallow one.
    #[test]
    fn finds_depth_behind_breadth() {
        let mut children: Vec<Value> = (0..1_000).map(Value::Integer).collect();
        children.push(nested(50));
        assert!(depth_exceeds(&Value::Array(children), 20));
    }

    // It must never recurse, or it would overflow on exactly the input it is meant to reject.
    #[test]
    fn does_not_itself_overflow_on_a_very_deep_value() {
        let deep = nested(100_000);
        assert!(depth_exceeds(&deep, MAX_VALUE_DEPTH));
        // Drop it iteratively too, so the test does not die tearing `deep` down.
        let mut cur = deep;
        loop {
            let next = match &mut cur {
                Value::Array(a) if !a.is_empty() => a.remove(0),
                _ => break,
            };
            cur = next;
        }
    }
}
