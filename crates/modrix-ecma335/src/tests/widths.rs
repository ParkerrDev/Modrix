// SPDX-License-Identifier: GPL-2.0-only
//! Direct tests on the column-width arithmetic.
//!
//! Everything else in this crate agrees with itself by construction: the
//! writer and the round-trip reader both call [`RowCounts::coded_width`], so a
//! wrong threshold would produce a file that reads back perfectly and is still
//! unreadable by Roslyn. These assertions pin the threshold to hard-coded
//! numbers taken from the spec, with no shared helper in the loop.

use crate::tables::{
    ALL_CODED_INDICES, CUSTOM_ATTRIBUTE_TYPE, HAS_CONSTANT, HAS_CUSTOM_ATTRIBUTE, HAS_SEMANTICS,
    MEMBER_REF_PARENT, RESOLUTION_SCOPE, RowCounts, TYPE_DEF_OR_REF, TYPE_OR_METHOD_DEF, table,
};

#[test]
fn thresholds_are_two_to_the_sixteen_minus_tag_bits() {
    assert_eq!(TYPE_DEF_OR_REF.threshold(), 16_384);
    assert_eq!(HAS_CONSTANT.threshold(), 16_384);
    assert_eq!(RESOLUTION_SCOPE.threshold(), 16_384);
    assert_eq!(MEMBER_REF_PARENT.threshold(), 8_192);
    assert_eq!(CUSTOM_ATTRIBUTE_TYPE.threshold(), 8_192);
    assert_eq!(HAS_CUSTOM_ATTRIBUTE.threshold(), 2_048);
    assert_eq!(HAS_SEMANTICS.threshold(), 32_768);
    assert_eq!(TYPE_OR_METHOD_DEF.threshold(), 32_768);
}

/// The whole point: 65 536 is the threshold for a *simple* index. Using it for
/// a coded index leaves `TypeDefOrRef` two bytes wide for four times too long.
#[test]
fn no_coded_family_widens_at_the_simple_index_threshold() {
    for family in ALL_CODED_INDICES {
        assert!(
            family.threshold() < (1 << 16),
            "{} must widen before 65536",
            family.name
        );
    }
}

#[test]
fn every_family_flips_exactly_at_its_threshold() {
    for family in ALL_CODED_INDICES {
        let threshold = family.threshold();
        for target in family.tags.iter().flatten() {
            let mut counts = RowCounts::default();

            counts.set(*target, threshold.saturating_sub(1));
            assert_eq!(
                counts.coded_width(family),
                2,
                "{} at {} rows of table 0x{target:02X}",
                family.name,
                threshold.saturating_sub(1)
            );

            counts.set(*target, threshold);
            assert_eq!(
                counts.coded_width(family),
                4,
                "{} at {threshold} rows of table 0x{target:02X}",
                family.name
            );

            counts.set(*target, threshold.saturating_add(1));
            assert_eq!(
                counts.coded_width(family),
                4,
                "{} at {} rows of table 0x{target:02X}",
                family.name,
                threshold.saturating_add(1)
            );
        }
    }
}

#[test]
fn simple_indices_flip_at_sixty_five_thousand_five_hundred_and_thirty_six() {
    let mut counts = RowCounts::default();
    counts.set(table::FIELD, 65_535);
    assert_eq!(counts.simple_width(table::FIELD), 2);
    counts.set(table::FIELD, 65_536);
    assert_eq!(counts.simple_width(table::FIELD), 4);
    counts.set(table::FIELD, 65_537);
    assert_eq!(counts.simple_width(table::FIELD), 4);
}

/// A family widens when *any* of its member tables crosses, not only the one
/// a particular row happens to point at.
#[test]
fn a_family_widens_from_any_member_table() {
    let mut counts = RowCounts::default();
    counts.set(table::TYPE_SPEC, TYPE_DEF_OR_REF.threshold());
    assert_eq!(counts.coded_width(TYPE_DEF_OR_REF), 4);
    assert_eq!(counts.coded_width(HAS_CONSTANT), 2);
}

#[test]
fn encode_and_decode_are_inverse() {
    for family in ALL_CODED_INDICES {
        for target in family.tags.iter().flatten() {
            for row in [1_u32, 2, 4095, 16_383, 16_384, 1_000_000] {
                let encoded = family.encode(*target, row).unwrap();
                assert_eq!(family.decode(encoded), Some((*target, row)));
            }
        }
    }
}

#[test]
fn encode_rejects_a_table_outside_the_family() {
    assert!(TYPE_DEF_OR_REF.encode(table::ASSEMBLY_REF, 1).is_err());
    assert!(CUSTOM_ATTRIBUTE_TYPE.encode(table::TYPE_DEF, 1).is_err());
}
