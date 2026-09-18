//! Stage 47 — A far-behind follower is caught up by snapshot.
//!
//! TODO: this stage is a stub. See README.md, "Adding a stage".

use crate::dist_test;
use crate::stages::{Ladder, Stage, Test};

/// Stage 47.
pub fn stage() -> Stage {
    Stage {
        number: 47,
        slug: "snapshot_catch_up",
        name: "A far-behind follower is caught up by snapshot",
        ext: true,
        ladder: Ladder::Cluster,
        hints: &[
            "When the entries a follower needs have been compacted away, send a snapshot instead",
            "The follower replaces its whole state with the snapshot and continues from its index",
            "After a snapshot install the follower answers the same values as everyone else",
            "Compaction on the leader is what makes this path necessary at all",
        ],
        examples: crate::examples::none,
        tests: vec![Test::new("placeholder", placeholder)],
    }
}

dist_test!(placeholder, |_ctx| { Ok(()) });
