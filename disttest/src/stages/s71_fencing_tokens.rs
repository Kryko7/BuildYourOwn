//! Stage 71 — Distributed locks and fencing tokens.
//!
//! A lock that only ever answers "yes, you hold it" is not a lock. The holder can be paused
//! — by a garbage collector, a hypervisor, a swapped-out page — for longer than its own
//! lease, wake up believing nothing has happened, and write over the work of whoever took
//! the lock in the meantime. No amount of checking before writing closes that window,
//! because the pause can land between the check and the write. What closes it is a token
//! that goes up on every grant and a resource that refuses anything below the highest token
//! it has already seen.
//!
//! The oracle is that one rule. The tester keeps the token counter and the resource's fence
//! itself, decides for every write whether it should have been accepted, and compares — it
//! never asks the program whether it thinks the write was legal. The seeded replay at the
//! end walks sixty interleavings of acquire, expiry and write and asserts the resource's
//! value is always the one written under the highest token it has accepted, which is the
//! property the whole pattern exists to provide.

use crate::assert::Check;
use crate::dist_test;
use crate::examples::{lines, prim_example, ExampleSpec};
use crate::stages::{Ladder, Stage, Test};
use rand::Rng;
use std::collections::BTreeMap;

/// Stage 71.
pub fn stage() -> Stage {
    Stage {
        number: 71,
        slug: "fencing_tokens",
        name: "Distributed locks and fencing tokens",
        ext: false,
        ladder: Ladder::Algorithms,
        hints: &[
            "Topic `fencing`: every grant hands out a token one higher than the last",
            "The resource keeps a fence: the highest token it has accepted a write from",
            "A write below the fence is refused, however firmly its client believes it holds the lock",
            "Checking `holder` before writing proves nothing: the pause happens after the check",
        ],
        examples,
        tests: vec![
            Test::new("a fresh lock has handed out no tokens", a_fresh_lock),
            Test::new(
                "every grant hands out a strictly higher token",
                tokens_only_go_up,
            ),
            Test::new("a held lock cannot be taken", a_held_lock_cannot_be_taken),
            Test::new("the fence rises with every accepted write", the_fence_rises),
            Test::new(
                "a write at the fence is the holder writing twice",
                a_write_at_the_fence_is_accepted,
            ),
            Test::new(
                "a paused holder's write is refused once the lock has moved on",
                the_fenced_scenario,
            ),
            Test::new(
                "the same story without a fence corrupts the resource",
                the_unfenced_scenario,
            ),
            Test::new(
                "a client that never acquired has no token",
                no_token_at_all,
            ),
            Test::new(
                "believing you hold the lock is not enough",
                believing_is_not_enough,
            )
            .ext(),
            Test::new(
                "sixty seeded interleavings keep the highest token's value",
                a_long_seeded_run,
            )
            .ext(),
        ],
    }
}

fn examples() -> Vec<ExampleSpec> {
    vec![
        prim_example("A paused holder, fenced out", "fencing", || {
            lines(&[
                "acquire c1",
                "expire",
                "acquire c2",
                "write c2 2 v2",
                "write c1 1 v1",
                "state",
            ])
        })
        .request("c1 takes the lock and stalls, the lease expires, c2 takes it and writes")
        .response("c2's write is accepted and raises the fence to 2; c1's write is refused")
        .note(
            "c1 is not misbehaving: from its own point of view it holds the lock and is \
             writing exactly once. Nothing it can check tells it otherwise, because the \
             pause happened after the check. Only the resource, comparing token 1 against a \
             fence of 2, is in a position to notice.",
        ),
        prim_example("The same story with no fence", "fencing", || {
            lines(&[
                "acquire c1",
                "expire",
                "acquire c2",
                "write-unfenced c2 v2",
                "write-unfenced c1 v1",
                "state",
            ])
        })
        .request("the identical sequence against a resource that does not check tokens")
        .response("c1's stale write lands last and the resource holds v1")
        .note(
            "This is the failure the pattern exists to prevent, and it is silent: no error \
             is reported anywhere, the lock service behaved correctly throughout, and the \
             data is simply wrong afterwards.",
        ),
    ]
}

// ---------------------------------------------------------------------------------------
// The tester's own model of the lock and the fence. Nothing below consults the program.
// ---------------------------------------------------------------------------------------

