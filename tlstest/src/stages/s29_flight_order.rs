//! Stage 29 — placeholder.
use crate::examples::ExampleSpec;
use crate::stages::{Stage, Test};
use crate::tls_test;

/// Stage definition.
pub fn stage() -> Stage {
    Stage { number: 29, slug: "flight_order", name: "placeholder", ext: false,
        hints: &["a", "b"], examples: examples,
        tests: vec![Test::new("placeholder", nothing)] }
}

tls_test!(nothing, |_ctx| { Ok(()) });

fn examples() -> Vec<ExampleSpec> {
    vec![ExampleSpec::text("x").request("x").response("x")]
}
