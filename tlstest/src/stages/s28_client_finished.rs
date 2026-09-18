//! Stage 28 — The client's Finished, right and wrong.

use crate::assert::{Check, Failure};
use crate::examples::{ExampleEnv, ExampleSpec, Part};
use crate::stages::{check_refused_with, Stage, Test};
use crate::tls::client::ClientConfig;
use crate::tls::msg::encode_handshake;
use crate::tls::{hex, AlertDescription, HandshakeType};
use crate::tls_test;

/// Stage definition.
pub fn stage() -> Stage {
    Stage {
        number: 28,
        slug: "client_finished",
        name: "The client's Finished, and what a wrong one costs",
        ext: false,
        hints: &[
            "The client's verify_data uses the *client* handshake traffic secret's finished \
             key, over Transcript-Hash(everything up to and including the server's Finished)",
            "It arrives encrypted under the client handshake keys, not the application keys — \
             those only come into force once it has been sent",
            "A wrong verify_data is decrypt_error(51), fatal, and the connection ends there",
            "Only after this message is the handshake complete: application data written \
             before it is early data, and that is a different feature",
        ],
        examples,
        tests: vec![
            Test::new("a correct Finished is accepted", accepted),
            Test::new(
                "the client's verify_data uses the client's finished key",
                uses_client_key,
            ),
            Test::new(
                "an all-zero verify_data is rejected with decrypt_error",
                zero_verify_data,
            ),
            Test::new(
                "a verify_data with one bit flipped is rejected",
                flipped_bit,
            ),
            Test::new(
                "the server's own verify_data echoed back is rejected",
                echoed_server_verify_data,
            ),
            Test::new("a Finished of the wrong length is rejected", wrong_length),
            Test::new(
                "the server still serves a new connection after refusing one",
                still_serving,
            ),
        ],
    }
}

tls_test!(accepted, |ctx| {
    let mut client = ctx.handshake().await?;
    let answer = client
        .echo_line("finished")
        .await
        .map_err(|e| crate::stages::handshake_failure(e, &client))?;
    let mut c = Check::new("a correct client Finished");
    c.block("client finished", &client.client_finished_bytes);
    c.eq("echo", "dehsinif".to_string(), answer);
    c.finish()
});

tls_test!(uses_client_key, |ctx| {
    let client = ctx.handshake().await?;
    let schedule = client
        .schedule
        .as_ref()
        .ok_or_else(|| crate::stages::harness("no key schedule"))?;
    let client_keys = schedule
        .client_handshake
        .as_ref()
        .ok_or_else(|| crate::stages::harness("no client handshake keys"))?;
    let server_keys = schedule
        .server_handshake
        .as_ref()
        .ok_or_else(|| crate::stages::harness("no server handshake keys"))?;
    let want = schedule
        .verify_data(client_keys, &client.hash_after_server_finished)
        .map_err(Failure::tls)?;
    let with_server_key = schedule
        .verify_data(server_keys, &client.hash_after_server_finished)
        .map_err(Failure::tls)?;
    let mut c = Check::new("the client's verify_data");
    c.block("client finished", &client.client_finished_bytes);
    c.keying(
        &client.hash_after_server_finished,
        "c hs traffic → finished_key → HMAC",
    );
    c.bytes_eq(
        "client finished.verify_data",
        &want,
        &client.client_finished_bytes[4..],
    );
    c.ne(
        "the same HMAC with the server's finished key",
        hex(&want),
        hex(&with_server_key),
    );
    c.finish()
});

