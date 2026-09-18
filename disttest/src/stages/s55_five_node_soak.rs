//! Stage 55 — Five members, mixed faults, and nothing left behind.
//!
//! TODO: this stage is a stub. See README.md, "Adding a stage".

use crate::dist_test;
use crate::stages::{Ladder, Stage, Test};

/// Stage 55.
pub fn stage() -> Stage {
    Stage {
        number: 55,
        slug: "five_node_soak",
        name: "Five members, mixed faults, and nothing left behind",
        ext: true,
        ladder: Ladder::Cluster,
        hints: &[
            "Five members tolerate two failures; the quorum arithmetic is the only thing that changes",
            "Partitions, kills, delays and message mangling all in one run",
            "After the faults heal, every member must converge on the same values",
            "No process, port or temporary file may survive the end of the run",
        ],
        examples: crate::examples::none,
        tests: vec![Test::new("placeholder", placeholder)],
    }
}

dist_test!(placeholder, |_ctx| { Ok(()) });
