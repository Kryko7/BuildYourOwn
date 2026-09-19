//! Stage 70 — Retry with an idempotency key.
//!
//! A client sends a charge, the answer is lost on the way back, and the client does the
//! only thing it can: it sends the charge again. The server has no way to tell that request
//! apart from a second, genuine purchase — unless the client names the attempt. An
//! idempotency key is that name: the server stores the answer under it and, on a retry,
//! replays the stored answer instead of taking the money again.
//!
//! The oracle is the pair of rules that make the scheme safe. A retry under a key that has
//! finished replays the original answer and the original charge id, so the balance does not
//! move; and a key is bound to the request it was first seen with, so reusing it for a
//! different amount is refused rather than silently answered with somebody else's receipt.
//! The stage opens with the naive path, because the double charge is the thing being fixed.

use crate::assert::Check;
use crate::dist_test;
use crate::examples::{lines, prim_example, ExampleSpec};
use crate::stages::{Ladder, Stage, Test};
use rand::Rng;
use std::collections::BTreeMap;

/// Stage 70.
pub fn stage() -> Stage {
    Stage {
        number: 70,
        slug: "idempotency_keys",
        name: "Retry with an idempotency key",
        ext: false,
        ladder: Ladder::Algorithms,
        hints: &[
            "Topic `idempotency-key`: store the answer under the key, and replay it on a retry",
            "A key is bound to its request: the same key with a different amount is an error",
            "A key whose first request is still running refuses the second, it does not race it",
            "The client cannot tell a lost answer from a lost request, which is why it retries",
        ],
        examples,
        tests: vec![
            Test::new("a naive retry charges twice", a_naive_retry_charges_twice),
            Test::new(
                "a retry with the same key replays the first answer",
                a_retry_replays_the_first_answer,
            ),
            Test::new(
                "a key reused with a different amount is refused",
                a_key_is_bound_to_its_request,
            ),
            Test::new(
                "a second request while the first is in progress is refused",
                an_in_progress_key_refuses_a_second_request,
            ),
            Test::new(
                "a key that has finished replays for ever",
                a_finished_key_replays_for_ever,
            ),
            Test::new(
                "different keys with the same amount are different charges",
                different_keys_are_different_charges,
            ),
            Test::new(
                "finishing a key twice does not charge twice",
                finishing_twice_charges_once,
            ),
            Test::new(
                "a begin whose amount disagrees with the key is refused",
                a_begin_with_the_wrong_amount_is_refused,
            )
            .ext(),
            Test::new(
                "the key survives any number of lost answers",
                the_key_survives_many_lost_answers,
            )
            .ext(),
            Test::new(
                "a seeded run of charges and retries sums the distinct keys",
                a_seeded_run_of_retries,
            )
            .ext(),
        ],
    }
}

fn examples() -> Vec<ExampleSpec> {
    vec![
        prim_example(
            "The same retry, with and without a key",
            "idempotency-key",
            || {
                lines(&[
                    "init",
                    "naive-charge 100",
                    "naive-charge 100",
                    "charge k1 100",
                    "lost-answer k1",
                    "charge k1 100",
                    "state",
                ])
            },
        )
        .request("one purchase retried once, first with no key and then with one")
        .response("the naive pair leaves 200 on the balance; the keyed pair adds only 100")
        .note(
            "The two requests are byte for byte identical in both halves. The only \
             difference is that the second pair names the attempt, which turns 'charge this \
             card' into 'make sure this particular charge has happened' — an instruction \
             that is safe to repeat.",
        ),
        prim_example(
            "A key reused for a different request",
            "idempotency-key",
            || lines(&["init", "charge k1 100", "charge k1 200", "state"]),
        )
        .request("a key that has already answered, sent again with a different amount")
        .response("refused, and the balance does not move")
        .note(
            "Replaying the stored answer here would be worse than double charging: the \
             client would be told its 200 succeeded and would be shown a receipt for 100. \
             A key that has been reused is a client bug, and the only safe response is to \
             say so rather than to guess which request was meant.",
        ),
    ]
}

// ---------------------------------------------------------------------------------------
// The tester's own model. Nothing below consults the program for an answer.
// ---------------------------------------------------------------------------------------

/// The balance the rules demand: one charge per distinct key, plus every unkeyed charge.
struct Model {
    keys: BTreeMap<String, i64>,
    unkeyed: i64,
}

impl Model {
    fn new() -> Model {
        Model {
            keys: BTreeMap::new(),
            unkeyed: 0,
        }
    }

