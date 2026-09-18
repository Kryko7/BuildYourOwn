//! Stage 46 — A long-partitioned follower catches up.
//!
//! TODO: this stage is a stub. See README.md, "Adding a stage".

use crate::dist_test;
use crate::stages::{Ladder, Stage, Test};

/// Stage 46.
pub fn stage() -> Stage {
    Stage {
        number: 46,
        slug: "follower_catch_up",
        name: "A long-partitioned follower catches up",
        ext: true,
        ladder: Ladder::Cluster,
        hints: &[
            "The leader keeps sending from the follower's next index until it catches up",
            "Hundreds of missed writes must arrive without a restart",
            "Until it has caught up, the follower must not answer linearizable reads from stale state",
            "Catching up must not disturb the members that were healthy",
        ],
        examples: crate::examples::none,
        tests: vec![Test::new("placeholder", placeholder)],
    }
}

dist_test!(placeholder, |_ctx| { Ok(()) });
