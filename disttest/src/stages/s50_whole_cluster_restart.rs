//! Stage 50 — Restarting the whole cluster loses nothing.
//!
//! TODO: this stage is a stub. See README.md, "Adding a stage".

use crate::dist_test;
use crate::stages::{Ladder, Stage, Test};

/// Stage 50.
pub fn stage() -> Stage {
    Stage {
        number: 50,
        slug: "whole_cluster_restart",
        name: "Restarting the whole cluster loses nothing",
        ext: true,
        ladder: Ladder::Cluster,
        hints: &[
            "Every member restarts from its own data directory with no reformatting",
            "The cluster re-elects a leader and continues from the revision it had",
            "Every acknowledged write is still there, with the same revisions",
            "A member that restarts must not rejoin as a brand new one",
        ],
        examples: crate::examples::none,
        tests: vec![Test::new("placeholder", placeholder)],
    }
}

dist_test!(placeholder, |_ctx| { Ok(()) });