/// A lock that hands out increasing tokens, and the one resource its holders write to.
#[derive(Default)]
struct Model {
    holder: Option<String>,
    token: i64,
    fence: i64,
    value: Option<String>,
    /// The last token each client was ever granted, which is what it would present.
    granted: BTreeMap<String, i64>,
}

impl Model {
    fn acquire(&mut self, client: &str) {
        if self.holder.is_some() {
            return;
        }
        self.token += 1;
        self.holder = Some(client.to_string());
        self.granted.insert(client.to_string(), self.token);
    }

    fn release(&mut self, client: &str) {
        if self.holder.as_deref() == Some(client) {
            self.holder = None;
        }
    }

    fn expire(&mut self) {
        self.holder = None;
    }

    /// The token this client would present, or zero when it has never held the lock.
    fn token_of(&self, client: &str) -> i64 {
        self.granted.get(client).copied().unwrap_or(0)
    }

    fn write(&mut self, client: &str, value: &str) {
        let token = self.token_of(client);
        if token <= 0 || token < self.fence {
            return;
        }
        self.fence = token;
        self.value = Some(value.to_string());
    }
}

// ---------------------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------------------

dist_test!(a_fresh_lock, |ctx| {
    let p = ctx.prim("fencing").await?;
    let init = p.send("init").await?;
    let s = p.send("state").await?;
    let mut c = Check::new("a lock nobody has touched");
    c.eq("init.token", 0, p.expect_i64(&init, "init", "token")?);
    c.eq("init.fence", 0, p.expect_i64(&init, "init", "fence")?);
    c.eq("state.holder", &serde_json::Value::Null, &s["holder"]);
    c.eq("state.value", &serde_json::Value::Null, &s["value"]);
    c.eq("state.fence", 0, p.expect_i64(&s, "state", "fence")?);
    c.block("transcript", p.transcript_block());
    c.finish()
});

dist_test!(tokens_only_go_up, |ctx| {
    let p = ctx.prim("fencing").await?;
    p.send("init").await?;
    let mut tokens = Vec::new();
    // Released, expired, released again: whichever way the lock comes free, the next token
    // is higher. Reusing a token would let two holders present the same credential.
    for (client, how) in [("c1", "release c1"), ("c2", "expire"), ("c1", "release c1")] {
        let a = p.send(&format!("acquire {client}")).await?;
        tokens.push(p.expect_i64(&a, "acquire", "token")?);
        p.send(how).await?;
    }
    let last = p.send("acquire c3").await?;
    tokens.push(p.expect_i64(&last, "acquire c3", "token")?);
    let mut c = Check::new("four grants of the same lock");
    c.eq("the tokens handed out", vec![1, 2, 3, 4], tokens.clone());
    for w in tokens.windows(2) {
        c.that(
            "tokens",
            "a strictly higher token on every grant",
            w[1] > w[0],
            (w[0], w[1]),
        );
    }
    c.block("transcript", p.transcript_block());
    c.finish()
});

dist_test!(a_held_lock_cannot_be_taken, |ctx| {
    let p = ctx.prim("fencing").await?;
    p.send("init").await?;
    p.send("acquire c1").await?;
    let refused = p.send("acquire c2").await?;
    let released = p.send("release c2").await?;
    let then = p.send("acquire c2").await?;
    let mut c = Check::new("a second client asking for a lock that is already held");
    c.eq(
        "acquire(c2).granted while c1 holds it",
        false,
        p.expect_bool(&refused, "acquire c2", "granted")?,
    );
    c.eq(
        "acquire(c2).holder",
        "c1".to_string(),
        p.expect_str(&refused, "acquire c2", "holder")?,
    );
    // Releasing a lock somebody else holds is a no-op, not a theft.
    c.eq(
        "release(c2).ok",
        false,
        p.expect_bool(&released, "release c2", "ok")?,
    );
    c.eq(
        "the second acquire(c2).granted",
        false,
        p.expect_bool(&then, "acquire c2", "granted")?,
    );
    c.block("transcript", p.transcript_block());
    c.finish()
});

