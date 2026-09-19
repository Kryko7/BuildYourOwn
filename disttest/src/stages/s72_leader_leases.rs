//! Stage 72 — Leader leases and clock skew.
//!
//! A lease is the cheapest way to let a leader answer a read out of its own memory: for as
//! long as it holds one, nobody else can be leader, so its own copy is authoritative and no
//! round trip is needed. The whole construction rests on two clocks agreeing about when the
//! lease ends, and they never quite do. A lease is therefore always shortened at the holder's
//! end and lengthened at the granter's end, by the same clock-error budget, and the gap
//! between the two is what keeps two leaders from serving at once.
//!
//! The oracle is that arithmetic. The tester computes `safe_until = expires_at -
//! clock_error` and `next grant >= expires_at + clock_error` itself and asserts both
//! boundaries exactly, one millisecond either side. The exhaustive sweep at the end walks
//! every permitted clock offset, finds the last instant the old holder will still serve a
//! read and the first instant the granter will hand the lease on, and asserts the first is
//! always strictly before the second — which is the safety property, stated directly.

use crate::assert::Check;
use crate::dist_test;
use crate::examples::{lines, prim_example, ExampleSpec};
use crate::stages::{Ladder, Stage, Test};

/// Stage 72.
pub fn stage() -> Stage {
    Stage {
        number: 72,
        slug: "leader_leases",
        name: "Leader leases and clock skew",
        ext: true,
        ladder: Ladder::Algorithms,
        hints: &[
            "Topic `leases`: `now` always arrives as an argument; never read a clock of your own",
            "The holder stops serving at `expires_at - clock_error`, because its clock may be slow",
            "The granter waits until `expires_at + clock_error`, because the old holder's may be slow too",
            "Renewing extends from `now`, not from the old expiry: a late renewal buys no extra time",
        ],
        examples,
        tests: vec![
            Test::new("a fresh lease has no holder", a_fresh_lease),
            Test::new(
                "a grant fixes the expiry and the safe point",
                a_grant_fixes_both_ends,
            ),
            Test::new(
                "the holder serves reads only inside the safe window",
                the_safe_window,
            ),
            Test::new(
                "a node that never held the lease is never served",
                a_stranger_is_never_served,
            ),
            Test::new(
                "renewing extends from now, not from the old expiry",
                a_renewal_extends_from_now,
            ),
            Test::new(
                "renewing a lease you do not hold changes nothing",
                a_renewal_by_a_stranger,
            ),
            Test::new(
                "the lease may not be handed on until the clock error has passed",
                the_two_leader_window,
            ),
            Test::new("a fast clock stops serving early", a_fast_clock_is_cautious),
            Test::new(
                "a slow clock serves past the safe point and the grant rule covers it",
                a_slow_clock_is_covered,
            )
            .ext(),
            Test::new(
                "no permitted clock offset lets two holders serve at once",
                the_windows_never_overlap,
            )
            .ext(),
        ],
    }
}

fn examples() -> Vec<ExampleSpec> {
    vec![
        prim_example("A lease and the margin at its end", "leases", || {
            lines(&[
                "init 1000 100",
                "grant n1 0",
                "read n1 899",
                "read n1 900",
                "read n1 1000",
            ])
        })
        .request("a one-second lease with a hundred milliseconds of clock error")
        .response("served until 899, then refused inside the margin, then expired")
        .note(
            "The lease runs to 1000 but the leader stops serving at 900. The hundred \
             milliseconds it gives up are not waste: they are the amount by which its own \
             clock might be behind, and serving into them is how two leaders end up \
             answering reads in the same instant.",
        ),
        prim_example("The window at the other end", "leases", || {
            lines(&[
                "init 1000 100",
                "grant n1 0",
                "grant n2 1000",
                "grant n2 1099",
                "grant n2 1100",
            ])
        })
        .request("a second node asking for the lease at and after the nominal expiry")
        .response("refused at 1000 and 1099, granted at 1100")
        .note(
            "Granting at 1000 looks obviously safe and is not: the old holder's clock may be \
             running a hundred milliseconds slow, so it can still believe the lease is live. \
             The error budget is spent twice — once shortening the holder's window and once \
             lengthening the granter's wait.",
        ),
    ]
}

