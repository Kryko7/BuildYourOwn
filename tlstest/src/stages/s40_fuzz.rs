//! Stage 40 — Fuzz: four hundred seeded mutations, and the server is still there.

use crate::assert::Check;
use crate::examples::{ExampleEnv, ExampleSpec, Expect};
use crate::stages::{hello_message, mutate, provoke_within, Mutation, Stage, Test};
use crate::tls::conn::Reaction;
use crate::tls::record::Record;
use crate::tls::LEGACY_VERSION_TLS12;
use crate::tls_test;
use rand::Rng;

/// How many mutated ClientHellos the main fuzz test sends.
const ROUNDS: usize = 400;
/// How long one fuzz round waits for the server to react.
///
/// Short on purpose: a round that says nothing is as informative as one that alerts, and
/// four hundred rounds at the normal window would make the stage take minutes.
const FUZZ_WINDOW_MS: u64 = 250;

/// Stage definition.
pub fn stage() -> Stage {
    Stage {
        number: 40,
        slug: "fuzz",
        name: "Fuzz: four hundred mutated hellos",
        ext: true,
        hints: &[
            "Validate every length against the bytes you actually have before you index — \
             that one rule survives most of this stage",
            "A malformed input is at worst a closed connection: never a panic, never an \
             unbounded allocation, never a loop that does not end",
            "The accept loop has to outlive every bad connection; test that by sending a \
             clean handshake afterwards, which is what this stage does",
            "Everything here comes from --seed, so a failure is reproducible: the report names \
             the round and the mutation that broke it",
        ],
        examples,
        tests: vec![
            Test::new(
                "four hundred mutated ClientHellos leave the server standing",
                mutated_hellos,
            )
            .min_timeout_ms(120_000)
            .tag("slow"),
            Test::new(
                "every mutation kind is survivable on its own",
                each_mutation,
            )
            .min_timeout_ms(60_000),
            Test::new(
                "a hundred records of pure noise are survivable",
                noise_records,
            )
            .min_timeout_ms(60_000)
            .tag("slow"),
            Test::new(
                "mutations after a completed handshake are survivable",
                post_handshake_noise,
            )
            .min_timeout_ms(60_000),
            Test::new("the same seed produces the same corpus", reproducible),
            Test::new(
                "a clean handshake still works after all of it",
                still_serving,
            )
            .min_timeout_ms(30_000),
        ],
    }
}

tls_test!(mutated_hellos, |ctx| {
    let original = Record::build(
        22,
        LEGACY_VERSION_TLS12,
        &hello_message(&ctx.config()).map_err(crate::stages::harness)?,
    );
    let mutations = Mutation::all();
    let mut survived = 0usize;
    let mut crashed: Vec<String> = Vec::new();
    let started = std::time::Instant::now();
    for round in 0..ROUNDS {
        let mutation = mutations[round % mutations.len()];
        let bytes = mutate(&original, mutation, &mut ctx.rng);
        let mut conn = match ctx.connect().await {
            Ok(conn) => conn,
            Err(f) => {
                return Err(f.note(format!(
                    "the server stopped accepting connections at round {round} of {ROUNDS}, \
                     after {}",
                    mutation.name()
                )))
            }
        };
        let reaction = provoke_within(&mut conn, &bytes, FUZZ_WINDOW_MS).await;
        match reaction {
            Reaction::Error(e) if crashed.len() < 5 => {
                crashed.push(format!("round {round} ({}): {e}", mutation.name()))
            }
            _ => survived += 1,
        }
        drop(conn);
    }
    ctx.note(format!(
        "{survived}/{ROUNDS} rounds in {:.2} s, seed {:#x}",
        started.elapsed().as_secs_f64(),
        ctx.seed
    ));
    let mut c = Check::new(format!("{ROUNDS} mutated ClientHellos"));
    c.note(
        "Every mutation is derived from --seed, so this exact corpus comes back on the next \
         run with the same seed.",
    );
    for line in &crashed {
        c.that(
            "a fuzz round",
            "a reaction, not an error",
            false,
            line.clone(),
        );
    }
    c.eq("rounds the server survived", ROUNDS, survived);
    c.finish()?;
    ctx.expect_still_serving(&format!("{ROUNDS} mutated ClientHellos"))
        .await
});

tls_test!(each_mutation, |ctx| {
    let original = Record::build(
        22,
        LEGACY_VERSION_TLS12,
        &hello_message(&ctx.config()).map_err(crate::stages::harness)?,
    );
    let mut c = Check::new("each mutation kind on its own");
    for mutation in Mutation::all() {
        let mut alive = true;
        for _ in 0..8 {
            let bytes = mutate(&original, mutation, &mut ctx.rng);
            let mut conn = ctx.connect().await.map_err(|f| {
                f.note(format!(
                    "the server stopped accepting after {}",
                    mutation.name()
                ))
            })?;
            let reaction = provoke_within(&mut conn, &bytes, FUZZ_WINDOW_MS).await;
            if matches!(reaction, Reaction::Error(_)) {
                alive = false;
                c.that(
                    mutation.name(),
                    "a reaction, not an error",
                    false,
                    reaction.describe(),
                );
                break;
            }
            drop(conn);
        }
        if alive {
            c.observe(mutation.name(), "survived eight rounds");
        }
    }
    c.finish()?;
    ctx.expect_still_serving("every mutation kind").await
});