    /// Returns whether this request was a replay of an answer already stored.
    fn charge(&mut self, key: &str, amount: i64) -> bool {
        match self.keys.get(key) {
            Some(_) => true,
            None => {
                self.keys.insert(key.to_string(), amount);
                false
            }
        }
    }

    fn balance(&self) -> i64 {
        self.unkeyed + self.keys.values().sum::<i64>()
    }
}

// ---------------------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------------------

dist_test!(a_naive_retry_charges_twice, |ctx| {
    let p = ctx.prim("idempotency-key").await?;
    p.send("init").await?;
    let first = p.send("naive-charge 100").await?;
    // The client never got the first answer, so it sent the request again. The server has
    // no way to tell this apart from a second genuine purchase, and charges for both.
    let second = p.send("naive-charge 100").await?;
    let state = p.send("state").await?;
    let mut c = Check::new("one purchase retried, with nothing naming the attempt");
    c.eq(
        "the first naive-charge.balance",
        100,
        p.expect_i64(&first, "naive-charge 100", "balance")?,
    );
    c.eq(
        "the second naive-charge.balance",
        200,
        p.expect_i64(&second, "naive-charge 100", "balance")?,
    );
    c.ne(
        "the two charge ids",
        p.expect_str(&first, "naive-charge 100", "charge_id")?,
        p.expect_str(&second, "naive-charge 100", "charge_id")?,
    );
    c.eq(
        "state.balance",
        200,
        p.expect_i64(&state, "state", "balance")?,
    );
    c.block("transcript", p.transcript_block());
    c.finish()
});

dist_test!(a_retry_replays_the_first_answer, |ctx| {
    let p = ctx.prim("idempotency-key").await?;
    p.send("init").await?;
    let first = p.send("charge k1 100").await?;
    p.send("lost-answer k1").await?;
    let retry = p.send("charge k1 100").await?;
    let state = p.send("state").await?;
    let id = p.expect_str(&first, "charge k1 100", "charge_id")?;
    let mut c = Check::new("the same purchase retried under one key");
    c.eq(
        "the first charge.replayed",
        false,
        p.expect_bool(&first, "charge k1 100", "replayed")?,
    );
    c.eq(
        "the retry's charge.replayed",
        true,
        p.expect_bool(&retry, "charge k1 100", "replayed")?,
    );
    // The stored answer is replayed rather than recomputed, so the client sees the receipt
    // it would have seen had the first answer arrived.
    c.eq(
        "the retry's charge_id",
        id,
        p.expect_str(&retry, "charge k1 100", "charge_id")?,
    );
    c.eq(
        "the retry's balance",
        100,
        p.expect_i64(&retry, "charge k1 100", "balance")?,
    );
    c.eq(
        "state.balance",
        100,
        p.expect_i64(&state, "state", "balance")?,
    );
    c.block("transcript", p.transcript_block());
    c.finish()
});

dist_test!(a_key_is_bound_to_its_request, |ctx| {
    let p = ctx.prim("idempotency-key").await?;
    p.send("init").await?;
    p.send("charge k1 100").await?;
    let reused = p.send("charge k1 200").await?;
    let state = p.send("state").await?;
    let error = p.expect_str(&reused, "charge k1 200", "error")?;
    let mut c = Check::new("one key used for two different requests");
    // Replaying the stored answer here would tell the client its 200 had succeeded, and
    // hand it a receipt for 100. That is worse than charging twice, because the client has
    // no way to discover the discrepancy.
    c.eq(
        "charge(k1, 200).ok",
        false,
        p.expect_bool(&reused, "charge k1 200", "ok")?,
    );
    c.that(
        "charge(k1, 200).error",
        "an error naming both amounts",
        error.contains("100") && error.contains("200"),
        &error,
    );
    c.eq(
        "state.balance",
        100,
        p.expect_i64(&state, "state", "balance")?,
    );
    c.block("transcript", p.transcript_block());
    c.finish()
});

