//! Stage 45 — An old leader rejoins without resurrecting anything.
//!
//! TODO: this stage is a stub. See README.md, "Adding a stage".

use crate::dist_test;
use crate::stages::{Ladder, Stage, Test};

/// Stage 45.
pub fn stage() -> Stage {
    Stage {
        number: 45,
        slug: "old_leader_rejoins",
        name: "An old leader rejoins without resurrecting anything",
        ext: true,
        ladder: Ladder::Cluster,
        hints: &[
            "A leader that was cut off may hold entries no quorum ever accepted",
            "On rejoining it sees a higher term, steps down, and truncates those entries",
            "A client that read from the new majority must never see the old entries appear",
            "Its own view of the key must become the cluster's, not the other way round",
        ],
        examples: crate::examples::none,
        tests: vec![Test::new("placeholder", placeholder)],
    }
}

dist_test!(placeholder, |_ctx| { Ok(()) });
