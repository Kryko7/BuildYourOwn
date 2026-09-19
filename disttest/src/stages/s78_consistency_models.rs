//! Stage 78 — Consistency models: linearizable, sequential, and the gap between them.
//!
//! Almost every argument about a distributed system is really an argument about which
//! history it is allowed to produce, and the words for that are worth owning. Two models do
//! most of the work.
//!
//! **Linearizable**: there is one total order of the operations that reads like a single
//! register, *and* it respects real time — if a finished before b started, a comes first.
//! **Sequential**: the same total order must exist and must read like a register, but only
//! each client's own program order has to be respected. Two clients' operations may be
//! reordered however far apart in wall-clock time they happened.
//!
//! The gap between them is the whole point. A client reads 0 a full second after another
//! client's write of 1 has been acknowledged: not linearizable, because real time says the
//! write came first — but perfectly sequential, because that read can be placed before the
//! write in the total order. Nothing local tells the reader anything is wrong, which is why
//! "our database is strongly consistent" is not a sentence with one meaning.
//!
//! The oracle is a brute-force search the tester runs itself: every ordering of the history
//! that respects the model's constraints, checked against register semantics. Histories are
//! small on purpose, because the point is the definition rather than the search.

use crate::assert::Check;
use crate::dist_test;
use crate::examples::{lines, prim_example, ExampleSpec};
use crate::stages::{Ladder, Stage, Test};
use rand::Rng;

/// Stage 78.
pub fn stage() -> Stage {
    Stage {
        number: 78,
        slug: "consistency_models",
        name: "Consistency models: linearizable against sequential",
        ext: false,
        ladder: Ladder::Algorithms,
        hints: &[
            "Topic `consistency`: `op <client> <read|write> <value> <start> <end>` records one \
             operation, `check <model>` answers whether the history satisfies it",
            "Both models ask the same question — is there a total order of the operations that \
             reads like one register — and differ only in which orderings are allowed",
            "Linearizable respects real time: if a ended before b started, a must come first. \
             Sequential respects only each client's own order",
            "A read returns the value of the most recent write before it in the chosen order, \
             and 0 when there has been no write",
        ],
        examples,
        tests: vec![
            Test::new("an empty history satisfies everything", empty_history),
            Test::new(
                "one write and a read of it is linearizable",
                the_simple_case,
            ),
            Test::new(
                "a read of a value never written is neither",
                a_value_from_nowhere,
            ),
            Test::new(
                "a client that reads its own stale write breaks both models",
                own_stale_write,
            ),
            Test::new(
                "a stale read by another client is sequential but not linearizable",
                the_gap_between_the_models,
            ),
            Test::new(
                "operations that overlap in time may be ordered either way",
                concurrency_allows_both,
            ),
            Test::new(
                "real time constrains only operations that do not overlap",
                real_time_only_binds_disjoint_ops,
            ),
            Test::new(
                "a read that skips back over two writes is not linearizable",
                skipping_backwards,
            ),
            Test::new("an unknown model is refused, not guessed", unknown_model).ext(),
            Test::new(
                "forty seeded histories agree with the tester's own search",
                a_long_seeded_run,
            )
            .ext(),
        ],
    }
}

fn examples() -> Vec<ExampleSpec> {
    vec![
        prim_example("The gap between the two models", "consistency", || {
            lines(&[
                "init",
                "op a write 1 0 10",
                "op b read 0 20 30",
                "check linearizable",
                "check sequential",
            ])
        })
        .request("b reads 0 a full ten ticks after a's write of 1 has been acknowledged")
        .response("`linearizable` is false and `sequential` is true")
        .note(
            "The same four operations, two different answers. Sequential consistency lets \
             b's read be placed before a's write because they are different clients, and \
             nothing in the model cares that the clock says otherwise. This is exactly what \
             a stale read from a follower looks like, and exactly why 'strongly consistent' \
             has to be said more precisely than that.",
        ),
        prim_example("A history no ordering can rescue", "consistency", || {
            lines(&[
                "init",
                "op a write 1 0 10",
                "op a read 0 20 30",
                "check linearizable",
                "check sequential",
            ])
        })
        .request("The same shape, but it is a reading its own earlier write")
        .response("both models are false")
        .note(
            "Program order is the one thing sequential consistency keeps, so a client that \
             fails to see its own completed write cannot be explained by any reordering. \
             That is the difference between a stale replica and a broken one.",
        ),
    ]
}

// ---------------------------------------------------------------------------------------
// The tester's own model
// ---------------------------------------------------------------------------------------

/// One operation in a recorded history.
#[derive(Debug, Clone)]
struct Op {
    client: String,
    write: bool,
    value: i64,
    start: i64,
    end: i64,
}

impl Op {
    fn line(&self) -> String {
        format!(
            "op {} {} {} {} {}",
            self.client,
            if self.write { "write" } else { "read" },
            self.value,
            self.start,
            self.end
        )
    }
}

fn w(client: &str, value: i64, start: i64, end: i64) -> Op {
    Op {
        client: client.into(),
        write: true,
        value,
        start,
        end,
    }
}

