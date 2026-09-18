//! Stage 44 — Fuzz: 500 seeded mutated frames. **[ext]**
//!
//! Five hundred frames derived from valid ApiVersions, DescribeTopicPartitions, Fetch and
//! Produce requests, each mutated by one family of corruptions and sent on its own
//! connection. Only three things are ever asserted, because only three things are actually
//! promised:
//!
//! * the broker process must survive (the runner turns an exit into a `broker crash`);
//! * it must still answer a clean `ApiVersions` on a **new** connection within 2 s, checked
//!   after every 50 mutations and at the end of every test — that is what "never hangs" means;
//! * whatever it does send must be framed correctly: a size prefix followed by exactly that
//!   many bytes. A response that promises more bytes than it delivers is the one failure mode
//!   a fuzzer can catch that a hand-written test cannot.
//!
//! Everything else is allowed: an error code, a close, or silence. Silence is normal — a
//! size prefix larger than the bytes sent leaves any correct broker waiting for the rest.
//!
//! Every failure names the seed and the mutation index, so `--seed <n> --stage 44 --only
//! "<family>"` replays exactly the frame that broke.

use crate::assert::{Check, Failure, FailureKind};
use crate::examples::{ExampleSpec, Wire, EXAMPLE_CLIENT_ID};
use crate::kafka_test;
use crate::proto::{encode_request, Conn, MAX_FRAME};
use crate::stages::{
    api_versions_body_v4, describe_request, expect_still_serving, fetch_request, produce_request,
    raw_flexible_frame, Ctx, Stage, Test, API_VERSIONS_KEY, API_VERSIONS_V4, DESCRIBE_V0,
    FETCH_V16, PRODUCE_V11,
};
use rand::Rng;
use std::time::Duration;
use uuid::Uuid;

/// How long a probe waits for the first byte, and for the connection to settle.
const QUIET: Duration = Duration::from_millis(120);
/// How long a probe waits for a TCP connection.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(2);
/// The liveness budget the plan fixes: a new connection must be answered inside this.
const LIVENESS: Duration = Duration::from_secs(2);
/// Liveness is re-checked this often inside a test.
const LIVENESS_EVERY: usize = 50;
/// Correlation id of the liveness probe, distinctive in a hex dump.
const LIVE_ID: i32 = 0x00a1_17e5;

/// One mutation family: takes the seeded RNG and a valid payload, returns the exact bytes
/// to put on the wire (length prefix included) and a sentence describing what it did.
type Mutation = fn(&mut rand::rngs::StdRng, &[u8]) -> (Vec<u8>, String);

/// Mutations per family; they add up to the 500 frames the plan asks for.
const PER_FAMILY: usize = 84;
/// The mixed family makes the total exactly 500.
const MIXED: usize = 500 - 5 * PER_FAMILY;

/// Stage definition.
pub fn stage() -> Stage {
    Stage {
        number: 44,
        slug: "fuzz",
        name: "Fuzz: 500 seeded mutated frames",
        ext: true,
        hints: &[
            "Bit flips, truncations, huge lengths and negative array sizes, all from --seed: \
             validate every length against the bytes you actually have before you index",
            "The broker must never crash, never hang and never leak memory — a bad frame is \
             at worst a closed connection, and the accept loop keeps running",
            "After the whole run it must still answer a clean ApiVersions request on a new \
             connection within two seconds",
            "Whatever you do send must be framed: a size prefix and then exactly that many \
             bytes, never a prefix that promises more than you write",
        ],
        examples: wire_examples,
        tests: vec![
            fuzz_test("bit flips never crash or unframe the broker", bit_flips),
            fuzz_test(
                "truncated and padded frames never crash or unframe the broker",
                truncation,
            ),
            fuzz_test(
                "a lying size prefix never crashes or unframes the broker",
                size_field,
            ),
            fuzz_test(
                "negative and huge array lengths never crash or unframe the broker",
                array_lengths,
            ),
            fuzz_test(
                "impossible header fields never crash or unframe the broker",
                header_fields,
            ),
            fuzz_test(
                "mutations combined at random never crash or unframe the broker",
                mixed,
            ),
        ],
    }
}

