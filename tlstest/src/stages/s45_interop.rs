//! Stage 45 — Real clients: `openssl s_client`, and a word about `curl`.

use crate::assert::{Check, Failure};
use crate::examples::ExampleSpec;
use crate::stages::{reversed, Stage, Test};
use crate::tls::{
    suite_name, TLS_AES_128_GCM_SHA256, TLS_AES_256_GCM_SHA384, TLS_CHACHA20_POLY1305_SHA256,
};
use crate::tls_test;
use std::process::Stdio;
use std::time::Duration;
use tokio::io::AsyncWriteExt;

/// How long `s_client` is given to finish once it has been asked to stop.
const CLIENT_TIMEOUT: Duration = Duration::from_secs(10);
/// How long to wait for the answer before closing `s_client`'s stdin.
const ANSWER_WAIT: Duration = Duration::from_millis(700);
/// How long to wait after the EOF before stopping `s_client` outright.
///
/// `s_client` does not exit on stdin EOF while the peer's connection is still open, and the
/// server under test has no reason to close first — so the run is ended from this side once
/// the answer and the session summary have had time to arrive.
const SHUTDOWN_WAIT: Duration = Duration::from_millis(400);

/// What one `s_client` run produced.
struct Run {
    /// Its standard output.
    stdout: String,
    /// Its standard error, where the session summary goes under `-quiet`.
    stderr: String,
    /// The command line, for the report.
    command: String,
}

impl Run {
    /// Everything it printed, for a substring search.
    fn all(&self) -> String {
        format!("{}{}", self.stdout, self.stderr)
    }
}

/// Run the system `openssl s_client` against the server under test, feeding it `line`.
async fn s_client(ctx: &crate::stages::Ctx, extra: &[&str], line: &str) -> Result<Run, Failure> {
    let Some(openssl) = ctx.openssl.clone() else {
        return Err(crate::stages::harness("no openssl on PATH"));
    };
    let mut argv: Vec<String> = vec![
        "s_client".into(),
        "-connect".into(),
        format!("127.0.0.1:{}", ctx.addr.port()),
        "-tls1_3".into(),
    ];
    argv.extend(extra.iter().map(|a| (*a).to_string()));
    let command = format!("{} {}", openssl.display(), argv.join(" "));
    let mut child = tokio::process::Command::new(&openssl)
        .args(&argv)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .map_err(|e| crate::stages::harness(format!("cannot run {command}: {e}")))?;
    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(format!("{line}\n").as_bytes()).await;
        let _ = stdin.flush().await;
        // Give the server time to answer before the EOF asks s_client to shut down.
        tokio::time::sleep(ANSWER_WAIT).await;
        drop(stdin);
    }
    tokio::time::sleep(SHUTDOWN_WAIT).await;
    let _ = child.start_kill();
    let output = tokio::time::timeout(CLIENT_TIMEOUT, child.wait_with_output())
        .await
        .map_err(|_| {
            crate::stages::harness(format!(
                "{command} did not exit within {} s of being stopped",
                CLIENT_TIMEOUT.as_secs()
            ))
        })?
        .map_err(|e| crate::stages::harness(format!("{command}: {e}")))?;
    Ok(Run {
        stdout: String::from_utf8_lossy(&output.stdout).to_string(),
        stderr: String::from_utf8_lossy(&output.stderr).to_string(),
        command,
    })
}

/// Stage definition.
pub fn stage() -> Stage {
    Stage {
        number: 45,
        slug: "interop",
        name: "Real-client interop",
        ext: true,
        hints: &[
            "The suite's own client is one implementation; a second, independent one is the \
             only way to find the places where both of yours agree and the RFC does not",
            "`openssl s_client -tls1_3 -connect host:port` is the shortest full TLS 1.3 \
             client there is, and it prints the negotiated parameters",
            "Its `-ciphersuites` and `-groups` flags pin the negotiation, so each of the \
             three suites and both groups can be exercised from the outside",
            "`-sess_out` then `-sess_in` proves resumption end to end: the second run prints \
             `Reused` when the ticket was accepted",
        ],
        examples,
        tests: vec![
            Test::new("openssl s_client completes a handshake", handshake)
                .min_timeout_ms(30_000)
                .tag("slow"),
            Test::new("openssl s_client gets its line reversed", echo)
                .min_timeout_ms(30_000)
                .tag("slow"),
            Test::new("openssl s_client reports TLSv1.3", reports_version)
                .min_timeout_ms(30_000)
                .tag("slow"),
            Test::new(
                "openssl s_client negotiates each of the three suites",
                each_suite,
            )
            .min_timeout_ms(60_000)
            .tag("slow"),
            Test::new(
                "openssl s_client negotiates over secp256r1 as well as x25519",
                each_group,
            )
            .min_timeout_ms(60_000)
            .tag("slow"),
            Test::new(
                "openssl s_client verifies the certificate against its own CA file",
                verifies_certificate,
            )
            .min_timeout_ms(30_000)
            .tag("slow"),
            Test::new("openssl s_client resumes with a ticket", resumes)
                .min_timeout_ms(60_000)
                .tag("slow"),
            Test::new(
                "curl over this contract is not a meaningful test",
                curl_is_skipped,
            ),
            Test::new(
                "the suite's own client still works after all of that",
                own_client_after,
            )
            .min_timeout_ms(30_000),
        ],
    }
}

