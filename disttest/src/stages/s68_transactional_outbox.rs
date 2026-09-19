//! Stage 68 — Transactional outbox.
//!
//! Two things have to happen together: a row goes into the database, and an event about it
//! goes onto the bus. They live in different systems, so there is no transaction that spans
//! them — and the window between the two is exactly where the process dies. The outbox
//! pattern closes the window by refusing to span anything: the event is written to a table
//! in the *same* transaction as the row, and a relay drains that table afterwards.
//!
//! The oracle is the atomicity claim, tested from both sides. A naive write followed by a
//! crash after the row must lose the event; a transactional write followed by the same
//! crash must lose nothing, and a crash before the commit must lose both rather than one.
//! The second half of the stage is the honest small print: draining the table is
//! at-least-once, so the same event reaches the bus twice when the relay dies before its
//! acknowledgement, and it is stage 69's consumer, not this pattern, that makes that safe.

use crate::assert::Check;
use crate::dist_test;
use crate::examples::{lines, prim_example, ExampleSpec};
use crate::stages::{Ladder, Stage, Test};
use rand::Rng;
use std::collections::BTreeMap;

/// Stage 68.
pub fn stage() -> Stage {
    Stage {
        number: 68,
        slug: "transactional_outbox",
        name: "Transactional outbox",
        ext: false,
        ladder: Ladder::Algorithms,
        hints: &[
            "Topic `outbox`: the row and its event record commit in one transaction",
            "A relay drains the table afterwards; publishing is not part of the write",
            "A crash between publishing and acknowledging leaves the record, so it is resent",
            "The outbox buys 'never lost'; only a deduplicating consumer buys 'never twice'",
        ],
        examples,
        tests: vec![
            Test::new(
                "a naive write loses its event when the process dies after the row",
                a_naive_write_loses_the_event,
            ),
            Test::new(
                "the row and the outbox record commit together",
                the_row_and_the_record_commit_together,
            ),
            Test::new(
                "a crash before the commit loses both, never one",
                a_crash_before_the_commit_loses_both,
            ),
            Test::new(
                "the relay publishes what the crash left behind",
                the_relay_publishes_what_survived,
            ),
            Test::new(
                "an unacknowledged publish is sent again after recovery",
                an_unacked_publish_is_sent_again,
            ),
            Test::new(
                "an acknowledged record is never sent again",
                an_acked_record_is_not_resent,
            ),
            Test::new(
                "the relay drains the outbox in insertion order",
                the_relay_drains_in_order,
            ),
            Test::new(
                "the outbox alone is at-least-once, not exactly-once",
                the_outbox_is_at_least_once,
            )
            .ext(),
            Test::new(
                "a crashed process refuses work until it has recovered",
                a_crashed_process_refuses_work,
            )
            .ext(),
            Test::new(
                "a seeded run of writes and crashes never loses a transactional event",
                a_seeded_run_never_loses_an_event,
            )
            .ext(),
        ],
    }
}

fn examples() -> Vec<ExampleSpec> {
    vec![
        prim_example("The window the pattern exists to close", "outbox", || {
            lines(&[
                "init",
                "naive-write k1 v1 e1",
                "crash after-row",
                "recover",
                "state",
            ])
        })
        .request("a row committed, the event published separately, and a crash in between")
        .response("the row is there and the event is gone: lost 1")
        .note(
            "Nothing was retried badly and no message was dropped by the broker. The row \
             simply committed, and the process died before the publish call was made. Every \
             'write to the database, then publish' handler has this window, and it is only \
             ever as narrow as the two operations are close together.",
        ),
        prim_example("The same crash with an outbox", "outbox", || {
            lines(&[
                "init",
                "write k1 v1 e1",
                "crash after-row",
                "recover",
                "publish",
            ])
        })
        .request("a row and its event record committed together, then the same crash")
        .response("lost 0: the record survived, and the relay publishes it on recovery")
        .note(
            "The event is not published by the writer at all, which is the trick — there is \
             no second system to fail against, only another table in the one transaction. \
             The publish becomes someone else's problem, and that someone can retry for as \
             long as it likes.",
        ),
    ]
}

// ---------------------------------------------------------------------------------------
// The tester's own model of the rules. Nothing below consults the program for an answer.
// ---------------------------------------------------------------------------------------

