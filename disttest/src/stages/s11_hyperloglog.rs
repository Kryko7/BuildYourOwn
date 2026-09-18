//! Stage 11 — HyperLogLog.
//!
//! TODO: this stage is a stub. See README.md, "Adding a stage".

use crate::dist_test;
use crate::stages::{Ladder, Stage, Test};

/// Stage 11.
pub fn stage() -> Stage {
    Stage {
        number: 11,
        slug: "hyperloglog",
        name: "HyperLogLog",
        ext: true,
        ladder: Ladder::Primitives,
        hints: &[
            "Topic `hll`: 2^p registers, each holding the longest run of leading zeros it has seen",
            "The estimate is the harmonic mean of the registers, times alpha times m squared",
            "Small cardinalities need linear counting, or the estimate is badly wrong",
            "The relative error must sit inside 1.04 / sqrt(2^p) over many trials",
        ],
        examples: crate::examples::none,
        tests: vec![Test::new("placeholder", placeholder)],
    }
}

dist_test!(placeholder, |_ctx| { Ok(()) });