dist_test!(an_in_progress_key_refuses_a_second_request, |ctx| {
    let p = ctx.prim("idempotency-key").await?;
    p.send("init").await?;
    let begun = p.send("begin k1 100").await?;
    // The client gave up waiting and retried while the first request is still running.
    // Letting the second one through would charge the card twice with one key.
    let racing = p.send("charge k1 100").await?;
    let state_during = p.send("state").await?;
    p.send("finish k1").await?;
    let after = p.send("charge k1 100").await?;
    let mut c = Check::new("a retry that overtook its own first attempt");
    c.eq(
        "begin(k1).ok",
        true,
        p.expect_bool(&begun, "begin k1 100", "ok")?,
    );
    c.eq(
        "begin(k1).state",
        "in-progress".to_string(),
        p.expect_str(&begun, "begin k1 100", "state")?,
    );
    c.eq(
        "the racing charge.ok",
        false,
        p.expect_bool(&racing, "charge k1 100", "ok")?,
    );
    c.eq(
        "the racing charge.error",
        "in progress".to_string(),
        p.expect_str(&racing, "charge k1 100", "error")?,
    );
    c.eq(
        "state.balance while the first request is running",
        0,
        p.expect_i64(&state_during, "state", "balance")?,
    );
    c.eq(
        "the charge after finish, replayed",
        true,
        p.expect_bool(&after, "charge k1 100", "replayed")?,
    );
    c.eq(
        "state.balance after finish",
        100,
        p.expect_i64(&after, "charge k1 100", "balance")?,
    );
    c.block("transcript", p.transcript_block());
    c.finish()
});

dist_test!(a_finished_key_replays_for_ever, |ctx| {
    let p = ctx.prim("idempotency-key").await?;
    p.send("init").await?;
    let first = p.send("charge k1 42").await?;
    let id = p.expect_str(&first, "charge k1 42", "charge_id")?;
    let mut balances = Vec::new();
    let mut ids = Vec::new();
    for _ in 0..5 {
        let r = p.send("charge k1 42").await?;
        balances.push(p.expect_i64(&r, "charge k1 42", "balance")?);
        ids.push(p.expect_str(&r, "charge k1 42", "charge_id")?);
    }
    let mut c = Check::new("five retries of a purchase that already succeeded");
    c.eq(
        "the balances through the retries",
        vec![42, 42, 42, 42, 42],
        balances,
    );
    c.eq("the charge ids through the retries", vec![id; 5], ids);
    c.block("transcript", p.transcript_block());
    c.finish()
});

dist_test!(different_keys_are_different_charges, |ctx| {
    let p = ctx.prim("idempotency-key").await?;
    p.send("init").await?;
    let first = p.send("charge k1 100").await?;
    let second = p.send("charge k2 100").await?;
    let state = p.send("state").await?;
    let mut c = Check::new("two purchases of the same thing, named differently");
    // Two customers buying the same item for the same price is not a retry, and a server
    // that deduplicates on the request body rather than the key swallows the second sale.
    c.eq(
        "charge(k2).replayed",
        false,
        p.expect_bool(&second, "charge k2 100", "replayed")?,
    );
    c.ne(
        "the two charge ids",
        p.expect_str(&first, "charge k1 100", "charge_id")?,
        p.expect_str(&second, "charge k2 100", "charge_id")?,
    );
    c.eq(
        "state.balance",
        200,
        p.expect_i64(&state, "state", "balance")?,
    );
    c.block("transcript", p.transcript_block());
    c.finish()
});

dist_test!(finishing_twice_charges_once, |ctx| {
    let p = ctx.prim("idempotency-key").await?;
    p.send("init").await?;
    p.send("begin k1 75").await?;
    let first = p.send("finish k1").await?;
    // The worker that completed the request died before recording that it had, so the next
    // one finishes it again. The key has the answer, so the second finish only reads it.
    let second = p.send("finish k1").await?;
    let state = p.send("state").await?;
    let mut c = Check::new("a request finished twice");
    c.eq(
        "the first finish.balance",
        75,
        p.expect_i64(&first, "finish k1", "balance")?,
    );
    c.eq(
        "the second finish.balance",
        75,
        p.expect_i64(&second, "finish k1", "balance")?,
    );
    c.eq(
        "the two charge ids",
        p.expect_str(&first, "finish k1", "charge_id")?,
        p.expect_str(&second, "finish k1", "charge_id")?,
    );
    c.eq(
        "state.balance",
        75,
        p.expect_i64(&state, "state", "balance")?,
    );
    c.block("transcript", p.transcript_block());
    c.finish()
});