/// What the most recent write did, so a crash rewinds to the right point.
#[derive(Clone)]
enum Last {
    /// A `write`: the row and the record were one transaction.
    Transactional { key: String, prev: Option<String> },
    /// A `naive-write`: the row committed, then the event was published on its own.
    Naive {
        key: String,
        prev: Option<String>,
        event: String,
    },
}

/// The store, its outbox table and the bus, as the specification describes them.
struct Model {
    rows: BTreeMap<String, String>,
    /// Each record and whether the relay has already put it on the bus.
    outbox: Vec<(String, bool)>,
    published: Vec<String>,
    lost: Vec<String>,
    down: bool,
    last: Option<Last>,
}

impl Model {
    fn new() -> Model {
        Model {
            rows: BTreeMap::new(),
            outbox: Vec::new(),
            published: Vec::new(),
            lost: Vec::new(),
            down: false,
            last: None,
        }
    }

    fn write(&mut self, key: &str, value: &str, event: &str) {
        let prev = self.rows.insert(key.to_string(), value.to_string());
        self.outbox.push((event.to_string(), false));
        self.last = Some(Last::Transactional {
            key: key.to_string(),
            prev,
        });
    }

    fn naive_write(&mut self, key: &str, value: &str, event: &str) {
        let prev = self.rows.insert(key.to_string(), value.to_string());
        self.published.push(event.to_string());
        self.last = Some(Last::Naive {
            key: key.to_string(),
            prev,
            event: event.to_string(),
        });
    }

    fn crash(&mut self, where_: &str) {
        for r in &mut self.outbox {
            r.1 = false;
        }
        self.down = true;
        let last = self.last.take();
        match (where_, last) {
            ("after-row", Some(Last::Naive { event, .. })) => {
                if let Some(i) = self.published.iter().rposition(|e| *e == event) {
                    self.published.remove(i);
                }
                self.lost.push(event);
            }
            ("before-commit", Some(Last::Naive { key, prev, event })) => {
                self.restore(&key, prev);
                if let Some(i) = self.published.iter().rposition(|e| *e == event) {
                    self.published.remove(i);
                }
            }
            ("before-commit", Some(Last::Transactional { key, prev })) => {
                self.restore(&key, prev);
                self.outbox.pop();
            }
            _ => {}
        }
    }

    fn restore(&mut self, key: &str, prev: Option<String>) {
        match prev {
            Some(v) => {
                self.rows.insert(key.to_string(), v);
            }
            None => {
                self.rows.remove(key);
            }
        }
    }

    fn publish(&mut self) -> Vec<String> {
        let mut sent = Vec::new();
        for r in &mut self.outbox {
            if !r.1 {
                r.1 = true;
                sent.push(r.0.clone());
            }
        }
        self.published.extend(sent.iter().cloned());
        sent
    }

    fn ack(&mut self, event: &str) {
        if let Some(i) = self.outbox.iter().position(|r| r.1 && r.0 == event) {
            self.outbox.remove(i);
        }
    }

    fn outbox_events(&self) -> Vec<String> {
        self.outbox.iter().map(|r| r.0.clone()).collect()
    }
}

/// One step of the seeded run: the command, and the three lists it must leave behind.
type Step = (String, Vec<String>, Vec<String>, Vec<String>);

// ---------------------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------------------

dist_test!(a_naive_write_loses_the_event, |ctx| {
    let p = ctx.prim("outbox").await?;
    p.send("init").await?;
    p.send("naive-write k1 v1 e1").await?;
    // The row committed; the process died before the publish call was ever made. No broker
    // dropped anything, and no retry was mishandled: the window is simply there.
    p.send("crash after-row").await?;
    let recovered = p.send("recover").await?;
    let state = p.send("state").await?;
    let delivered = p.send("delivered e1").await?;
    let mut c = Check::new("a row committed and an event published separately");
    c.eq(
        "recover.rows",
        1,
        p.expect_i64(&recovered, "recover", "rows")?,
    );
    c.eq(
        "recover.lost",
        1,
        p.expect_i64(&recovered, "recover", "lost")?,
    );
    c.eq(
        "recover.published",
        0,
        p.expect_i64(&recovered, "recover", "published")?,
    );
    c.eq(
        "state.lost",
        vec!["e1".to_string()],
        p.expect_strs(&state, "state", "lost")?,
    );
    c.eq(
        "delivered(e1).count",
        0,
        p.expect_i64(&delivered, "delivered e1", "count")?,
    );
    c.block("transcript", p.transcript_block());
    c.finish()
});

