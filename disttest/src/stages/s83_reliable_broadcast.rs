//! Stage 83 — Broadcast: best-effort, reliable, uniform.
//!
//! "Send it to everyone" is three different promises, and which one a system makes decides
//! what it can be built out of. All three agree that a message is delivered at most once and
//! only if somebody sent it. They differ in exactly one place: what happens when the sender
//! dies halfway through.
//!
//! **Best-effort** promises nothing there. The sender managed two of five sends before it
//! crashed, so two processes have the message and three never will.
//!
//! **Reliable** adds agreement among the survivors: if a *correct* process delivers, every
//! correct process delivers. It is built by relaying — whoever delivers first passes it on —
//! so a half-finished broadcast is finished by its recipients rather than by its sender.
//!
//! **Uniform reliable** goes one step further: if *any* process delivers, even one that
//! crashes a microsecond later, every correct process must deliver too. That extra step
//! costs a round trip, and you need it exactly when delivering has an effect the outside
//! world can see — a payment taken, a message sent, a door unlocked. A process that did
//! that and then died has left a fact behind, and the survivors have to agree with it.

use crate::assert::Check;
use crate::dist_test;
use crate::examples::{lines, prim_example, ExampleSpec};
use crate::stages::{Ladder, Stage, Test};
use rand::Rng;

/// Stage 83.
pub fn stage() -> Stage {
    Stage {
        number: 83,
        slug: "reliable_broadcast",
        name: "Broadcast: best-effort, reliable, uniform",
        ext: false,
        ladder: Ladder::Algorithms,
        hints: &[
            "Topic `broadcast`: `mode <best-effort|reliable|uniform>`, `broadcast <from>`, \
             `hand <id> <to>` for one network delivery, `crash <p>`, `settle` to run to quiet",
            "All three modes deliver at most once and never invent a message; they differ only \
             in what a sender's crash costs",
            "Reliable broadcast is built by relaying on first delivery, so the guarantee holds \
             among correct processes without the sender being alive to finish",
            "Uniform adds the promise that a delivery by a process that then crashes still \
             obliges everyone else — the difference matters when delivering has a visible effect",
        ],
        examples,
        tests: vec![
            Test::new(
                "a broadcast with nobody failing reaches everyone",
                the_happy_case,
            ),
            Test::new("a message is never delivered twice", no_duplicates),
            Test::new(
                "best-effort loses what the sender never sent",
                best_effort_loses_it,
            ),
            Test::new(
                "reliable finishes what the sender started",
                reliable_relays_it,
            ),
            Test::new(
                "but only when a correct process delivered first",
                reliable_needs_a_correct_deliverer,
            ),
            Test::new(
                "uniform obliges everyone once anyone has delivered",
                uniform_obliges_everyone,
            ),
            Test::new(
                "which is the one case where uniform and reliable differ",
                the_difference_in_one_run,
            ),
            Test::new(
                "a dropped message is simply not delivered",
                a_dropped_message,
            ),
            Test::new("a crashed sender cannot broadcast at all", a_crashed_sender).ext(),
            Test::new(
                "twenty seeded runs never deliver to a process twice",
                a_long_seeded_run,
            )
            .ext(),
        ],
    }
}

fn examples() -> Vec<ExampleSpec> {
    vec![
        prim_example("A sender that dies halfway", "broadcast", || {
            lines(&[
                "init 3",
                "mode best-effort",
                "broadcast 0",
                "hand 1 1",
                "crash 0",
                "settle",
                "delivered 1",
            ])
        })
        .request("Process 0 broadcasts, one process gets it, then 0 crashes")
        .response("`delivered_to` is [0, 1] and `all_correct` is false — process 2 never gets it")
        .note(
            "Nothing went wrong that best-effort broadcast promised would not happen. Two \
             correct processes now disagree about whether the message exists, which is fine \
             until something is built on top that assumes they agree.",
        ),
        prim_example("The same run, one mode stronger", "broadcast", || {
            lines(&[
                "init 3",
                "mode reliable",
                "broadcast 0",
                "hand 1 1",
                "crash 0",
                "settle",
                "delivered 1",
            ])
        })
        .request("The identical sequence under reliable broadcast")
        .response("`all_correct` is true — process 1 relayed it to process 2")
        .note(
            "The sender is just as dead. What changed is that delivery now means \"I will \
             make sure everyone else gets this too\", so the broadcast is completed by its \
             recipients. That is the whole implementation: relay on first delivery.",
        ),
    ]
}

// ---------------------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------------------

/// Everyone who delivered message `id`.
async fn delivered_to(
    p: &mut crate::prim::PrimProc,
    id: i64,
) -> Result<Vec<i64>, crate::stages::Failure> {
    let v = p.send(&format!("delivered {id}")).await?;
    Ok(v["delivered_to"]
        .as_array()
        .map(|a| a.iter().filter_map(serde_json::Value::as_i64).collect())
        .unwrap_or_default())
}

