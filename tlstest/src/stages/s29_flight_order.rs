//! Stage 29 — Messages that arrive in the wrong order.

use crate::assert::{Check, Failure};
use crate::examples::{ExampleEnv, ExampleSpec, Expect};
use crate::stages::{check_refused_with, hello_message, provoke, Stage, Test};
use crate::tls::msg::{encode_handshake, KeyUpdate};
use crate::tls::record::Record;
use crate::tls::{AlertDescription, ContentType, HandshakeType, LEGACY_VERSION_TLS12};
use crate::tls_test;

/// Stage definition.
pub fn stage() -> Stage {
    Stage {
        number: 29,
        slug: "flight_order",
        name: "Wrong-order flights",
        ext: false,
        hints: &[
            "A TLS server is a state machine: decide what message is legal *next*, and refuse \
             everything else with unexpected_message(10)",
            "The client's whole first flight is one ClientHello; a second one is only ever \
             legal as the answer to a HelloRetryRequest",
            "Application data before the client's Finished is early data, and without an \
             early_data extension there is nowhere for it to be decrypted",
            "The check belongs before the parse: a message type that cannot occur here should \
             never reach the code that decodes its body",
        ],
        examples,
        tests: vec![
            Test::new(
                "the server's own flight is in the order RFC 8446 fixes",
                server_flight_order,
            ),
            Test::new(
                "a Finished sent instead of a ClientHello is refused",
                finished_first,
            ),
            Test::new(
                "a second ClientHello with no HelloRetryRequest is refused",
                second_hello,
            ),
            Test::new(
                "a KeyUpdate before the handshake completes is refused",
                early_key_update,
            ),
            Test::new(
                "a Certificate the server never asked for is refused",
                unsolicited_certificate,
            ),
            Test::new(
                "a ServerHello sent by the client is refused",
                client_sends_server_hello,
            ),
            Test::new(
                "application data before the client's Finished is refused",
                app_data_before_finished,
            ),
            Test::new(
                "the server still serves a well-ordered handshake afterwards",
                still_serving,
            ),
        ],
    }
}

tls_test!(server_flight_order, |ctx| {
    let client = ctx.handshake().await?;
    let labels: Vec<String> = client
        .transcript
        .as_ref()
        .map(|t| t.checkpoints.iter().map(|(l, _)| l.clone()).collect())
        .unwrap_or_default();
    let mut c = Check::new("the order of the whole handshake");
    c.note_all(client.transcript_lines());
    c.note(
        "RFC 8446 section 4: ClientHello, ServerHello, EncryptedExtensions, \
         [CertificateRequest], Certificate, CertificateVerify, Finished — and then the \
         client's Finished.",
    );
    c.eq(
        "the handshake message order",
        vec![
            "client_hello".to_string(),
            "server_hello".to_string(),
            "encrypted_extensions".to_string(),
            "certificate".to_string(),
            "certificate_verify".to_string(),
            "server finished".to_string(),
            "client finished".to_string(),
        ],
        labels,
    );
    c.finish()
});

/// Send a ClientHello, complete nothing, then put `message` on the wire in the clear and
/// see what the server does.
async fn out_of_order(
    ctx: &mut crate::stages::Ctx,
    before_hello: bool,
    message: Vec<u8>,
    what: &str,
    allowed: &[AlertDescription],
) -> Result<(), Failure> {
    let mut bytes = Vec::new();
    if !before_hello {
        bytes.extend_from_slice(&Record::build(
            22,
            LEGACY_VERSION_TLS12,
            &hello_message(&ctx.config()).map_err(crate::stages::harness)?,
        ));
    }
    bytes.extend_from_slice(&Record::build(22, LEGACY_VERSION_TLS12, &message));
    let mut conn = ctx.connect().await?;
    let reaction = provoke(&mut conn, &bytes).await;
    let mut c = Check::new(format!("what the server does with {what}"));
    c.block("the handshake message that was sent out of order", &message);
    check_refused_with(&mut c, "the server's reaction", &reaction, allowed);
    c.finish()?;
    drop(conn);
    ctx.expect_still_serving(what).await
}

