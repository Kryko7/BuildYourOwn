//! Stage 35 — Durability across SIGKILL.
//!
//! TODO: this stage is a stub. See README.md, "Adding a stage".

use crate::dist_test;
use crate::stages::{Ladder, Stage, Test};

/// Stage 35.
pub fn stage() -> Stage {
    Stage {
        number: 35,
        slug: "durability",
        name: "Durability across SIGKILL",
        ext: true,
        ladder: Ladder::Node,
        hints: &[
            "An acknowledged write must survive kill -9 and a restart: fsync before you answer",
            "The data directory must be reopenable without being reformatted",
            "The revision sequence continues where it left off; it never restarts at 1",
            "A torn record at the tail of the log may be dropped, but never an acknowledged write",
        ],
        examples: crate::examples::none,
        tests: vec![Test::new("placeholder", placeholder)],
    }
}

dist_test!(placeholder, |_ctx| { Ok(()) });