dist_test!(the_happy_case, |ctx| {
    let p = ctx.prim("broadcast").await?;
    p.send("init 4").await?;
    p.send("broadcast 0").await?;
    p.send("settle").await?;
    let to = delivered_to(p, 1).await?;
    let transcript = p.transcript_block();
    let mut c = Check::new("nobody fails");
    c.note(
        "With no failures all three modes are the same thing, which is why the mode is only \
         ever interesting in the runs where somebody dies.",
    );
    c.eq("delivered to", vec![0, 1, 2, 3], to);
    c.block("transcript", transcript);
    c.finish()
});

dist_test!(no_duplicates, |ctx| {
    let p = ctx.prim("broadcast").await?;
    p.send("init 3").await?;
    p.send("broadcast 0").await?;
    p.send("hand 1 1").await?;
    let again = p.send("hand 1 1").await?;
    p.send("settle").await?;
    let to = delivered_to(p, 1).await?;
    let transcript = p.transcript_block();
    let mut c = Check::new("the same message offered twice");
    c.note(
        "No-duplication is common to all three modes and is usually the easiest to get \
         wrong, because a relay-based implementation hands the same message around several \
         times by design. Delivering is what must happen once, not receiving.",
    );
    c.eq(
        "the second hand does nothing",
        false,
        p.expect_bool(&again, "hand", "handed")?,
    );
    c.eq("and it was delivered once each", vec![0, 1, 2], to);
    c.block("transcript", transcript);
    c.finish()
});

dist_test!(best_effort_loses_it, |ctx| {
    let p = ctx.prim("broadcast").await?;
    p.send("init 3").await?;
    p.send("mode best-effort").await?;
    p.send("broadcast 0").await?;
    p.send("hand 1 1").await?;
    p.send("crash 0").await?;
    p.send("settle").await?;
    let answer = p.send("delivered 1").await?;
    let transcript = p.transcript_block();
    let mut c = Check::new("a half-finished best-effort broadcast");
    c.note(
        "Process 2 is correct and never receives the message. Best-effort broadcast is \
         allowed to leave two correct processes disagreeing, and that is exactly what makes \
         it too weak to build anything ordered on top of.",
    );
    c.eq(
        "all correct processes got it",
        false,
        p.expect_bool(&answer, "delivered", "all_correct")?,
    );
    c.eq("delivered to", vec![0, 1], delivered_to(p, 1).await?);
    c.block("transcript", transcript);
    c.finish()
});

dist_test!(reliable_relays_it, |ctx| {
    let p = ctx.prim("broadcast").await?;
    p.send("init 3").await?;
    p.send("mode reliable").await?;
    p.send("broadcast 0").await?;
    p.send("hand 1 1").await?;
    p.send("crash 0").await?;
    p.send("settle").await?;
    let answer = p.send("delivered 1").await?;
    let transcript = p.transcript_block();
    let mut c = Check::new("the same run, under reliable broadcast");
    c.note(
        "Process 1 delivered while it was correct, so every correct process must deliver — \
         and process 1 is the one that makes that happen. The sender's death is no longer \
         the last word on who gets the message.",
    );
    c.eq(
        "all correct processes got it",
        true,
        p.expect_bool(&answer, "delivered", "all_correct")?,
    );
    c.block("transcript", transcript);
    c.finish()
});

dist_test!(reliable_needs_a_correct_deliverer, |ctx| {
    let p = ctx.prim("broadcast").await?;
    p.send("init 3").await?;
    p.send("mode reliable").await?;
    p.send("broadcast 0").await?;
    // Only the sender delivered — to itself — and then it died.
    p.send("crash 0").await?;
    p.send("settle").await?;
    let answer = p.send("delivered 1").await?;
    let transcript = p.transcript_block();
    let mut c = Check::new("reliable broadcast with no correct deliverer");
    c.note(
        "Reliable broadcast says: *if a correct process delivers*, all correct processes \
         deliver. Nobody correct delivered here, so nothing is owed and the message simply \
         disappears. That conditional is not a loophole — it is where uniform broadcast \
         gets its name and its extra round trip.",
    );
    c.eq("delivered to", vec![0], delivered_to(p, 1).await?);
    c.eq(
        "no correct process got it",
        false,
        p.expect_bool(&answer, "delivered", "all_correct")?,
    );
    c.block("transcript", transcript);
    c.finish()
});

dist_test!(uniform_obliges_everyone, |ctx| {
    let p = ctx.prim("broadcast").await?;
    p.send("init 3").await?;
    p.send("mode uniform").await?;
    p.send("broadcast 0").await?;
    p.send("crash 0").await?;
    p.send("settle").await?;
    let answer = p.send("delivered 1").await?;
    let transcript = p.transcript_block();
    let mut c = Check::new("the same run, under uniform reliable broadcast");
    c.note(
        "The only process that delivered is dead, and everyone else must still deliver. \
         Think of the delivery as having taken money or opened a door: the fact happened, \
         the process that observed it is gone, and the rest of the system has to be \
         consistent with it anyway.",
    );
    c.eq(
        "all correct processes got it",
        true,
        p.expect_bool(&answer, "delivered", "all_correct")?,
    );
    c.block("transcript", transcript);
    c.finish()
});