/// Drive the handshake to the point where the client's Finished is due, send `body`
/// instead of the right one, and report what the server did.
async fn bad_finished(
    ctx: &crate::stages::Ctx,
    make_body: impl FnOnce(&crate::tls::client::Client) -> Vec<u8>,
    what: &str,
) -> Result<(), Failure> {
    let mut client = ctx.client().await?;
    client.send_client_hello().await.map_err(Failure::tls)?;
    client.maybe_send_ccs().await.map_err(Failure::tls)?;
    if let Err(e) = client.read_server_hello().await {
        return Err(crate::stages::handshake_failure(e, &client));
    }
    if let Err(e) = client.read_server_flight().await {
        return Err(crate::stages::handshake_failure(e, &client));
    }
    let body = make_body(&client);
    let message = encode_handshake(HandshakeType::FINISHED, &body);
    if let Err(e) = client.send_client_finished_bytes(&message).await {
        return Err(crate::stages::handshake_failure(e, &client));
    }
    let reaction = crate::stages::reaction_past_tickets(&mut client.conn).await;
    let mut c = Check::new(format!("what the server does with {what}"));
    c.block("the Finished that was sent", &message);
    c.keying(
        &client.hash_after_server_finished,
        "c hs traffic → finished_key → HMAC",
    );
    c.note(format!(
        "the correct verify_data would have been {}",
        client
            .client_verify_data()
            .map(|v| hex(&v))
            .unwrap_or_else(|_| "unavailable".into())
    ));
    check_refused_with(
        &mut c,
        "the server's reaction",
        &reaction,
        &[
            AlertDescription::DECRYPT_ERROR,
            AlertDescription::BAD_RECORD_MAC,
            AlertDescription::DECODE_ERROR,
            AlertDescription::ILLEGAL_PARAMETER,
            AlertDescription::HANDSHAKE_FAILURE,
        ],
    );
    c.finish()
}

tls_test!(zero_verify_data, |ctx| {
    bad_finished(
        ctx,
        |client| vec![0u8; client.suite.map(|s| s.hash.len()).unwrap_or(32)],
        "a Finished whose verify_data is all zeros",
    )
    .await
});

tls_test!(flipped_bit, |ctx| {
    bad_finished(
        ctx,
        |client| {
            let mut body = client.client_verify_data().unwrap_or_default();
            if let Some(b) = body.first_mut() {
                *b ^= 0x01;
            }
            body
        },
        "a Finished with one bit of verify_data flipped",
    )
    .await
});

tls_test!(echoed_server_verify_data, |ctx| {
    bad_finished(
        ctx,
        |client| client.server_verify_data.clone(),
        "the server's own verify_data echoed back",
    )
    .await
});

tls_test!(wrong_length, |ctx| {
    bad_finished(
        ctx,
        |client| {
            let mut body = client.client_verify_data().unwrap_or_default();
            body.truncate(8);
            body
        },
        "a Finished whose verify_data is only eight bytes",
    )
    .await
});

tls_test!(still_serving, |ctx| {
    bad_finished(
        ctx,
        |client| vec![0xff; client.suite.map(|s| s.hash.len()).unwrap_or(32)],
        "a Finished of 0xff bytes",
    )
    .await?;
    ctx.expect_still_serving("a refused client Finished").await
});

fn config(env: &ExampleEnv) -> ClientConfig {
    env.config()
}

fn examples() -> Vec<ExampleSpec> {
    vec![
        ExampleSpec::handshake(
            "The client's Finished closes the handshake",
            config,
            Part::ServerFinished,
            Part::ClientFinished,
        )
        .request("The server's Finished, which the client has just verified")
        .response(
            "The client's own `14` + uint24 length + verify_data, encrypted under the *client* \
             handshake keys — the last message before application keys take over.",
        )
        .note(
            "Two Finished messages, two different finished keys, two different transcripts. \
             The client's covers the server's, so it can only be produced after it.",
        ),
        ExampleSpec::text("What a wrong Finished looks like")
            .request(
                "A Finished whose verify_data is anything other than \
                 HMAC(client finished_key, Transcript-Hash(CH..server Finished))",
            )
            .response(
                "A fatal decrypt_error(51) alert and a closed connection. The record itself \
                 decrypted perfectly — it is the MAC inside it that is wrong.",
            )
            .note(
                "decrypt_error, not bad_record_mac: the record layer was fine. \
                 bad_record_mac(20) is for a record whose AEAD tag failed, which is stage 35.",
            ),
    ]
}
