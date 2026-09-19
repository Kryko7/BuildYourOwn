//! Stage 74 — Gossip dissemination.
//!
//! Gossip spreads a rumour the way an epidemic spreads: every node that has it tells a few
//! others, so the number that know it multiplies each round and the whole cluster hears in
//! a number of rounds that grows with the *logarithm* of its size. Real gossip picks its
//! peers at random, which makes it robust and makes it impossible to test; this topic pins
//! the peer choice to a fixed schedule instead, so the round counts and the message counts
//! are exact numbers rather than distributions, and the two things worth learning — that
//! rounds grow logarithmically and that the fanout buys them with messages — are still
//! exactly what the schedule shows.
//!
//! The oracle is the schedule, replayed. The tester holds its own infected set and message
//! counter, steps them forward by the same rule, and compares; the sweep at the end walks
//! every cluster size from two to forty against every fanout from one to four and checks the
//! round count, the message count and the final infected set for all of them.

use crate::assert::Check;
use crate::dist_test;
use crate::examples::{lines, prim_example, ExampleSpec};
use crate::prim::PrimProc;
use crate::stages::{Ladder, Stage, Test};
use std::collections::BTreeSet;

/// Stage 74.
pub fn stage() -> Stage {
    Stage {
        number: 74,
        slug: "gossip_dissemination",
        name: "Gossip dissemination",
        ext: false,
        ladder: Ladder::Algorithms,
        hints: &[
            "Topic `gossip`: nodes are 0..n-1, node 0 starts infected, and the schedule is fixed",
            "In round r every infected node i contacts (i + (fanout+1)^(r-1) * k) mod n for k = 1..=fanout",
            "Everyone contacted becomes infected at the end of the round, not during it",
            "Count every message, including the ones that land on a node that already knew",
        ],
        examples,
        tests: vec![
            Test::new("a fresh run has one infected node", a_fresh_run),
            Test::new(
                "the first round with fanout one infects exactly one more node",
                the_first_round,
            ),
            Test::new(
                "the infected set doubles each round at fanout one",
                the_set_doubles,
            ),
            Test::new(
                "a wider fanout infects more in the same first round",
                a_wider_fanout_starts_faster,
            ),
            Test::new(
                "convergence takes a logarithmic number of rounds",
                convergence_is_logarithmic,
            ),
            Test::new(
                "a wider fanout buys rounds with messages",
                the_fanout_trade,
            ),
            Test::new(
                "one node needs no rounds and a cluster-wide fanout needs one",
                the_degenerate_cases,
            ),
            Test::new(
                "messages that land on a node that already knew are still messages",
                wasted_messages_are_counted,
            ),
            Test::new(
                "a round after everyone knows infects nobody and still costs",
                a_round_past_convergence,
            )
            .ext(),
            Test::new(
                "every size from two to forty matches the tester's own replay",
                a_sweep_of_sizes_and_fanouts,
            )
            .ext(),
        ],
    }
}

fn examples() -> Vec<ExampleSpec> {
    vec![
        prim_example("Eight nodes, fanout one", "gossip", || {
            lines(&["init 8 1", "round", "round", "round", "infected"])
        })
        .request("a rumour starting at node 0 in a cluster of eight, told to one peer a round")
        .response("one, two then four new nodes a round: 1, 2, 4, 8 infected after three rounds")
        .note(
            "Three rounds for eight nodes, and it would be ten for a thousand. That growth \
             is the whole reason gossip is used for cluster-wide state: the round count \
             barely notices the cluster getting bigger.",
        ),
        prim_example("What a wider fanout costs", "gossip", || {
            lines(&["init 32 1", "converge", "init 32 4", "converge"])
        })
        .request("the same thirty-two nodes converged at fanout one and at fanout four")
        .response("five rounds and 31 messages, against three rounds and 124 messages")
        .note(
            "Two rounds saved for four times the traffic. The fanout is not a free tuning \
             knob: past a point the extra copies land on nodes that already knew, and the \
             message count keeps climbing while the round count stops falling.",
        ),
    ]
}

// ---------------------------------------------------------------------------------------
// The tester's own replay of the schedule. Nothing below consults the program.
// ---------------------------------------------------------------------------------------

/// `base^exp mod m`, so a long run's step never overflows.
fn pow_mod(base: u64, exp: u32, m: u64) -> u64 {
    if m == 0 {
        return 0;
    }
    let (mut result, mut b, mut e) = (1u64 % m, base % m, exp);
    while e > 0 {
        if e & 1 == 1 {
            result = result * b % m;
        }
        b = b * b % m;
        e >>= 1;
    }
    result
}

