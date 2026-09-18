//! Stage 06 — Two handshake messages coalesced in one record.

use crate::assert::{Check, Failure};
use crate::examples::Expect;
use crate::examples::{ExampleEnv, ExampleSpec, Part};
use crate::stages::{hello_message, provoke};
use crate::stages::{Stage, Test};
use crate::tls::client::ClientConfig;
use crate::tls::msg::{encode_handshake, HandshakeMessage};
use crate::tls::record::Record;
use crate::tls::{ContentType, HandshakeType, LEGACY_VERSION_TLS12};
use crate::tls_test;

/// Stage definition.
pub fn stage() -> Stage {
    Stage {
        number: 6,
        slug: "coalesced_handshake",
        name: "Coalesced handshake messages",
        ext: false,
        hints: &[
            "One record may hold several handshake messages end to end; loop over the \
             fragment until it is empty instead of parsing it once",
            "The server's own flight is usually coalesced: EncryptedExtensions, Certificate, \
             CertificateVerify and Finished often arrive in one or two records",
            "Each message still gets its own four-byte header, and each goes into the \
             transcript separately, in order",
            "A record must never end in the middle of a message *and* start a new one — but a \
             message may start in one record and finish in the next",
        ],
        examples,
        tests: vec![
            Test::new(
                "the server's encrypted flight is read whatever way it is packed",
                flight_is_read,
            ),
            Test::new(
                "every message of the flight arrives exactly once",
                flight_messages,
            ),
            Test::new(
                "the flight's messages are in the order RFC 8446 fixes",
                flight_order,
            ),
            Test::new(
                "a record carrying a ClientHello and trailing bytes is refused",
                trailing_bytes,
            ),
            Test::new(
                "the client's ChangeCipherSpec and Finished may be written together",
                client_flight_together,
            ),
            Test::new(
                "the server's records are counted and reported",
                record_count,
            ),
        ],
    }
}

tls_test!(flight_is_read, |ctx| {
    let client = ctx.handshake().await?;
    let mut c = Check::new("the server's encrypted flight");
    c.note(format!(
        "the server used {} records for the whole handshake",
        client.conn.records_in.len()
    ));
    c.that(
        "encrypted_extensions",
        "present",
        !client.encrypted_extensions_bytes.is_empty(),
        "missing",
    );
    c.that(
        "certificate",
        "present",
        !client.certificate_bytes.is_empty(),
        "missing",
    );
    c.that(
        "certificate_verify",
        "present",
        !client.certificate_verify_bytes.is_empty(),
        "missing",
    );
    c.that(
        "finished",
        "present",
        !client.server_finished_bytes.is_empty(),
        "missing",
    );
    c.finish()
});

tls_test!(flight_messages, |ctx| {
    let client = ctx.handshake().await?;
    let mut flight = client.encrypted_extensions_bytes.clone();
    flight.extend_from_slice(&client.certificate_bytes);
    flight.extend_from_slice(&client.certificate_verify_bytes);
    flight.extend_from_slice(&client.server_finished_bytes);
    let mut types = Vec::new();
    let mut at = 0usize;
    while at < flight.len() {
        let (message, used) = HandshakeMessage::parse(&flight[at..]).map_err(Failure::tls)?;
        types.push(message.msg_type.name());
        at += used;
    }
    let mut c = Check::new("the messages of the server's flight");
    c.block("the flight, decrypted and laid end to end", &flight);
    c.eq("flight.messages.len()", 4usize, types.len());
    c.observe("flight.messages", &types);
    c.finish()
});

tls_test!(flight_order, |ctx| {
    let client = ctx.handshake().await?;
    let order: Vec<u8> = [
        &client.encrypted_extensions_bytes,
        &client.certificate_bytes,
        &client.certificate_verify_bytes,
        &client.server_finished_bytes,
    ]
    .iter()
    .filter_map(|b| b.first().copied())
    .collect();
    let mut c = Check::new("the order of the server's flight");
    c.note(
        "RFC 8446 section 4.3.1: EncryptedExtensions is always the first message under the \
         handshake keys, and Finished is always the last.",
    );
    c.eq(
        "flight order",
        vec![
            HandshakeType::ENCRYPTED_EXTENSIONS.0,
            HandshakeType::CERTIFICATE.0,
            HandshakeType::CERTIFICATE_VERIFY.0,
            HandshakeType::FINISHED.0,
        ],
        order,
    );
    c.finish()
});

