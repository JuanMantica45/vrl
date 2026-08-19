//! All types related to inserting one [`Kind`] into another.

use crate::path::{BorrowedSegment, ValuePath};
use crate::value::kind::{Collection, Index};
use crate::value::Kind;

/// Largest number of `known` array entries this module will materialize while
/// modelling an array index (OBE-10721).
///
/// Array index insertion normally records one `known` entry per index between
/// the existing entries and the target index, so an index such as `-99999999`
/// would allocate ~100M `Kind`s and exhaust memory *at compile time*. Beyond
/// this threshold we stop enumerating and widen the collection instead — see
/// [`collapse_array_to_unknown`].
const MAX_KNOWN_INDEX_ENTRIES: usize = 128;

/// Replace a precise, per-index array type with an imprecise but sound one.
///
/// Every `known` entry is folded into the `unknown` kind, together with `null`
/// (extending an array to reach the index creates null holes) and the kind
/// resulting from the insertion itself. Callers use this when enumerating the
/// indices individually would exhaust memory.
///
/// This only ever *widens* the type: each index that previously had a precise
/// `known` kind is now described by an `unknown` that is a superset of it. That
/// makes later expressions more fallible, never less, so it cannot mask a
/// type error.
fn collapse_array_to_unknown<'b>(
    collection: &mut Collection<Index>,
    iter: impl Iterator<Item = BorrowedSegment<'b>> + Clone,
    kind: Kind,
) {
    let mut widened = collection.unknown_kind();
    for known_kind in collection.known().values() {
        widened = widened.union(known_kind.clone());
    }
    // Holes created by extending the array to reach the index.
    widened = widened.union(Kind::null());

    // The insertion may land on any index in the collapsed range.
    let mut with_insertion = widened.clone();
    with_insertion.insert_recursive(iter, kind);
    widened = widened.union(with_insertion);

    collection.known_mut().clear();
    collection.set_unknown(widened);
}