dist_test!(the_fence_rises, |ctx| {
    let p = ctx.prim("fencing").await?;
    p.send("init").await?;
    let mut fences = Vec::new();
    for (client, token, value) in [("c1", 1, "a"), ("c2", 2, "b"), ("c3", 3, "c")] {
        p.send(&format!("acquire {client}")).await?;
        let w = p.send(&format!("write {client} {token} {value}")).await?;
        fences.push(p.expect_i64(&w, "write", "fence")?);
        p.send("expire").await?;
    }
    let s = p.send("state").await?;
    let mut c = Check::new("three holders in turn, each writing once");
    c.eq("the fence after each write", vec![1, 2, 3], fences);
    c.eq(
        "state.value",
        "c".to_string(),
        p.expect_str(&s, "state", "value")?,
    );
    c.eq("state.fence", 3, p.expect_i64(&s, "state", "fence")?);
    c.block("transcript", p.transcript_block());
    c.finish()
});

dist_test!(a_write_at_the_fence_is_accepted, |ctx| {
    let p = ctx.prim("fencing").await?;
    p.send("init").await?;
    p.send("acquire c1").await?;
    let first = p.send("write c1 1 a").await?;
    let second = p.send("write c1 1 b").await?;
    let s = p.send("state").await?;
    let mut c = Check::new("the current holder writing twice under one token");
    c.eq(
        "the first write(c1, 1).ok",
        true,
        p.expect_bool(&first, "write c1 1 a", "ok")?,
    );
    // The rule is `token < fence`, not `token <= fence`: a holder that writes twice is
    // ordinary, and refusing its second write would make the lock useless.
    c.eq(
        "the second write(c1, 1).ok",
        true,
        p.expect_bool(&second, "write c1 1 b", "ok")?,
    );
    c.eq(
        "the second write(c1, 1).reason",
        "accepted".to_string(),
        p.expect_str(&second, "write c1 1 b", "reason")?,
    );
    c.eq(
        "state.value",
        "b".to_string(),
        p.expect_str(&s, "state", "value")?,
    );
    c.block("transcript", p.transcript_block());
    c.finish()
});

dist_test!(the_fenced_scenario, |ctx| {
    let p = ctx.prim("fencing").await?;
    p.send("init").await?;
    // c1 takes the lock and is then paused for longer than its own lease.
    let c1 = p.send("acquire c1").await?;
    p.send("expire").await?;
    let c2 = p.send("acquire c2").await?;
    let good = p.send("write c2 2 v2").await?;
    // c1 wakes up with no way of knowing any of that happened, and writes.
    let stale = p.send("write c1 1 v1").await?;
    let s = p.send("state").await?;
    let mut c = Check::new("the Kleppmann scenario against a resource that checks tokens");
    c.eq(
        "acquire(c1).token",
        1,
        p.expect_i64(&c1, "acquire c1", "token")?,
    );
    c.eq(
        "acquire(c2).token",
        2,
        p.expect_i64(&c2, "acquire c2", "token")?,
    );
    c.eq(
        "write(c2, 2).ok",
        true,
        p.expect_bool(&good, "write c2 2 v2", "ok")?,
    );
    c.eq(
        "write(c1, 1).ok",
        false,
        p.expect_bool(&stale, "write c1 1 v1", "ok")?,
    );
    c.eq(
        "write(c1, 1).reason",
        "stale token".to_string(),
        p.expect_str(&stale, "write c1 1 v1", "reason")?,
    );
    c.eq(
        "state.value",
        "v2".to_string(),
        p.expect_str(&s, "state", "value")?,
    );
    c.eq("state.fence", 2, p.expect_i64(&s, "state", "fence")?);
    c.note("the same sequence without a fence is the next test");
    c.block("transcript", p.transcript_block());
    c.finish()
});

dist_test!(the_unfenced_scenario, |ctx| {
    let p = ctx.prim("fencing").await?;
    p.send("init").await?;
    p.send("acquire c1").await?;
    p.send("expire").await?;
    p.send("acquire c2").await?;
    p.send("write-unfenced c2 v2").await?;
    p.send("write-unfenced c1 v1").await?;
    let s = p.send("state").await?;
    let mut c = Check::new("the identical sequence against a resource with no fence");
    // Nothing errored anywhere. The lock service was correct, both clients behaved, and the
    // resource now holds the value the stalled client wrote — which is the whole problem.
    c.eq(
        "state.value",
        "v1".to_string(),
        p.expect_str(&s, "state", "value")?,
    );
    c.eq(
        "state.writes",
        vec!["c2:v2".to_string(), "c1:v1".to_string()],
        p.expect_strs(&s, "state", "writes")?,
    );
    c.note("compare with 'a paused holder's write is refused once the lock has moved on'");
    c.block("transcript", p.transcript_block());
    c.finish()
});