tls_test!(finished_first, |ctx| {
    out_of_order(
        ctx,
        true,
        encode_handshake(HandshakeType::FINISHED, &[0u8; 32]),
        "a Finished sent where a ClientHello belongs",
        &[
            AlertDescription::UNEXPECTED_MESSAGE,
            AlertDescription::DECODE_ERROR,
            AlertDescription::ILLEGAL_PARAMETER,
            AlertDescription::HANDSHAKE_FAILURE,
            AlertDescription::PROTOCOL_VERSION,
        ],
    )
    .await
});

/// Drive the handshake to the point where the client's Finished is due, send `message`
/// instead, and report what the server did about it.
///
/// This is where a wrong-order message can actually be *read*: by then both sides have
/// handshake keys, so the server's alert comes back encrypted and legible rather than as an
/// opaque record.
async fn instead_of_finished(
    ctx: &mut crate::stages::Ctx,
    message: Vec<u8>,
    what: &str,
    allowed: &[AlertDescription],
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
    let (write, read) = {
        let schedule = client
            .schedule
            .as_ref()
            .ok_or_else(|| crate::stages::harness("no key schedule"))?;
        (
            schedule
                .client_handshake
                .clone()
                .ok_or_else(|| crate::stages::harness("no client handshake keys"))?,
            schedule.server_application.clone(),
        )
    };
    client.conn.layer.set_write(write);
    client
        .conn
        .write_record(ContentType::Handshake, &message)
        .await
        .map_err(Failure::tls)?;
    // A server switches its own write keys to the application ones as soon as it has sent
    // its Finished, so whatever it says next is under those, at sequence zero.
    if let Some(keys) = read {
        client.conn.layer.set_read(keys);
    }
    let reaction = crate::stages::reaction_past_tickets(&mut client.conn).await;
    let mut c = Check::new(format!("what the server does with {what}"));
    c.block(
        "the message that was sent in place of the Finished",
        &message,
    );
    check_refused_with(&mut c, "the server's reaction", &reaction, allowed);
    c.finish()?;
    drop(client);
    ctx.expect_still_serving(what).await
}

tls_test!(second_hello, |ctx| {
    let second = hello_message(&ctx.config_n(9)).map_err(crate::stages::harness)?;
    instead_of_finished(
        ctx,
        second,
        "a second ClientHello with no HelloRetryRequest between them",
        &[
            AlertDescription::UNEXPECTED_MESSAGE,
            AlertDescription::DECODE_ERROR,
            AlertDescription::ILLEGAL_PARAMETER,
            AlertDescription::HANDSHAKE_FAILURE,
        ],
    )
    .await
});

tls_test!(early_key_update, |ctx| {
    instead_of_finished(
        ctx,
        KeyUpdate::NOT_REQUESTED.encode(),
        "a KeyUpdate sent before the handshake has finished",
        &[
            AlertDescription::UNEXPECTED_MESSAGE,
            AlertDescription::DECODE_ERROR,
            AlertDescription::ILLEGAL_PARAMETER,
            AlertDescription::HANDSHAKE_FAILURE,
        ],
    )
    .await
});

tls_test!(unsolicited_certificate, |ctx| {
    // A Certificate message with an empty list: well-formed, and completely unexpected,
    // because this server never sent a CertificateRequest.
    let body = vec![0u8, 0, 0, 0];
    instead_of_finished(
        ctx,
        encode_handshake(HandshakeType::CERTIFICATE, &body),
        "a client Certificate the server never asked for",
        &[
            AlertDescription::UNEXPECTED_MESSAGE,
            AlertDescription::DECODE_ERROR,
            AlertDescription::ILLEGAL_PARAMETER,
            AlertDescription::HANDSHAKE_FAILURE,
            AlertDescription::CERTIFICATE_REQUIRED,
        ],
    )
    .await
});

tls_test!(client_sends_server_hello, |ctx| {
    // A byte-for-byte legal ServerHello, sent by the client. Nothing about its contents is
    // wrong; only the direction is.
    let mut body = vec![0x03, 0x03];
    body.extend_from_slice(&[0x11; 32]);
    body.push(0);
    body.extend_from_slice(&0x1301u16.to_be_bytes());
    body.push(0);
    body.extend_from_slice(&[0, 0]);
    out_of_order(
        ctx,
        true,
        encode_handshake(HandshakeType::SERVER_HELLO, &body),
        "a ServerHello sent by the client",
        &[
            AlertDescription::UNEXPECTED_MESSAGE,
            AlertDescription::DECODE_ERROR,
            AlertDescription::ILLEGAL_PARAMETER,
            AlertDescription::HANDSHAKE_FAILURE,
            AlertDescription::PROTOCOL_VERSION,
        ],
    )
    .await
});