dist_test!(a_begin_with_the_wrong_amount_is_refused, |ctx| {
    let p = ctx.prim("idempotency-key").await?;
    p.send("init").await?;
    p.send("charge k1 100").await?;
    let wrong = p.send("begin k1 999").await?;
    p.send("begin k2 50").await?;
    // The binding applies while a request is still running, too: a retry that disagrees
    // with the request in flight is a client bug whichever state the key is in.
    let wrong_again = p.send("begin k2 51").await?;
    let state = p.send("state").await?;
    let mut c = Check::new("a begin that disagrees with the key it names");
    c.eq(
        "begin(k1, 999).ok",
        false,
        p.expect_bool(&wrong, "begin k1 999", "ok")?,
    );
    c.eq(
        "begin(k2, 51).ok",
        false,
        p.expect_bool(&wrong_again, "begin k2 51", "ok")?,
    );
    c.eq(
        "state.balance",
        100,
        p.expect_i64(&state, "state", "balance")?,
    );
    c.block("transcript", p.transcript_block());
    c.finish()
});

dist_test!(the_key_survives_many_lost_answers, |ctx| {
    let p = ctx.prim("idempotency-key").await?;
    p.send("init").await?;
    let first = p.send("charge k1 30").await?;
    let id = p.expect_str(&first, "charge k1 30", "charge_id")?;
    // A client behind a flapping link: every answer is lost, so it keeps retrying. The
    // balance must be the same after ten attempts as after one.
    let mut last = 0;
    for _ in 0..10 {
        p.send("lost-answer k1").await?;
        let r = p.send("charge k1 30").await?;
        last = p.expect_i64(&r, "charge k1 30", "balance")?;
    }
    let state = p.send("state").await?;
    let final_id = p.send("charge k1 30").await?;
    let mut c = Check::new("ten retries of one purchase over a flapping link");
    c.eq("the balance after the last retry", 30, last);
    c.eq(
        "state.balance",
        30,
        p.expect_i64(&state, "state", "balance")?,
    );
    c.eq(
        "the charge id after ten retries",
        id,
        p.expect_str(&final_id, "charge k1 30", "charge_id")?,
    );
    c.block("transcript", p.transcript_block());
    c.finish()
});

dist_test!(a_seeded_run_of_retries, |ctx| {
    // Keys are drawn from a small pool so retries are frequent, and each key keeps whatever
    // amount it was first seen with. The balance the rules demand is the sum over distinct
    // keys, and nothing else can be right however the requests were ordered.
    let pool: Vec<String> = (0..6).map(|i| format!("k{i}")).collect();
    let mut model = Model::new();
    let mut script: Vec<(String, bool, i64)> = Vec::new();
    for _ in 0..60 {
        let command = if ctx.rng.random_range(0..8) == 0 {
            let amount = ctx.rng.random_range(1..20);
            model.unkeyed += amount;
            (format!("naive-charge {amount}"), false, model.balance())
        } else {
            let key = pool[ctx.rng.random_range(0..pool.len())].clone();
            // A retry always carries the same request, which is what a real client does.
            let amount = *model
                .keys
                .get(&key)
                .unwrap_or(&((key.len() as i64) + ctx.rng.random_range(1..20)));
            let replayed = model.charge(&key, amount);
            (format!("charge {key} {amount}"), replayed, model.balance())
        };
        script.push(command);
    }
    let seed = ctx.seed;
    let want_balance = model.balance();
    let p = ctx.prim("idempotency-key").await?;
    p.send("init").await?;
    let mut actual: Vec<(Option<bool>, i64)> = Vec::new();
    for (command, ..) in &script {
        let r = p.send(command).await?;
        let replayed = if command.starts_with("charge") {
            Some(p.expect_bool(&r, command, "replayed")?)
        } else {
            None
        };
        actual.push((replayed, p.expect_i64(&r, command, "balance")?));
    }
    let state = p.send("state").await?;
    let got_balance = p.expect_i64(&state, "state", "balance")?;
    let transcript = p.transcript_block();
    let mut c = Check::new("sixty seeded charges and retries over six keys");
    c.note(format!(
        "seed {seed}, {} requests, {} distinct keys",
        script.len(),
        model.keys.len()
    ));
    for (i, ((command, replayed, balance), (got_r, got_b))) in
        script.iter().zip(&actual).enumerate()
    {
        if let Some(got_r) = got_r {
            c.eq(
                &format!("step[{i}] {command} → replayed"),
                *replayed,
                *got_r,
            );
        }
        c.eq(&format!("step[{i}] {command} → balance"), *balance, *got_b);
        if !c.ok() {
            break;
        }
    }
    c.eq("state.balance", want_balance, got_balance);
    c.block("transcript", transcript);
    c.finish()
});
