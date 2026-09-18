//! Stage 28 — Transactions: compare, success, failure.
//!
//! TODO: this stage is a stub. See README.md, "Adding a stage".

use crate::dist_test;
use crate::stages::{Ladder, Stage, Test};

/// Stage 28.
pub fn stage() -> Stage {
    Stage {
        number: 28,
        slug: "txn_branches",
        name: "Transactions: compare, success, failure",
        ext: false,
        ladder: Ladder::Node,
        hints: &[
            "POST /v3/kv/txn runs the success branch when every comparison holds, the failure one otherwise",
            "succeeded is false when the failure branch ran, and may be omitted rather than sent as false",
            "responses carries one entry per operation of the branch that ran, in order",
            "A transaction is one atomic step: its writes share one revision",
        ],
        examples: crate::examples::none,
        tests: vec![Test::new("placeholder", placeholder)],
    }
}

dist_test!(placeholder, |_ctx| { Ok(()) });
