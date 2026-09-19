//! Stage 85 — Failure detectors, by what they promise rather than how they guess.
//!
//! Stage 20 built a phi-accrual detector and SWIM's suspicion mechanism: the machinery of
//! guessing whether a silent process is dead. This stage is about the other half, which is
//! the half the impossibility results are stated in — what a detector *guarantees*,
//! regardless of how it decides.
//!
//! Every class is two properties. **Completeness** is about not missing failures: strong
//! completeness says every crashed process is eventually suspected by every correct one.
//! **Accuracy** is about not crying wolf, and it is where the classes differ.
//!
//! | class | accuracy |
//! |---|---|
//! | **P**, perfect | no correct process is *ever* suspected |
//! | **◇P**, eventually perfect | after some unknown time, no correct process is suspected |
//! | **◇S**, eventually strong | after some unknown time, *some one* correct process is never suspected — the others may be suspected for ever |
//!
//! ◇S looks uselessly weak and is the interesting one: Chandra and Toueg proved it is the
//! *weakest* detector that makes consensus solvable with a majority of correct processes.
//! That single never-suspected process is enough to be a leader nobody deposes, which is
//! why a real Raft or Paxos needs no more than a timeout that is eventually right — and why
//! it cannot manage with anything less.

use crate::assert::Check;
use crate::dist_test;
use crate::examples::{lines, prim_example, ExampleSpec};
use crate::stages::{Ladder, Stage, Test};

/// Stage 85.
pub fn stage() -> Stage {
    Stage {
        number: 85,
        slug: "failure_detectors",
        name: "Failure detectors: completeness, accuracy and ◇S",
        ext: false,
        ladder: Ladder::Algorithms,
        hints: &[
            "Topic `detector`: `class <P|eventually-perfect|eventually-strong>`, `crash <p>`, \
             `stabilise` to make time pass, `suspects <p>`, `properties` for the verdict",
            "Strong completeness — every crashed process eventually suspected by every correct \
             one — is common to all three classes; only accuracy differs",
            "The ◇ means the guarantee holds only after some unknown time, so before \
             `stabilise` a detector may suspect anybody at all",
            "◇S promises only that *one* correct process is eventually never suspected, and \
             that is exactly enough to elect a leader nobody deposes",
        ],
        examples,
        tests: vec![
            Test::new(
                "a perfect detector suspects exactly the dead",
                perfect_is_exact,
            ),
            Test::new("and never suspects a correct process", perfect_never_wrong),
            Test::new(
                "an eventually-perfect detector may be wrong at first",
                eventually_perfect_starts_wrong,
            ),
            Test::new(
                "and is right once it has stabilised",
                eventually_perfect_settles,
            ),
            Test::new(
                "an eventually-strong detector stays wrong about some processes",
                eventually_strong_stays_wrong,
            ),
            Test::new(
                "but leaves one correct process unsuspected, which is the whole point",
                eventually_strong_keeps_one,
            ),
            Test::new(
                "completeness holds in every class, before and after stabilising",
                completeness_is_universal,
            ),
            Test::new(
                "a detector that suspects nobody fails completeness",
                a_silent_detector_is_incomplete,
            ),
            Test::new("nobody suspects themselves", no_self_suspicion).ext(),
            Test::new(
                "the three classes are ordered by strength, not by completeness",
                the_classes_are_ordered,
            )
            .ext(),
        ],
    }
}

fn examples() -> Vec<ExampleSpec> {
    vec![
        prim_example("Eventually perfect, before and after", "detector", || {
            lines(&[
                "init 3",
                "class eventually-perfect",
                "crash 2",
                "properties",
                "stabilise",
                "properties",
            ])
        })
        .request("A three-process group where one crashes, under ◇P")
        .response("before stabilising, accuracy is false; after, it is true — completeness holds throughout")
        .note(
            "The unstable phase is not a bug to be engineered away: any detector built on \
             timeouts has one, because a slow process and a dead process look identical for \
             as long as the timeout allows. What ◇P promises is that the phase ends, not \
             when.",
        ),
        prim_example("Why ◇S is enough", "detector", || {
            lines(&[
                "init 4",
                "class eventually-strong",
                "stabilise",
                "suspects 1",
                "properties",
            ])
        })
        .request("Four correct processes under ◇S, after stabilisation")
        .response("process 1 suspects other correct processes, but strong accuracy is false while weak accuracy is true")
        .note(
            "A detector this inaccurate still suspects correct processes for ever — and one \
             correct process is never suspected by anyone. Elect that one and it is never \
             deposed, so the protocol makes progress. Chandra and Toueg's result is that \
             nothing weaker will do.",
        ),
    ]
}