dist_test!(the_row_and_the_record_commit_together, |ctx| {
    let p = ctx.prim("outbox").await?;
    p.send("init").await?;
    let written = p.send("write k1 v1 e1").await?;
    p.send("crash after-row").await?;
    let recovered = p.send("recover").await?;
    let state = p.send("state").await?;
    let mut c = Check::new("a row and its event record written in one transaction");
    c.eq(
        "write.outbox",
        1,
        p.expect_i64(&written, "write", "outbox")?,
    );
    c.eq(
        "recover.rows",
        1,
        p.expect_i64(&recovered, "recover", "rows")?,
    );
    // The same crash that lost the event a moment ago now loses nothing, because there was
    // never a second system for the writer to fail against.
    c.eq(
        "recover.outbox",
        1,
        p.expect_i64(&recovered, "recover", "outbox")?,
    );
    c.eq(
        "recover.lost",
        0,
        p.expect_i64(&recovered, "recover", "lost")?,
    );
    c.eq(
        "state.outbox",
        vec!["e1".to_string()],
        p.expect_strs(&state, "state", "outbox")?,
    );
    c.eq(
        "state.lost",
        Vec::<String>::new(),
        p.expect_strs(&state, "state", "lost")?,
    );
    c.block("transcript", p.transcript_block());
    c.finish()
});

dist_test!(a_crash_before_the_commit_loses_both, |ctx| {
    let p = ctx.prim("outbox").await?;
    p.send("init").await?;
    p.send("write k1 v1 e1").await?;
    p.send("crash before-commit").await?;
    let transactional = p.send("recover").await?;
    // The other direction: the naive path is atomic in neither, so the same crash point
    // leaves nothing behind there either. What distinguishes the two is where they are
    // *not* atomic, which is why both directions are worth asserting.
    p.send("naive-write k2 v2 e2").await?;
    p.send("crash before-commit").await?;
    let naive = p.send("recover").await?;
    let state = p.send("state").await?;
    let mut c = Check::new("a crash inside the transaction itself");
    c.eq(
        "recover.rows after the transactional write",
        0,
        p.expect_i64(&transactional, "recover", "rows")?,
    );
    c.eq(
        "recover.outbox after the transactional write",
        0,
        p.expect_i64(&transactional, "recover", "outbox")?,
    );
    c.eq(
        "recover.rows after the naive write",
        0,
        p.expect_i64(&naive, "recover", "rows")?,
    );
    c.eq(
        "recover.published after the naive write",
        0,
        p.expect_i64(&naive, "recover", "published")?,
    );
    c.eq("recover.lost", 0, p.expect_i64(&naive, "recover", "lost")?);
    c.eq(
        "state.outbox",
        Vec::<String>::new(),
        p.expect_strs(&state, "state", "outbox")?,
    );
    c.block("transcript", p.transcript_block());
    c.finish()
});

dist_test!(the_relay_publishes_what_survived, |ctx| {
    let p = ctx.prim("outbox").await?;
    p.send("init").await?;
    p.send("write k1 v1 e1").await?;
    p.send("crash after-row").await?;
    p.send("recover").await?;
    let published = p.send("publish").await?;
    let delivered = p.send("delivered e1").await?;
    let mut c = Check::new("the relay draining a table that survived a crash");
    c.eq(
        "publish.published",
        vec!["e1".to_string()],
        p.expect_strs(&published, "publish", "published")?,
    );
    // The record stays until it is acknowledged: publishing is not the same as being sure.
    c.eq(
        "publish.outbox",
        1,
        p.expect_i64(&published, "publish", "outbox")?,
    );
    c.eq(
        "publish.unacked",
        1,
        p.expect_i64(&published, "publish", "unacked")?,
    );
    c.eq(
        "delivered(e1).count",
        1,
        p.expect_i64(&delivered, "delivered e1", "count")?,
    );
    c.block("transcript", p.transcript_block());
    c.finish()
});