// ---------------------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------------------

dist_test!(a_fresh_lease, |ctx| {
    let p = ctx.prim("leases").await?;
    let init = p.send("init 1000 100").await?;
    let s = p.send("state").await?;
    let h = p.send("holder 0").await?;
    let mut c = Check::new("a lease nobody has been granted");
    c.eq(
        "init.lease_ms",
        1000,
        p.expect_i64(&init, "init", "lease_ms")?,
    );
    c.eq(
        "init.clock_error_ms",
        100,
        p.expect_i64(&init, "init", "clock_error_ms")?,
    );
    c.eq("state.holder", &serde_json::Value::Null, &s["holder"]);
    c.eq("holder(0).holder", &serde_json::Value::Null, &h["holder"]);
    c.block("transcript", p.transcript_block());
    c.finish()
});

dist_test!(a_grant_fixes_both_ends, |ctx| {
    let p = ctx.prim("leases").await?;
    p.send("init 1000 100").await?;
    let g = p.send("grant n1 250").await?;
    let s = p.send("state").await?;
    let mut c = Check::new("a lease granted a quarter of a second in");
    c.eq("grant.ok", true, p.expect_bool(&g, "grant n1 250", "ok")?);
    c.eq(
        "grant.holder",
        "n1".to_string(),
        p.expect_str(&g, "grant n1 250", "holder")?,
    );
    c.eq(
        "grant.expires_at",
        1250,
        p.expect_i64(&g, "grant n1 250", "expires_at")?,
    );
    // The safe point is always one clock-error before the expiry, never the expiry itself.
    c.eq(
        "grant.safe_until",
        1150,
        p.expect_i64(&g, "grant n1 250", "safe_until")?,
    );
    c.eq(
        "state.granted_at",
        250,
        p.expect_i64(&s, "state", "granted_at")?,
    );
    c.block("transcript", p.transcript_block());
    c.finish()
});

dist_test!(the_safe_window, |ctx| {
    let p = ctx.prim("leases").await?;
    p.send("init 1000 100").await?;
    p.send("grant n1 0").await?;
    let mut seen = Vec::new();
    for now in [0, 899, 900, 999, 1000, 5000] {
        let r = p.send(&format!("read n1 {now}")).await?;
        seen.push((
            p.expect_bool(&r, "read", "served")?,
            p.expect_str(&r, "read", "reason")?,
        ));
    }
    let mut c = Check::new("reads across the whole life of one lease");
    let expected = vec![
        (true, "holds the lease".to_string()),
        (true, "holds the lease".to_string()),
        // 900 is the first instant inside the margin: the comparison is `now < safe_until`.
        (false, "inside the clock-error margin".to_string()),
        (false, "inside the clock-error margin".to_string()),
        (false, "the lease has expired".to_string()),
        (false, "the lease has expired".to_string()),
    ];
    c.eq(
        "the answers at 0, 899, 900, 999, 1000 and 5000",
        expected,
        seen,
    );
    c.block("transcript", p.transcript_block());
    c.finish()
});

dist_test!(a_stranger_is_never_served, |ctx| {
    let p = ctx.prim("leases").await?;
    p.send("init 1000 100").await?;
    p.send("grant n1 0").await?;
    let mut seen = Vec::new();
    for now in [0, 500, 899, 2000] {
        let r = p.send(&format!("read n2 {now}")).await?;
        seen.push(p.expect_str(&r, "read", "reason")?);
    }
    let mut c = Check::new("reads from a node that was never granted the lease");
    c.eq(
        "the reasons at 0, 500, 899 and 2000",
        vec!["not the holder".to_string(); 4],
        seen,
    );
    c.block("transcript", p.transcript_block());
    c.finish()
});