tls_test!(handshake, |ctx| {
    if ctx.openssl.is_none() {
        ctx.skip("no `openssl` on PATH to run an independent client with");
        return Ok(());
    }
    let run = s_client(ctx, &["-quiet"], "interop").await?;
    let mut c = Check::new("a handshake driven by openssl s_client");
    c.note(run.command.clone());
    c.note(
        "An independent implementation is the point: it agrees with the RFC rather than with \
         this suite's client.",
    );
    c.that(
        "s_client",
        "no handshake failure in its output",
        !run.all().contains("CONNECTION FAILURE") && !run.all().contains("no protocols available"),
        run.all(),
    );
    c.finish()
});

tls_test!(echo, |ctx| {
    if ctx.openssl.is_none() {
        ctx.skip("no `openssl` on PATH to run an independent client with");
        return Ok(());
    }
    let run = s_client(ctx, &["-quiet"], "interop").await?;
    let mut c = Check::new("what s_client got back");
    c.note(run.command.clone());
    c.note(
        "With `-quiet` the only thing on stdout is the application data, so this is the echo \
         and nothing else.",
    );
    c.eq(
        "s_client stdout",
        format!("{}\n", reversed("interop")),
        run.stdout.clone(),
    );
    c.finish()
});

tls_test!(reports_version, |ctx| {
    if ctx.openssl.is_none() {
        ctx.skip("no `openssl` on PATH to run an independent client with");
        return Ok(());
    }
    // Without -quiet, s_client prints the session summary.
    let run = s_client(ctx, &[], "version").await?;
    let all = run.all();
    let mut c = Check::new("the parameters s_client reports");
    c.note(run.command.clone());
    c.that(
        "s_client output",
        "mentions TLSv1.3",
        all.contains("TLSv1.3"),
        all.lines().take(20).collect::<Vec<_>>().join("\n"),
    );
    if let Some(line) = all.lines().find(|l| l.contains("Cipher is")) {
        c.observe("s_client cipher line", line.trim());
    }
    c.that(
        "s_client output",
        "no mention of TLSv1.2",
        !all.contains("Protocol  : TLSv1.2"),
        "s_client reported TLS 1.2",
    );
    c.finish()
});

tls_test!(each_suite, |ctx| {
    if ctx.openssl.is_none() {
        ctx.skip("no `openssl` on PATH to run an independent client with");
        return Ok(());
    }
    let mut c = Check::new("s_client pinned to each of the three suites");
    for (suite, name) in [
        (TLS_AES_128_GCM_SHA256, "TLS_AES_128_GCM_SHA256"),
        (TLS_AES_256_GCM_SHA384, "TLS_AES_256_GCM_SHA384"),
        (TLS_CHACHA20_POLY1305_SHA256, "TLS_CHACHA20_POLY1305_SHA256"),
    ] {
        let run = s_client(ctx, &["-quiet", "-ciphersuites", name], "suite").await?;
        c.note(run.command.clone());
        c.eq(
            &format!("s_client stdout under {}", suite_name(suite)),
            format!("{}\n", reversed("suite")),
            run.stdout.clone(),
        );
    }
    c.finish()
});

tls_test!(each_group, |ctx| {
    if ctx.openssl.is_none() {
        ctx.skip("no `openssl` on PATH to run an independent client with");
        return Ok(());
    }
    let mut c = Check::new("s_client pinned to each named group");
    for group in ["x25519", "secp256r1"] {
        let run = s_client(ctx, &["-quiet", "-groups", group], "group").await?;
        c.note(run.command.clone());
        c.eq(
            &format!("s_client stdout over {group}"),
            format!("{}\n", reversed("group")),
            run.stdout.clone(),
        );
    }
    c.finish()
});

