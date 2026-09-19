# tlstest

A self-contained, stage-by-stage conformance tester for **TLS 1.3 servers** (RFC 8446),
written in Rust. It drives **any** server binary as a black box over TCP — real records,
real key schedules, real signatures — and compares what comes back with a suite written in
Rust: 48 stages, 343 tests, all validated against `openssl s_server` from the system
OpenSSL 3.x. Every stage also carries worked examples — 95 of them — showing the exact bytes
the reference sent and received, annotated field by field (see [Examples](#examples)).

> Commands below are run from the **repo root**: this is a cargo workspace, so every
> binary and example lands in the one `target/` directory at the top.
```
cargo build --release -p tlstest
target/release/tlstest --server openssl --validate --all   # suite self-check, all green
target/release/tlstest --server ./your_program.sh --stage 1  # your first red/green
target/release/tlstest --server my_server --until 12
target/release/tlstest --list                                # stages, counts, PLAN.md tickboxes
target/release/tlstest --list --json catalog.json            # stage catalog for the site
```

There is nothing to download: the reference is the `openssl` already on your PATH, and the
certificates are generated per run. Linux and macOS.

## The tester is a TLS 1.3 client

`tlstest` contains a TLS 1.3 client written from primitives for this suite: X25519 and
secp256r1 by hand, HKDF and the key schedule with every label spelled out, transcript
hashes taken at each message boundary, AEAD record protection with the nonce rule written
where you can read it, and CertificateVerify checked against the key in the certificate the
server presented.

It is deliberately **not** `rustls`. A full client stack would be a tenth of the code and
would hide exactly the bytes a learner needs to see. Because the client is ours, a failure
can say which handshake message went wrong, dump it as annotated hex, name the field path
inside it, and print the transcript hash that was in force and the key-schedule label being
derived at that moment.

`tests/rfc8448_vectors.rs` replays the published RFC 8448 "Simple 1-RTT Handshake" trace
through that key schedule — every secret, every `HkdfLabel`, both traffic keys and both
Finished messages — which is the evidence that the suite's own maths is right before it
judges anybody else's.

## What a run looks like

```
Stage 02 TLSPlaintext framing
  ✔ the ClientHello is answered when its record says 0x0303 (20 ms)
  ✔ the ClientHello is answered when its record says 0x0301 (31 ms)
  ✔ the server's first record is a handshake record (31 ms)
  ✘ the server's first record carries legacy_record_version 0x0303 FAIL (31 ms)
      record.legacy_record_version: expected 771, got 769
    ┌ context
    │ while checking legacy_record_version on the server's first record
    │ The 0x0301 exception of RFC 8446 section 5.1 is for the client's first flight only;
    │ a server always writes 0x0303.
    ┌ expected
    │ record.legacy_record_version: 771
    ┌ actual
    │ record.legacy_record_version: 769
    ┌ the server's first record (53 bytes, ^^ marks the bytes holding the actual value)
    │ 0000  16 03 01 00 30 02 00 00  2c 03 04 5b 5b 5b 5b 5b  |....0...,..[[[[[|
    │          ^^ ^^
    │ 0010  5b 5b 5b 5b 5b 5b 5b 5b  5b 5b 5b 5b 5b 5b 5b 5b  |[[[[[[[[[[[[[[[[|
    ┌ server output
    │ server stdout (last 1 lines):
    │   broken_server: listening on 0.0.0.0:46171 (and answering nonsense)
Stage 02  TLSPlaintext framing                               6/7 passed
```

A failure inside the encrypted flight also carries the connection trace, the transcript
hashes and the key schedule:

```
  ✘ the handshake completes FAIL [crypto] (31 ms)
      crypto check failed: the server's Finished does not match: expected verify_data 59f7…,
      got f3c9… (HMAC of the finished key over the transcript hash edb7…)
    ┌ context
    │ connection trace:
    │ --> client_hello(1)                      455 bytes  010001c303036e07fab5a942…
    │ <-- record handshake(22)                 133 bytes  16030300800200007c03030a…
    │ <-- decrypted handshake(22)               28 bytes  [encrypted] 080000180016000a…
    │ transcript hashes:
    │ after client_hello                 f796e207fd067e26fc71305581d1f5ef…
    │ after server_hello                 30372e485e3b860815b4fa06fee9f255…
    │ key schedule:
    │ HKDF-Extract(0, PSK)             → Early Secret       33ad0a1c607ec03b
    │ derived                          → Derived Secret     6f2615a108c702c5 over transcript e3b0c442
    │ s hs traffic                     → server_handshake…  b67b7d690cc16c4e over transcript 860c06ed
```

To see the red path without writing a server:

```
cargo build --release -p tlstest --example broken_server
target/release/tlstest --server target/release/examples/broken_server --until 5
```

That example accepts connections and says nothing until spoken to — so stage 01 is green —
and then answers with a ServerHello that has three deliberate mistakes in it.

## CLI

```
tlstest --server <name|path> [--stage N] [--until N] [--from N] [--all]
        [--only "substring"] [--tag ext] [--skip-ext]
        [--verbose] [--keep-tmp] [--timeout-ms N] [--seed N]
        [--validate] [--list] [--json report.json] [--no-color]
        [--servers-file path]
        [--capture-examples examples/captured.json]
```

| Flag | Meaning |
|---|---|
| `--server` | A name from `servers.yaml` (`openssl`, `my_server`) **or a path**. A path is run as `<path> -accept PORT -cert C -key K -rev` with a fresh process per test. The client-authentication stages add `-verify 2 -CAfile CA` (request a client certificate) or `-Verify 2 -CAfile CA` (require one). |
| `--stage N` / `--until N` / `--from N` / `--all` | Stage selection. |
| `--only SUBSTR` | Only tests whose name contains the substring. |
| `--tag T` | Only tests carrying a tag. `slow` selects the long ones (fuzz, soak-ish, interop). |
| `--skip-ext` | Hide everything beyond the core track: 73 tests, including all of stages 39–48. |
| `--validate` | Run against the reference server and word failures as suite bugs. `tlstest --server openssl --validate --all` must be all green. |
| `--verbose` | Print each server process's command line as it is started. |
| `--keep-tmp` | Keep every server instance's temp dir (certificates, captured output) and print the paths. |
| `--timeout-ms N` | Per-test timeout, default 10 000. Server start-up is not counted, and slow stages raise their own floor. |
| `--seed N` | Seeds every random choice: client randoms, session ids, ephemeral key shares, payloads and the fuzz corpus. The same seed reproduces the same bytes. |
| `--json FILE` | Machine-readable report (schema below). With `--list`, writes the stage catalog instead; `--list --json` with no value prints the catalog on stdout. |
| `--list` | Stages, source files, test counts, example counts and the `PLAN.md` tickbox state. |
| `--no-color` | No ANSI. `NO_COLOR` in the environment does the same. |
| `--servers-file` | Path to `servers.yaml`; found next to the cwd or the binary by default. |
| `--capture-examples FILE` | Replay every stage's worked examples against the server and record the bytes and their annotations (see [Examples](#examples)). Runs no tests. |

Exit code is **0** only when every selected test passed, **1** when something failed, **2**
on a usage or harness error (unknown server, no stage selected, server will not start).

## The contract your server must follow

Your program takes a subset of `openssl s_server`'s own flags, which is what lets the
reference be the real thing rather than a model of it:

```
./your_program.sh -accept <port> -cert <cert.pem> -key <key.pem> -rev [-naccept <n>]
```

| Thing | Convention |
|---|---|
| `-accept <port>` | The TCP port to listen on. **Never hardcode one**: the harness picks a free port per test, because other agents run their own servers on this machine. |
| `-cert <file>` | A PEM file holding the certificate, leaf first. The harness generates it for this run. |
| `-key <file>` | A PEM file holding the matching private key, PKCS#8. |
| `-rev` | The application layer (below). |
| `-naccept <n>` | Exit after serving `n` connections. Optional; the harness passes it for one test only. |
| Version | **TLS 1.3 only.** The reference is run with `-tls1_3`, so a hello that cannot do 1.3 is refused rather than downgraded. |
| Suites | `TLS_AES_128_GCM_SHA256`, `TLS_AES_256_GCM_SHA384`, `TLS_CHACHA20_POLY1305_SHA256`. |
| Groups | `x25519` and `secp256r1` — the two the suite's client does the maths for. |
| Signatures | `ecdsa_secp256r1_sha256`, `rsa_pss_rsae_sha256/384/512`, `ed25519`. |
| Robustness | A malformed record closes *that* connection at worst. The process survives and the accept loop keeps running. |

### What `-rev` does, exactly

`-rev` is `s_server`'s line-reversing echo, and this is its behaviour, verified against
OpenSSL 3.6.4 rather than assumed:

| In | Out |
|---|---|
| `hello\n` | `olleh\n` |
| `abc\r\n` | `cba\n` — both the CR and the LF are stripped, and exactly one LF is put back |
| `\n` | `\n` |
| `abc` with no newline | nothing, until a newline arrives |
| `first\nsecond\n` | `tsrif\ndnoces\n` — two lines, two answers, in order |

It is a byte-stream protocol: a line split across two records is one line, and two lines in
one record are two answers. The reference reads into a 16 KiB buffer and answers a longer
line in chunks, each reversed on its own, so the suite keeps its lines well under that.

## Stages

45 stages in seven sections; `PLAN.md` has the tickboxes and the hints.

| Section | Stages | About |
|---|---|---|
| A | 01–07 | TCP and the record layer: framing, size limits, garbage, fragmentation, half-close |
| B | 08–16 | ClientHello, extensions and negotiation: versions, suites, groups, signatures, GREASE, SNI, ALPN, malformed extensions |
| C | 17–23 | The key schedule and handshake encryption: ServerHello, secrets, transcripts, traffic keys, ChangeCipherSpec, EncryptedExtensions, sequence numbers |
| D | 24–29 | Authentication: Certificate, CertificateVerify, both Finished messages, flight order |
| E | 30–35 | Application data and AEAD: the echo, every suite, padding, sizes, close_notify, `bad_record_mac` |
| F | 36–40 | Alerts, errors and robustness: alert format, bad handshakes, no renegotiation, fifty connections, fuzz |
| G | 41–45 | Advanced: HelloRetryRequest, KeyUpdate, tickets and resumption, early-data rejection, real-client interop |

Sections A and B check liveness by sending a fresh ClientHello and requiring a handshake
record back — not a completed handshake — so the record layer can be finished before the
key schedule is started. From section C onward a test may reasonably expect the handshake
to complete, because the stages that build it come first.

Stages 39–45 are `ext`, along with a few individual tests earlier on: `--skip-ext` leaves a
270-test core path.

## What the failure blocks mean

A failure prints as many of these as apply, in this order:

| Block | What it holds |
|---|---|
| the red line(s) | One per failed check: `field.path: expected X, got Y`. The path is the RFC's own name for the field — `server_hello.legacy_session_id_echo`, `certificate_verify.algorithm`. |
| `context` | What the test was doing, plus, for anything under encryption, the **transcript hash in force** and the **key schedule step in force**. Those two lines answer most TLS questions on their own. |
| `expected` / `actual` | The same values again as a table, which is what the JSON report carries. |
| hex blocks | One per named byte string — `client_hello`, `server_hello`, `certificate`, `the record that would not open` — with `^^` under the bytes holding the actual value. |
| `connection trace` | Every record and message that crossed the connection, in order, with lengths and a hex prefix. `[encrypted]` marks the ones that were protected. |
| `transcript hashes` | The hash after each handshake message, labelled. |
| `key schedule` | Every derivation in order: label → what it produced → the first bytes → the transcript it was taken over. |
| `server output` | The tail of your process's stdout and stderr. |

The failure *kind* is in the header line when it is not a plain assertion:

| Kind | Means |
|---|---|
| (none) | An assertion: a field did not have the expected value. |
| `[timeout]` | The server did not answer in time. |
| `[connection]` | The connection could not be opened, or closed unexpectedly. |
| `[protocol]` | The bytes could not be framed as records or parsed as handshake messages. |
| `[crypto]` | A signature, a MAC or an AEAD tag did not check out. |
| `[alert]` | The server sent an alert the test was not expecting. |
| `[server crash]` | Your process exited while the test was running. |
| `[harness error]` | The suite could not set the test up — that one is a bug here, not there. |

## Certificates

Nothing is committed: a certificate in a repository expires, and a learner has no way to see
where its bytes came from. Everything is minted per run with `rcgen` into the run's
temporary directory and handed over as `-cert` and `-key`.

| Kind | What it is for |
|---|---|
| `Leaf(EcdsaP256)` | The default. CertificateVerify is `ecdsa_secp256r1_sha256`. |
| `Leaf(Rsa2048)` | CertificateVerify is `rsa_pss_rsae_sha256` — PSS, never PKCS#1 v1.5. |
| `Leaf(Ed25519)` | CertificateVerify is `ed25519`, which signs the content itself, unhashed. |
| `Chain` | leaf → intermediate → root, so `certificate_list` has three entries in the right order. |
| `Expired` | A self-signed leaf whose `notAfter` is in the past. |

A test asks for one with `Test::with_server(...)`, and the harness restarts the server with
that material. Generation is lazy: a run that never touches RSA never pays for an RSA key.

Key generation is the one thing `--seed` cannot make deterministic — `rcgen`'s backend takes
its randomness from the OS, and a seeded RSA key would be a footgun waiting to be copied.
Everything else in the suite is seeded, including the client's ephemeral key shares.

`s_server -cert` reads only the first certificate out of a PEM, so for the `Chain` material
the reference is additionally handed `-cert_chain` with the issuers. A hand-written server
is expected to send everything its `-cert` file holds, which is why the chain stage requires
a leaf-first list of valid certificates rather than a fixed length.

## The reference server

`--server openssl` is the system OpenSSL, run as:

```
openssl s_server -accept <port> -cert <cert.pem> -key <key.pem> -rev -tls1_3 -quiet
```

Two flags beyond the contract: `-tls1_3` pins it to the version this suite is about (without
it, stage 09 would negotiate TLS 1.2 and the expectation would be wrong), and `-quiet` stops
it printing the session and the peer certificate on every connection.

Set `TLSTEST_OPENSSL` to point at a different build. OpenSSL 3.x is required; the suite
checks for `-rev`, `-naccept` and `-tls1_3` in `cargo test`.

The server is started in its own process group and killed as a group (`SIGTERM`, then
`SIGKILL`), so a wrapper script never leaves a process behind. Every server instance gets a
free port from the kernel; the suite never binds a fixed one, and never 4433.

**The reference serves connections one at a time.** That is conformant, so no test requires
two handshakes to be in flight at the same moment — stage 39 opens fifty sockets at once and
requires that all fifty are eventually served, which is true of a threaded server and of a
serial one.

`-naccept n` counts the harness's own boot probe, which opens a TCP connection to find out
whether the port is listening. `ServerOptions::with_naccept(n)` means "this test will make n
connections" and the flag on the command line carries one more.

## JSON report

`--json report.json` writes the schema `shelltest` and `kafkatest` also write, so `byo`
reads all three tracks with one parser:

```jsonc
{
  "target": "openssl",
  "validate": true,
  "passed": 320, "failed": 0, "skipped": 1, "elapsed_ms": 72043,
  "stages": [
    {
      "stage": 25, "name": "CertificateVerify: context and transcript",
      "file": "src/stages/s25_certificate_verify.rs",
      "passed": 7, "failed": 0, "skipped": 0,
      "tests": [
        {
          "name": "the signature verifies over the documented content",
          "status": "pass",              // "pass" | "fail" | "skip"
          "ext": false,
          "duration_ms": 37,
          "failures": [],                // one line per failed check
          "skip_reason": null,
          "actual": [],                  // [field path, value] pairs that were seen
          "notes": [],                   // ctx.note(..) lines, pass or fail
          "failure_kind": null           // assertion | timeout | connection | protocol |
                                         // crypto | alert | server_crash | harness
        }
      ]
    }
  ]
}
```

`catalog.json` (`--list --json`) is the file the site consumes:

```jsonc
{
  "track": "tls",
  "generatedAt": "2026-09-19T00:50:11Z",
  "sections": [{ "id": "a", "title": "TCP & the record layer", "stages": [1,2,3,4,5,6,7] }],
  "stages": [{
    "number": 25, "slug": "certificate_verify",
    "name": "CertificateVerify: context and transcript",
    "ext": false, "file": "src/stages/s25_certificate_verify.rs",
    "hints": ["The signed content is 64 bytes of 0x20, then \"TLS 1.3, server …", "…"],
    "tests": [{ "name": "the signature verifies over the documented content" }],
    "examples": [{ "title": "…", "kind": "wire", "request": "…", "request_hex": "…",
                   "response": "…", "response_hex": "…", "note": "…",
                   "request_fields": [{ "offset": 0, "length": 1,
                                        "field": "certificate.msg_type",
                                        "value": "certificate(11)" }],
                   "response_fields": [ /* … */ ] }]
  }]
}
```

`sections` lists all 45 stages; `catalog.json` is committed and `cargo test` fails if it is
stale.

## Examples

Every stage carries one to three **worked examples**: the exact bytes the reference saw and
answered, with every field annotated, so a learner can see what a stage is about before
writing a line of code.

```
target/release/tlstest --capture-examples examples/captured.json --server openssl
target/release/tlstest --list --json catalog.json     # merges them into the catalog
```

An example is declared in the stage's own file and names a **scenario** the capture
replays — it is never typed as hex, so it cannot drift away from the suite:

| | |
|---|---|
| `ExampleSpec::raw(title, build, expect)` | Put exact bytes on the wire at connect time and record what came back. `expect` is `Records(n)`, `UntilAlert`, `UntilClose` or `Silence`. |
| `ExampleSpec::handshake(title, config, request, response)` | Run a full handshake with that configuration and show two named messages of it: `ClientHello`, `ServerHello`, `HelloRetryRequest`, `EncryptedExtensions`, `Certificate`, `CertificateVerify`, `ServerFinished`, `ClientFinished`, `NewSessionTicket` or `ServerFlight`. |
| `ExampleSpec::echo(title, line)` | Handshake, echo one line, and show both application-data records as they were on the wire. |
| `ExampleSpec::text(title)` | No wire exchange at all: the words carry the whole example. |
| `.with_server(f)` | Start the server with different material — `rsa_server`, `chain_server`. |
| `.malformed()` | These bytes are deliberately not valid TLS, so the annotator is expected to stop part way through them. |

The words (`title`, `request`, `response`, `note`) come from the stage file at `--list`
time, so editing a summary needs no server. The bytes and the annotations come from
`examples/captured.json`, so **anything that moves a byte needs a recapture** — which is what
`tests/examples_are_current.rs` checks.

The capture **fails if an annotation walk does not land exactly on the last byte**. That is
the check that keeps the annotations honest: an annotation that has drifted by two bytes is
worse than none. Adding an example for a message the annotator does not know yet means
teaching `src/tls/annotate.rs` that message first.

Annotations are `{offset, length, field, value}` with the offset measured into the hex
string's own bytes. Values that legitimately differ from run to run — randoms, key shares,
signatures, tickets, `verify_data` — carry `"varies": true`.

## Registering a server

`servers.yaml`:

```yaml
servers:
  openssl:                 # the reference, and the --validate target
    kind: reference
    restart: per_test
  my_server:
    command: ["./your_program.sh"]
    cwd: "."
    restart: per_test      # per_test | per_stage
    env: { RUST_LOG: "debug" }
    boot_timeout_ms: 10000
```

`command` says only how to *start* the program; the harness appends the contract's flags
itself. Placeholders substituted in `command`, `cwd` and `env`, for a program that would
rather read them itself: `{PORT}`, `{CERT}`, `{KEY}`, `{TMP}`.

`per_test` is the default and the safe one: a test that abandons a handshake, or that asks
for `-naccept`, changes what the next test would see.

## Adding a stage

One file per stage, `src/stages/sNN_slug.rs`, so several people can work at once without
touching the same file. The whole API is four things: `Stage`, `Test`, `Ctx` and `Check`.

```rust
//! Stage 25 — CertificateVerify: context and transcript.

use crate::assert::Check;
use crate::examples::{ExampleEnv, ExampleSpec, Part};
use crate::stages::{Stage, Test};
use crate::tls_test;

/// Stage definition.
pub fn stage() -> Stage {
    Stage {
        number: 25,
        slug: "certificate_verify",       // the file must be src/stages/s25_certificate_verify.rs
        name: "CertificateVerify: context and transcript",
        ext: false,                       // true hides every test of the stage behind --skip-ext
        hints: &[                         // 2-4 lines; they go into PLAN.md and catalog.json
            "The signed content is 64 bytes of 0x20, then the context string, then 0x00, \
             then Transcript-Hash(CH..Certificate)",
            "Sign the transcript as it stood before this message; it cannot cover itself",
        ],
        examples,                         // 1-3 worked examples
        tests: vec![
            Test::new("the signature verifies over the documented content", verifies),
            Test::new("an RSA key signs with RSA-PSS", rsa)
                .with_server(rsa_server)      // start the server with other material
                .ext()                        // --skip-ext hides it
                .restart()                    // force a fresh process for this test
                .min_timeout_ms(60_000)       // a floor under --timeout-ms, for slow tests
                .tag("slow")                  // --tag slow selects it
                .skip_on("openssl", "OpenSSL does this and a hand-written server need not"),
        ],
    }
}

tls_test!(verifies, |ctx| {
    let client = ctx.handshake().await?;               // a completed handshake
    let cv = client.certificate_verify.as_ref()
        .ok_or_else(|| crate::stages::harness("no CertificateVerify"))?;
    let mut c = Check::new("the CertificateVerify signature");
    c.block("certificate_verify", &client.certificate_verify_bytes);
    c.keying(&client.hash_after_certificate, "Transcript-Hash(CH..Certificate)");
    c.eq("certificate_verify.algorithm", SIG_ECDSA_SECP256R1_SHA256, cv.algorithm);
    c.finish()                                          // Result<(), Failure>
});
```

Then add two lines to `src/stages/mod.rs` (`mod s25_certificate_verify;` and
`s25_certificate_verify::stage(),`), and prove it against the reference:

```
cargo test                                                   # registry invariants + catalog
target/release/tlstest --server openssl --validate --stage 25
target/release/tlstest --capture-examples examples/captured.json --server openssl
target/release/tlstest --list --json catalog.json          # catalog.json must be regenerated
```

`PLAN.md` is generated from the catalog's stage names, files, test counts and hints; a cargo
test checks that every stage's line and every hint is in it.

### What `Ctx` gives a test

| | |
|---|---|
| `ctx.connect().await?` | A raw `TlsConn` with no TLS state: the record layer and nothing else. |
| `ctx.client().await?` / `ctx.client_with(config)` | A `Client` that has connected but sent nothing. |
| `ctx.handshake().await?` / `ctx.handshake_with(config)` | A `Client` that has completed a full handshake, with every intermediate value on it. |
| `ctx.config()` / `ctx.config_n(n)` | A `ClientConfig` seeded from `--seed`; `config_n` varies it, for a test that opens several connections. |
| `ctx.expect_accepting(after)` | Prove the server still accepts TCP. Stage 01's liveness check. |
| `ctx.expect_still_answering(after)` | Prove a fresh ClientHello still earns a handshake record. Sections A and B. |
| `ctx.expect_still_serving(after)` | Prove a fresh connection still completes a handshake. Section C onward. |
| `ctx.material`, `ctx.certs` | The certificate the server was started with, and the store to ask for another. |
| `ctx.rng`, `ctx.seed`, `ctx.salt` | The seeded RNG. Every random choice must come from it, or `--seed` stops meaning anything. |
| `ctx.openssl` | The `openssl` binary, when one is on PATH — stage 45's interop runs. |
| `ctx.note("…")` | An informational line shown under the test **whatever the outcome**, and carried in the JSON report's `notes`. |
| `ctx.skip("reason")` | Decide at run time that the test does not apply — a missing `openssl`, say. `Test::skip_on` is the static version. |

### What `Client` gives a test

`handshake()` is the whole thing; `send_client_hello`, `read_server_hello`,
`read_server_flight` and `send_client_finished` are the flights, so a test can stop in the
middle or substitute one message. Afterwards every intermediate value is a public field:
`client_hello`, `server_hello`, `hello_retry_request`, `encrypted_extensions`,
`certificate`, `certificate_verify`, `server_verify_data`, `schedule`, `transcript`,
`hash_after_*`, `tickets`, `alpn_selected` — and the `_bytes` of each message, for hex
blocks.

`echo_line(line)`, `write_app_data`, `read_app_data`, `send_key_update`, `collect_tickets`,
`psk_from_ticket`, `close` and `close_both_ways` cover the data path.

### What `Check` gives a test

`Check::new(what)` starts a block. Then:

`eq` · `ne` · `at_least` · `at_most` · `that(path, expected, ok, actual)` ·
`bytes_eq(path, expected, actual)` · `block(title, bytes)` · `mark(range)` ·
`keying(transcript_hash, label)` · `note(line)` · `note_all(lines)` · `observe(path, value)` ·
`finish() -> Result<(), Failure>`.

Every failed `eq` on a numeric field marks the bytes holding the actual value in **every**
attached block, so the hex dumps point at the mistake. `keying` is the one to remember: in
TLS almost every wrong byte is really a wrong transcript or a wrong label.

Stage tests must never `unwrap()` on a network, crypto or decode path —
`Failure::tls(e)` and `crate::stages::handshake_failure(e, &client)` turn a `TlsError` into
a `Failure` with the right kind and everything attached.

## Skips

One test is skipped against the reference, with its reason printed in the run:

- **Stage 45, "curl over this contract is not a meaningful test"** — the track's program
  contract is `-rev`, a line-reversing echo, not HTTPS. `curl --tlsv1.3` would send an HTTP
  request and get its own bytes back reversed, which tests nothing about TLS. Changing the
  contract to serve HTTP would make every other stage harder to write and would not make
  this one stronger. Real-client interop is covered by seven `openssl s_client` tests
  instead, including a pinned suite, a pinned group, certificate verification and
  ticket resumption.

Stage 45's other tests skip themselves with a reason if no `openssl` is on PATH.

## Loose expectations, and why

Where RFC 8446 permits more than one behaviour, the suite accepts all of them and says so in
the failure's `context` block. The ones worth knowing about:

- **A refusal is an alert *or* a close.** A server may answer a broken flight with a fatal
  alert or simply drop the connection. `check_refused_with` accepts either, and checks the
  description only when an alert was actually sent.
- **Alert descriptions are a list.** RFC 8446 often names one description but implementations
  reasonably send a neighbouring one; each test names the set it accepts.
- **ChangeCipherSpec is optional in both directions**, and any number of them is allowed
  between the first ClientHello and the peer's Finished.
- **Tickets are optional.** Stage 43 requires one to test resumption at all, but does not
  require a particular number, lifetime or nonce.
- **ALPN with nothing configured is not an error.** `s_server -rev` has no protocols, so it
  sends no ALPN extension back; a server that selects one must select an offered one.
- **A HelloRetryRequest is a valid answer** to a hello that offers a group with no share, as
  well as `handshake_failure` — stage 11 accepts both.
- **How a server packs its flight is its business.** Stage 06 records how many records the
  reference used and requires only that every message arrives, once, in order.

## Deviations from the plan

- **The client is hand-written, not `rustls`.** That was the plan's instruction and it is
  also what makes the failure output possible. `rustls` is not a dependency at all — not
  even for the one interop cross-check the plan allowed, because `openssl s_client` is a
  better independent implementation for the purpose and needs no new crate.
- **RSA keys are generated by the `rsa` crate, not `rcgen`.** `rcgen`'s `ring` backend can
  sign with an RSA key but cannot generate one; the key is made with `rsa` and fed back in
  as PKCS#8.
- **`--capture-examples` doubles as the annotation self-check.** It refuses to write a
  capture whose walk does not end on a frame boundary, which is why adding an example for a
  new message means teaching the annotator that message first.
- **`--json` doubles as the catalog writer** when combined with `--list`, so that both
  `tlstest --list --json > catalog.json` and `--list --json catalog.json` work.
- **`PLAN.md` is generated** from the stage registry rather than hand-maintained, and a
  cargo test proves the two agree.

## Development

```
cargo build --release -p tlstest
cargo test                                  # 98 unit + 44 integration tests
cargo clippy --all-targets -- -D warnings
cargo fmt --check
target/release/tlstest --server openssl --validate --all
pgrep -f 's_server' # must print nothing afterwards
```

Layout:

```
src/main.rs              CLI (clap), stage selection, --list, --capture-examples
src/lib.rs               everything else, so tests/ can use it
src/config.rs            servers.yaml, ServerOptions, placeholder substitution
src/certs.rs             per-run certificates: RSA-2048, ECDSA P-256, Ed25519, a chain, an expired one
src/server/mod.rs        spawn, wait-for-port, process-group kill, crash detection
src/server/reference.rs  resolving the system openssl and building its command line
src/tls/mod.rs           code points, names, TlsError — the vocabulary
src/tls/buf.rs           the TLS byte grammar: uint8/16/24 and length-prefixed vectors
src/tls/crypto.rs        HKDF, HKDF-Expand-Label, the key schedule, the transcript, the AEADs
src/tls/record.rs        TLSPlaintext, TLSCiphertext, TLSInnerPlaintext, sequence numbers
src/tls/msg.rs           ClientHello builder, every parsed server message, key exchange
src/tls/sig.rs           CertificateVerify: the signed content and its verification
src/tls/conn.rs          one connection: records in, messages out, and a trace of both
src/tls/client.rs        the handshake driver, flight by flight
src/tls/annotate.rs      the byte walk that produces the field annotations
src/stages/mod.rs        the registry, Stage/Test/Ctx, the tls_test! macro
src/stages/helpers.rs    hello building, provoke/refusal checks, the fuzz mutations
src/stages/sNN_*.rs      one file per stage
src/assert.rs            Check, Failure, FailureKind, the hex dump
src/report.rs            terminal output + the JSON report
src/catalog.rs           --list and catalog.json
src/examples/mod.rs      ExampleSpec: the worked examples a stage declares
src/examples/capture.rs  --capture-examples: replay them and record the answers
tests/                   RFC 8448 vectors, record layer, catalog and example freshness, CLI smoke
examples/broken_server.rs  a deliberately wrong server, for the red path
PLAN.md                  tickboxes and hints for all 45 stages
catalog.json             committed; a test fails if it is stale
examples/captured.json   committed; the example bytes the reference answered
```
