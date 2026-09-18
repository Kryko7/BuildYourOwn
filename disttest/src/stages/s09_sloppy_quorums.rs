//! Stage 09 — Sloppy quorums and hinted handoff.
//!
//! TODO: this stage is a stub. See README.md, "Adding a stage".

use crate::dist_test;
use crate::stages::{Ladder, Stage, Test};

/// Stage 09.
pub fn stage() -> Stage {
    Stage {
        number: 9,
        slug: "sloppy_quorums",
        name: "Sloppy quorums and hinted handoff",
        ext: true,
        ladder: Ladder::Primitives,
        hints: &[
            "A sloppy quorum accepts a write on a node outside the preference list when a member is down",
            "The stand-in records a hint naming the node the write really belongs to",
            "When the real owner comes back, the hint is handed off and then forgotten",
            "A sloppy quorum is not a quorum: it can lose the overlap guarantee, and must say so",
        ],
        examples: crate::examples::none,
        tests: vec![Test::new("placeholder", placeholder)],
    }
}

dist_test!(placeholder, |_ctx| { Ok(()) });
