//! Stage 21 — The ChangeCipherSpec compatibility record.

use crate::assert::{Check, Failure};
use crate::examples::{ExampleEnv, ExampleSpec, Expect};
use crate::stages::{hello_message, provoke, Stage, Test};
use crate::tls::record::Record;
use crate::tls::{ContentType, LEGACY_VERSION_TLS12};
use crate::tls_test;

/// Stage definition.
pub fn stage() -> Stage {
    Stage {
        number: 21,
        slug: "change_cipher_spec",
        name: "The ChangeCipherSpec compatibility record",
        ext: false,
        hints: &[
            "TLS 1.3 has no ChangeCipherSpec message; the record survives only so the \
             handshake looks like TLS 1.2 to a middlebox",
            "Accept one (or several) at any point after the first ClientHello, ignore it, and \
             keep the transcript and the sequence numbers untouched",
            "It is always plaintext, even after keys are in force: content type 20, one \
             fragment byte 0x01",
            "Sending one is optional both ways; a client that sends none, and a client that \
             sends three, must both get the same handshake",
        ],
        examples,
        tests: vec![
            Test::new("a client that sends no CCS completes", no_ccs),
            Test::new("a client that sends one CCS completes", one_ccs),
            Test::new("a client that sends three CCS records completes", three_ccs),
            Test::new(
                "a CCS after the client's Finished is an unexpected record",
                ccs_after_finished,
            ),
            Test::new(
                "the server's own CCS, if any, is plaintext and one byte",
                server_ccs_shape,
            ),
            Test::new(
                "a CCS carrying a byte other than 0x01 does not crash the server",
                odd_ccs_payload,
            ),
            Test::new(
                "a CCS before the ClientHello does not crash the server",
                ccs_first,
            ),
        ],
    }
}

tls_test!(no_ccs, |ctx| {
    let mut client = ctx.handshake_with(ctx.config().with_ccs(false)).await?;
    let answer = client
        .echo_line("noccs")
        .await
        .map_err(|e| crate::stages::handshake_failure(e, &client))?;
    let mut c = Check::new("a handshake with no ChangeCipherSpec from the client");
    c.note(
        "Compatibility mode is optional. A server that waits for a CCS before accepting the \
         client's Finished will hang here for ever.",
    );
    c.eq("echo", "sccon".to_string(), answer);
    c.finish()
});

tls_test!(one_ccs, |ctx| {
    let mut client = ctx.handshake_with(ctx.config().with_ccs(true)).await?;
    let answer = client
        .echo_line("oneccs")
        .await
        .map_err(|e| crate::stages::handshake_failure(e, &client))?;
    let mut c = Check::new("a handshake with the usual single ChangeCipherSpec");
    c.eq("echo", crate::stages::reversed("oneccs"), answer);
    c.finish()
});

tls_test!(three_ccs, |ctx| {
    let mut client = ctx.client_with(ctx.config().with_ccs(false)).await?;
    client.send_client_hello().await.map_err(Failure::tls)?;
    for _ in 0..3 {
        client
            .conn
            .write_change_cipher_spec()
            .await
            .map_err(Failure::tls)?;
    }
    if let Err(e) = async {
        client.read_server_hello().await?;
        client.read_server_flight().await?;
        client.send_client_finished().await
    }
    .await
    {
        return Err(crate::stages::handshake_failure(e, &client)
            .note("three ChangeCipherSpec records were sent after the ClientHello"));
    }
    let answer = client
        .echo_line("threeccs")
        .await
        .map_err(|e| crate::stages::handshake_failure(e, &client))?;
    let mut c = Check::new("a handshake with three ChangeCipherSpec records");
    c.note(
        "RFC 8446 appendix D.4 says an implementation may receive an unencrypted CCS 'at any \
         time after the first ClientHello' and must simply drop it. It does not say once.",
    );
    c.eq("echo", crate::stages::reversed("threeccs"), answer);
    c.finish()
});

tls_test!(ccs_after_finished, |ctx| {
    let mut client = ctx.handshake().await?;
    client
        .conn
        .write_change_cipher_spec()
        .await
        .map_err(Failure::tls)?;
    let reaction = crate::stages::reaction_past_tickets(&mut client.conn).await;
    let mut c = Check::new("a ChangeCipherSpec sent after the client's Finished");
    c.note(
        "RFC 8446 appendix D.4: a CCS is only droppable between the first ClientHello and the \
         peer's Finished. Outside that window 'it MUST be treated as an unexpected record \
         type', which is unexpected_message(10).",
    );
    crate::stages::check_refused_with(
        &mut c,
        "the server's reaction",
        &reaction,
        &[
            crate::tls::AlertDescription::UNEXPECTED_MESSAGE,
            crate::tls::AlertDescription::DECODE_ERROR,
            crate::tls::AlertDescription::ILLEGAL_PARAMETER,
        ],
    );
    c.finish()?;
    drop(client);
    ctx.expect_still_serving("a ChangeCipherSpec after the client's Finished")
        .await
});