/// The infected set and the message count after `rounds` rounds of the fixed schedule.
fn replay(nodes: usize, fanout: usize, rounds: u32) -> (BTreeSet<usize>, u64) {
    let mut infected = BTreeSet::from([0usize]);
    let mut messages = 0u64;
    for r in 1..=rounds {
        let step = pow_mod((fanout + 1) as u64, r - 1, nodes as u64);
        let senders: Vec<usize> = infected.iter().copied().collect();
        for i in senders {
            for k in 1..=fanout {
                let target = (i as u64 + step * k as u64) % nodes as u64;
                messages += 1;
                infected.insert(target as usize);
            }
        }
    }
    (infected, messages)
}

/// How many rounds the schedule needs to reach every node, and what it spends doing it.
fn converge(nodes: usize, fanout: usize) -> (u32, u64, usize) {
    let mut rounds = 0;
    loop {
        let (infected, messages) = replay(nodes, fanout, rounds);
        if infected.len() == nodes {
            return (rounds, messages, infected.len());
        }
        let (next, _) = replay(nodes, fanout, rounds + 1);
        if next.len() == infected.len() {
            // A schedule that has stopped making progress will never make any more.
            return (rounds + 1, replay(nodes, fanout, rounds + 1).1, next.len());
        }
        rounds += 1;
    }
}

/// Read an array of whole numbers out of an answer.
fn numbers(
    p: &PrimProc,
    v: &serde_json::Value,
    command: &str,
    field: &str,
) -> Result<Vec<i64>, crate::assert::Failure> {
    match v.get(field).and_then(serde_json::Value::as_array) {
        Some(items) => items
            .iter()
            .map(|x| {
                x.as_i64()
                    .ok_or_else(|| p.shape(command, &format!("{field} holds {x}, not a number")))
            })
            .collect(),
        None => Err(p.shape(command, &format!("the answer has no {field:?} array"))),
    }
}

// ---------------------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------------------

dist_test!(a_fresh_run, |ctx| {
    let p = ctx.prim("gossip").await?;
    let init = p.send("init 12 2").await?;
    let who = p.send("infected").await?;
    let mut c = Check::new("a rumour that has not been told to anyone yet");
    c.eq("init.nodes", 12, p.expect_i64(&init, "init 12 2", "nodes")?);
    c.eq(
        "init.fanout",
        2,
        p.expect_i64(&init, "init 12 2", "fanout")?,
    );
    c.eq(
        "init.infected",
        1,
        p.expect_i64(&init, "init 12 2", "infected")?,
    );
    c.eq("init.round", 0, p.expect_i64(&init, "init 12 2", "round")?);
    c.eq(
        "infected.nodes",
        vec![0],
        numbers(p, &who, "infected", "nodes")?,
    );
    c.block("transcript", p.transcript_block());
    c.finish()
});

dist_test!(the_first_round, |ctx| {
    let p = ctx.prim("gossip").await?;
    p.send("init 16 1").await?;
    let r = p.send("round").await?;
    let who = p.send("infected").await?;
    let mut c = Check::new("the first round of a fanout-one run");
    // The step in round one is (fanout+1)^0, which is 1 whatever the fanout is, so node 0
    // always starts by telling its immediate neighbour.
    c.eq("round.round", 1, p.expect_i64(&r, "round", "round")?);
    c.eq("round.new", 1, p.expect_i64(&r, "round", "new")?);
    c.eq("round.infected", 2, p.expect_i64(&r, "round", "infected")?);
    c.eq("round.messages", 1, p.expect_i64(&r, "round", "messages")?);
    c.eq(
        "infected.nodes",
        vec![0, 1],
        numbers(p, &who, "infected", "nodes")?,
    );
    c.block("transcript", p.transcript_block());
    c.finish()
});