tls_test!(app_data_before_finished, |ctx| {
    // Complete the server's flight, then send application data instead of the Finished.
    let mut client = ctx.client().await?;
    client.send_client_hello().await.map_err(Failure::tls)?;
    client.maybe_send_ccs().await.map_err(Failure::tls)?;
    if let Err(e) = client.read_server_hello().await {
        return Err(crate::stages::handshake_failure(e, &client));
    }
    if let Err(e) = client.read_server_flight().await {
        return Err(crate::stages::handshake_failure(e, &client));
    }
    // Install the client's handshake write keys — where the Finished would go — and send
    // application data under them instead.
    let keys = client
        .schedule
        .as_ref()
        .and_then(|s| s.client_handshake.clone())
        .ok_or_else(|| crate::stages::harness("no client handshake keys"))?;
    client.conn.layer.set_write(keys);
    client
        .conn
        .write_record(ContentType::ApplicationData, b"too early\n")
        .await
        .map_err(Failure::tls)?;
    if let Some(keys) = client
        .schedule
        .as_ref()
        .and_then(|s| s.server_application.clone())
    {
        client.conn.layer.set_read(keys);
    }
    let reaction = crate::stages::reaction_past_tickets(&mut client.conn).await;
    let mut c = Check::new("application data sent instead of the client's Finished");
    c.note(
        "Without an early_data extension there is no key schedule branch this could belong \
         to: the handshake is not finished, so there is no application traffic secret yet.",
    );
    check_refused_with(
        &mut c,
        "the server's reaction",
        &reaction,
        &[
            AlertDescription::UNEXPECTED_MESSAGE,
            AlertDescription::BAD_RECORD_MAC,
            AlertDescription::DECRYPT_ERROR,
            AlertDescription::DECODE_ERROR,
            AlertDescription::HANDSHAKE_FAILURE,
        ],
    );
    c.finish()?;
    drop(client);
    ctx.expect_still_serving("application data in place of a Finished")
        .await
});

tls_test!(still_serving, |ctx| {
    let mut client = ctx.handshake().await?;
    let answer = client
        .echo_line("order")
        .await
        .map_err(|e| crate::stages::handshake_failure(e, &client))?;
    let mut c = Check::new("a well-ordered handshake after all of those");
    c.eq("echo", "redro".to_string(), answer);
    c.finish()
});

fn hello_then_finished(env: &ExampleEnv) -> Result<Vec<u8>, String> {
    let mut bytes = Record::build(22, LEGACY_VERSION_TLS12, &hello_message(&env.config())?);
    bytes.extend_from_slice(&Record::build(
        22,
        LEGACY_VERSION_TLS12,
        &encode_handshake(HandshakeType::FINISHED, &[0u8; 32]),
    ));
    Ok(bytes)
}

fn examples() -> Vec<ExampleSpec> {
    vec![
        ExampleSpec::raw(
            "A ClientHello followed immediately by a Finished",
            hello_then_finished,
            Expect::UntilAlert,
        )
        .request(
            "A perfectly good ClientHello, and then a plaintext Finished where the server \
             expects to be talking next",
        )
        .response(
            "The ServerHello and its flight may go out first, and then a fatal alert — \
             unexpected_message(10) is the one RFC 8446 names.",
        )
        .note(
            "The Finished is well-formed. It is only wrong because of where it is, which is \
             why the state machine has to be a real one and not a sequence of parsers.",
        ),
        ExampleSpec::text("The legal order, once")
            .request(
                "client:  ClientHello  ...  [Finished]\n\
                 server:  ServerHello, EncryptedExtensions, [CertificateRequest], \
                 Certificate, CertificateVerify, Finished",
            )
            .response(
                "Anything else — a repeated message, a message from the other side's list, a \
                 post-handshake message during the handshake — is unexpected_message(10).",
            )
            .note(
                "The only legal second ClientHello is the answer to a HelloRetryRequest, and a \
                 server that sent one knows it is waiting for exactly that.",
            ),
    ]
}
