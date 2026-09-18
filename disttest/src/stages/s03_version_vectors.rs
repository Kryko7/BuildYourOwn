//! Stage 03 — Version vectors and sibling detection.
//!
//! TODO: this stage is a stub. See README.md, "Adding a stage".

use crate::dist_test;
use crate::stages::{Ladder, Stage, Test};

/// Stage 03.
pub fn stage() -> Stage {
    Stage {
        number: 3,
        slug: "version_vectors",
        name: "Version vectors and sibling detection",
        ext: true,
        ladder: Ladder::Primitives,
        hints: &[
            "Topic `version-vector`: a value carries the vector of the replica that wrote it",
            "A write that does not dominate an existing value creates a sibling instead of replacing it",
            "Sync merges both replicas' sets and drops any value dominated by another",
            "Siblings come back sorted so two replicas that agree answer identically",
        ],
        examples: crate::examples::none,
        tests: vec![Test::new("placeholder", placeholder)],
    }
}

dist_test!(placeholder, |_ctx| { Ok(()) });
