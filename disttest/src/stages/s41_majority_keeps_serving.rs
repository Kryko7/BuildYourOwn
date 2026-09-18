//! Stage 41 — The majority of a partition keeps serving.
//!
//! TODO: this stage is a stub. See README.md, "Adding a stage".

use crate::dist_test;
use crate::stages::{Ladder, Stage, Test};

/// Stage 41.
pub fn stage() -> Stage {
    Stage {
        number: 41,
        slug: "majority_keeps_serving",
        name: "The majority of a partition keeps serving",
        ext: true,
        ladder: Ladder::Cluster,
        hints: &[
            "A quorum of members is enough: the cluster does not need everyone",
            "The majority side elects a leader among itself if the old one was cut off",
            "Writes keep succeeding on the majority side while the partition stands",
            "The revision keeps advancing, and the minority side knows nothing about it",
        ],
        examples: crate::examples::none,
        tests: vec![Test::new("placeholder", placeholder)],
    }
}

dist_test!(placeholder, |_ctx| { Ok(()) });