dist_test!(an_unacked_publish_is_sent_again, |ctx| {
    let p = ctx.prim("outbox").await?;
    p.send("init").await?;
    p.send("write k1 v1 e1").await?;
    p.send("publish").await?;
    // The relay put the event on the bus and then died before it could mark the record
    // done. The record is still there, so the next relay sends it again.
    p.send("crash now").await?;
    let recovered = p.send("recover").await?;
    let second = p.send("publish").await?;
    let delivered = p.send("delivered e1").await?;
    let mut c = Check::new("a relay that crashed between publishing and acknowledging");
    c.eq(
        "recover.outbox",
        1,
        p.expect_i64(&recovered, "recover", "outbox")?,
    );
    c.eq(
        "the second publish.published",
        vec!["e1".to_string()],
        p.expect_strs(&second, "publish", "published")?,
    );
    c.eq(
        "delivered(e1).count",
        2,
        p.expect_i64(&delivered, "delivered e1", "count")?,
    );
    c.eq(
        "delivered(e1).distinct",
        1,
        p.expect_i64(&delivered, "delivered e1", "distinct")?,
    );
    c.block("transcript", p.transcript_block());
    c.finish()
});

dist_test!(an_acked_record_is_not_resent, |ctx| {
    let p = ctx.prim("outbox").await?;
    p.send("init").await?;
    p.send("write k1 v1 e1").await?;
    p.send("publish").await?;
    let acked = p.send("ack e1").await?;
    let again = p.send("publish").await?;
    let delivered = p.send("delivered e1").await?;
    let mut c = Check::new("a record whose publish was acknowledged");
    c.eq(
        "ack(e1).outbox",
        0,
        p.expect_i64(&acked, "ack e1", "outbox")?,
    );
    c.eq(
        "ack(e1).unacked",
        0,
        p.expect_i64(&acked, "ack e1", "unacked")?,
    );
    c.eq(
        "the second publish.published",
        Vec::<String>::new(),
        p.expect_strs(&again, "publish", "published")?,
    );
    c.eq(
        "delivered(e1).count",
        1,
        p.expect_i64(&delivered, "delivered e1", "count")?,
    );
    c.block("transcript", p.transcript_block());
    c.finish()
});

dist_test!(the_relay_drains_in_order, |ctx| {
    let p = ctx.prim("outbox").await?;
    p.send("init").await?;
    for (i, event) in ["e1", "e2", "e3", "e4"].iter().enumerate() {
        p.send(&format!("write k{i} v{i} {event}")).await?;
    }
    let published = p.send("publish").await?;
    let state = p.send("state").await?;
    let mut c = Check::new("four records drained in one pass");
    let order: Vec<String> = ["e1", "e2", "e3", "e4"]
        .iter()
        .map(|e| (*e).to_string())
        .collect();
    // The table is the order the transactions committed in, and a consumer that relies on
    // per-aggregate ordering gets nothing from an outbox that drains it arbitrarily.
    c.eq(
        "publish.published",
        order.clone(),
        p.expect_strs(&published, "publish", "published")?,
    );
    c.eq(
        "state.published",
        order,
        p.expect_strs(&state, "state", "published")?,
    );
    c.block("transcript", p.transcript_block());
    c.finish()
});

dist_test!(the_outbox_is_at_least_once, |ctx| {
    let p = ctx.prim("outbox").await?;
    p.send("init").await?;
    p.send("write k1 v1 e1").await?;
    // Three relays in a row, each dying before its acknowledgement: three copies on the bus
    // of one event that was written once.
    for _ in 0..3 {
        p.send("publish").await?;
        p.send("crash now").await?;
        p.send("recover").await?;
    }
    let delivered = p.send("delivered e1").await?;
    let state = p.send("state").await?;
    let mut c = Check::new("one write, three publishes");
    c.eq(
        "delivered(e1).count",
        3,
        p.expect_i64(&delivered, "delivered e1", "count")?,
    );
    // The outbox guarantees the event is never lost. It says nothing at all about the event
    // arriving once, and it cannot: the acknowledgement is the thing that keeps going
    // missing. That half of exactly-once belongs to the consumer, in stage 69.
    c.eq(
        "delivered(e1).distinct",
        1,
        p.expect_i64(&delivered, "delivered e1", "distinct")?,
    );
    c.eq(
        "state.lost",
        Vec::<String>::new(),
        p.expect_strs(&state, "state", "lost")?,
    );
    c.eq(
        "state.outbox",
        vec!["e1".to_string()],
        p.expect_strs(&state, "state", "outbox")?,
    );
    c.block("transcript", p.transcript_block());
    c.finish()
});

