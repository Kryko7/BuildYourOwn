//! Stage 22 — EncryptedExtensions is the first message under the handshake keys.

use crate::assert::Check;
use crate::examples::{ExampleEnv, ExampleSpec, Part};
use crate::stages::{Stage, Test};
use crate::tls::client::ClientConfig;
use crate::tls::{ext_name, EXT_KEY_SHARE, EXT_PRE_SHARED_KEY, EXT_SUPPORTED_VERSIONS};
use crate::tls_test;

/// Stage definition.
pub fn stage() -> Stage {
    Stage {
        number: 22,
        slug: "encrypted_extensions",
        name: "EncryptedExtensions",
        ext: false,
        hints: &[
            "EncryptedExtensions is always the first message the server sends under the \
             handshake traffic keys, and it is always sent — even when it is empty",
            "Its body is one extensions block and nothing else: `08 00 00 02 00 00` is a \
             perfectly good empty one",
            "Only extensions that are *not* needed to establish keys go here: server_name, \
             ALPN, max_fragment_length, and so on",
            "key_share, pre_shared_key and supported_versions belong in the ServerHello; \
             sending them here is unsupported_extension(110)",
        ],
        examples: examples,
        tests: vec![
            Test::new(
                "EncryptedExtensions is sent on every handshake",
                always_sent,
            ),
            Test::new(
                "it is the first message under the handshake keys",
                first_encrypted_message,
            ),
            Test::new("its body is exactly one extensions block", body_shape),
            Test::new(
                "it never carries key_share, pre_shared_key or supported_versions",
                no_key_extensions,
            ),
            Test::new(
                "an empty EncryptedExtensions is six bytes long",
                empty_is_six_bytes,
            ),
            Test::new(
                "it goes into the transcript like any other message",
                in_the_transcript,
            ),
            Test::new(
                "it arrives before Certificate, never after",
                before_certificate,
            ),
        ],
    }
}

tls_test!(always_sent, |ctx| {
    let client = ctx.handshake().await?;
    let mut c = Check::new("that EncryptedExtensions was sent at all");
    c.block("encrypted_extensions", &client.encrypted_extensions_bytes);
    c.that(
        "encrypted_extensions",
        "present",
        !client.encrypted_extensions_bytes.is_empty(),
        "missing",
    );
    c.eq(
        "encrypted_extensions.msg_type",
        crate::tls::HandshakeType::ENCRYPTED_EXTENSIONS.0,
        client
            .encrypted_extensions_bytes
            .first()
            .copied()
            .unwrap_or(0),
    );
    c.finish()
});

tls_test!(first_encrypted_message, |ctx| {
    let client = ctx.handshake().await?;
    // The driver refuses a Certificate that arrives before EncryptedExtensions, so reaching
    // here already proves the order; this states it against the transcript's own labels.
    let labels: Vec<String> = client
        .transcript
        .as_ref()
        .map(|t| t.checkpoints.iter().map(|(l, _)| l.clone()).collect())
        .unwrap_or_default();
    let mut c = Check::new("the first message under the handshake traffic keys");
    c.note_all(client.transcript_lines());
    c.note(
        "RFC 8446 section 4.3.1: EncryptedExtensions 'MUST be sent immediately after the \
         ServerHello', before anything else that is encrypted.",
    );
    let index = labels.iter().position(|l| l == "encrypted_extensions");
    c.eq(
        "the position of encrypted_extensions in the transcript",
        Some(2usize),
        index,
    );
    c.finish()
});

tls_test!(body_shape, |ctx| {
    let client = ctx.handshake().await?;
    let bytes = &client.encrypted_extensions_bytes;
    let mut c = Check::new("the body of EncryptedExtensions");
    c.block("encrypted_extensions", bytes);
    if bytes.len() >= 6 {
        let body_len = u32::from_be_bytes([0, bytes[1], bytes[2], bytes[3]]) as usize;
        let block_len = u16::from_be_bytes([bytes[4], bytes[5]]) as usize;
        c.mark(4..6);
        c.eq("encrypted_extensions.length", bytes.len() - 4, body_len);
        c.eq(
            "encrypted_extensions.extensions.length",
            body_len - 2,
            block_len,
        );
        c.note(
            "The body is a single Extension block: two bytes of length and then that many \
             bytes. There is no other field.",
        );
    } else {
        c.that(
            "encrypted_extensions",
            "at least six bytes",
            false,
            bytes.len(),
        );
    }
    c.finish()
});

