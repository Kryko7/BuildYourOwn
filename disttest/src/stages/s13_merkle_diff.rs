//! Stage 13 — Merkle diff with the fewest round trips.
//!
//! TODO: this stage is a stub. See README.md, "Adding a stage".

use crate::dist_test;
use crate::stages::{Ladder, Stage, Test};

/// Stage 13.
pub fn stage() -> Stage {
    Stage {
        number: 13,
        slug: "merkle_diff",
        name: "Merkle diff with the fewest round trips",
        ext: true,
        ladder: Ladder::Primitives,
        hints: &[
            "Compare roots first: equal roots mean equal trees and no further work",
            "Descend only into subtrees whose hashes differ — that is the whole saving",
            "Report the differing leaves as ranges, so a caller can fetch them in one go",
            "Count the nodes you compared: a walk that visits everything has learnt nothing",
        ],
        examples: crate::examples::none,
        tests: vec![Test::new("placeholder", placeholder)],
    }
}

dist_test!(placeholder, |_ctx| { Ok(()) });
