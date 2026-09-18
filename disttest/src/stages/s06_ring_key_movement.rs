//! Stage 06 — Key movement when the ring changes.
//!
//! TODO: this stage is a stub. See README.md, "Adding a stage".

use crate::dist_test;
use crate::stages::{Ladder, Stage, Test};

/// Stage 06.
pub fn stage() -> Stage {
    Stage {
        number: 6,
        slug: "ring_key_movement",
        name: "Key movement when the ring changes",
        ext: true,
        ladder: Ladder::Primitives,
        hints: &[
            "Adding a node may only take keys, never shuffle keys between two nodes that stayed",
            "Removing a node may only give its keys away; everything else stays put",
            "Roughly 1/N of the keys move when the Nth node joins — that is the whole point",
            "Re-adding a node that was removed must restore the mapping exactly",
        ],
        examples: crate::examples::none,
        tests: vec![Test::new("placeholder", placeholder)],
    }
}

dist_test!(placeholder, |_ctx| { Ok(()) });