dist_test!(a_crashed_process_refuses_work, |ctx| {
    let p = ctx.prim("outbox").await?;
    p.send("init").await?;
    p.send("write k1 v1 e1").await?;
    p.send("crash now").await?;
    let while_down = p.send("write k2 v2 e2").await?;
    let publish_down = p.send("publish").await?;
    p.send("recover").await?;
    let after = p.send("write k2 v2 e2").await?;
    let mut c = Check::new("work offered to a process that has not recovered yet");
    // A crashed process accepting a write would be a test that never proves anything about
    // durability, because nothing would ever be at risk.
    c.eq(
        "write while down, ok",
        false,
        p.expect_bool(&while_down, "write k2 v2 e2", "ok")?,
    );
    c.eq(
        "publish while down, ok",
        false,
        p.expect_bool(&publish_down, "publish", "ok")?,
    );
    c.eq(
        "write after recovery, ok",
        true,
        p.expect_bool(&after, "write k2 v2 e2", "ok")?,
    );
    c.eq(
        "write after recovery, outbox",
        2,
        p.expect_i64(&after, "write k2 v2 e2", "outbox")?,
    );
    c.block("transcript", p.transcript_block());
    c.finish()
});

dist_test!(a_seeded_run_never_loses_an_event, |ctx| {
    // A seeded sequence of writes, relays and crashes. Every command and the state it must
    // leave behind is worked out first, from the rules alone.
    let mut model = Model::new();
    let mut script: Vec<Step> = Vec::new();
    let mut transactional: Vec<String> = Vec::new();
    for i in 0..60 {
        let event = format!("e{i}");
        let command = if model.down {
            // Only two things can be asked of a process that is down.
            model.down = false;
            "recover".to_string()
        } else {
            match ctx.rng.random_range(0..6) {
                0 => {
                    model.naive_write(&format!("k{i}"), "v", &event);
                    format!("naive-write k{i} v {event}")
                }
                1 => {
                    let where_ = if ctx.rng.random_bool(0.5) {
                        "after-row"
                    } else {
                        "now"
                    };
                    model.crash(where_);
                    format!("crash {where_}")
                }
                2 => {
                    model.publish();
                    "publish".to_string()
                }
                3 => match model.outbox_events().first().cloned() {
                    Some(e) => {
                        model.ack(&e);
                        format!("ack {e}")
                    }
                    None => {
                        model.publish();
                        "publish".to_string()
                    }
                },
                _ => {
                    model.write(&format!("k{i}"), "v", &event);
                    transactional.push(event.clone());
                    format!("write k{i} v {event}")
                }
            }
        };
        script.push((
            command,
            model.outbox_events(),
            model.published.clone(),
            model.lost.clone(),
        ));
    }
    let seed = ctx.seed;
    let p = ctx.prim("outbox").await?;
    p.send("init").await?;
    let mut actual: Vec<(Vec<String>, Vec<String>, Vec<String>)> = Vec::new();
    for (command, ..) in &script {
        p.send(command).await?;
        let s = p.send("state").await?;
        actual.push((
            p.expect_strs(&s, "state", "outbox")?,
            p.expect_strs(&s, "state", "published")?,
            p.expect_strs(&s, "state", "lost")?,
        ));
    }
    let transcript = p.transcript_block();
    let (last_outbox, last_published, last_lost) = actual
        .last()
        .cloned()
        .unwrap_or_else(|| (Vec::new(), Vec::new(), Vec::new()));
    let mut c = Check::new("sixty seeded writes, relays and crashes");
    c.note(format!(
        "seed {seed}, {} commands, {} transactional writes",
        script.len(),
        transactional.len()
    ));
    for (i, ((command, outbox, published, lost), (got_o, got_p, got_l))) in
        script.iter().zip(&actual).enumerate()
    {
        c.eq(
            &format!("step[{i}] {command} → outbox"),
            outbox.clone(),
            got_o.clone(),
        );
        c.eq(
            &format!("step[{i}] {command} → published"),
            published.clone(),
            got_p.clone(),
        );
        c.eq(
            &format!("step[{i}] {command} → lost"),
            lost.clone(),
            got_l.clone(),
        );
        if !c.ok() {
            break;
        }
    }
    // The property the whole pattern exists for, stated on its own so a failure says it:
    // an event written through the outbox is never lost, whatever the crash schedule was.
    let orphaned: Vec<&String> = transactional
        .iter()
        .filter(|e| {
            !last_outbox.contains(e) && !last_published.contains(e) || last_lost.contains(e)
        })
        .collect();
    c.that(
        "the transactional events",
        "every one still in the outbox or already on the bus, and none lost",
        orphaned.is_empty(),
        &orphaned,
    );
    c.block("transcript", transcript);
    c.finish()
});
