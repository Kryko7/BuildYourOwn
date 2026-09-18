//! Stage 37 — A write on any member is visible on every member.
//!
//! TODO: this stage is a stub. See README.md, "Adding a stage".

use crate::dist_test;
use crate::stages::{Ladder, Stage, Test};

/// Stage 37.
pub fn stage() -> Stage {
    Stage {
        number: 37,
        slug: "replicated_writes",
        name: "A write on any member is visible on every member",
        ext: false,
        ladder: Ladder::Cluster,
        hints: &[
            "A write accepted by a follower is forwarded to the leader, not applied locally",
            "The revision a member answers with is the cluster's, not its own counter",
            "Every member must end up holding the same value for the key",
            "Writes through different members still produce one increasing revision sequence",
        ],
        examples: crate::examples::none,
        tests: vec![Test::new("placeholder", placeholder)],
    }
}

dist_test!(placeholder, |_ctx| { Ok(()) });
