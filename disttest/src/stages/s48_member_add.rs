//! Stage 48 — Adding a member.
//!
//! TODO: this stage is a stub. See README.md, "Adding a stage".

use crate::dist_test;
use crate::stages::{Ladder, Stage, Test};

/// Stage 48.
pub fn stage() -> Stage {
    Stage {
        number: 48,
        slug: "member_add",
        name: "Adding a member",
        ext: true,
        ladder: Ladder::Cluster,
        hints: &[
            "POST /v3/cluster/member/add takes the new member's peer URLs and changes the configuration",
            "The new member starts with --initial-cluster-state existing and the full member list",
            "The cluster keeps serving throughout: a configuration change is one more log entry",
            "The new member catches up from the leader and then counts towards the quorum",
        ],
        examples: crate::examples::none,
        tests: vec![Test::new("placeholder", placeholder)],
    }
}

dist_test!(placeholder, |_ctx| { Ok(()) });