tls_test!(no_key_extensions, |ctx| {
    let client = ctx
        .handshake_with(ctx.config().with_alpn(&["http/1.1"]))
        .await?;
    let forbidden = [EXT_KEY_SHARE, EXT_PRE_SHARED_KEY, EXT_SUPPORTED_VERSIONS];
    let mut c = Check::new("which extensions EncryptedExtensions carries");
    c.block("encrypted_extensions", &client.encrypted_extensions_bytes);
    c.note(
        "RFC 8446 section 4.2 table: these three are ServerHello-only, because a client has \
         to read them before it can derive the keys this message is encrypted under.",
    );
    for e in &client.encrypted_extensions {
        c.that(
            &format!("encrypted_extensions[{}]", ext_name(e.ext_type)),
            "not a ServerHello-only extension",
            !forbidden.contains(&e.ext_type),
            format!("{} belongs in the ServerHello", ext_name(e.ext_type)),
        );
    }
    c.observe(
        "encrypted_extensions",
        client
            .encrypted_extensions
            .iter()
            .map(|e| ext_name(e.ext_type))
            .collect::<Vec<_>>(),
    );
    c.finish()
});

tls_test!(empty_is_six_bytes, |ctx| {
    let client = ctx.handshake().await?;
    let mut c = Check::new("the size of the EncryptedExtensions the server sent");
    c.block("encrypted_extensions", &client.encrypted_extensions_bytes);
    c.note(
        "With no extensions to send the whole message is `08 00 00 02 00 00` — six bytes. It \
         is still sent: omitting it is not an option.",
    );
    c.at_least(
        "encrypted_extensions.len()",
        6usize,
        client.encrypted_extensions_bytes.len(),
    );
    c.observe(
        "encrypted_extensions.len()",
        client.encrypted_extensions_bytes.len(),
    );
    c.finish()
});

tls_test!(in_the_transcript, |ctx| {
    let client = ctx.handshake().await?;
    let suite = client
        .suite
        .ok_or_else(|| crate::stages::harness("no suite"))?;
    let mut joined = client.client_hello_bytes.clone();
    joined.extend_from_slice(&client.server_hello_bytes);
    joined.extend_from_slice(&client.encrypted_extensions_bytes);
    let expected = suite.hash.digest(&joined);
    let mut c = Check::new("EncryptedExtensions in the transcript");
    c.note(
        "It is hashed exactly as it was sent — header included, decrypted, without the record \
         framing it travelled in.",
    );
    c.bytes_eq(
        "Transcript-Hash(CH..EncryptedExtensions)",
        &expected,
        &client.hash_after_encrypted_extensions,
    );
    c.finish()
});

tls_test!(before_certificate, |ctx| {
    let client = ctx.handshake().await?;
    let labels: Vec<String> = client
        .transcript
        .as_ref()
        .map(|t| t.checkpoints.iter().map(|(l, _)| l.clone()).collect())
        .unwrap_or_default();
    let mut c = Check::new("the order of EncryptedExtensions and Certificate");
    c.note_all(client.transcript_lines());
    let ee = labels.iter().position(|l| l == "encrypted_extensions");
    let cert = labels.iter().position(|l| l == "certificate");
    c.that(
        "encrypted_extensions before certificate",
        "EncryptedExtensions first",
        matches!((ee, cert), (Some(a), Some(b)) if a < b),
        format!("encrypted_extensions at {ee:?}, certificate at {cert:?}"),
    );
    c.finish()
});

fn alpn_config(env: &ExampleEnv) -> ClientConfig {
    env.config().with_alpn(&["http/1.1"])
}

fn examples() -> Vec<ExampleSpec> {
    vec![
        ExampleSpec::handshake(
            "The first message the client has to decrypt",
            alpn_config,
            Part::ClientHello,
            Part::EncryptedExtensions,
        )
        .request("A ClientHello offering SNI and ALPN")
        .response(
            "`08`, a uint24 length, and one extensions block. For `s_server -rev` that block \
             is usually empty or holds only supported_groups.",
        )
        .note(
            "This is the message that tells you whether the handshake keys are right. If \
             EncryptedExtensions decrypts, the ServerHello, the transcript, the (EC)DHE and \
             the whole schedule up to `s hs traffic` were all correct.",
        ),
        ExampleSpec::text("Which extension goes where")
            .request(
                "ServerHello: key_share, pre_shared_key, supported_versions — the three a \
                 client needs before it has keys.",
            )
            .response(
                "EncryptedExtensions: server_name, ALPN, max_fragment_length, \
                 supported_groups, early_data, and anything else that is not key material.\n\
                 Certificate: status_request and signed_certificate_timestamp, per entry.",
            )
            .note(
                "The split exists so that everything which does not have to be public is not. \
                 Putting a ServerHello extension here is unsupported_extension(110); putting \
                 an EncryptedExtensions one in the ServerHello leaks it.",
            ),
    ]
}
