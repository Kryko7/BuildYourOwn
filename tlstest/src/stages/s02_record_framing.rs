//! Stage 02 — TLSPlaintext framing (RFC 8446 §5.1).

use crate::assert::Check;
use crate::examples::{ExampleEnv, ExampleSpec, Expect, Part};
use crate::stages::{hello_record, Stage, Test};
use crate::tls::client::ClientConfig;
use crate::tls::record::Record;
use crate::tls::{ContentType, LEGACY_VERSION_TLS10, LEGACY_VERSION_TLS12, MAX_PLAINTEXT};
use crate::tls_test;

/// Stage definition.
pub fn stage() -> Stage {
    Stage {
        number: 2,
        slug: "record_framing",
        name: "TLSPlaintext framing",
        ext: false,
        hints: &[
            "Every record is five header bytes — type(1), legacy_record_version(2), \
             length(2) — and then exactly `length` fragment bytes",
            "Read the five bytes first, then read `length` more; one read() is never one \
             record, and one record is not always one message",
            "legacy_record_version is 0x0303 everywhere, with one exception: the record \
             carrying the first ClientHello may say 0x0301, and a server must accept it",
            "The server's own records carry 0x0303; the real version is negotiated inside \
             the ServerHello's supported_versions extension",
        ],
        examples: examples,
        tests: vec![
            Test::new(
                "the ClientHello is answered when its record says 0x0303",
                version_0303,
            ),
            Test::new(
                "the ClientHello is answered when its record says 0x0301",
                version_0301,
            ),
            Test::new(
                "the server's first record is a handshake record",
                first_record_is_handshake,
            ),
            Test::new(
                "the server's first record carries legacy_record_version 0x0303",
                server_record_version,
            ),
            Test::new(
                "the server's records never exceed 2^14 fragment bytes",
                server_record_length,
            ),
            Test::new(
                "the record length agrees with the bytes that follow it",
                length_agrees,
            ),
            Test::new(
                "the ServerHello is a complete handshake message inside the record",
                server_hello_is_complete,
            ),
        ],
    }
}

/// Send a ClientHello inside a record whose `legacy_record_version` is `version`, and read
/// the first record that comes back.
async fn hello_with_record_version(
    ctx: &crate::stages::Ctx,
    version: u16,
) -> Result<Record, crate::assert::Failure> {
    let config = ctx.config();
    let message =
        crate::stages::hello_message(&config).map_err(|e| crate::stages::harness(e))?;
    let mut conn = ctx.connect().await?;
    conn.write_raw(&Record::build(22, version, &message))
        .await
        .map_err(crate::assert::Failure::tls)?;
    conn.read_record()
        .await
        .map_err(|e| crate::assert::Failure::tls(e).note(format!(
            "after a ClientHello in a record whose legacy_record_version was 0x{version:04x}"
        )))
}

tls_test!(version_0303, |ctx| {
    let record = hello_with_record_version(ctx, LEGACY_VERSION_TLS12).await?;
    let mut c = Check::new("the answer to a ClientHello framed with 0x0303");
    c.block("the server's first record", &record.raw);
    c.eq(
        "record.type",
        ContentType::Handshake.as_u8(),
        record.content_type.as_u8(),
    );
    c.finish()
});

tls_test!(version_0301, |ctx| {
    let record = hello_with_record_version(ctx, LEGACY_VERSION_TLS10).await?;
    let mut c = Check::new("the answer to a ClientHello framed with 0x0301");
    c.block("the server's first record", &record.raw);
    c.note(
        "RFC 8446 section 5.1: the record carrying the first ClientHello may say 0x0301 for \
         compatibility with middleboxes, and a server must not reject it.",
    );
    c.eq(
        "record.type",
        ContentType::Handshake.as_u8(),
        record.content_type.as_u8(),
    );
    c.finish()
});

tls_test!(first_record_is_handshake, |ctx| {
    let record = hello_with_record_version(ctx, LEGACY_VERSION_TLS12).await?;
    let mut c = Check::new("the type of the server's first record");
    c.block("the server's first record", &record.raw);
    c.mark(0..1);
    c.eq(
        "record.type",
        ContentType::Handshake.as_u8(),
        record.content_type.as_u8(),
    );
    c.finish()
});