dist_test!(a_renewal_extends_from_now, |ctx| {
    let p = ctx.prim("leases").await?;
    p.send("init 1000 100").await?;
    p.send("grant n1 0").await?;
    let r = p.send("renew n1 300").await?;
    let late = p.send("renew n1 900").await?;
    let served = p.send("read n1 1700").await?;
    let mut c = Check::new("two renewals of a live lease");
    // Extending from the old expiry would give a slow renewal the time it spent in flight
    // for free, which is exactly the time the holder cannot account for.
    c.eq(
        "renew(300).expires_at",
        1300,
        p.expect_i64(&r, "renew n1 300", "expires_at")?,
    );
    c.eq(
        "renew(300).safe_until",
        1200,
        p.expect_i64(&r, "renew n1 300", "safe_until")?,
    );
    c.eq(
        "renew(900).expires_at",
        1900,
        p.expect_i64(&late, "renew n1 900", "expires_at")?,
    );
    c.eq(
        "read(n1, 1700).served",
        true,
        p.expect_bool(&served, "read n1 1700", "served")?,
    );
    c.block("transcript", p.transcript_block());
    c.finish()
});

dist_test!(a_renewal_by_a_stranger, |ctx| {
    let p = ctx.prim("leases").await?;
    p.send("init 1000 100").await?;
    p.send("grant n1 0").await?;
    let r = p.send("renew n2 400").await?;
    let s = p.send("state").await?;
    let mut c = Check::new("a node renewing a lease it does not hold");
    c.eq(
        "renew(n2).ok",
        false,
        p.expect_bool(&r, "renew n2 400", "ok")?,
    );
    c.eq(
        "state.expires_at",
        1000,
        p.expect_i64(&s, "state", "expires_at")?,
    );
    c.eq(
        "state.holder",
        "n1".to_string(),
        p.expect_str(&s, "state", "holder")?,
    );
    c.block("transcript", p.transcript_block());
    c.finish()
});

dist_test!(the_two_leader_window, |ctx| {
    let p = ctx.prim("leases").await?;
    p.send("init 1000 100").await?;
    p.send("grant n1 0").await?;
    let mut seen = Vec::new();
    for now in [500, 999, 1000, 1099, 1100] {
        let g = p.send(&format!("grant n2 {now}")).await?;
        seen.push(p.expect_bool(&g, "grant", "ok")?);
    }
    let s = p.send("state").await?;
    let mut c = Check::new("a second node asking for the lease as the first one runs out");
    // Granting at the nominal expiry is the mistake this stage is about: it leaves no room
    // for the old holder's clock to be behind.
    c.eq(
        "grant(n2) at 500, 999, 1000, 1099 and 1100",
        vec![false, false, false, false, true],
        seen,
    );
    c.eq(
        "state.holder",
        "n2".to_string(),
        p.expect_str(&s, "state", "holder")?,
    );
    c.eq(
        "state.expires_at",
        2100,
        p.expect_i64(&s, "state", "expires_at")?,
    );
    c.block("transcript", p.transcript_block());
    c.finish()
});

dist_test!(a_fast_clock_is_cautious, |ctx| {
    let p = ctx.prim("leases").await?;
    p.send("init 1000 100").await?;
    p.send("grant n1 0").await?;
    p.send("skew n1 100").await?;
    let last = p.send("read n1 799").await?;
    let first_refusal = p.send("read n1 800").await?;
    let mut c = Check::new("a holder whose clock runs a hundred milliseconds fast");
    // It gives up the lease early and loses a little availability. That is the safe
    // direction of the error, and it costs nothing but throughput.
    c.eq(
        "read(n1, 799).served",
        true,
        p.expect_bool(&last, "read n1 799", "served")?,
    );
    c.eq(
        "read(n1, 800).served",
        false,
        p.expect_bool(&first_refusal, "read n1 800", "served")?,
    );
    c.eq(
        "read(n1, 800).reason",
        "inside the clock-error margin".to_string(),
        p.expect_str(&first_refusal, "read n1 800", "reason")?,
    );
    c.block("transcript", p.transcript_block());
    c.finish()
});