tls_test!(trailing_bytes, |ctx| {
    let message = hello_message(&ctx.config()).map_err(crate::stages::harness)?;
    let mut fragment = message.clone();
    // A second "message" that is only a header with a length nobody will ever satisfy,
    // plus junk: the record ends mid-message, which RFC 8446 section 5.1 forbids.
    fragment.extend_from_slice(&[1u8, 0xff, 0xff, 0xff, 0x01, 0x02]);
    let mut conn = ctx.connect().await?;
    let reaction = provoke(
        &mut conn,
        &Record::build(22, LEGACY_VERSION_TLS12, &fragment),
    )
    .await;
    let mut c = Check::new("a record holding a ClientHello and half of another message");
    c.note(
        "A server may refuse this outright, or wait for the rest of the second message. What \
         it must not do is answer the first one and forget the leftover bytes.",
    );
    c.that(
        "the server's reaction",
        "a refusal, or patience — but never a crash",
        !matches!(reaction, crate::tls::conn::Reaction::Error(_)),
        reaction.describe(),
    );
    c.finish()?;
    drop(conn);
    ctx.expect_still_answering("a record that ended mid-message")
        .await
});

tls_test!(client_flight_together, |ctx| {
    // The client's last flight is a ChangeCipherSpec record and an encrypted Finished. A
    // server has to cope with them arriving in one write, which is what a real client does.
    let mut client = ctx.client().await?;
    client.send_client_hello().await.map_err(Failure::tls)?;
    client.maybe_send_ccs().await.map_err(Failure::tls)?;
    if let Err(e) = client.read_server_hello().await {
        return Err(crate::stages::handshake_failure(e, &client));
    }
    if let Err(e) = client.read_server_flight().await {
        return Err(crate::stages::handshake_failure(e, &client));
    }
    if let Err(e) = client.send_client_finished().await {
        return Err(crate::stages::handshake_failure(e, &client));
    }
    let answer = client
        .echo_line("together")
        .await
        .map_err(|e| crate::stages::handshake_failure(e, &client))?;
    let mut c = Check::new("the echo after a coalesced client flight");
    c.eq("echo", "rehtegot".to_string(), answer);
    c.finish()
});

tls_test!(record_count, |ctx| {
    let client = ctx.handshake().await?;
    let handshake_records = client
        .conn
        .records_in
        .iter()
        .filter(|r| r.content_type != ContentType::ChangeCipherSpec)
        .count();
    ctx.note(format!(
        "the server sent {handshake_records} non-CCS records for a four-message flight"
    ));
    let mut c = Check::new("how the server packed its flight");
    c.note(
        "Anything from one record to one per message is conformant. The count is recorded so \
         a learner can see what the reference does.",
    );
    c.at_least("server.records", 1usize, handshake_records);
    c.finish()
});

fn config(env: &ExampleEnv) -> ClientConfig {
    env.config()
}

fn two_messages(env: &ExampleEnv) -> Result<Vec<u8>, String> {
    let hello = hello_message(&env.config())?;
    // A ClientHello and a KeyUpdate in one record: legal framing, illegal content, which is
    // the point — the framing must be read before the content is judged.
    let mut fragment = hello;
    fragment.extend_from_slice(&encode_handshake(HandshakeType::KEY_UPDATE, &[0]));
    Ok(Record::build(22, LEGACY_VERSION_TLS12, &fragment))
}

fn examples() -> Vec<ExampleSpec> {
    vec![
        ExampleSpec::handshake(
            "The server's whole flight, decrypted and laid end to end",
            config,
            Part::ClientHello,
            Part::ServerFlight,
        )
        .request("A ClientHello")
        .response(
            "EncryptedExtensions, Certificate, CertificateVerify and Finished, each with its \
             own four-byte header, one after the other.",
        )
        .note(
            "These four usually travel in one or two records. Loop over a record's fragment \
             until it is empty; do not assume one record is one message.",
        ),
        ExampleSpec::raw(
            "Two handshake messages in one record",
            two_messages,
            Expect::UntilClose,
        )
        .request("A ClientHello followed by a KeyUpdate, inside a single handshake record")
        .response(
            "A refusal: the framing is legal but a KeyUpdate before the handshake exists is \
             not, so the server alerts or closes.",
        )
        .note(
            "The framing and the content are two separate decisions. Parse both messages out \
             of the record first, and only then object to the second one.",
        ),
    ]
}