impl Kind {
    /// Insert the `Kind` at the given `path` within `self`.
    /// This has the same behavior as `Value::insert`.
    #[allow(clippy::needless_pass_by_value)] // only reference types implement Path
    pub fn insert<'a>(&mut self, path: impl ValuePath<'a>, kind: Self) {
        self.insert_recursive(path.segment_iter(), kind.upgrade_undefined());
    }

    /// Set the `Kind` at the given `path` within `self`.
    /// There is a subtle difference
    /// between this and `Kind::insert` where this function does _not_ convert undefined to null.
    #[allow(clippy::needless_pass_by_value)] // only reference types implement Path
    pub fn set_at_path<'a>(&mut self, path: impl ValuePath<'a>, kind: Self) {
        self.insert_recursive(path.segment_iter(), kind);
    }

    /// Insert the `Kind` at the given `path` within `self`.
    /// This has the same behavior as `Value::insert`.
    ///
    /// # Panics
    /// Object/Array not present in `self`.
    #[allow(clippy::too_many_lines)]
    #[allow(clippy::needless_pass_by_value)] // only reference types implement Path
    pub fn insert_recursive<'a, 'b>(
        &'a mut self,
        mut iter: impl Iterator<Item = BorrowedSegment<'b>> + Clone,
        kind: Self,
    ) {
        if kind.is_never() {
            // If `kind` is `never`, the program would have already terminated
            // so this assignment can't happen.
            return;
        }

        if let Some(segment) = iter.next() {
            match segment {
                BorrowedSegment::Field(field) => {
                    // Field insertion converts the value to an object, so remove all other types.
                    *self = Self::object(self.object.clone().unwrap_or_else(Collection::empty));

                    let collection = self.object.as_mut().expect("object was just inserted");
                    let unknown_kind = collection.unknown_kind();

                    collection
                        .known_mut()
                        .entry(field.into_owned().into())
                        .or_insert(unknown_kind)
                        .insert_recursive(iter, kind);
                }
                BorrowedSegment::Index(mut index) => {
                    // Array insertion converts the value to an array, so remove all other types.
                    *self = Self::array(self.array.clone().unwrap_or_else(Collection::empty));
                    let collection = self.array.as_mut().expect("array was just inserted");

                    // OBE-10721: every branch below records one `known` entry per index
                    // between the existing entries and `index`. For a far-away index that
                    // exhausts memory (and time) at compile time, so widen instead of
                    // enumerating. `unsigned_abs` also avoids the `-index` overflow that
                    // `isize::MIN` would otherwise cause.
                    let indices_required = if index < 0 {
                        index.unsigned_abs()
                    } else {
                        (index as usize).saturating_add(1)
                    };
                    if indices_required > MAX_KNOWN_INDEX_ENTRIES {
                        collapse_array_to_unknown(collection, iter, kind);
                        return;
                    }

                    if index < 0 {
                        let largest_known_index = collection.largest_known_index();
                        // The minimum size of the resulting array.
                        let len_required = index.unsigned_abs();

                        let unknown_kind = collection.unknown_kind();
                        if unknown_kind.contains_any_defined() {
                            // The array may be larger, but this is the largest we can prove the array is from the type information.
                            let min_length = collection.min_length();

                            if len_required > min_length {
                                // We can't prove the array is large enough, so "holes" may be created
                                // which set the value to null.
                                // Holes are inserted to the front, which shifts everything to the right.
                                // We don't know the exact number of holes/shifts, but can determine an upper bound.
                                let max_shifts = len_required - min_length;

                                // The number of possible shifts is 0 ..= max_shifts.
                                // Each shift will be calculated independently and merged into the collection.
                                // A shift of 0 is the original collection, so that is skipped.
                                let zero_shifts = collection.clone();
                                for shift_count in 1..=max_shifts {
                                    let mut shifted_collection = zero_shifts.clone();
                                    // Clear all known values and replace with new ones. (in-place shift can overwrite).
                                    shifted_collection.known_mut().clear();

                                    // Add the "null" from holes.
                                    for i in 1..shift_count {
                                        shifted_collection
                                            .known_mut()
                                            .insert(i.into(), Self::null());
                                    }

                                    // Shift known values by the exact "shift_count".
                                    for (i, i_kind) in zero_shifts.known() {
                                        shifted_collection
                                            .known_mut()
                                            .insert(*i + shift_count, i_kind.clone());
                                    }

                                    // Add this shift count as another possible type definition.
                                    collection.merge(shifted_collection, false);
                                }
                            }

                            // We can prove the positive index won't be less than "min_index".
                            let min_index = (min_length as isize + index).max(0) as usize;

                            // Sanity check: if holes are added to the type, min_index must be 0.
                            debug_assert!(min_index == 0 || min_length >= len_required);

                            // Apply the current "unknown" to indices that don't have an explicit known
                            // since the "unknown" is about to change.
                            for i in 0..len_required {
                                collection
                                    .known_mut()
                                    .entry(i.into())
                                    .or_insert_with(|| unknown_kind.clone())
                                    // These indices are guaranteed to exist, so they can't be undefined.
                                    .remove_undefined();
                            }
                            for (i, i_kind) in collection.known_mut() {
                                // This index might be set by the insertion. Add the insertion type to the existing type.
                                if i.to_usize() >= min_index {
                                    let mut kind_with_insertion = i_kind.clone();
                                    let remaining_path_segments = iter.clone().collect::<Vec<_>>();
                                    kind_with_insertion
                                        .insert(&remaining_path_segments, kind.clone());
                                    *i_kind = i_kind.union(kind_with_insertion);
                                }
                            }

                            let mut unknown_kind_with_insertion = unknown_kind.clone();
                            let remaining_path_segments = iter.clone().collect::<Vec<_>>();
                            unknown_kind_with_insertion.insert(&remaining_path_segments, kind);
                            let mut new_unknown_kind = unknown_kind;
                            new_unknown_kind.merge_keep(unknown_kind_with_insertion, false);
                            collection.set_unknown(new_unknown_kind);

                            return;
                        }
                        debug_assert!(
                            collection.unknown_kind().is_undefined(),
                            "all cases with an unknown have been handled"
                        );

                        // If there is no unknown, the exact position of the negative index can be determined.
                        let exact_array_len =
                            largest_known_index.map_or(0, |max_index| max_index + 1);

                        if len_required > exact_array_len {
                            // Fill in holes from extending to fit a negative index.
                            for i in exact_array_len..len_required {
                                // There is no unknown, so the exact type "null" can be inserted.
                                collection.known_mut().insert(i.into(), Self::null());
                            }
                        }
                        index += (len_required as isize).max(exact_array_len as isize);
                    }

                    debug_assert!(index >= 0, "all negative cases have been handled");
                    let index = index as usize;

                    let index_exists = collection.known().contains_key(&index.into());
                    if !index_exists {
                        // Add "null" to all holes, adding it to the "unknown" if it exists.
                        // Holes can never be undefined.
                        let hole_type = collection.unknown_kind().without_undefined().or_null();

                        for i in 0..index {
                            collection
                                .known_mut()
                                .entry(i.into())
                                .or_insert_with(|| hole_type.clone());
                        }
                    }

                    let unknown_kind = collection.unknown_kind();
                    collection
                        .known_mut()
                        .entry(index.into())
                        .or_insert(unknown_kind)
                        .insert_recursive(iter, kind);
                }
                BorrowedSegment::Invalid => { /* An invalid path does nothing. */ }
            };
        } else {
            *self = kind;
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use crate::owned_value_path;
    use crate::path::{parse_value_path, OwnedValuePath};
    use crate::value::kind::Collection;

    use super::*;

    #[test]
    #[allow(clippy::too_many_lines)]
    fn test_insert() {
        struct TestCase {
            this: Kind,
            path: OwnedValuePath,
            kind: Kind,
            expected: Kind,
        }

        for (
            title,
            TestCase {
                mut this,
                path,
                kind,
                expected,
            },
        ) in [
            (
                "root insert",
                TestCase {
                    this: Kind::bytes(),
                    path: owned_value_path!(),
                    kind: Kind::integer(),
                    expected: Kind::integer(),
                },
            ),
            (
                "root insert object",
                TestCase {
                    this: Kind::bytes(),
                    path: owned_value_path!(),
                    kind: Kind::object(BTreeMap::from([("a".into(), Kind::integer())])),
                    expected: Kind::object(BTreeMap::from([("a".into(), Kind::integer())])),
                },
            ),
            (
                "empty object insert field",
                TestCase {
                    this: Kind::object(Collection::empty()),
                    path: owned_value_path!("a"),
                    kind: Kind::integer(),
                    expected: Kind::object(BTreeMap::from([("a".into(), Kind::integer())])),
                },
            ),
            (
                "non-empty object insert field",
                TestCase {
                    this: Kind::object(BTreeMap::from([("b".into(), Kind::bytes())])),
                    path: owned_value_path!("a"),
                    kind: Kind::integer(),
                    expected: Kind::object(BTreeMap::from([
                        ("a".into(), Kind::integer()),
                        ("b".into(), Kind::bytes()),
                    ])),
                },
            ),
            (
                "object overwrite field",
                TestCase {
                    this: Kind::object(BTreeMap::from([("a".into(), Kind::bytes())])),
                    path: owned_value_path!("a"),
                    kind: Kind::integer(),
                    expected: Kind::object(BTreeMap::from([("a".into(), Kind::integer())])),
                },
            ),
            (
                "set array index on empty array",
                TestCase {
                    this: Kind::array(Collection::empty()),
                    path: owned_value_path!(0),
                    kind: Kind::integer(),
                    expected: Kind::array(BTreeMap::from([(0.into(), Kind::integer())])),
                },
            ),
            (
                "set array index past the end without unknown",
                TestCase {
                    this: Kind::array(Collection::empty()),
                    path: owned_value_path!(1),
                    kind: Kind::integer(),
                    expected: Kind::array(BTreeMap::from([
                        (0.into(), Kind::null()),
                        (1.into(), Kind::integer()),
                    ])),
                },
            ),
            (
                "set array index past the end with unknown",
                TestCase {
                    this: Kind::array(Collection::empty().with_unknown(Kind::integer())),
                    path: owned_value_path!(1),
                    kind: Kind::bytes(),
                    expected: Kind::array(
                        Collection::from(BTreeMap::from([
                            (0.into(), Kind::integer().or_null()),
                            (1.into(), Kind::bytes()),
                        ]))
                        .with_unknown(Kind::integer()),
                    ),
                },
            ),
            (
                "set array index past the end with unknown, nested",
                TestCase {
                    this: Kind::array(Collection::empty().with_unknown(Kind::integer())),
                    path: owned_value_path!(1, "foo"),
                    kind: Kind::bytes(),
                    expected: Kind::array(
                        Collection::from(BTreeMap::from([
                            (0.into(), Kind::integer().or_null()),
                            (
                                1.into(),
                                Kind::object(BTreeMap::from([("foo".into(), Kind::bytes())])),
                            ),
                        ]))
                        .with_unknown(Kind::integer()),
                    ),
                },
            ),
            (
                "set array index past the end with null unknown",
                TestCase {
                    this: Kind::array(Collection::empty().with_unknown(Kind::null())),
                    path: owned_value_path!(1),
                    kind: Kind::integer(),
                    expected: Kind::array(
                        Collection::from(BTreeMap::from([
                            (0.into(), Kind::null()),
                            (1.into(), Kind::integer()),
                        ]))
                        .with_unknown(Kind::null()),
                    ),
                },
            ),
            (
                "set field on non-object",
                TestCase {
                    this: Kind::integer(),
                    path: owned_value_path!("a"),
                    kind: Kind::integer(),
                    expected: Kind::object(BTreeMap::from([("a".into(), Kind::integer())])),
                },
            ),
            (
                "set array index on non-array",
                TestCase {
                    this: Kind::integer(),
                    path: owned_value_path!(0),
                    kind: Kind::integer(),
                    expected: Kind::array(BTreeMap::from([(0.into(), Kind::integer())])),
                },
            ),
            (
                "set negative array index (no unknown)",
                TestCase {
                    this: Kind::array(BTreeMap::from([
                        (0.into(), Kind::integer()),
                        (1.into(), Kind::integer()),
                    ])),
                    path: owned_value_path!(-1),
                    kind: Kind::bytes(),
                    expected: Kind::array(BTreeMap::from([
                        (0.into(), Kind::integer()),
                        (1.into(), Kind::bytes()),
                    ])),
                },
            ),
            (
                "set negative array index past the end (no unknown)",
                TestCase {
                    this: Kind::array(BTreeMap::from([(0.into(), Kind::integer())])),
                    path: owned_value_path!(-2),
                    kind: Kind::bytes(),
                    expected: Kind::array(BTreeMap::from([
                        (0.into(), Kind::bytes()),
                        (1.into(), Kind::null()),
                    ])),
                },
            ),
            (
                "set negative array index size 1 unknown array",
                TestCase {
                    this: Kind::array(
                        Collection::from(BTreeMap::from([(0.into(), Kind::integer())]))
                            .with_unknown(Kind::integer()),
                    ),
                    path: owned_value_path!(-1),
                    kind: Kind::bytes(),
                    expected: Kind::array(
                        Collection::from(BTreeMap::from([(0.into(), Kind::bytes().or_integer())]))
                            .with_unknown(Kind::integer().or_bytes().or_undefined()),
                    ),
                },
            ),
            (
                "set negative array index empty unknown array",
                TestCase {
                    this: Kind::array(Collection::empty().with_unknown(Kind::integer())),
                    path: owned_value_path!(-1),
                    kind: Kind::bytes(),
                    expected: Kind::array(
                        Collection::from(BTreeMap::from([
                            // we can prove the first index will not be undefined
                            (0.into(), Kind::bytes().or_integer()),
                        ]))
                        .with_unknown(Kind::integer().or_bytes().or_undefined()),
                    ),
                },
            ),
            (
                "set negative array index empty unknown array (2)",
                TestCase {
                    this: Kind::array(Collection::empty().with_unknown(Kind::integer())),
                    path: owned_value_path!(-2),
                    kind: Kind::bytes(),
                    expected: Kind::array(
                        Collection::from(BTreeMap::from([
                            (0.into(), Kind::integer().or_bytes()),
                            // This is the only location a hole could potentially be inserted, so it
                            // is the only index that gets "null", rather than adding it to the
                            // entire unknown type.
                            (1.into(), Kind::integer().or_bytes().or_null()),
                        ]))
                        .with_unknown(Kind::integer().or_bytes().or_undefined()),
                    ),
                },
            ),
            (
                "set negative array index unknown array",
                TestCase {
                    this: Kind::array(
                        Collection::from(BTreeMap::from([
                            // This would be an invalid type without index 0 (it can't be undefined).
                            (0.into(), Kind::integer()),
                            (1.into(), Kind::float()),
                        ]))
                        .with_unknown(Kind::integer()),
                    ),
                    path: owned_value_path!(-3),
                    kind: Kind::bytes(),
                    expected: Kind::array(
                        Collection::from(BTreeMap::from([
                            // Either the unknown (integer) or the inserted value, depending on the actual length.
                            (0.into(), Kind::integer().or_bytes()),
                            // The original float if it wasn't shifted, or bytes/integer if it was shifted.
                            // Can't be a hole.
                            (1.into(), Kind::float().or_bytes().or_integer()),
                            (2.into(), Kind::float().or_bytes().or_integer()),
                        ]))
                        .with_unknown(Kind::integer().or_bytes().or_undefined()),
                    ),
                },
            ),
            (
                "set negative array index unknown array no holes",
                TestCase {
                    this: Kind::array(
                        Collection::from(BTreeMap::from([
                            (0.into(), Kind::float()),
                            (1.into(), Kind::float()),
                            (2.into(), Kind::float()),
                        ]))
                        .with_unknown(Kind::integer()),
                    ),
                    path: owned_value_path!(-3),
                    kind: Kind::bytes(),
                    expected: Kind::array(
                        Collection::from(BTreeMap::from([
                            (0.into(), Kind::float().or_bytes()),
                            (1.into(), Kind::float().or_bytes()),
                            (2.into(), Kind::float().or_bytes()),
                        ]))
                        .with_unknown(Kind::integer().or_bytes().or_undefined()),
                    ),
                },
            ),
            (
                "set negative array index on non-array",
                TestCase {
                    this: Kind::integer(),
                    path: owned_value_path!(-3),
                    kind: Kind::bytes(),
                    expected: Kind::array(Collection::from(BTreeMap::from([
                        (0.into(), Kind::bytes()),
                        (1.into(), Kind::null()),
                        (2.into(), Kind::null()),
                    ]))),
                },
            ),
            (
                "set nested negative array index on unknown array",
                TestCase {
                    this: Kind::array(Collection::empty().with_unknown(Kind::integer())),
                    path: owned_value_path!(-3, "foo"),
                    kind: Kind::bytes(),
                    expected: Kind::array(
                        Collection::from(BTreeMap::from([
                            (
                                0.into(),
                                Kind::integer()
                                    .or_object(BTreeMap::from([("foo".into(), Kind::bytes())])),
                            ),
                            (
                                1.into(),
                                Kind::integer()
                                    .or_null()
                                    .or_object(BTreeMap::from([("foo".into(), Kind::bytes())])),
                            ),
                            (
                                2.into(),
                                Kind::integer()
                                    .or_null()
                                    .or_object(BTreeMap::from([("foo".into(), Kind::bytes())])),
                            ),
                        ]))
                        .with_unknown(
                            Kind::integer()
                                .or_undefined()
                                .or_object(BTreeMap::from([("foo".into(), Kind::bytes())])),
                        ),
                    ),
                },
            ),
            (
                "set nested negative array index on unknown array (no holes)",
                TestCase {
                    this: Kind::array(
                        Collection::from(BTreeMap::from([(0.into(), Kind::integer())]))
                            .with_unknown(Kind::integer()),
                    ),
                    path: owned_value_path!(-1, "foo"),
                    kind: Kind::bytes(),
                    expected: Kind::array(
                        Collection::from(BTreeMap::from([(
                            0.into(),
                            Kind::integer()
                                .or_object(BTreeMap::from([("foo".into(), Kind::bytes())])),
                        )]))
                        .with_unknown(
                            Kind::integer()
                                .or_undefined()
                                .or_object(BTreeMap::from([("foo".into(), Kind::bytes())])),
                        ),
                    ),
                },
            ),
            (
                "insert into never",
                TestCase {
                    this: Kind::never(),
                    path: parse_value_path(".").unwrap(),
                    kind: Kind::bytes(),
                    expected: Kind::bytes(),
                },
            ),
            (
                "insert never",
                TestCase {
                    this: Kind::object(Collection::empty()),
                    path: parse_value_path(".x").unwrap(),
                    kind: Kind::never(),
                    expected: Kind::object(Collection::empty()),
                },
            ),
            (
                "insert undefined",
                TestCase {
                    this: Kind::object(Collection::empty()),
                    path: parse_value_path(".x").unwrap(),
                    kind: Kind::undefined(),
                    expected: Kind::object(BTreeMap::from([("x".into(), Kind::null())])),
                },
            ),
            (
                "array insert into any",
                TestCase {
                    this: Kind::any(),
                    path: owned_value_path!(2),
                    kind: Kind::bytes(),
                    expected: Kind::array(
                        Collection::from(BTreeMap::from([
                            (0.into(), Kind::any().without_undefined()),
                            (1.into(), Kind::any().without_undefined()),
                            (2.into(), Kind::bytes()),
                        ]))
                        .with_unknown(Kind::any()),
                    ),
                },
            ),
            (
                "object insert into any",
                TestCase {
                    this: Kind::any(),
                    path: owned_value_path!("b"),
                    kind: Kind::bytes(),
                    expected: Kind::object(
                        Collection::from(BTreeMap::from([("b".into(), Kind::bytes())]))
                            .with_unknown(Kind::any()),
                    ),
                },
            ),
            (
                "nested object/array insert into any",
                TestCase {
                    this: Kind::any(),
                    path: owned_value_path!("x", 2),
                    kind: Kind::bytes(),
                    expected: Kind::object(
                        Collection::from(BTreeMap::from([(
                            "x".into(),
                            Kind::array(
                                Collection::from(BTreeMap::from([
                                    (0.into(), Kind::any().without_undefined()),
                                    (1.into(), Kind::any().without_undefined()),
                                    (2.into(), Kind::bytes()),
                                ]))
                                .with_unknown(Kind::any()),
                            ),
                        )]))
                        .with_unknown(Kind::any()),
                    ),
                },
            ),
            (
                "nested array/array insert into any",
                TestCase {
                    this: Kind::any(),
                    path: owned_value_path!(0, 0),
                    kind: Kind::bytes(),
                    expected: Kind::array(
                        Collection::from(BTreeMap::from([(
                            0.into(),
                            Kind::array(
                                Collection::from(BTreeMap::from([(0.into(), Kind::bytes())]))
                                    .with_unknown(Kind::any()),
                            ),
                        )]))
                        .with_unknown(Kind::any()),
                    ),
                },
            ),
        ] {
            this.insert(&path, kind);
            assert_eq!(this, expected, "{title}");
        }
    }

    // OBE-10721: a far-away index must not OOM/hang the type-checker, in any of the
    // three branches that materialize one `known` entry per index. Each of these
    // enumerated ~100M entries before the fix. The wall-clock bound is the real
    // assertion — unfixed, each of these takes hours.
    #[test]
    fn far_away_index_does_not_oom() {
        let cases: Vec<(&str, Kind, i64)> = vec![
            // Negative index, collection has a defined `unknown` (shift-simulation path).
            (
                "negative with unknown",
                Kind::array(Collection::empty().with_unknown(Kind::integer())),
                -99_999_999,
            ),
            // Negative index, no `unknown` (exact-position hole-fill path).
            (
                "negative without unknown",
                Kind::array(Collection::from_parts(
                    [(0.into(), Kind::integer()), (1.into(), Kind::integer())].into(),
                    Kind::undefined(),
                )),
                -99_999_999,
            ),
            // Positive index (hole-fill path).
            ("positive", Kind::array(Collection::empty()), 99_999_999),
        ];

        for (name, mut kind, index) in cases {
            let start = std::time::Instant::now();
            kind.insert(&owned_value_path!(index as isize), Kind::bytes());
            let elapsed = start.elapsed();

            assert!(
                elapsed < std::time::Duration::from_secs(5),
                "{name}: insert took {elapsed:?}, expected well under 5s"
            );

            let arr = kind.as_array().expect("result is an array");
            assert!(
                arr.known().len() <= MAX_KNOWN_INDEX_ENTRIES,
                "{}: expected at most {} known entries, got {}",
                name,
                MAX_KNOWN_INDEX_ENTRIES,
                arr.known().len()
            );

            // Soundness: the collapsed `unknown` must still admit everything that
            // could really be at those indices — the inserted kind, the null holes,
            // and any kind that was previously known.
            let unknown = arr.unknown_kind();
            assert!(
                unknown.contains_bytes(),
                "{name}: collapsed unknown must admit the inserted bytes kind"
            );
            assert!(
                unknown.contains_null(),
                "{name}: collapsed unknown must admit null holes"
            );
        }
    }
}
