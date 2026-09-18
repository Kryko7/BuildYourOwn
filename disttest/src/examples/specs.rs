//! The small constructors a stage file uses to declare its examples.
//!
//! A stage never builds an [`ExampleSpec`] by hand; it calls one of these and chains the
//! three sentences that describe the exchange.

use super::{ClusterStep, ExampleBody, ExampleSpec};
use serde_json::{json, Value};

/// No setup requests.
pub fn no_setup() -> Vec<(String, Value)> {
    Vec::new()
}

/// An empty JSON body, `{}`.
pub fn empty_body() -> Value {
    json!({})
}

/// One request against a single reference node.
pub fn node_example(title: &'static str, path: &'static str, body: fn() -> Value) -> ExampleSpec {
    ExampleSpec::new(
        title,
        ExampleBody::Node {
            setup: no_setup,
            path,
            body,
        },
    )
}

/// One request against a single reference node, after some preparation.
pub fn node_example_after(
    title: &'static str,
    setup: fn() -> Vec<(String, Value)>,
    path: &'static str,
    body: fn() -> Value,
) -> ExampleSpec {
    ExampleSpec::new(title, ExampleBody::Node { setup, path, body })
}

/// A scripted sequence against a three-member reference cluster.
pub fn cluster_example(title: &'static str, steps: fn() -> Vec<ClusterStep>) -> ExampleSpec {
    ExampleSpec::new(title, ExampleBody::Cluster { steps })
}

/// A conversation with the reference primitives CLI.
pub fn prim_example(
    title: &'static str,
    topic: &'static str,
    commands: fn() -> Vec<String>,
) -> ExampleSpec {
    ExampleSpec::new(title, ExampleBody::Primitives { topic, commands })
}

/// A short recorded workload against a three-member reference cluster, with the checker's
/// verdict on the history it produced.
pub fn workload_example(
    title: &'static str,
    duration_ms: u64,
    clients: usize,
    keys: usize,
) -> ExampleSpec {
    ExampleSpec::new(
        title,
        ExampleBody::Workload {
            duration_ms,
            clients,
            keys,
        },
    )
}

/// Turn a list of `&str` into the `Vec<String>` a `commands` function returns.
pub fn lines(items: &[&str]) -> Vec<String> {
    items.iter().map(|s| (*s).to_string()).collect()
}

/// One step of a cluster example.
pub fn step(member: usize, path: &str, body: Value, note: &str) -> ClusterStep {
    ClusterStep::new(member, path, body, note)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::examples::ExampleKind;

    fn cmds() -> Vec<String> {
        lines(&["send a", "get a"])
    }

    #[test]
    fn constructors_pick_the_right_kind() {
        assert_eq!(
            node_example("t", "/v3/kv/put", empty_body).body.kind(),
            ExampleKind::Node
        );
        assert_eq!(
            prim_example("t", "lamport", cmds).body.kind(),
            ExampleKind::Primitives
        );
        assert_eq!(
            workload_example("t", 1000, 2, 2).body.kind(),
            ExampleKind::Workload
        );
    }

    #[test]
    fn a_spec_chains_its_three_sentences() {
        let s = node_example("t", "/v3/kv/put", empty_body)
            .request("ask")
            .response("answer")
            .note("trap");
        assert_eq!(s.request, "ask");
        assert_eq!(s.response, "answer");
        assert_eq!(s.note, Some("trap"));
    }
}
