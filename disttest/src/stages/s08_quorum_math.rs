//! Stage 08 — Quorum math: N, R, W and overlap.
//!
//! TODO: this stage is a stub. See README.md, "Adding a stage".

use crate::dist_test;
use crate::stages::{Ladder, Stage, Test};

/// Stage 08.
pub fn stage() -> Stage {
    Stage {
        number: 8,
        slug: "quorum_math",
        name: "Quorum math: N, R, W and overlap",
        ext: false,
        ladder: Ladder::Primitives,
        hints: &[
            "Topic `quorum`: a read and a write quorum overlap when R + W > N",
            "Two writes can conflict when 2W <= N — that is a different question from overlap",
            "The smallest read quorum that always sees the latest write is N - W + 1",
            "Answer for the numbers you were given, not for the defaults",
        ],
        examples: crate::examples::none,
        tests: vec![Test::new("placeholder", placeholder)],
    }
}

dist_test!(placeholder, |_ctx| { Ok(()) });