tls_test!(server_ccs_shape, |ctx| {
    let client = ctx.handshake().await?;
    let ccs: Vec<&crate::tls::record::Record> = client
        .conn
        .records_in
        .iter()
        .filter(|r| r.content_type == ContentType::ChangeCipherSpec)
        .collect();
    let mut c = Check::new("the shape of the server's ChangeCipherSpec records");
    c.note(format!("the server sent {} of them", ccs.len()));
    c.note(
        "Sending one is optional; a server that received a non-empty legacy_session_id \
         usually does, because that is the signal the client is in compatibility mode.",
    );
    for (i, record) in ccs.iter().enumerate() {
        c.block(format!("change_cipher_spec record {i}"), &record.raw);
        c.eq(
            &format!("records[{i}].type"),
            ContentType::ChangeCipherSpec.as_u8(),
            record.content_type.as_u8(),
        );
        c.eq(
            &format!("records[{i}].length"),
            1usize,
            record.fragment.len(),
        );
        c.eq(
            &format!("records[{i}].fragment[0]"),
            1u8,
            record.fragment.first().copied().unwrap_or(0),
        );
        c.eq(
            &format!("records[{i}].legacy_record_version"),
            LEGACY_VERSION_TLS12,
            record.legacy_version,
        );
    }
    c.finish()
});

tls_test!(odd_ccs_payload, |ctx| {
    let message = hello_message(&ctx.config()).map_err(crate::stages::harness)?;
    let mut bytes = Record::build(22, LEGACY_VERSION_TLS12, &message);
    // A CCS whose single byte is 0x02 rather than 0x01.
    bytes.extend_from_slice(&Record::build(20, LEGACY_VERSION_TLS12, &[2]));
    let mut conn = ctx.connect().await?;
    let reaction = provoke(&mut conn, &bytes).await;
    let mut c = Check::new("a ChangeCipherSpec whose payload byte is 0x02");
    c.note(
        "RFC 8446 appendix D.4 allows an implementation to reject a CCS whose value is not \
         0x01, and allows it to ignore the record entirely. Both are fine; crashing is not.",
    );
    c.that(
        "the server's reaction",
        "an alert, a close, or a normal ServerHello",
        !matches!(reaction, crate::tls::conn::Reaction::Error(_)),
        reaction.describe(),
    );
    c.finish()?;
    drop(conn);
    ctx.expect_still_serving("a ChangeCipherSpec with a wrong payload")
        .await
});

tls_test!(ccs_first, |ctx| {
    // A CCS before any ClientHello: outside the window appendix D.4 sanctions.
    let mut bytes = Record::build(20, LEGACY_VERSION_TLS12, &[1]);
    bytes.extend_from_slice(&Record::build(
        22,
        LEGACY_VERSION_TLS12,
        &hello_message(&ctx.config()).map_err(crate::stages::harness)?,
    ));
    let mut conn = ctx.connect().await?;
    let reaction = provoke(&mut conn, &bytes).await;
    let mut c = Check::new("a ChangeCipherSpec sent before the ClientHello");
    c.note(
        "RFC 8446 appendix D.4 puts this outside the droppable window too, so a refusal is \
         right; a server that quietly ignores it and answers the hello is also alive and \
         well, which is what the check requires.",
    );
    c.observe("the server's reaction", reaction.describe());
    c.that(
        "the server's reaction",
        "an alert, a close, or a ServerHello — but not a crash",
        !matches!(reaction, crate::tls::conn::Reaction::Error(_)),
        reaction.describe(),
    );
    c.finish()?;
    drop(conn);
    ctx.expect_still_serving("a ChangeCipherSpec before the ClientHello")
        .await
});

fn hello_and_ccs(env: &ExampleEnv) -> Result<Vec<u8>, String> {
    let mut bytes = Record::build(22, LEGACY_VERSION_TLS12, &hello_message(&env.config())?);
    bytes.extend_from_slice(&Record::build(20, LEGACY_VERSION_TLS12, &[1]));
    Ok(bytes)
}

fn examples() -> Vec<ExampleSpec> {
    vec![
        ExampleSpec::raw(
            "A ClientHello followed by the compatibility record",
            hello_and_ccs,
            Expect::Records(2),
        )
        .request(
            "The ClientHello record, then `14 03 03 00 01 01` — six bytes that mean nothing \
             and change nothing",
        )
        .response(
            "The ServerHello, and usually the server's own `14 03 03 00 01 01` straight after \
             it.",
        )
        .note(
            "Drop it on the floor. It does not go into the transcript, it does not advance a \
             sequence number, and it is not encrypted even when it arrives after the keys \
             have changed.",
        ),
        ExampleSpec::text("When each side sends one")
            .request(
                "The client sends one after its ClientHello when it put a non-empty \
                 legacy_session_id there — the two together are what 'compatibility mode' \
                 means.",
            )
            .response(
                "The server sends one straight after its ServerHello, before the first \
                 encrypted record, for the same reason.",
            )
            .note(
                "Both are optional in both directions. Requiring one — or refusing a second — \
                 breaks real clients, so accept any number and require none.",
            ),
    ]
}