// ---------------------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------------------

/// Who process `who` suspects.
async fn suspects(
    p: &mut crate::prim::PrimProc,
    who: usize,
) -> Result<Vec<i64>, crate::stages::Failure> {
    let v = p.send(&format!("suspects {who}")).await?;
    Ok(v["suspects"]
        .as_array()
        .map(|a| a.iter().filter_map(serde_json::Value::as_i64).collect())
        .unwrap_or_default())
}

dist_test!(perfect_is_exact, |ctx| {
    let p = ctx.prim("detector").await?;
    p.send("init 4").await?;
    p.send("class P").await?;
    p.send("crash 3").await?;
    let s = suspects(p, 0).await?;
    let props = p.send("properties").await?;
    let transcript = p.transcript_block();
    let mut c = Check::new("a perfect detector");
    c.note(
        "P is what a detector would be if the network were synchronous and bounded: exactly \
         the dead, nobody else. Real networks do not offer it, which is why every result \
         about consensus is stated in terms of the weaker classes.",
    );
    c.eq("process 0 suspects", vec![3], s);
    c.eq(
        "completeness",
        true,
        p.expect_bool(&props, "properties", "strong_completeness")?,
    );
    c.eq(
        "strong accuracy",
        true,
        p.expect_bool(&props, "properties", "strong_accuracy")?,
    );
    c.block("transcript", transcript);
    c.finish()
});

dist_test!(perfect_never_wrong, |ctx| {
    let p = ctx.prim("detector").await?;
    p.send("init 3").await?;
    p.send("class P").await?;
    let before = suspects(p, 0).await?;
    p.send("crash 1").await?;
    let after = suspects(p, 0).await?;
    let transcript = p.transcript_block();
    let mut c = Check::new("a perfect detector with nothing wrong");
    c.note(
        "With no failures a perfect detector suspects nobody. A detector that suspects a \
         correct process even once has failed strong accuracy for ever — the property is \
         stated over the whole run, not over the current moment.",
    );
    c.eq("before the crash", Vec::<i64>::new(), before);
    c.eq("after it", vec![1], after);
    c.block("transcript", transcript);
    c.finish()
});

dist_test!(eventually_perfect_starts_wrong, |ctx| {
    let p = ctx.prim("detector").await?;
    p.send("init 3").await?;
    p.send("class eventually-perfect").await?;
    p.send("crash 2").await?;
    let props = p.send("properties").await?;
    let transcript = p.transcript_block();
    let mut c = Check::new("◇P before it stabilises");
    c.note(
        "A correct process is being suspected, and the detector is still within its \
         guarantee. This is the phase a timeout-based detector spends its early life in, and \
         a protocol that cannot survive it is not safe on a real network.",
    );
    c.eq(
        "completeness already holds",
        true,
        p.expect_bool(&props, "properties", "strong_completeness")?,
    );
    c.eq(
        "accuracy does not yet",
        false,
        p.expect_bool(&props, "properties", "strong_accuracy")?,
    );
    c.block("transcript", transcript);
    c.finish()
});

dist_test!(eventually_perfect_settles, |ctx| {
    let p = ctx.prim("detector").await?;
    p.send("init 3").await?;
    p.send("class eventually-perfect").await?;
    p.send("crash 2").await?;
    p.send("stabilise").await?;
    let props = p.send("properties").await?;
    let s = suspects(p, 0).await?;
    let transcript = p.transcript_block();
    let mut c = Check::new("◇P after it stabilises");
    c.note(
        "Once the unstable phase ends, ◇P is indistinguishable from P for the rest of the \
         run. That is the whole content of the ◇: not a weaker promise, a later one.",
    );
    c.eq(
        "strong accuracy",
        true,
        p.expect_bool(&props, "properties", "strong_accuracy")?,
    );
    c.eq("and only the dead are suspected", vec![2], s);
    c.block("transcript", transcript);
    c.finish()
});

dist_test!(eventually_strong_stays_wrong, |ctx| {
    let p = ctx.prim("detector").await?;
    p.send("init 4").await?;
    p.send("class eventually-strong").await?;
    p.send("stabilise").await?;
    let props = p.send("properties").await?;
    let transcript = p.transcript_block();
    let mut c = Check::new("◇S after stabilising");
    c.note(
        "Stabilised, and still suspecting correct processes. ◇S never promises to stop being \
         wrong about everybody — only about one particular process, and it does not say \
         which.",
    );
    c.eq(
        "strong accuracy",
        false,
        p.expect_bool(&props, "properties", "strong_accuracy")?,
    );
    c.eq(
        "weak accuracy",
        true,
        p.expect_bool(&props, "properties", "weak_accuracy")?,
    );
    c.block("transcript", transcript);
    c.finish()
});

