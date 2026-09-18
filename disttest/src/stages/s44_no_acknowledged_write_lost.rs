//! Stage 44 — No acknowledged write is lost.
//!
//! TODO: this stage is a stub. See README.md, "Adding a stage".

use crate::dist_test;
use crate::stages::{Ladder, Stage, Test};

/// Stage 44.
pub fn stage() -> Stage {
    Stage {
        number: 44,
        slug: "no_acknowledged_write_lost",
        name: "No acknowledged write is lost",
        ext: true,
        ladder: Ladder::Cluster,
        hints: &[
            "A write is only acknowledged once a quorum has it on disk",
            "After the leader is killed, every acknowledged write must still be readable",
            "A write that was never acknowledged may be there or not: both are correct",
            "This is the property the whole design exists to provide",
        ],
        examples: crate::examples::none,
        tests: vec![Test::new("placeholder", placeholder)],
    }
}

dist_test!(placeholder, |_ctx| { Ok(()) });
