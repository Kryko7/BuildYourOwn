//! Stage 16 — RGA: a replicated sequence.
//!
//! TODO: this stage is a stub. See README.md, "Adding a stage".

use crate::dist_test;
use crate::stages::{Ladder, Stage, Test};

/// Stage 16.
pub fn stage() -> Stage {
    Stage {
        number: 16,
        slug: "rga_sequence",
        name: "RGA: a replicated sequence",
        ext: true,
        ladder: Ladder::Primitives,
        hints: &[
            "Topic `rga`: every element has an id and points at the element it was inserted after",
            "Concurrent inserts at the same position are ordered by id, consistently on every replica",
            "A delete leaves a tombstone: later inserts may still point at it",
            "Every delivery order of the same operations must read back the same sequence",
        ],
        examples: crate::examples::none,
        tests: vec![Test::new("placeholder", placeholder)],
    }
}

dist_test!(placeholder, |_ctx| { Ok(()) });