fn fuzz_test(name: &'static str, run: crate::stages::TestFn) -> Test {
    Test::new(name, run)
        .ext()
        .tag("slow")
        .min_timeout_ms(60_000)
}

// ---------------------------------------------------------------------------------------
// The valid frames every mutation starts from
// ---------------------------------------------------------------------------------------

/// A request payload (header + body, no length prefix) and the name it is known by.
struct Seed {
    what: &'static str,
    payload: Vec<u8>,
}

/// The four request payloads the fuzzer mutates, all of them valid as they stand.
fn seeds(ctx: &mut Ctx) -> Result<Vec<Seed>, Failure> {
    let enc = |what: &'static str, r: Result<Vec<u8>, crate::proto::ProtoError>| {
        r.map(|payload| Seed { what, payload })
            .map_err(|e| Failure::harness(format!("cannot build the {what} fuzz seed: {e}")))
    };
    let topic = ctx.unique("fuzz");
    let batch = crate::proto::records::RecordBatch::of(
        0,
        1_700_000_000_000,
        vec![crate::proto::records::RecordItem::value("fuzz")],
    )
    .encode();
    Ok(vec![
        Seed {
            what: "ApiVersions v4",
            payload: raw_flexible_frame(
                API_VERSIONS_KEY,
                API_VERSIONS_V4,
                1,
                Some("kafkatest"),
                &[],
                &api_versions_body_v4(),
            ),
        },
        enc(
            "DescribeTopicPartitions v0",
            encode_request(
                DESCRIBE_V0,
                2,
                Some("kafkatest"),
                &describe_request(&[&topic], 100),
            ),
        )?,
        enc(
            "Fetch v16",
            encode_request(
                FETCH_V16,
                3,
                Some("kafkatest"),
                &fetch_request(&[(Uuid::from_u128(0x4242), 0, 0)], 100),
            ),
        )?,
        enc(
            "Produce v11",
            encode_request(
                PRODUCE_V11,
                4,
                Some("kafkatest"),
                &produce_request(&[(topic, 0, batch)], 1),
            ),
        )?,
    ])
}

/// Prefix a payload with its own length, the way a well-behaved client frames a request.
fn framed(payload: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(payload.len() + 4);
    out.extend_from_slice(&(payload.len() as i32).to_be_bytes());
    out.extend_from_slice(payload);
    out
}

// ---------------------------------------------------------------------------------------
// Framing of whatever came back
// ---------------------------------------------------------------------------------------

/// How the bytes the broker sent parse as a sequence of length-prefixed frames.
enum Framing {
    /// A whole number of frames and nothing left over.
    Complete(usize),
    /// A frame has started but not finished.
    Partial(String),
    /// A size prefix that can never be right.
    Bad(String),
}

fn framing(buf: &[u8]) -> Framing {
    let mut at = 0usize;
    let mut frames = 0usize;
    while at < buf.len() {
        let Some(head) = buf.get(at..at + 4) else {
            return Framing::Partial(format!(
                "{} trailing byte(s) after {frames} frame(s): not even a 4-byte size prefix",
                buf.len() - at
            ));
        };
        let size = i32::from_be_bytes([head[0], head[1], head[2], head[3]]);
        if size < 0 {
            return Framing::Bad(format!("frame {frames} declares a negative size ({size})"));
        }
        if size == 0 {
            return Framing::Bad(format!("frame {frames} declares a size of 0 bytes"));
        }
        if size as usize > MAX_FRAME {
            return Framing::Bad(format!(
                "frame {frames} declares {size} bytes, past the {MAX_FRAME} byte limit"
            ));
        }
        let end = at + 4 + size as usize;
        if end > buf.len() {
            return Framing::Partial(format!(
                "frame {frames} declares {size} bytes but only {} followed",
                buf.len() - at - 4
            ));
        }
        at = end;
        frames += 1;
    }
    Framing::Complete(frames)
}

/// Everything a probe can legitimately end in.
enum Outcome {
    /// The broker answered with `n` well-formed frames.
    Answered(usize),
    /// The broker closed the connection.
    Closed,
    /// The broker said nothing — correct when the frame promised bytes we never sent.
    Silent,
}