fn r(client: &str, value: i64, start: i64, end: i64) -> Op {
    Op {
        client: client.into(),
        write: false,
        value,
        start,
        end,
    }
}

/// Does this order read like one register? A read must return the most recent write's
/// value, and 0 before any write.
fn register_legal(order: &[&Op]) -> bool {
    let mut current = 0i64;
    for op in order {
        if op.write {
            current = op.value;
        } else if op.value != current {
            return false;
        }
    }
    true
}

/// Is there an ordering respecting `precedes` that reads like a register?
fn exists_legal_order(ops: &[Op], precedes: &dyn Fn(&Op, &Op) -> bool) -> bool {
    fn go<'a>(
        ops: &'a [Op],
        placed: &mut Vec<bool>,
        order: &mut Vec<&'a Op>,
        precedes: &dyn Fn(&Op, &Op) -> bool,
    ) -> bool {
        if order.len() == ops.len() {
            return register_legal(order);
        }
        for i in 0..ops.len() {
            if placed[i] {
                continue;
            }
            if (0..ops.len()).any(|j| !placed[j] && j != i && precedes(&ops[j], &ops[i])) {
                continue;
            }
            placed[i] = true;
            order.push(&ops[i]);
            if register_legal(order) && go(ops, placed, order, precedes) {
                return true;
            }
            order.pop();
            placed[i] = false;
        }
        false
    }
    let mut placed = vec![false; ops.len()];
    let mut order = Vec::with_capacity(ops.len());
    go(ops, &mut placed, &mut order, precedes)
}

fn linearizable(ops: &[Op]) -> bool {
    exists_legal_order(ops, &|a, b| a.end < b.start)
}

fn sequential(ops: &[Op]) -> bool {
    exists_legal_order(ops, &|a, b| a.client == b.client && a.end < b.start)
}

/// Feed a history to the program and ask it about one model.
async fn ask(
    ctx: &mut crate::stages::Ctx,
    ops: &[Op],
    model: &str,
) -> Result<bool, crate::stages::Failure> {
    let p = ctx.prim("consistency").await?;
    p.send("init").await?;
    for op in ops {
        p.send(&op.line()).await?;
    }
    let answer = p.send(&format!("check {model}")).await?;
    p.expect_bool(&answer, "check", "holds")
}

// ---------------------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------------------

dist_test!(empty_history, |ctx| {
    let lin = ask(ctx, &[], "linearizable").await?;
    let seq = ask(ctx, &[], "sequential").await?;
    let mut c = Check::new("a history with nothing in it");
    c.note(
        "Vacuously true, and worth pinning down: a model is a statement about the orderings \
         that exist, and with no operations the empty ordering satisfies both.",
    );
    c.eq("check linearizable", true, lin);
    c.eq("check sequential", true, seq);
    c.finish()
});

dist_test!(the_simple_case, |ctx| {
    let ops = vec![w("a", 1, 0, 10), r("b", 1, 20, 30)];
    let lin = ask(ctx, &ops, "linearizable").await?;
    let seq = ask(ctx, &ops, "sequential").await?;
    let mut c = Check::new("a write, then a read of it");
    c.note("The uninteresting case, which still has to come out right.");
    c.eq("check linearizable", linearizable(&ops), lin);
    c.eq("check sequential", sequential(&ops), seq);
    c.eq(
        "the tester's own model says linearizable",
        true,
        linearizable(&ops),
    );
    c.finish()
});

dist_test!(a_value_from_nowhere, |ctx| {
    let ops = vec![w("a", 1, 0, 10), r("b", 7, 20, 30)];
    let lin = ask(ctx, &ops, "linearizable").await?;
    let seq = ask(ctx, &ops, "sequential").await?;
    let mut c = Check::new("a read of a value nobody wrote");
    c.note(
        "No ordering can produce it, so no model accepts it. A checker that only compares \
         timestamps and never checks register semantics passes this and should not.",
    );
    c.eq("check linearizable", false, lin);
    c.eq("check sequential", false, seq);
    c.finish()
});

dist_test!(own_stale_write, |ctx| {
    let ops = vec![w("a", 1, 0, 10), r("a", 0, 20, 30)];
    let lin = ask(ctx, &ops, "linearizable").await?;
    let seq = ask(ctx, &ops, "sequential").await?;
    let mut c = Check::new("a client that cannot see its own write");
    c.note(
        "Sequential consistency keeps each client's program order, so this cannot be \
         reordered away. It is the one thing even the weaker model refuses, and it is what \
         read-your-writes is named after.",
    );
    c.eq("check linearizable", false, lin);
    c.eq("check sequential", false, seq);
    c.finish()
});

