//! Stage 42 — A healed partition catches the minority up.
//!
//! TODO: this stage is a stub. See README.md, "Adding a stage".

use crate::dist_test;
use crate::stages::{Ladder, Stage, Test};

/// Stage 42.
pub fn stage() -> Stage {
    Stage {
        number: 42,
        slug: "partition_heals",
        name: "A healed partition catches the minority up",
        ext: true,
        ladder: Ladder::Cluster,
        hints: &[
            "When the link comes back, the stale member learns the entries it missed",
            "It adopts the higher term and steps down if it still thought it was leader",
            "Nothing the minority side accepted may appear in the log afterwards",
            "The catch-up must finish without a restart or an operator",
        ],
        examples: crate::examples::none,
        tests: vec![Test::new("placeholder", placeholder)],
    }
}

dist_test!(placeholder, |_ctx| { Ok(()) });
