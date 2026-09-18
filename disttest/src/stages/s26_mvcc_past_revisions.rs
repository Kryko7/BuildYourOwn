//! Stage 26 — MVCC: reading at a past revision.
//!
//! TODO: this stage is a stub. See README.md, "Adding a stage".

use crate::dist_test;
use crate::stages::{Ladder, Stage, Test};

/// Stage 26.
pub fn stage() -> Stage {
    Stage {
        number: 26,
        slug: "mvcc_past_revisions",
        name: "MVCC: reading at a past revision",
        ext: true,
        ladder: Ladder::Node,
        hints: &[
            "revision in a range request reads the store as it was at that revision",
            "A key deleted later is still there when read at a revision before the delete",
            "The header of a historical read still reports the store's current revision",
            "Revision 0 means 'now', because 0 is the zero value and cannot mean 'the beginning'",
        ],
        examples: crate::examples::none,
        tests: vec![Test::new("placeholder", placeholder)],
    }
}

dist_test!(placeholder, |_ctx| { Ok(()) });