tls_test!(noise_records, |ctx| {
    let mut survived = 0usize;
    for round in 0..100 {
        let len = ctx.rng.random_range(1..=512usize);
        let bytes: Vec<u8> = (0..len).map(|_| ctx.rng.random::<u8>()).collect();
        let mut conn = ctx.connect().await.map_err(|f| {
            f.note(format!(
                "the server stopped accepting at noise round {round}"
            ))
        })?;
        let reaction = provoke_within(&mut conn, &bytes, FUZZ_WINDOW_MS).await;
        if !matches!(reaction, Reaction::Error(_)) {
            survived += 1;
        }
        drop(conn);
    }
    let mut c = Check::new("a hundred records of pure noise");
    c.note(
        "No framing at all: the first byte is rarely even a content type. A server has to \
         decide that from byte one and close.",
    );
    c.eq("rounds the server survived", 100usize, survived);
    c.finish()?;
    ctx.expect_still_serving("a hundred noise records").await
});

tls_test!(post_handshake_noise, |ctx| {
    let mut survived = 0usize;
    for round in 0..20 {
        let mut client = ctx.handshake_with(ctx.config_n(round)).await?;
        client
            .echo_line("before")
            .await
            .map_err(|e| crate::stages::handshake_failure(e, &client))?;
        let len = ctx.rng.random_range(1..=128usize);
        let bytes: Vec<u8> = (0..len).map(|_| ctx.rng.random::<u8>()).collect();
        // Wrap the noise in an honest record header so the record layer accepts the frame
        // and the AEAD is the thing that rejects it.
        let record = Record::build(23, LEGACY_VERSION_TLS12, &bytes);
        client
            .conn
            .write_raw(&record)
            .await
            .map_err(crate::assert::Failure::tls)?;
        let reaction = crate::stages::reaction_past_tickets(&mut client.conn).await;
        if !matches!(reaction, Reaction::Error(_)) {
            survived += 1;
        }
        drop(client);
    }
    let mut c = Check::new("twenty noisy records on established connections");
    c.note(
        "After the handshake the record layer is doing the rejecting, so every one of these \
         is a bad_record_mac — and a fatal one, which means a fresh connection each round.",
    );
    c.eq("rounds the server survived", 20usize, survived);
    c.finish()?;
    ctx.expect_still_serving("noise on established connections")
        .await
});

tls_test!(reproducible, |ctx| {
    use rand::SeedableRng;
    let original = Record::build(
        22,
        LEGACY_VERSION_TLS12,
        &hello_message(&ctx.config()).map_err(crate::stages::harness)?,
    );
    let corpus = |seed: u64| {
        let mut rng = rand::rngs::StdRng::seed_from_u64(seed);
        Mutation::all()
            .iter()
            .map(|m| mutate(&original, *m, &mut rng))
            .collect::<Vec<_>>()
    };
    let mut c = Check::new("that --seed really reproduces a corpus");
    c.note(
        "A fuzz failure that cannot be replayed is a rumour. Every mutation here comes from \
         the run's seed, and nothing else.",
    );
    c.that(
        "the corpus for a seed",
        "identical on a second pass",
        corpus(ctx.seed) == corpus(ctx.seed),
        "two passes over the same seed differed",
    );
    c.that(
        "the corpus for a different seed",
        "different",
        corpus(ctx.seed) != corpus(ctx.seed.wrapping_add(1)),
        "two different seeds produced the same corpus",
    );
    c.finish()
});

tls_test!(still_serving, |ctx| {
    let mut client = ctx.handshake().await?;
    let answer = client
        .echo_line("unbroken")
        .await
        .map_err(|e| crate::stages::handshake_failure(e, &client))?;
    let mut c = Check::new("a clean handshake after the fuzzing");
    c.eq("echo", "nekorbnu".to_string(), answer);
    c.finish()
});

fn truncated_hello(env: &ExampleEnv) -> Result<Vec<u8>, String> {
    let record = Record::build(22, LEGACY_VERSION_TLS12, &hello_message(&env.config())?);
    Ok(record[..record.len() / 3].to_vec())
}

fn examples() -> Vec<ExampleSpec> {
    vec![
        ExampleSpec::raw(
            "One of the four hundred: a hello cut to a third",
            truncated_hello,
            Expect::UntilClose,
        )
        .malformed()
        .request(
            "The first third of a perfectly good ClientHello record. The header promises far \
             more than follows.",
        )
        .response(
            "Nothing, or an alert, and then a close. The next connection is accepted as \
             though nothing had happened.",
        )
        .note(
            "The other seven mutations are a flipped bit, a length replaced with 0xffff, a \
             length replaced with zero, a run of noise, a duplicated slice, an undefined \
             content type and an undefined handshake type.",
        ),
        ExampleSpec::text("What a fuzz failure looks like")
            .request("`tlstest --server my_server --stage 40 --seed 0x1234`")
            .response(
                "The report names the round and the mutation: 'round 137 (a length field \
                 replaced with 0xffff)'. The same seed replays the same corpus exactly.",
            )
            .note(
                "Reproducibility is the whole value. Re-run with `--seed` and the same bytes \
                 come back, so the crash can be debugged rather than described.",
            ),
    ]
}
