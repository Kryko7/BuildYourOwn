//! Stage 09 — Sloppy quorums and hinted handoff.
//!
//! Stage 08 was arithmetic about nodes that are all up. This one is about the day they are
//! not: a write that cannot reach W of its preference list is taken by whoever is around,
//! and the stand-in writes down whose key it is really holding. Availability bought with a
//! promise to hand the data over later.
//!
//! The one thing the suite insists on is honesty about it. A sloppy quorum is not a quorum:
//! the R + W > N overlap from stage 08 was an argument about the preference list, and a
//! write that landed outside it is not covered by that argument. So `strict` is false
//! exactly when a stand-in was used, and a program that reports every accepted write as
//! strict fails here however well it handles the hints.

use crate::assert::{Check, Failure};
use crate::dist_test;
use crate::examples::{lines, prim_example, ExampleSpec};
use crate::prim::PrimProc;
use crate::stages::{Ladder, Stage, Test};

/// Stage 09.
pub fn stage() -> Stage {
    Stage {
        number: 9,
        slug: "sloppy_quorums",
        name: "Sloppy quorums and hinted handoff",
        ext: true,
        ladder: Ladder::Primitives,
        hints: &[
            "A sloppy quorum accepts a write on a node outside the preference list when a member is down",
            "The stand-in records a hint naming the node the write really belongs to",
            "When the real owner comes back, the hint is handed off and then forgotten",
            "A sloppy quorum is not a quorum: it can lose the overlap guarantee, and must say so",
        ],
        examples,
        tests: vec![
            Test::new(
                "a write with every replica alive needs no stand-in",
                all_alive_is_strict,
            ),
            Test::new(
                "a write with a member down is accepted on a stand-in",
                a_member_down_is_accepted_with_a_hint,
            ),
            Test::new(
                "the number of hints is exactly the number of replicas that were down",
                hints_count_the_shortfall,
            ),
            Test::new(
                "strict is false exactly when a stand-in was used",
                strict_means_no_stand_in,
            )
            .ext(),
            Test::new(
                "hints lists the keys a stand-in is holding",
                hints_lists_the_keys,
            ),
            Test::new(
                "a handoff delivers every hint and leaves none",
                a_handoff_delivers_everything,
            ),
            Test::new(
                "handing off twice delivers nothing the second time",
                a_second_handoff_delivers_nothing,
            ),
            Test::new(
                "a hint for an owner that never comes back is still held",
                a_hint_for_a_dead_owner_is_kept,
            ),
            Test::new(
                "an owner nobody hinted for is holding nothing",
                an_owner_with_no_hints,
            )
            .ext(),
        ],
    }
}

fn examples() -> Vec<ExampleSpec> {
    vec![
        prim_example("A write while one replica is down", "quorum", || {
            lines(&["sloppy 3 3 3", "sloppy 3 3 2", "sloppy 3 2 1"])
        })
        .request("W = 3 of N = 3 with everybody up, then with one down, then W = 2 with one up")
        .response("accepted every time; `hints` is 0, then 1, then 1, and `strict` follows it")
        .note(
            "All three writes are accepted, which is the whole point of being sloppy, but \
             only the first one is a quorum write. The second and third were completed by a \
             node outside the preference list, so the overlap argument from stage 08 does not \
             cover them and `strict` has to say so.",
        ),
        prim_example("Hinted handoff", "quorum", || {
            lines(&[
                "hint n1 n4 user-42",
                "hint n1 n4 user-99",
                "hints n1",
                "handoff n1",
                "hints n1",
            ])
        })
        .request("`n4` takes two writes belonging to `n1`, then `n1` comes back")
        .response("two keys held for `n1`, then a handoff of two, then nothing left")
        .note(
            "A hint is per owner, and the handoff is what makes it temporary. A store that \
             delivers the data but keeps the hint hands the same keys over again on the next \
             handoff, and one that drops the hint without delivering has quietly lost a write \
             that was acknowledged.",
        ),
    ]
}

/// What one `sloppy` answered.
struct Sloppy {
    /// The `(n, w, alive)` it was asked about.
    asked: (i64, i64, i64),
    /// Whether the write was taken at all.
    accepted: bool,
    /// How many of the replicas were covered by a stand-in.
    hints: i64,
    /// Whether the write was a real quorum write.
    strict: bool,
}

/// Ask `sloppy n w alive` and decode all three fields.
async fn sloppy(p: &mut PrimProc, n: i64, w: i64, alive: i64) -> Result<Sloppy, Failure> {
    let command = format!("sloppy {n} {w} {alive}");
    let answer = p.send(&command).await?;
    Ok(Sloppy {
        asked: (n, w, alive),
        accepted: p.expect_bool(&answer, &command, "accepted")?,
        hints: p.expect_i64(&answer, &command, "hints")?,
        strict: p.expect_bool(&answer, &command, "strict")?,
    })
}