/// Send one mutated frame on a fresh connection and classify what came back.
async fn probe(ctx: &Ctx, wire: &[u8]) -> Result<Outcome, String> {
    let mut conn = match Conn::connect(ctx.addr, CONNECT_TIMEOUT).await {
        Ok(c) => c,
        Err(e) => return Err(format!("the broker would not accept a connection: {e}")),
    };
    // A peer that has already reset the socket is a close, not a send error.
    if conn.send_bytes(wire).await.is_err() {
        return Ok(Outcome::Closed);
    }
    let mut buf = Vec::new();
    let closed = loop {
        match conn.read_silence(QUIET).await {
            Ok(chunk) if chunk.is_empty() => break false,
            Ok(chunk) => {
                buf.extend_from_slice(&chunk);
                match framing(&buf) {
                    Framing::Complete(n) if n > 0 => break false,
                    Framing::Bad(why) => return Err(why),
                    _ if buf.len() > MAX_FRAME => break false,
                    _ => {}
                }
            }
            Err(_) => break true,
        }
    };
    match (framing(&buf), closed) {
        (Framing::Complete(0), true) => Ok(Outcome::Closed),
        (Framing::Complete(0), false) => Ok(Outcome::Silent),
        (Framing::Complete(n), _) => Ok(Outcome::Answered(n)),
        (Framing::Bad(why), _) => Err(why),
        (Framing::Partial(why), true) => Err(format!("{why}, then the connection was closed")),
        (Framing::Partial(why), false) => Err(format!("{why} before the broker went quiet")),
    }
}

/// A fresh connection must be answered inside `LIVENESS`, or the broker is wedged.
async fn expect_alive(ctx: &Ctx, after: &str) -> Result<(), Failure> {
    let mut conn = Conn::connect(ctx.addr, LIVENESS).await.map_err(|e| {
        Failure::proto(e, None).note(format!(
            "the broker stopped accepting connections after {after}"
        ))
    })?;
    conn.timeout = LIVENESS;
    let payload = raw_flexible_frame(
        API_VERSIONS_KEY,
        API_VERSIONS_V4,
        LIVE_ID,
        Some("kafkatest"),
        &[],
        &api_versions_body_v4(),
    );
    conn.send_frame(&payload)
        .await
        .map_err(|e| Failure::proto(e, Some(&conn)))?;
    let resp = conn.read_frame().await.map_err(|e| {
        Failure::proto(e, Some(&conn)).note(format!(
            "a clean ApiVersions on a new connection was not answered within {} s after {after}",
            LIVENESS.as_secs()
        ))
    })?;
    let mut c = Check::new(
        format!("that a new connection is still answered after {after}"),
        &conn,
    );
    c.mark(0..4);
    c.eq(
        "response.correlation_id",
        LIVE_ID,
        crate::stages::correlation_id_of(&resp, &conn)?,
    );
    c.finish()
}

// ---------------------------------------------------------------------------------------
// The driver
// ---------------------------------------------------------------------------------------

/// Run `count` mutations of one family, checking liveness along the way.
///
/// `mutate` gets the seed payload and the RNG and returns the exact bytes to put on the
/// wire, length prefix included — that is what lets the size-field family lie.
async fn sweep(
    ctx: &mut Ctx,
    family: &'static str,
    count: usize,
    mutate: Mutation,
) -> Result<(), Failure> {
    let seeds = seeds(ctx)?;
    let mut answered = 0usize;
    let mut frames = 0usize;
    let mut closed = 0usize;
    let mut silent = 0usize;
    for i in 0..count {
        let seed = &seeds[i % seeds.len()];
        let (wire, how) = mutate(&mut ctx.rng, &seed.payload);
        match probe(ctx, &wire).await {
            Ok(Outcome::Answered(n)) => {
                answered += 1;
                frames += n;
            }
            Ok(Outcome::Closed) => closed += 1,
            Ok(Outcome::Silent) => silent += 1,
            Err(why) => {
                let mut f = Failure::new(
                    FailureKind::Protocol,
                    format!("mutation {i} of family '{family}': {why}"),
                );
                f.request = Some(wire.clone());
                return Err(f
                    .note(format!("the frame was {}, {how}", seed.what))
                    .note(format!(
                        "replay it with --seed {:#x} --stage 44 --only \"{}\"",
                        ctx.seed, family
                    ))
                    .note("the bytes below are the whole wire frame, length prefix included"));
            }
        }
        if (i + 1) % LIVENESS_EVERY == 0 {
            expect_alive(ctx, &format!("{} '{family}' mutations", i + 1)).await?;
        }
    }
    expect_alive(ctx, &format!("all {count} '{family}' mutations")).await?;
    ctx.note(format!(
        "{count} '{family}' frames: {answered} answered (with {frames} well-formed response \
         frames), {closed} closed the connection, {silent} left it open and silent \
         (seed {:#x})",
        ctx.seed
    ));
    Ok(())
}