dist_test!(the_difference_in_one_run, |ctx| {
    let mut results = Vec::new();
    for mode in ["best-effort", "reliable", "uniform"] {
        let p = ctx.prim_fresh("broadcast").await?;
        let mut p = p;
        p.send("init 3").await?;
        p.send(&format!("mode {mode}")).await?;
        p.send("broadcast 0").await?;
        p.send("crash 0").await?;
        p.send("settle").await?;
        let answer = p.send("delivered 1").await?;
        results.push(p.expect_bool(&answer, "delivered", "all_correct")?);
    }
    let mut c = Check::new("one run, three modes");
    c.note(
        "The sender delivered to itself and died before anyone else received anything. \
         Best-effort owes nothing, reliable owes nothing — no *correct* process delivered — \
         and uniform owes everyone, because a delivery happened at all. Three promises, one \
         run, and only the strongest one is obliged to act.",
    );
    c.eq(
        "best-effort, reliable, uniform",
        vec![false, false, true],
        results,
    );
    c.finish()
});

dist_test!(a_dropped_message, |ctx| {
    let p = ctx.prim("broadcast").await?;
    p.send("init 3").await?;
    p.send("mode best-effort").await?;
    p.send("broadcast 0").await?;
    p.send("drop 1 2").await?;
    p.send("settle").await?;
    let to = delivered_to(p, 1).await?;
    let transcript = p.transcript_block();
    let mut c = Check::new("a message the network loses");
    c.note(
        "Best-effort broadcast over a lossy network is allowed to lose messages even when \
         nothing crashes. Reliable broadcast is not, which is why a real implementation \
         retransmits rather than sending once and hoping.",
    );
    c.eq("delivered to", vec![0, 1], to);
    c.block("transcript", transcript);
    c.finish()
});

dist_test!(a_crashed_sender, |ctx| {
    let p = ctx.prim("broadcast").await?;
    p.send("init 3").await?;
    p.send("crash 1").await?;
    let sent = p.send("broadcast 1").await?;
    let transcript = p.transcript_block();
    let mut c = Check::new("a broadcast from a process that is already gone");
    c.note(
        "Nothing is delivered and nothing is invented. The validity property of every \
         broadcast abstraction says a message is delivered only if some process actually \
         broadcast it, and a crashed process broadcasts nothing.",
    );
    c.eq("sent", false, p.expect_bool(&sent, "broadcast", "sent")?);
    c.block("transcript", transcript);
    c.finish()
});

dist_test!(a_long_seeded_run, |ctx| {
    enum Step {
        Broadcast(usize),
        Hand(i64, usize),
        Drop(i64, usize),
        Crash(usize),
    }
    let mut plan = Vec::new();
    let mut ids = 0i64;
    for _ in 0..20 {
        plan.push(match ctx.rng.random_range(0..6) {
            0 | 1 => {
                ids += 1;
                Step::Broadcast(ctx.rng.random_range(0..4usize))
            }
            2..=4 => Step::Hand(
                ctx.rng.random_range(1..=ids.max(1)),
                ctx.rng.random_range(0..4usize),
            ),
            5 if ids > 0 => Step::Drop(
                ctx.rng.random_range(1..=ids),
                ctx.rng.random_range(0..4usize),
            ),
            _ => Step::Crash(ctx.rng.random_range(0..4usize)),
        });
    }
    let mut twice: Vec<String> = Vec::new();
    {
        let p = ctx.prim("broadcast").await?;
        p.send("init 4").await?;
        p.send("mode reliable").await?;
        let mut seen: std::collections::BTreeMap<(i64, usize), usize> = Default::default();
        for step in plan {
            match step {
                Step::Broadcast(from) => {
                    p.send(&format!("broadcast {from}")).await?;
                }
                Step::Hand(id, to) => {
                    let v = p.send(&format!("hand {id} {to}")).await?;
                    if p.expect_bool(&v, "hand", "handed").unwrap_or(false) {
                        let n = seen.entry((id, to)).or_insert(0);
                        *n += 1;
                        if *n > 1 {
                            twice.push(format!("message {id} delivered to {to} {n} times"));
                        }
                    }
                }
                Step::Drop(id, to) => {
                    p.send(&format!("drop {id} {to}")).await?;
                }
                Step::Crash(i) => {
                    p.send(&format!("crash {i}")).await?;
                }
            }
        }
    }
    let mut c = Check::new("twenty seeded broadcasts, hands, drops and crashes");
    c.note(
        "Whatever the network and the failures do, no process may deliver the same message \
         twice. It is the property a relay-based implementation is most likely to break, \
         because relaying means the same bytes arrive more than once on purpose.",
    );
    c.eq("duplicate deliveries", Vec::<String>::new(), twice);
    c.finish()
});