dist_test!(no_token_at_all, |ctx| {
    let p = ctx.prim("fencing").await?;
    p.send("init").await?;
    p.send("acquire c1").await?;
    p.send("write c1 1 a").await?;
    let nobody = p.send("write c3 0 x").await?;
    let s = p.send("state").await?;
    let mut c = Check::new("a write from a client that never held the lock");
    c.eq(
        "write(c3, 0).ok",
        false,
        p.expect_bool(&nobody, "write c3 0 x", "ok")?,
    );
    c.eq(
        "write(c3, 0).reason",
        "no token".to_string(),
        p.expect_str(&nobody, "write c3 0 x", "reason")?,
    );
    c.eq(
        "state.value",
        "a".to_string(),
        p.expect_str(&s, "state", "value")?,
    );
    c.block("transcript", p.transcript_block());
    c.finish()
});

dist_test!(believing_is_not_enough, |ctx| {
    let p = ctx.prim("fencing").await?;
    p.send("init").await?;
    p.send("acquire c1").await?;
    // c1 does the responsible thing and checks that it still holds the lock. It does.
    let checked = p.send("state").await?;
    let holder_at_check = p.expect_str(&checked, "state", "holder")?;
    // The pause lands here, between the check and the write, which is the one place no
    // amount of checking can cover.
    p.send("expire").await?;
    p.send("acquire c2").await?;
    p.send("write c2 2 v2").await?;
    let stale = p.send("write c1 1 v1").await?;
    let mut c = Check::new("a holder that verified the lock before writing");
    c.eq(
        "state.holder at the moment c1 checked",
        "c1".to_string(),
        holder_at_check,
    );
    c.eq(
        "write(c1, 1).ok after a correct check",
        false,
        p.expect_bool(&stale, "write c1 1 v1", "ok")?,
    );
    c.eq(
        "write(c1, 1).reason",
        "stale token".to_string(),
        p.expect_str(&stale, "write c1 1 v1", "reason")?,
    );
    c.block("transcript", p.transcript_block());
    c.finish()
});

dist_test!(a_long_seeded_run, |ctx| {
    // The whole conversation is worked out here first — every command and the fence and
    // value it must leave behind — by replaying the token rule over the seeded plan. The
    // program is then asked the same questions and never consulted about the answers.
    let clients = ["c1", "c2", "c3"];
    let mut model = Model::default();
    let mut script: Vec<(String, i64, i64, Option<String>)> = Vec::new();
    for step in 0..60 {
        let who = clients[ctx.rng.random_range(0..clients.len())];
        let command = match ctx.rng.random_range(0..5) {
            0 => {
                model.acquire(who);
                format!("acquire {who}")
            }
            1 => {
                model.release(who);
                format!("release {who}")
            }
            2 => {
                model.expire();
                "expire".to_string()
            }
            _ => {
                // The client presents whatever token it was last given, which is exactly
                // what a client that has been asleep would do.
                let token = model.token_of(who);
                let value = format!("v{step}");
                model.write(who, &value);
                format!("write {who} {token} {value}")
            }
        };
        script.push((command, model.token, model.fence, model.value.clone()));
    }
    let seed = ctx.seed;
    let p = ctx.prim("fencing").await?;
    p.send("init").await?;
    let mut actual = Vec::with_capacity(script.len());
    for (command, ..) in &script {
        p.send(command).await?;
        let s = p.send("state").await?;
        actual.push((
            p.expect_i64(&s, "state", "token")?,
            p.expect_i64(&s, "state", "fence")?,
            s["value"].as_str().map(str::to_string),
        ));
    }
    let transcript = p.transcript_block();
    let mut c = Check::new("sixty seeded interleavings replayed against the tester's own rules");
    c.note(format!("seed {seed}, {} steps", script.len()));
    for (i, ((command, token, fence, value), (got_token, got_fence, got_value))) in
        script.iter().zip(&actual).enumerate()
    {
        c.eq(&format!("step[{i}] {command} → token"), *token, *got_token);
        c.eq(&format!("step[{i}] {command} → fence"), *fence, *got_fence);
        c.eq(
            &format!("step[{i}] {command} → value"),
            value.clone(),
            got_value.clone(),
        );
        if !c.ok() {
            break;
        }
    }
    c.block("transcript", transcript);
    c.finish()
});