// ---------------------------------------------------------------------------------------
// Mutation families
// ---------------------------------------------------------------------------------------

fn flip_bits(rng: &mut rand::rngs::StdRng, payload: &[u8]) -> (Vec<u8>, String) {
    let mut p = payload.to_vec();
    if p.is_empty() {
        return (framed(&p), "empty".to_string());
    }
    let flips = rng.random_range(1..=3);
    let mut where_ = Vec::new();
    for _ in 0..flips {
        let at = rng.random_range(0..p.len());
        let bit = rng.random_range(0..8);
        p[at] ^= 1 << bit;
        where_.push(format!("byte {at} bit {bit}"));
    }
    (framed(&p), format!("with {} flipped", where_.join(", ")))
}

fn truncate_or_pad(rng: &mut rand::rngs::StdRng, payload: &[u8]) -> (Vec<u8>, String) {
    if payload.is_empty() {
        return (framed(payload), "already empty".to_string());
    }
    if rng.random_bool(0.5) {
        let keep = rng.random_range(0..payload.len());
        (
            framed(&payload[..keep]),
            format!("truncated to {keep} of {} bytes", payload.len()),
        )
    } else {
        let extra = rng.random_range(1..=64);
        let mut p = payload.to_vec();
        for _ in 0..extra {
            p.push(rng.random::<u8>());
        }
        (framed(&p), format!("with {extra} garbage bytes appended"))
    }
}

fn lying_size(rng: &mut rand::rngs::StdRng, payload: &[u8]) -> (Vec<u8>, String) {
    let real = payload.len() as i32;
    let (size, how) = match rng.random_range(0..6) {
        0 => (0i32, "a size prefix of 0".to_string()),
        1 => (-1i32, "a size prefix of -1".to_string()),
        2 => (i32::MIN, "a size prefix of i32::MIN".to_string()),
        3 => (
            real / 2,
            format!("a size prefix of {} for {real} bytes", real / 2),
        ),
        4 => (
            real + 1_000,
            format!("a size prefix of {} for {real} bytes", real + 1_000),
        ),
        _ => (i32::MAX, "a size prefix of i32::MAX".to_string()),
    };
    let mut wire = size.to_be_bytes().to_vec();
    wire.extend_from_slice(payload);
    (wire, how)
}

fn bad_lengths(rng: &mut rand::rngs::StdRng, payload: &[u8]) -> (Vec<u8>, String) {
    let mut p = payload.to_vec();
    // Past the 8-byte fixed header prefix is where arrays, strings and varints live.
    if p.len() < 16 {
        return (framed(&p), "too short to corrupt a length".to_string());
    }
    if rng.random_bool(0.5) {
        let at = rng.random_range(8..p.len() - 4);
        let value = match rng.random_range(0..4) {
            0 => -1i32,
            1 => i32::MIN,
            2 => i32::MAX,
            _ => 100_000_000,
        };
        p[at..at + 4].copy_from_slice(&value.to_be_bytes());
        (
            framed(&p),
            format!("with the i32 at byte {at} overwritten with {value}"),
        )
    } else {
        // Five 0xff bytes never decode as a valid LEB128 varint or compact length.
        let at = rng.random_range(8..p.len() - 5);
        for b in &mut p[at..at + 5] {
            *b = 0xff;
        }
        (
            framed(&p),
            format!("with an invalid compact varint (five 0xff) at byte {at}"),
        )
    }
}