dist_test!(the_set_doubles, |ctx| {
    let p = ctx.prim("gossip").await?;
    p.send("init 8 1").await?;
    let mut rounds = Vec::new();
    let mut sets = Vec::new();
    for _ in 0..3 {
        let r = p.send("round").await?;
        rounds.push((
            p.expect_i64(&r, "round", "infected")?,
            p.expect_i64(&r, "round", "new")?,
            p.expect_i64(&r, "round", "messages")?,
        ));
        let who = p.send("infected").await?;
        sets.push(numbers(p, &who, "infected", "nodes")?);
    }
    let mut c = Check::new("three rounds over eight nodes at fanout one");
    c.eq(
        "the (infected, new, messages) of each round",
        vec![(2, 1, 1), (4, 2, 3), (8, 4, 7)],
        rounds,
    );
    c.eq(
        "the infected set after each round",
        vec![vec![0, 1], vec![0, 1, 2, 3], vec![0, 1, 2, 3, 4, 5, 6, 7]],
        sets,
    );
    c.block("transcript", p.transcript_block());
    c.finish()
});

dist_test!(a_wider_fanout_starts_faster, |ctx| {
    let p = ctx.prim("gossip").await?;
    p.send("init 9 2").await?;
    let first = p.send("round").await?;
    let after_first = p.send("infected").await?;
    let second = p.send("round").await?;
    let mut c = Check::new("two rounds over nine nodes at fanout two");
    c.eq(
        "the first round.new",
        2,
        p.expect_i64(&first, "round", "new")?,
    );
    c.eq(
        "infected after the first round",
        vec![0, 1, 2],
        numbers(p, &after_first, "infected", "nodes")?,
    );
    // The step triples rather than doubles, so three nodes reach the remaining six at once.
    c.eq(
        "the second round.new",
        6,
        p.expect_i64(&second, "round", "new")?,
    );
    c.eq(
        "the second round.infected",
        9,
        p.expect_i64(&second, "round", "infected")?,
    );
    c.eq(
        "the second round.messages",
        8,
        p.expect_i64(&second, "round", "messages")?,
    );
    c.block("transcript", p.transcript_block());
    c.finish()
});

dist_test!(convergence_is_logarithmic, |ctx| {
    let p = ctx.prim("gossip").await?;
    let cases = [(8usize, 1usize), (32, 1), (9, 2), (32, 2), (32, 3), (40, 4)];
    let mut seen = Vec::new();
    let mut want = Vec::new();
    for (n, f) in cases {
        p.send(&format!("init {n} {f}")).await?;
        let c = p.send("converge").await?;
        seen.push((
            p.expect_i64(&c, "converge", "rounds")?,
            p.expect_i64(&c, "converge", "infected")?,
        ));
        let (rounds, _, infected) = converge(n, f);
        want.push((rounds as i64, infected as i64));
    }
    let mut c = Check::new("convergence over six different shapes of cluster");
    c.eq("the (rounds, infected) of each case", want, seen);
    c.block("transcript", p.transcript_block());
    c.finish()
});

dist_test!(the_fanout_trade, |ctx| {
    let p = ctx.prim("gossip").await?;
    let mut rows = Vec::new();
    for f in [1usize, 2, 3, 4] {
        p.send(&format!("init 32 {f}")).await?;
        let c = p.send("converge").await?;
        rows.push((
            f,
            p.expect_i64(&c, "converge", "rounds")?,
            p.expect_i64(&c, "converge", "messages")?,
        ));
    }
    let transcript = p.transcript_block();
    let mut c = Check::new("thirty-two nodes converged at four different fanouts");
    for (f, rounds, messages) in &rows {
        let (want_rounds, want_messages, _) = converge(32, *f);
        c.eq(&format!("fanout {f}: rounds"), want_rounds as i64, *rounds);
        c.eq(
            &format!("fanout {f}: messages"),
            want_messages as i64,
            *messages,
        );
    }
    for w in rows.windows(2) {
        c.that(
            &format!("fanout {} against {}", w[0].0, w[1].0),
            "no more rounds than the narrower fanout needed",
            w[1].1 <= w[0].1,
            (w[0].1, w[1].1),
        );
    }
    // The trade, in one line: four times the fanout saves two rounds and costs four times
    // the traffic. Every round saved is bought, and the price only goes up.
    if let (Some(narrow), Some(wide)) = (rows.first(), rows.last()) {
        c.that(
            "fanout 4 against fanout 1: rounds",
            "strictly fewer rounds",
            wide.1 < narrow.1,
            (narrow.1, wide.1),
        );
        c.that(
            "fanout 4 against fanout 1: messages",
            "strictly more messages",
            wide.2 > narrow.2,
            (narrow.2, wide.2),
        );
    }
    c.block("transcript", transcript);
    c.finish()
});

