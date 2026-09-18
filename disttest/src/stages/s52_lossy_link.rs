//! Stage 52 — A slow, lossy link degrades throughput, not safety.
//!
//! TODO: this stage is a stub. See README.md, "Adding a stage".

use crate::dist_test;
use crate::stages::{Ladder, Stage, Test};

/// Stage 52.
pub fn stage() -> Stage {
    Stage {
        number: 52,
        slug: "lossy_link",
        name: "A slow, lossy link degrades throughput, not safety",
        ext: true,
        ladder: Ladder::Cluster,
        hints: &[
            "Delay and connection loss slow the cluster down; they never change what it answers",
            "A transport that cannot deliver must retry, not drop the entry",
            "Throughput falls and latency rises: the test records both and asserts neither",
            "Every acknowledged write must still be readable when the link recovers",
        ],
        examples: crate::examples::none,
        tests: vec![Test::new("placeholder", placeholder)],
    }
}

dist_test!(placeholder, |_ctx| { Ok(()) });