dist_test!(the_gap_between_the_models, |ctx| {
    let ops = vec![w("a", 1, 0, 10), r("b", 0, 20, 30)];
    let lin = ask(ctx, &ops, "linearizable").await?;
    let seq = ask(ctx, &ops, "sequential").await?;
    let mut c = Check::new("a stale read by a different client");
    c.note(
        "This is the stage in one test. b's read is placed before a's write in the total \
         order, which sequential consistency permits and real time forbids. Every follower \
         read that has not caught up looks exactly like this.",
    );
    c.eq("check linearizable", false, lin);
    c.eq("check sequential", true, seq);
    c.eq(
        "and the tester's own model agrees",
        (false, true),
        (linearizable(&ops), sequential(&ops)),
    );
    c.finish()
});

dist_test!(concurrency_allows_both, |ctx| {
    // The read overlaps the write, so either order is available.
    let saw_new = vec![w("a", 1, 0, 20), r("b", 1, 5, 15)];
    let saw_old = vec![w("a", 1, 0, 20), r("b", 0, 5, 15)];
    let new_ok = ask(ctx, &saw_new, "linearizable").await?;
    let old_ok = ask(ctx, &saw_old, "linearizable").await?;
    let mut c = Check::new("overlapping operations");
    c.note(
        "While a write is in flight, a concurrent read may return either the old value or \
         the new one and the history is linearizable both ways. Linearizability constrains \
         what happens to operations that do *not* overlap; it says nothing about which of \
         two concurrent operations appears to happen first.",
    );
    c.eq("the read that saw the new value", true, new_ok);
    c.eq("the read that saw the old value", true, old_ok);
    c.finish()
});

dist_test!(real_time_only_binds_disjoint_ops, |ctx| {
    // Same values, the only change is that the read is moved to after the write finished.
    let overlapping = vec![w("a", 1, 0, 20), r("b", 0, 5, 15)];
    let disjoint = vec![w("a", 1, 0, 20), r("b", 0, 25, 35)];
    let a = ask(ctx, &overlapping, "linearizable").await?;
    let b = ask(ctx, &disjoint, "linearizable").await?;
    let mut c = Check::new("the same read, moved later");
    c.note(
        "One number changed — when the read started — and the verdict flips. That number is \
         the entire content of the real-time constraint, and it is why linearizability is a \
         property of a history rather than of an implementation.",
    );
    c.eq("while the write was in flight", true, a);
    c.eq("after the write was acknowledged", false, b);
    c.finish()
});

dist_test!(skipping_backwards, |ctx| {
    let ops = vec![w("a", 1, 0, 10), w("a", 2, 11, 20), r("b", 1, 30, 40)];
    let lin = ask(ctx, &ops, "linearizable").await?;
    let seq = ask(ctx, &ops, "sequential").await?;
    let mut c = Check::new("a read that goes back one version");
    c.note(
        "Both writes are a's, so their order is fixed even under sequential consistency — \
         but b's read can still be slipped in between them. Reading 1 after 2 was \
         acknowledged is therefore sequential and not linearizable, which is the shape of a \
         replica that is exactly one write behind.",
    );
    c.eq("check linearizable", false, lin);
    c.eq("check sequential", true, seq);
    c.finish()
});

dist_test!(unknown_model, |ctx| {
    let p = ctx.prim("consistency").await?;
    p.send("init").await?;
    let answer = p.send("check eventual-ish").await?;
    let transcript = p.transcript_block();
    let mut c = Check::new("a model the program does not know");
    c.note(
        "Consistency models are a long list and this topic implements two of them. A name \
         outside the list is an error, not a default — answering `true` to a question you \
         did not understand is the one reply a checker must never give.",
    );
    c.eq(
        "check eventual-ish → ok",
        false,
        answer["ok"] == serde_json::Value::Bool(true),
    );
    c.block("transcript", transcript);
    c.finish()
});

dist_test!(a_long_seeded_run, |ctx| {
    let mut c = Check::new("forty seeded histories");
    c.note(
        "Each history is three or four operations over two clients with times drawn from the \
         seed, classified independently by the tester's own search and by the program. \
         Disagreements are reported with the history that caused them.",
    );
    let mut disagreements: Vec<String> = Vec::new();
    for _ in 0..40 {
        let n = ctx.rng.random_range(2..=4);
        let mut ops: Vec<Op> = Vec::new();
        let mut t = 0i64;
        for _ in 0..n {
            let client = if ctx.rng.random_bool(0.5) { "a" } else { "b" };
            let start = t + ctx.rng.random_range(0..3);
            let end = start + ctx.rng.random_range(1..4);
            t = end;
            if ctx.rng.random_bool(0.5) {
                ops.push(w(client, ctx.rng.random_range(1..=3), start, end));
            } else {
                ops.push(r(client, ctx.rng.random_range(0..=3), start, end));
            }
        }
        for model in ["linearizable", "sequential"] {
            let want = if model == "linearizable" {
                linearizable(&ops)
            } else {
                sequential(&ops)
            };
            let got = ask(ctx, &ops, model).await?;
            if want != got {
                disagreements.push(format!(
                    "{model}: expected {want}, got {got}, for [{}]",
                    ops.iter().map(|o| o.line()).collect::<Vec<_>>().join("; ")
                ));
            }
        }
    }
    c.eq(
        "histories the program classified differently",
        Vec::<String>::new(),
        disagreements,
    );
    c.finish()
});
