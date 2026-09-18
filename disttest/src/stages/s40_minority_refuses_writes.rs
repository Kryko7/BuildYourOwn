//! Stage 40 — The minority of a partition refuses writes.
//!
//! TODO: this stage is a stub. See README.md, "Adding a stage".

use crate::dist_test;
use crate::stages::{Ladder, Stage, Test};

/// Stage 40.
pub fn stage() -> Stage {
    Stage {
        number: 40,
        slug: "minority_refuses_writes",
        name: "The minority of a partition refuses writes",
        ext: true,
        ladder: Ladder::Cluster,
        hints: &[
            "A member that cannot reach a quorum must refuse a write rather than apply it locally",
            "It must also refuse a linearizable read: answering from stale state is the same bug",
            "Refuse with an error the client can see, and do not hang forever",
            "A serializable read may still be answered, and may be stale — that is what it means",
        ],
        examples: crate::examples::none,
        tests: vec![Test::new("placeholder", placeholder)],
    }
}

dist_test!(placeholder, |_ctx| { Ok(()) });