fn impossible_header(rng: &mut rand::rngs::StdRng, payload: &[u8]) -> (Vec<u8>, String) {
    let mut p = payload.to_vec();
    if p.len() < 10 {
        return (framed(&p), "too short to carry a header".to_string());
    }
    let how = match rng.random_range(0..5) {
        0 => {
            p[0..2].copy_from_slice(&32767i16.to_be_bytes());
            "api_key 32767".to_string()
        }
        1 => {
            let key = rng.random_range(200i16..32000);
            p[0..2].copy_from_slice(&key.to_be_bytes());
            format!("api_key {key}")
        }
        2 => {
            p[2..4].copy_from_slice(&(-1i16).to_be_bytes());
            "api_version -1".to_string()
        }
        3 => {
            p[2..4].copy_from_slice(&32767i16.to_be_bytes());
            "api_version 32767".to_string()
        }
        _ => {
            // client_id is a plain string: an int16 length that runs past the frame.
            let len = rng.random_range(1i16..=i16::MAX);
            p[8..10].copy_from_slice(&len.to_be_bytes());
            format!("a client_id length of {len} in a {}-byte frame", p.len())
        }
    };
    (framed(&p), format!("with {how}"))
}

fn mixed_mutation(rng: &mut rand::rngs::StdRng, payload: &[u8]) -> (Vec<u8>, String) {
    // Two payload-level mutations, then optionally a lying prefix on top of both.
    let steps: [Mutation; 4] = [flip_bits, truncate_or_pad, bad_lengths, impossible_header];
    let first = steps[rng.random_range(0..steps.len())];
    let second = steps[rng.random_range(0..steps.len())];
    let (once, how1) = first(rng, payload);
    let inner = &once[4..];
    let (twice, how2) = second(rng, inner);
    if rng.random_bool(0.5) {
        let (wire, how3) = lying_size(rng, &twice[4..]);
        (wire, format!("{how1}, then {how2}, then {how3}"))
    } else {
        (twice, format!("{how1}, then {how2}"))
    }
}

// ---------------------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------------------

kafka_test!(bit_flips, |ctx| {
    sweep(ctx, "bit flips", PER_FAMILY, flip_bits).await?;
    expect_still_serving(ctx, "a sweep of bit-flipped frames").await
});

kafka_test!(truncation, |ctx| {
    sweep(ctx, "truncated and padded", PER_FAMILY, truncate_or_pad).await?;
    expect_still_serving(ctx, "a sweep of truncated and padded frames").await
});

kafka_test!(size_field, |ctx| {
    sweep(ctx, "a lying size prefix", PER_FAMILY, lying_size).await?;
    expect_still_serving(ctx, "a sweep of frames with a lying size prefix").await
});

kafka_test!(array_lengths, |ctx| {
    sweep(
        ctx,
        "negative and huge array lengths",
        PER_FAMILY,
        bad_lengths,
    )
    .await?;
    expect_still_serving(ctx, "a sweep of frames with impossible array lengths").await
});

kafka_test!(header_fields, |ctx| {
    sweep(
        ctx,
        "impossible header fields",
        PER_FAMILY,
        impossible_header,
    )
    .await?;
    expect_still_serving(ctx, "a sweep of frames with impossible header fields").await
});

kafka_test!(mixed, |ctx| {
    sweep(ctx, "mutations combined at random", MIXED, mixed_mutation).await?;
    expect_still_serving(ctx, "a sweep of frames with several mutations each").await
});