tls_test!(verifies_certificate, |ctx| {
    if ctx.openssl.is_none() {
        ctx.skip("no `openssl` on PATH to run an independent client with");
        return Ok(());
    }
    let ca = ctx.material.cert_pem.to_string_lossy().to_string();
    let run = s_client(
        ctx,
        &[
            "-quiet",
            "-CAfile",
            &ca,
            "-verify_return_error",
            "-verify",
            "2",
        ],
        "verify",
    )
    .await?;
    let mut c = Check::new("s_client verifying the chain it was sent");
    c.note(run.command.clone());
    c.note(
        "The certificate is the one this run generated, so handing s_client the same PEM as \
         its trust anchor should make path building succeed.",
    );
    c.that(
        "s_client",
        "no verification error",
        !run.all().contains("verify error"),
        run.all().lines().take(10).collect::<Vec<_>>().join("\n"),
    );
    c.eq(
        "s_client stdout",
        format!("{}\n", reversed("verify")),
        run.stdout.clone(),
    );
    c.finish()
});

tls_test!(resumes, |ctx| {
    if ctx.openssl.is_none() {
        ctx.skip("no `openssl` on PATH to run an independent client with");
        return Ok(());
    }
    let dir = tempfile::tempdir()
        .map_err(|e| crate::stages::harness(format!("cannot make a temp dir: {e}")))?;
    let session = dir.path().join("session.pem");
    let path = session.to_string_lossy().to_string();
    let first = s_client(ctx, &["-quiet", "-sess_out", &path], "one").await?;
    let mut c = Check::new("s_client resuming with its own ticket");
    c.note(first.command.clone());
    c.eq(
        "the first run's stdout",
        format!("{}\n", reversed("one")),
        first.stdout.clone(),
    );
    if !session.is_file() {
        c.note(
            "s_client wrote no session file, which means the server offered no ticket it \
             wanted to keep; there is nothing to resume with",
        );
        return c.finish();
    }
    let second = s_client(ctx, &["-sess_in", &path], "two").await?;
    c.note(second.command.clone());
    let all = second.all();
    c.that(
        "the second run",
        "reports a reused session",
        all.contains("Reused"),
        all.lines()
            .filter(|l| l.contains("TLSv1.3") || l.contains("Session-ID"))
            .collect::<Vec<_>>()
            .join("\n"),
    );
    c.that(
        "the second run's output",
        "carries the reversed line",
        all.contains(&reversed("two")),
        all.lines().take(5).collect::<Vec<_>>().join("\n"),
    );
    c.finish()
});

tls_test!(curl_is_skipped, |ctx| {
    ctx.skip(
        "the track's program contract is `-rev`, a line-reversing echo, not HTTPS; `curl \
         --tlsv1.3` would send an HTTP request and get its own bytes back reversed, which \
         tests nothing about TLS. Changing the contract to serve HTTP would make every other \
         stage harder to write and would not make this one stronger.",
    );
    Ok(())
});

tls_test!(own_client_after, |ctx| {
    let mut client = ctx.handshake().await?;
    let answer = client
        .echo_line("mine")
        .await
        .map_err(|e| crate::stages::handshake_failure(e, &client))?;
    let mut c = Check::new("the suite's own client after the interop runs");
    c.eq("echo", reversed("mine"), answer);
    c.finish()
});

fn examples() -> Vec<ExampleSpec> {
    vec![
        ExampleSpec::text("The shortest independent client there is")
            .request(
                "printf 'interop\\n' | openssl s_client -connect 127.0.0.1:PORT -tls1_3 -quiet",
            )
            .response(
                "`ponretni` on stdout. With `-quiet` dropped, s_client also prints the \
                 negotiated protocol, cipher suite, temp key and certificate chain.",
            )
            .note(
                "This is the test that catches the bugs two implementations by the same \
                 author agree on. Run it early and often — it costs one command line.",
            ),
        ExampleSpec::text("Pinning the negotiation from the outside")
            .request(
                "-ciphersuites TLS_CHACHA20_POLY1305_SHA256   pins the suite\n\
                 -groups secp256r1                            pins the group\n\
                 -sess_out f / -sess_in f                     saves and resumes a session\n\
                 -CAfile cert.pem -verify_return_error        makes verification fatal",
            )
            .response(
                "Each flag exercises one negotiation path end to end, with an implementation \
                 that has no idea what your server does internally.",
            )
            .note(
                "`curl --tlsv1.3` is deliberately not used here: this track's contract is a \
                 line-reversing echo, not HTTP, so curl would have nothing meaningful to say.",
            ),
    ]
}
