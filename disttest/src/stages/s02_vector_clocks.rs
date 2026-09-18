//! Stage 02 — Vector clocks and causal comparison.
//!
//! TODO: this stage is a stub. See README.md, "Adding a stage".

use crate::dist_test;
use crate::stages::{Ladder, Stage, Test};

/// Stage 02.
pub fn stage() -> Stage {
    Stage {
        number: 2,
        slug: "vector_clocks",
        name: "Vector clocks and causal comparison",
        ext: false,
        ladder: Ladder::Primitives,
        hints: &[
            "Topic `vector-clock`: a map from process name to counter, and a missing entry is a zero",
            "`cmp` answers before, after, equal or concurrent — four cases, not three",
            "Merging on receive takes the componentwise maximum, then increments the receiver's own entry",
            "Two clocks are concurrent when neither dominates: test both directions",
        ],
        examples: crate::examples::none,
        tests: vec![Test::new("placeholder", placeholder)],
    }
}

dist_test!(placeholder, |_ctx| { Ok(()) });