dist_test!(a_slow_clock_is_covered, |ctx| {
    let p = ctx.prim("leases").await?;
    p.send("init 1000 100").await?;
    p.send("grant n1 0").await?;
    p.send("skew n1 -100").await?;
    // The holder's clock is behind, so it keeps serving a hundred milliseconds past the
    // safe point it computed — right up to the nominal expiry. This is the unsafe
    // direction, and nothing the holder can do detects it.
    let past_safe = p.send("read n1 950").await?;
    let last = p.send("read n1 999").await?;
    let done = p.send("read n1 1000").await?;
    // The granter's own wait is what covers it: n2 cannot take the lease until 1100.
    let too_early = p.send("grant n2 1000").await?;
    let ok = p.send("grant n2 1100").await?;
    let mut c = Check::new("a holder whose clock runs a hundred milliseconds slow");
    c.eq(
        "read(n1, 950).served past the computed safe point",
        true,
        p.expect_bool(&past_safe, "read n1 950", "served")?,
    );
    c.eq(
        "read(n1, 999).served",
        true,
        p.expect_bool(&last, "read n1 999", "served")?,
    );
    c.eq(
        "read(n1, 1000).served",
        false,
        p.expect_bool(&done, "read n1 1000", "served")?,
    );
    c.eq(
        "grant(n2, 1000).ok",
        false,
        p.expect_bool(&too_early, "grant n2 1000", "ok")?,
    );
    c.eq(
        "grant(n2, 1100).ok",
        true,
        p.expect_bool(&ok, "grant n2 1100", "ok")?,
    );
    c.block("transcript", p.transcript_block());
    c.finish()
});

dist_test!(the_windows_never_overlap, |ctx| {
    // For every clock offset the budget permits, find the last instant the old holder will
    // still answer a read and the first instant the granter will hand the lease on. The
    // tester works both out from the arithmetic and then checks the program agrees; the
    // property being proved is that the first is always strictly before the second.
    const LEASE: i64 = 1000;
    const ERROR: i64 = 100;
    const STEP: i64 = 25;
    let p = ctx.prim("leases").await?;
    let mut rows = Vec::new();
    for offset in [-ERROR, -ERROR / 2, 0, ERROR / 2, ERROR] {
        p.send(&format!("init {LEASE} {ERROR}")).await?;
        p.send("grant n1 0").await?;
        p.send(&format!("skew n1 {offset}")).await?;
        let mut last_served = -1i64;
        let mut now = 0;
        while now <= LEASE + 3 * ERROR {
            let r = p.send(&format!("read n1 {now}")).await?;
            if p.expect_bool(&r, "read", "served")? {
                last_served = now;
            }
            now += STEP;
        }
        let mut first_grant = -1i64;
        let mut now = LEASE - ERROR;
        while now <= LEASE + 3 * ERROR {
            let g = p.send(&format!("grant n2 {now}")).await?;
            if p.expect_bool(&g, "grant", "ok")? {
                first_grant = now;
                break;
            }
            now += STEP;
        }
        // What the tester says the answers must be, from the arithmetic alone.
        let want_served = (0..)
            .map(|i| i * STEP)
            .take_while(|t| *t <= LEASE + 3 * ERROR)
            .filter(|t| t + offset < LEASE - ERROR)
            .last()
            .unwrap_or(-1);
        let want_grant = (0..)
            .map(|i| LEASE - ERROR + i * STEP)
            .take_while(|t| *t <= LEASE + 3 * ERROR)
            .find(|t| *t >= LEASE + ERROR)
            .unwrap_or(-1);
        rows.push((offset, last_served, first_grant, want_served, want_grant));
    }
    let transcript = p.transcript_block();
    let mut c = Check::new("every clock offset the error budget permits");
    for (offset, last_served, first_grant, want_served, want_grant) in rows {
        c.eq(
            &format!("offset {offset}: the last instant n1 is served"),
            want_served,
            last_served,
        );
        c.eq(
            &format!("offset {offset}: the first instant n2 may be granted"),
            want_grant,
            first_grant,
        );
        c.that(
            &format!("offset {offset}: the two serving windows"),
            "no overlap at all — the old holder stops before the new one starts",
            last_served < first_grant,
            (last_served, first_grant),
        );
    }
    c.block("transcript", transcript);
    c.finish()
});
