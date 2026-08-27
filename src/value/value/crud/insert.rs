use super::{ValueCollection, MAX_ARRAY_INDEX};
use crate::path::BorrowedSegment;
use crate::value::Value;
use std::borrow::Borrow;
use std::collections::BTreeMap;

pub fn insert<'a, T: ValueCollection>(
    value: &mut T,
    key: T::Key,
    mut path_iter: impl Iterator<Item = BorrowedSegment<'a>>,
    insert_value: Value,
) -> Option<Value> {
    match path_iter.next() {
        Some(BorrowedSegment::Field(field)) => {
            if let Some(Value::Object(map)) = value.get_mut_value(key.borrow()) {
                insert(map, field.to_string().into(), path_iter, insert_value)
            } else {
                let mut map = BTreeMap::new();
                let prev_value =
                    insert(&mut map, field.to_string().into(), path_iter, insert_value);
                value.insert_value(key, Value::Object(map));
                prev_value
            }
        }
        Some(BorrowedSegment::Index(index)) => {
            if let Some(Value::Array(array)) = value.get_mut_value(key.borrow()) {
                insert(array, index, path_iter, insert_value)
            } else {
                // Bounded by the same cap `insert_value` enforces, so an out-of-range index
                // cannot reserve memory here before being rejected there.
                let max_capacity = MAX_ARRAY_INDEX + 1;
                let capacity = if index >= 0 {
                    ((index as usize) + 1).min(max_capacity)
                } else {
                    // `unsigned_abs` rather than `-index`, which overflows on `isize::MIN`.
                    index.unsigned_abs().min(max_capacity)
                };
                let mut array = Vec::with_capacity(capacity);
                let prev_value = insert(&mut array, index, path_iter, insert_value);
                value.insert_value(key, Value::Array(array));
                prev_value
            }
        }
        Some(BorrowedSegment::Invalid) => None,
        None => value.insert_value(key, insert_value),
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_insert_nested() {
        let mut value = Value::Null;
        value.insert("a.b.c", 3);
        let expected = Value::from(json!({
            "a": {
                "b":{
                    "c": 3
                }
            }
        }));
        assert_eq!(value, expected);
    }

    #[test]
    fn test_insert_array() {
        let mut value = Value::Null;
        value.insert("a.b[0].c[2]", 10);
        value.insert("a.b[0].c[0]", 5);

        let expected = Value::from(json!({
            "a": {
                "b": [{
                    "c": [5, null, 10]
                }]
            }
        }));
        assert_eq!(value, expected);
    }

    // OBE-10735: `insert_value` padded the array with `Value::Null` up to an arbitrary index,
    // and `Vec::with_capacity(index + 1)` allocated for it up front — an event-controlled path
    // index was enough to exhaust memory.
    #[test]
    fn test_insert_beyond_max_array_index_is_rejected() {
        let mut value = Value::Null;
        assert_eq!(value.insert("[1048577]", 1), None);
        assert_eq!(value, Value::from(json!([])));
    }

    #[test]
    fn test_insert_beyond_max_negative_array_index_is_rejected() {
        let mut value = Value::Null;
        assert_eq!(value.insert("[-1048577]", 1), None);
        assert_eq!(value, Value::from(json!([])));
    }

    #[test]
    fn test_insert_at_max_array_index_is_allowed() {
        let mut value = Value::Null;
        assert_eq!(value.insert("[1048576]", 1), None);
        let array = value.as_array().expect("expected an array");
        assert_eq!(array.len(), 1_048_577);
        assert_eq!(array[1_048_576], Value::Integer(1));
    }

    // OBE-10735: the capacity calculation negated the index with `(-index) as usize`, which
    // overflows on `isize::MIN` (there is no positive `isize` counterpart). `unsigned_abs` is
    // the total operation.
    #[test]
    fn test_insert_at_isize_min_does_not_panic() {
        let mut value = Value::Null;
        let path = vec![BorrowedSegment::Index(isize::MIN)].into_iter();
        assert_eq!(insert(&mut value, (), path, Value::Integer(1)), None);
        assert_eq!(value, Value::from(json!([])));
    }

    // Drift detector, not a correctness assertion: the cap is justified in terms of the memory a
    // single indexed write may commit (`MAX_ARRAY_INDEX + 1` elements of this size, ~42 MB today).
    // If `Value` grows a variant, that budget changes and the cap deserves a fresh look.
    #[test]
    fn test_value_size_is_pinned() {
        assert_eq!(
            std::mem::size_of::<Value>(),
            40,
            "size_of::<Value>() changed; re-check the MAX_ARRAY_INDEX memory budget \
             (cap x size = worst-case allocation for one indexed write)"
        );
    }

    #[test]
    fn test_insert_negative_index() {
        let mut value = Value::Null;
        assert_eq!(value.insert("[-2]", 10), None);
        assert_eq!(value.insert("[-1]", 5), Some(Value::Null));
        assert_eq!(value.insert("[-2]", 2), Some(Value::Integer(10)));
        assert_eq!(value.insert("[-1][1]", 3), None);
        assert_eq!(value, Value::from(json!([2, [null, 3]])));
    }
}
