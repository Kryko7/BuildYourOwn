//! Stage 39 — Revisions and terms agree across members.
//!
//! TODO: this stage is a stub. See README.md, "Adding a stage".

use crate::dist_test;
use crate::stages::{Ladder, Stage, Test};

/// Stage 39.
pub fn stage() -> Stage {
    Stage {
        number: 39,
        slug: "revision_agreement",
        name: "Revisions and terms agree across members",
        ext: true,
        ladder: Ladder::Cluster,
        hints: &[
            "cluster_id is the same on every member; member_id is not",
            "Revisions are cluster-wide: two members never answer different revisions for the same state",
            "raft_term is the same on every member once an election has settled",
            "raft_applied_index catches up to raft_index on a quiet cluster",
        ],
        examples: crate::examples::none,
        tests: vec![Test::new("placeholder", placeholder)],
    }
}

dist_test!(placeholder, |_ctx| { Ok(()) });
