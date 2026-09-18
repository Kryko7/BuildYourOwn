//! Stage 43 — A killed leader is replaced.
//!
//! TODO: this stage is a stub. See README.md, "Adding a stage".

use crate::dist_test;
use crate::stages::{Ladder, Stage, Test};

/// Stage 43.
pub fn stage() -> Stage {
    Stage {
        number: 43,
        slug: "leader_election",
        name: "A killed leader is replaced",
        ext: true,
        ladder: Ladder::Cluster,
        hints: &[
            "A follower that stops hearing from the leader starts an election",
            "Only a member whose log is at least as up to date may win",
            "The new term is strictly greater than the old one",
            "The cluster must be serving again within a bounded time, not eventually",
        ],
        examples: crate::examples::none,
        tests: vec![Test::new("placeholder", placeholder)],
    }
}

dist_test!(placeholder, |_ctx| { Ok(()) });