/// Record a hint and read back how many are now held for that owner.
async fn hint(p: &mut PrimProc, owner: &str, standin: &str, key: &str) -> Result<i64, Failure> {
    let command = format!("hint {owner} {standin} {key}");
    let answer = p.send(&command).await?;
    p.expect_i64(&answer, &command, "hints")
}

/// The keys held for one owner.
async fn held(p: &mut PrimProc, owner: &str) -> Result<Vec<String>, Failure> {
    let command = format!("hints {owner}");
    let answer = p.send(&command).await?;
    let mut keys = p.expect_strs(&answer, &command, "keys")?;
    keys.sort();
    Ok(keys)
}

/// Hand off to an owner: `(delivered, still held)`.
async fn handoff(p: &mut PrimProc, owner: &str) -> Result<(i64, i64), Failure> {
    let command = format!("handoff {owner}");
    let answer = p.send(&command).await?;
    Ok((
        p.expect_i64(&answer, &command, "delivered")?,
        p.expect_i64(&answer, &command, "hints")?,
    ))
}

dist_test!(all_alive_is_strict, |ctx| {
    let p = ctx.prim("quorum").await?;
    let three = sloppy(p, 3, 3, 3).await?;
    let majority = sloppy(p, 5, 3, 5).await?;
    let spare = sloppy(p, 5, 3, 4).await?;
    let transcript = p.transcript_block();
    let mut c = Check::new("writes that reached W of the preference list on their own");
    c.eq("sloppy(3,3,3).accepted", true, three.accepted);
    c.eq("sloppy(3,3,3).hints", 0, three.hints);
    c.eq("sloppy(3,3,3).strict", true, three.strict);
    c.eq("sloppy(5,3,5).hints", 0, majority.hints);
    c.eq("sloppy(5,3,5).strict", true, majority.strict);
    // Four of five up still covers a write of three without leaving the list.
    c.eq("sloppy(5,3,4).hints", 0, spare.hints);
    c.eq("sloppy(5,3,4).strict", true, spare.strict);
    c.block("transcript", transcript);
    c.finish()
});

dist_test!(a_member_down_is_accepted_with_a_hint, |ctx| {
    let p = ctx.prim("quorum").await?;
    let one_down = sloppy(p, 3, 3, 2).await?;
    let two_down = sloppy(p, 3, 3, 1).await?;
    let transcript = p.transcript_block();
    let mut c = Check::new("a write of three when only two of the three replicas are up");
    c.eq("sloppy(3,3,2).accepted", true, one_down.accepted);
    c.eq("sloppy(3,3,2).hints", 1, one_down.hints);
    c.eq("sloppy(3,3,2).strict", false, one_down.strict);
    c.eq("sloppy(3,3,1).accepted", true, two_down.accepted);
    c.eq("sloppy(3,3,1).hints", 2, two_down.hints);
    c.eq("sloppy(3,3,1).strict", false, two_down.strict);
    c.block("transcript", transcript);
    c.finish()
});

dist_test!(hints_count_the_shortfall, |ctx| {
    // One stand-in per replica that could not take the write, and not one more: a hint is a
    // debt, and inventing extra ones means handing the same key over twice.
    let p = ctx.prim("quorum").await?;
    let mut answers = Vec::new();
    for (n, w) in [(5, 5), (5, 3), (3, 2), (7, 4)] {
        for alive in (0..=n).rev() {
            answers.push(sloppy(p, n, w, alive).await?);
        }
    }
    let transcript = p.transcript_block();
    let wrong: Vec<((i64, i64, i64), i64, i64)> = answers
        .iter()
        .map(|a| {
            let (_, w, alive) = a.asked;
            (a.asked, (w - alive).max(0), a.hints)
        })
        .filter(|(_, want, got)| want != got)
        .take(5)
        .collect();
    let refused: Vec<(i64, i64, i64)> = answers
        .iter()
        .filter(|a| !a.accepted)
        .map(|a| a.asked)
        .take(5)
        .collect();
    let mut c = Check::new("the hint count over every (n, w, alive) of four configurations");
    c.observe("writes asked about", answers.len());
    c.that(
        "((n, w, alive), expected, sloppy.hints)",
        "one hint per replica that was down, floored at zero",
        wrong.is_empty(),
        wrong,
    );
    c.that(
        "sloppy.accepted",
        "true: a sloppy quorum takes the write rather than refusing it",
        refused.is_empty(),
        refused,
    );
    c.block("transcript", transcript);
    c.finish()
});

dist_test!(strict_means_no_stand_in, |ctx| {
    let p = ctx.prim("quorum").await?;
    let mut answers = Vec::new();
    for (n, w) in [(3, 3), (3, 1), (5, 4), (6, 3)] {
        for alive in (0..=n).rev() {
            answers.push(sloppy(p, n, w, alive).await?);
        }
    }
    let transcript = p.transcript_block();
    let wrong: Vec<((i64, i64, i64), bool, i64)> = answers
        .iter()
        .filter(|a| a.strict != (a.hints == 0))
        .map(|a| (a.asked, a.strict, a.hints))
        .take(5)
        .collect();
    let mut c = Check::new("strict against the hint count, over four configurations");
    c.observe("writes asked about", answers.len());
    c.that(
        "((n, w, alive), sloppy.strict, sloppy.hints)",
        "strict exactly when no hint was needed",
        wrong.is_empty(),
        wrong,
    );
    c.block("transcript", transcript);
    c.finish()
});