tls_test!(server_record_version, |ctx| {
    let record = hello_with_record_version(ctx, LEGACY_VERSION_TLS12).await?;
    let mut c = Check::new("legacy_record_version on the server's first record");
    c.block("the server's first record", &record.raw);
    c.mark(1..3);
    c.note(
        "The 0x0301 exception of RFC 8446 section 5.1 is for the client's first flight only; \
         a server always writes 0x0303.",
    );
    c.eq(
        "record.legacy_record_version",
        LEGACY_VERSION_TLS12,
        record.legacy_version,
    );
    c.finish()
});

tls_test!(server_record_length, |ctx| {
    let client = ctx.handshake().await?;
    let mut c = Check::new("the length of every record the server sent");
    for (i, record) in client.conn.records_in.iter().enumerate() {
        let limit = if record.content_type == ContentType::ApplicationData {
            crate::tls::MAX_CIPHERTEXT
        } else {
            MAX_PLAINTEXT
        };
        c.at_most(
            &format!("records[{i}].length"),
            limit,
            record.fragment.len(),
        );
    }
    c.observe("records.count", client.conn.records_in.len());
    c.finish()
});

tls_test!(length_agrees, |ctx| {
    let record = hello_with_record_version(ctx, LEGACY_VERSION_TLS12).await?;
    let mut c = Check::new("that the record's length field describes its fragment");
    c.block("the server's first record", &record.raw);
    c.eq(
        "record.length",
        record.fragment.len(),
        record.raw.len().saturating_sub(5),
    );
    c.eq(
        "record.raw.len()",
        record.fragment.len() + 5,
        record.raw.len(),
    );
    c.finish()
});

tls_test!(server_hello_is_complete, |ctx| {
    let record = hello_with_record_version(ctx, LEGACY_VERSION_TLS12).await?;
    let mut c = Check::new("the handshake message inside the server's first record");
    c.block("the server's first record", &record.raw);
    let parsed = crate::tls::msg::HandshakeMessage::parse(&record.fragment);
    match parsed {
        Ok((message, used)) => {
            c.eq("server_hello.msg_type", 2u8, message.msg_type.0);
            c.eq(
                "the fragment holds exactly one complete handshake message",
                record.fragment.len(),
                used,
            );
        }
        Err(e) => {
            c.that(
                "record.fragment",
                "a complete handshake message",
                false,
                e.to_string(),
            );
        }
    }
    c.finish()
});

fn hello_bytes(env: &ExampleEnv) -> Result<Vec<u8>, String> {
    hello_record(&env.config())
}

fn hello_bytes_0301(env: &ExampleEnv) -> Result<Vec<u8>, String> {
    let message = crate::stages::hello_message(&env.config())?;
    Ok(Record::build(22, LEGACY_VERSION_TLS10, &message))
}

fn config(env: &ExampleEnv) -> ClientConfig {
    env.config()
}

fn examples() -> Vec<ExampleSpec> {
    vec![
        ExampleSpec::raw(
            "A ClientHello record, and the record that answers it",
            hello_bytes,
            Expect::Records(1),
        )
        .request(
            "type 22 (handshake), legacy_record_version 0x0303, a two-byte length, then the \
             ClientHello message",
        )
        .response(
            "One handshake record carrying the ServerHello: type 22, version 0x0303, length, \
             message.",
        )
        .note(
            "Five header bytes, then exactly `length` more. Read the header first and then \
             read that many bytes — one read() is neither one record nor one message.",
        ),
        ExampleSpec::raw(
            "The same ClientHello with legacy_record_version 0x0301",
            hello_bytes_0301,
            Expect::Records(1),
        )
        .request("Byte for byte the hello above, except that the record header says 0x0301")
        .response("The same ServerHello. The 0x0301 changes nothing.")
        .note(
            "RFC 8446 section 5.1 allows 0x0301 on the record carrying the first ClientHello, \
             for middleboxes that inspect it. Rejecting it is a real interoperability bug.",
        ),
        ExampleSpec::handshake(
            "The ServerHello message itself",
            config,
            Part::ClientHello,
            Part::ServerHello,
        )
        .request("The ClientHello handshake message: msg_type 1, a three-byte length, the body")
        .response(
            "server_hello(2), a three-byte length, legacy_version 0x0303, 32 random bytes, the \
             echoed session id, the chosen cipher suite, compression 0, extensions.",
        )
        .note(
            "The handshake header is one type byte and a uint24 length — note the three bytes, \
             not four. Only this framing, never the record header, goes into the transcript.",
        ),
    ]
}