dist_test!(the_degenerate_cases, |ctx| {
    let p = ctx.prim("gossip").await?;
    p.send("init 1 1").await?;
    let alone = p.send("converge").await?;
    p.send("init 5 5").await?;
    let wide = p.send("converge").await?;
    p.send("init 2 1").await?;
    let pair = p.send("converge").await?;
    let mut c = Check::new("the three shapes of cluster where the answer is obvious");
    // A rumour that nobody else needs to hear takes no rounds at all.
    c.eq(
        "converge(1, 1).rounds",
        0,
        p.expect_i64(&alone, "converge", "rounds")?,
    );
    c.eq(
        "converge(1, 1).messages",
        0,
        p.expect_i64(&alone, "converge", "messages")?,
    );
    c.eq(
        "converge(5, 5).rounds",
        1,
        p.expect_i64(&wide, "converge", "rounds")?,
    );
    c.eq(
        "converge(5, 5).infected",
        5,
        p.expect_i64(&wide, "converge", "infected")?,
    );
    c.eq(
        "converge(2, 1).rounds",
        1,
        p.expect_i64(&pair, "converge", "rounds")?,
    );
    c.block("transcript", p.transcript_block());
    c.finish()
});

dist_test!(wasted_messages_are_counted, |ctx| {
    let p = ctx.prim("gossip").await?;
    p.send("init 6 1").await?;
    p.send("round").await?;
    p.send("round").await?;
    let third = p.send("round").await?;
    let mut c = Check::new("the round where the schedule starts wrapping round the ring");
    // Four nodes each sent one message, and two of the four landed on somebody who already
    // knew. Counting only the useful ones would make gossip look free, which it is not.
    c.eq(
        "the third round.new",
        2,
        p.expect_i64(&third, "round", "new")?,
    );
    c.eq(
        "the third round.infected",
        6,
        p.expect_i64(&third, "round", "infected")?,
    );
    c.eq(
        "the third round.messages",
        7,
        p.expect_i64(&third, "round", "messages")?,
    );
    c.block("transcript", p.transcript_block());
    c.finish()
});

dist_test!(a_round_past_convergence, |ctx| {
    let p = ctx.prim("gossip").await?;
    p.send("init 8 1").await?;
    let done = p.send("converge").await?;
    let extra = p.send("round").await?;
    let s = p.send("state").await?;
    let mut c = Check::new("one more round after everybody already knows");
    c.eq(
        "converge.messages",
        7,
        p.expect_i64(&done, "converge", "messages")?,
    );
    c.eq(
        "the extra round.new",
        0,
        p.expect_i64(&extra, "round", "new")?,
    );
    c.eq(
        "the extra round.infected",
        8,
        p.expect_i64(&extra, "round", "infected")?,
    );
    // Eight more messages for nothing. A real implementation stops gossiping a rumour once
    // it keeps hearing it back, and this is the cost it is avoiding.
    c.eq(
        "the extra round.messages",
        15,
        p.expect_i64(&extra, "round", "messages")?,
    );
    c.eq("state.round", 4, p.expect_i64(&s, "state", "round")?);
    c.block("transcript", p.transcript_block());
    c.finish()
});

dist_test!(a_sweep_of_sizes_and_fanouts, |ctx| {
    let p = ctx.prim("gossip").await?;
    let mut rows = Vec::new();
    for n in 2usize..=40 {
        for f in 1usize..=4 {
            p.send(&format!("init {n} {f}")).await?;
            let c = p.send("converge").await?;
            rows.push((
                n,
                f,
                p.expect_i64(&c, "converge", "rounds")?,
                p.expect_i64(&c, "converge", "messages")?,
                p.expect_i64(&c, "converge", "infected")?,
            ));
        }
    }
    let transcript = p.transcript_block();
    let mut c = Check::new("every cluster size from two to forty at every fanout from one to four");
    for (n, f, rounds, messages, infected) in rows {
        let (want_rounds, want_messages, want_infected) = converge(n, f);
        c.eq(
            &format!("converge({n}, {f}).rounds"),
            want_rounds as i64,
            rounds,
        );
        c.eq(
            &format!("converge({n}, {f}).messages"),
            want_messages as i64,
            messages,
        );
        c.eq(
            &format!("converge({n}, {f}).infected"),
            want_infected as i64,
            infected,
        );
        c.that(
            &format!("converge({n}, {f})"),
            "a schedule that reaches every node",
            infected == n as i64,
            infected,
        );
        if !c.ok() {
            break;
        }
    }
    c.block("transcript", transcript);
    c.finish()
});