dist_test!(eventually_strong_keeps_one, |ctx| {
    let p = ctx.prim("detector").await?;
    p.send("init 4").await?;
    p.send("class eventually-strong").await?;
    p.send("stabilise").await?;
    let mut suspected_by_someone = Vec::new();
    for who in 0..4usize {
        for s in suspects(p, who).await? {
            suspected_by_someone.push(s);
        }
    }
    let never: Vec<i64> = (0..4i64)
        .filter(|i| !suspected_by_someone.contains(i))
        .collect();
    let transcript = p.transcript_block();
    let mut c = Check::new("the one process nobody suspects");
    c.note(
        "Somebody survives every suspicion list. Make that process the leader and the \
         protocol has a leader nobody will depose, which is all consensus needs to \
         terminate — and it is the reason ◇S is the weakest class that works rather than \
         merely a weak one that happens to.",
    );
    c.at_least("correct processes nobody suspects", 1, never.len());
    c.block("transcript", transcript);
    c.finish()
});

dist_test!(completeness_is_universal, |ctx| {
    let mut results = Vec::new();
    for class in ["P", "eventually-perfect", "eventually-strong"] {
        let mut p = ctx.prim_fresh("detector").await?;
        p.send("init 4").await?;
        p.send(&format!("class {class}")).await?;
        p.send("crash 3").await?;
        let before = p.send("properties").await?;
        results.push(p.expect_bool(&before, "properties", "strong_completeness")?);
        p.send("stabilise").await?;
        let after = p.send("properties").await?;
        results.push(p.expect_bool(&after, "properties", "strong_completeness")?);
    }
    let mut c = Check::new("completeness across all three classes");
    c.note(
        "No class trades away completeness. Suspecting the dead is the easy half — a process \
         that has stopped answering will stop answering for ever — and every interesting \
         difference between detectors is about the other half.",
    );
    c.eq("six verdicts, all true", vec![true; 6], results);
    c.finish()
});

dist_test!(a_silent_detector_is_incomplete, |ctx| {
    let p = ctx.prim("detector").await?;
    p.send("init 3").await?;
    p.send("class P").await?;
    let props = p.send("properties").await?;
    let transcript = p.transcript_block();
    let mut c = Check::new("a detector with nothing to detect");
    c.note(
        "With no crashes, suspecting nobody is both complete and accurate — the properties \
         are vacuous. It is worth checking because a detector that always answers \"nobody \
         has failed\" passes this and fails the moment anything does.",
    );
    c.eq(
        "completeness",
        true,
        p.expect_bool(&props, "properties", "strong_completeness")?,
    );
    c.eq(
        "accuracy",
        true,
        p.expect_bool(&props, "properties", "strong_accuracy")?,
    );
    c.block("transcript", transcript);
    c.finish()
});

dist_test!(no_self_suspicion, |ctx| {
    let p = ctx.prim("detector").await?;
    p.send("init 3").await?;
    p.send("class eventually-strong").await?;
    let mut selves = Vec::new();
    for who in 0..3usize {
        selves.push(suspects(p, who).await?.contains(&(who as i64)));
    }
    let transcript = p.transcript_block();
    let mut c = Check::new("what each process thinks of itself");
    c.note(
        "A detector is a local module answering \"who do I think has failed\", and a process \
         that suspects itself has said something with no meaning. It is also a real bug in \
         implementations that build the list by iterating over all members.",
    );
    c.eq(
        "nobody suspects themselves",
        vec![false, false, false],
        selves,
    );
    c.block("transcript", transcript);
    c.finish()
});

dist_test!(the_classes_are_ordered, |ctx| {
    let mut accuracy = Vec::new();
    for class in ["P", "eventually-perfect", "eventually-strong"] {
        let mut p = ctx.prim_fresh("detector").await?;
        p.send("init 4").await?;
        p.send(&format!("class {class}")).await?;
        p.send("stabilise").await?;
        let props = p.send("properties").await?;
        accuracy.push((
            p.expect_bool(&props, "properties", "strong_accuracy")?,
            p.expect_bool(&props, "properties", "weak_accuracy")?,
        ));
    }
    let mut c = Check::new("strong and weak accuracy per class, once stabilised");
    c.note(
        "P and ◇P end up in the same place — the difference between them is when, and this \
         is after. ◇S is genuinely weaker: it never gets strong accuracy at all, and it is \
         still enough for consensus.",
    );
    c.eq(
        "(strong, weak) for P, ◇P, ◇S",
        vec![(true, true), (true, true), (false, true)],
        accuracy,
    );
    c.finish()
});
