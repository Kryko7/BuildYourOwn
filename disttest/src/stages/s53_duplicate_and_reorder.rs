//! Stage 53 — Duplicated and reordered peer messages are tolerated.
//!
//! TODO: this stage is a stub. See README.md, "Adding a stage".

use crate::dist_test;
use crate::stages::{Ladder, Stage, Test};

/// Stage 53.
pub fn stage() -> Stage {
    Stage {
        number: 53,
        slug: "duplicate_and_reorder",
        name: "Duplicated and reordered peer messages are tolerated",
        ext: true,
        ladder: Ladder::Cluster,
        hints: &[
            "A retransmitted append must be idempotent: applying it twice changes nothing",
            "An out-of-order append is rejected on its previous index, not applied at the wrong place",
            "Terms and indexes are what make this safe, not the order bytes happen to arrive in",
            "The cluster must keep making progress, not merely avoid corruption",
        ],
        examples: crate::examples::none,
        tests: vec![Test::new("placeholder", placeholder)],
    }
}

dist_test!(placeholder, |_ctx| { Ok(()) });