dist_test!(hints_lists_the_keys, |ctx| {
    let p = ctx.prim("quorum").await?;
    let first = hint(p, "n1", "n4", "user-42").await?;
    let second = hint(p, "n1", "n4", "user-99").await?;
    let keys = held(p, "n1").await?;
    let transcript = p.transcript_block();
    let mut c = Check::new("two writes taken for n1 by the stand-in n4");
    c.eq("hint(n1,n4,user-42).hints", 1, first);
    c.eq("hint(n1,n4,user-99).hints", 2, second);
    c.eq(
        "hints(n1).keys",
        vec!["user-42".to_string(), "user-99".to_string()],
        keys,
    );
    c.block("transcript", transcript);
    c.finish()
});

dist_test!(a_handoff_delivers_everything, |ctx| {
    let p = ctx.prim("quorum").await?;
    for i in 0..4 {
        hint(p, "n1", "n5", &format!("k{i}")).await?;
    }
    let before = held(p, "n1").await?;
    let (delivered, left) = handoff(p, "n1").await?;
    let after = held(p, "n1").await?;
    let transcript = p.transcript_block();
    let mut c = Check::new("four hints handed to the owner that came back");
    c.eq("hints(n1).keys.len() before", 4, before.len());
    c.eq("handoff(n1).delivered", 4, delivered);
    c.eq("handoff(n1).hints", 0, left);
    c.eq("hints(n1).keys after", Vec::<String>::new(), after);
    c.block("transcript", transcript);
    c.finish()
});

dist_test!(a_second_handoff_delivers_nothing, |ctx| {
    // The failure this catches is a handoff that copies the data over without dropping the
    // hint: a second round trip then delivers the same keys again, and a store that treats
    // a handoff as an authoritative write has just resurrected them.
    let p = ctx.prim("quorum").await?;
    hint(p, "n2", "n6", "user-1").await?;
    hint(p, "n2", "n6", "user-2").await?;
    let (first, left_after_first) = handoff(p, "n2").await?;
    let (second, left_after_second) = handoff(p, "n2").await?;
    let keys = held(p, "n2").await?;
    let transcript = p.transcript_block();
    let mut c = Check::new("two handoffs in a row for the same owner");
    c.eq("handoff(n2).delivered, first time", 2, first);
    c.eq("handoff(n2).hints, first time", 0, left_after_first);
    c.eq("handoff(n2).delivered, second time", 0, second);
    c.eq("handoff(n2).hints, second time", 0, left_after_second);
    c.eq("hints(n2).keys", Vec::<String>::new(), keys);
    c.block("transcript", transcript);
    c.finish()
});

dist_test!(a_hint_for_a_dead_owner_is_kept, |ctx| {
    // Hints are per owner. `n3` is never coming back, and its keys have to sit there until
    // it does — throwing them away with somebody else's handoff loses an acknowledged write.
    let p = ctx.prim("quorum").await?;
    hint(p, "n1", "n5", "alive-1").await?;
    hint(p, "n3", "n5", "dead-1").await?;
    hint(p, "n3", "n6", "dead-2").await?;
    let (delivered, _) = handoff(p, "n1").await?;
    let n1_keys = held(p, "n1").await?;
    let n3_keys = held(p, "n3").await?;
    let (later, left) = handoff(p, "n3").await?;
    let transcript = p.transcript_block();
    let mut c = Check::new("one owner returning while another stays down");
    c.eq("handoff(n1).delivered", 1, delivered);
    c.eq("hints(n1).keys", Vec::<String>::new(), n1_keys);
    c.eq(
        "hints(n3).keys",
        vec!["dead-1".to_string(), "dead-2".to_string()],
        n3_keys,
    );
    c.eq("handoff(n3).delivered, whenever it happens", 2, later);
    c.eq("handoff(n3).hints", 0, left);
    c.block("transcript", transcript);
    c.finish()
});

dist_test!(an_owner_with_no_hints, |ctx| {
    let p = ctx.prim("quorum").await?;
    hint(p, "n1", "n5", "k1").await?;
    let stranger = held(p, "n9").await?;
    let (delivered, left) = handoff(p, "n9").await?;
    let untouched = held(p, "n1").await?;
    let transcript = p.transcript_block();
    let mut c = Check::new("asking about an owner nobody ever hinted for");
    c.eq("hints(n9).keys", Vec::<String>::new(), stranger);
    c.eq("handoff(n9).delivered", 0, delivered);
    c.eq("handoff(n9).hints", 0, left);
    c.eq("hints(n1).keys", vec!["k1".to_string()], untouched);
    c.block("transcript", transcript);
    c.finish()
});