/// Worked examples: one mutation with a deterministic answer, and the loop around it.
fn wire_examples() -> Vec<ExampleSpec> {
    vec![
        ExampleSpec::wire("One bit flipped in api_version", |_env| {
            let mut payload = raw_flexible_frame(
                API_VERSIONS_KEY,
                API_VERSIONS_V4,
                441,
                Some(EXAMPLE_CLIENT_ID),
                &[],
                &api_versions_body_v4(),
            );
            // api_version is bytes 2..4 of the payload; flipping bit 14 turns 4 into 16388.
            payload[2] ^= 0x40;
            Ok(Wire::Frames(vec![payload]))
        })
        .response_version(0)
        .request(
            "A valid ApiVersions v4 frame with a single bit flipped in the api_version \
             field: 0x0004 becomes 0x4004, api version 16388. api_key 18, the correlation \
             id, the client id and the v4 body are untouched",
        )
        .response(
            "error_code 35 (UNSUPPORTED_VERSION), written in the v0 ApiVersions shape — \
             response header v0, then error_code and an int32-counted api_keys array, no \
             tagged fields — because the broker cannot answer in a version it does not know. \
             The connection stays open and the next valid request on it is answered normally",
        )
        .note(
            "ApiVersions is the one api key where an unknown version has a defined answer, \
             which is why it is the example: a client must be able to discover the broker's \
             range before it can speak anything else, so the refusal is written in the one \
             shape every version can parse. For any other api key an unsupported version is \
             an invalid request and Apache Kafka closes the connection. Range-check every \
             version field before you index a table with it.",
        ),
        ExampleSpec::text("The fuzz loop itself")
            .request(
                "500 frames derived from four valid requests — ApiVersions, \
                 DescribeTopicPartitions, Fetch and Produce — mutated by one family each: bit \
                 flips, truncation and padding, a lying size prefix, negative and huge array \
                 lengths, impossible header fields, and the five combined at random. Every \
                 frame goes on its own connection, and every mutation comes from the run's \
                 --seed, so --seed <n> --stage 44 replays the exact byte string that broke",
            )
            .response(
                "Three things, and only three: the process is still alive, a brand-new \
                 connection is still answered a clean ApiVersions within 2 s (re-checked \
                 every 50 mutations and at the end of every test), and every byte the broker \
                 does send is correctly framed — a size prefix followed by exactly that many \
                 bytes. An error code, a closed connection and complete silence are all \
                 acceptable answers to a mutated frame",
            )
            .note(
                "Silence is normal: a size prefix larger than the bytes that followed leaves \
                 any correct broker waiting for the rest. What is never acceptable is a reply \
                 that promises more bytes than it writes — that desynchronizes the client for \
                 every later request on the connection, and it is the one bug a fuzzer finds \
                 that a hand-written test cannot.",
            ),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand::SeedableRng;

    fn rng() -> rand::rngs::StdRng {
        rand::rngs::StdRng::seed_from_u64(7)
    }

    #[test]
    fn the_families_add_up_to_five_hundred_frames() {
        assert_eq!(5 * PER_FAMILY + MIXED, 500);
        assert_ne!(MIXED, 0, "the mixed family must actually run");
    }

    #[test]
    fn framing_counts_whole_frames_and_spots_a_short_one() {
        let mut buf = Vec::new();
        buf.extend_from_slice(&3i32.to_be_bytes());
        buf.extend_from_slice(&[1, 2, 3]);
        assert!(matches!(framing(&buf), Framing::Complete(1)));
        buf.extend_from_slice(&9i32.to_be_bytes());
        buf.extend_from_slice(&[1, 2]);
        assert!(matches!(framing(&buf), Framing::Partial(_)));
        assert!(matches!(framing(&(-4i32).to_be_bytes()), Framing::Bad(_)));
        assert!(matches!(framing(&0i32.to_be_bytes()), Framing::Bad(_)));
        assert!(matches!(framing(&[]), Framing::Complete(0)));
    }

    #[test]
    fn every_mutation_still_produces_a_sendable_frame() {
        let payload = raw_flexible_frame(
            API_VERSIONS_KEY,
            API_VERSIONS_V4,
            1,
            Some("kafkatest"),
            &[],
            &api_versions_body_v4(),
        );
        let mut r = rng();
        for f in [
            flip_bits,
            truncate_or_pad,
            lying_size,
            bad_lengths,
            impossible_header,
            mixed_mutation,
        ] {
            for _ in 0..50 {
                let (wire, how) = f(&mut r, &payload);
                assert!(wire.len() >= 4, "a frame is at least a size prefix: {how}");
                assert!(!how.is_empty());
            }
        }
    }

    #[test]
    fn a_lying_prefix_keeps_the_payload_but_changes_the_size() {
        let payload = vec![1u8, 2, 3, 4, 5, 6, 7, 8];
        let mut r = rng();
        let mut saw_a_lie = false;
        for _ in 0..40 {
            let (wire, _) = lying_size(&mut r, &payload);
            assert_eq!(&wire[4..], &payload[..]);
            if i32::from_be_bytes([wire[0], wire[1], wire[2], wire[3]]) != payload.len() as i32 {
                saw_a_lie = true;
            }
        }
        assert!(saw_a_lie, "the size-field family must actually lie");
    }
}
