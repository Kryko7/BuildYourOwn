//! Stage 19 — The transcript hash at every message boundary.

use crate::assert::Check;
use crate::examples::ExampleSpec;
use crate::stages::{Stage, Test};
use crate::tls::crypto::Transcript;
use crate::tls::hex;
use crate::tls_test;

/// Stage definition.
pub fn stage() -> Stage {
    Stage {
        number: 19,
        slug: "transcript_hash",
        name: "The transcript hash at every boundary",
        ext: false,
        hints: &[
            "The transcript is the concatenation of complete handshake messages — msg_type, \
             the uint24 length and the body — and nothing else",
            "Record headers, ChangeCipherSpec records and the AEAD's own bytes never enter it; \
             a message that arrived in three records is hashed once",
            "Keep a running hash context and take a snapshot at each boundary: four different \
             hashes are needed at four different moments",
            "The hash a signature or a MAC covers is the one *before* that message was added — \
             CertificateVerify covers up to Certificate, Finished covers up to CertificateVerify",
        ],
        examples,
        tests: vec![
            Test::new(
                "the hash after the ClientHello is the hash of its bytes",
                after_client_hello,
            ),
            Test::new(
                "the hash after the ServerHello covers both hellos",
                after_server_hello,
            ),
            Test::new("every boundary has a different hash", boundaries_differ),
            Test::new(
                "CertificateVerify signs the hash taken after Certificate",
                certificate_verify_boundary,
            ),
            Test::new(
                "the server's Finished covers the hash before itself",
                finished_boundary,
            ),
            Test::new(
                "ChangeCipherSpec records leave the transcript alone",
                ccs_not_hashed,
            ),
            Test::new(
                "the transcript is rebuilt from the messages and matches",
                rebuilt,
            ),
        ],
    }
}

tls_test!(after_client_hello, |ctx| {
    let client = ctx.handshake().await?;
    let hash = client
        .suite
        .ok_or_else(|| crate::stages::harness("no suite"))?
        .hash;
    let expected = hash.digest(&client.client_hello_bytes);
    let mut c = Check::new("Transcript-Hash(ClientHello)");
    c.block("client_hello", &client.client_hello_bytes);
    c.note(format!(
        "{} over the {} bytes of the handshake message",
        hash.name(),
        client.client_hello_bytes.len()
    ));
    c.bytes_eq(
        "Transcript-Hash(ClientHello)",
        &expected,
        &client.hash_after_client_hello,
    );
    c.finish()
});

tls_test!(after_server_hello, |ctx| {
    let client = ctx.handshake().await?;
    let hash = client
        .suite
        .ok_or_else(|| crate::stages::harness("no suite"))?
        .hash;
    let mut joined = client.client_hello_bytes.clone();
    joined.extend_from_slice(&client.server_hello_bytes);
    let expected = hash.digest(&joined);
    let mut c = Check::new("Transcript-Hash(ClientHello, ServerHello)");
    c.block("client_hello", &client.client_hello_bytes);
    c.block("server_hello", &client.server_hello_bytes);
    c.note(
        "This is the hash the handshake traffic secrets are derived over, and the first one \
         that has to be right or nothing after it decrypts.",
    );
    c.bytes_eq(
        "Transcript-Hash(CH..SH)",
        &expected,
        &client.hash_after_server_hello,
    );
    c.finish()
});

tls_test!(boundaries_differ, |ctx| {
    let client = ctx.handshake().await?;
    let boundaries = [
        ("after ClientHello", &client.hash_after_client_hello),
        ("after ServerHello", &client.hash_after_server_hello),
        (
            "after EncryptedExtensions",
            &client.hash_after_encrypted_extensions,
        ),
        ("after Certificate", &client.hash_after_certificate),
        (
            "after CertificateVerify",
            &client.hash_after_certificate_verify,
        ),
        (
            "after the server's Finished",
            &client.hash_after_server_finished,
        ),
        (
            "after the client's Finished",
            &client.hash_after_client_finished,
        ),
    ];
    let mut c = Check::new("the seven transcript boundaries of a full handshake");
    c.note_all(client.transcript_lines());
    let mut seen: Vec<(&str, String)> = Vec::new();
    for (name, value) in boundaries {
        let h = hex(value);
        c.that(
            &format!("Transcript-Hash {name}"),
            "a non-empty hash",
            !value.is_empty(),
            "empty",
        );
        if let Some((other, _)) = seen.iter().find(|(_, v)| *v == h) {
            c.that(
                &format!("Transcript-Hash {name}"),
                "different from every earlier boundary",
                false,
                format!("equal to the hash {other}"),
            );
        }
        seen.push((name, h));
    }
    c.finish()
});

tls_test!(certificate_verify_boundary, |ctx| {
    let client = ctx.handshake().await?;
    let key = client
        .server_public_key
        .as_ref()
        .ok_or_else(|| crate::stages::harness("no server public key"))?;
    let cv = client
        .certificate_verify
        .as_ref()
        .ok_or_else(|| crate::stages::harness("no CertificateVerify"))?;
    let mut c = Check::new("which transcript hash the signature covers");
    c.block("certificate_verify", &client.certificate_verify_bytes);
    c.keying(
        &client.hash_after_certificate,
        "Transcript-Hash(CH..Certificate)",
    );
    c.note(
        "The signature is over the hash taken *after* Certificate and *before* \
         CertificateVerify itself — a message cannot sign its own bytes.",
    );
    c.that(
        "certificate_verify.signature over Transcript-Hash(CH..Certificate)",
        "verifies",
        crate::tls::sig::verify(
            key,
            cv.algorithm,
            &cv.signature,
            &client.hash_after_certificate,
        )
        .is_ok(),
        "does not verify",
    );
    c.that(
        "certificate_verify.signature over Transcript-Hash(CH..EncryptedExtensions)",
        "does not verify — the wrong boundary",
        crate::tls::sig::verify(
            key,
            cv.algorithm,
            &cv.signature,
            &client.hash_after_encrypted_extensions,
        )
        .is_err(),
        "verified, which would mean the transcript boundaries are indistinguishable",
    );
    c.finish()
});

tls_test!(finished_boundary, |ctx| {
    let client = ctx.handshake().await?;
    let schedule = client
        .schedule
        .as_ref()
        .ok_or_else(|| crate::stages::harness("no key schedule"))?;
    let keys = schedule
        .server_handshake
        .as_ref()
        .ok_or_else(|| crate::stages::harness("no server handshake keys"))?;
    let want = schedule
        .verify_data(keys, &client.hash_before_server_finished)
        .map_err(crate::assert::Failure::tls)?;
    let mut c = Check::new("which transcript hash the server's Finished covers");
    c.block("server finished", &client.server_finished_bytes);
    c.keying(
        &client.hash_before_server_finished,
        "s hs traffic → finished_key, then HMAC over the transcript",
    );
    c.bytes_eq("finished.verify_data", &want, &client.server_verify_data);
    let wrong = schedule
        .verify_data(keys, &client.hash_after_server_finished)
        .map_err(crate::assert::Failure::tls)?;
    c.ne(
        "verify_data over the hash that includes the Finished itself",
        hex(&want),
        hex(&wrong),
    );
    c.finish()
});

tls_test!(ccs_not_hashed, |ctx| {
    // One handshake with the client's compatibility CCS, one without. The transcripts have
    // to agree message for message — only the randoms differ, and those are seeded.
    let config = ctx.config();
    let with_ccs = {
        let client = ctx.handshake_with(config.clone().with_ccs(true)).await?;
        (
            client.client_hello_bytes.clone(),
            client.hash_after_client_hello.clone(),
            client.server_ccs_records,
        )
    };
    let without = {
        let client = ctx.handshake_with(config.with_ccs(false)).await?;
        (
            client.client_hello_bytes.clone(),
            client.hash_after_client_hello.clone(),
            client.server_ccs_records,
        )
    };
    let mut c = Check::new("that ChangeCipherSpec records stay out of the transcript");
    c.note(format!(
        "the server sent {} CCS record(s) in the first handshake and {} in the second",
        with_ccs.2, without.2
    ));
    c.note(
        "Both handshakes completed, which means both sides agreed on every hash — and one of \
         them had an extra CCS record on the wire.",
    );
    c.bytes_eq(
        "client_hello bytes (identical configuration)",
        &with_ccs.0,
        &without.0,
    );
    c.eq(
        "Transcript-Hash(ClientHello)",
        hex(&with_ccs.1),
        hex(&without.1),
    );
    c.finish()
});

tls_test!(rebuilt, |ctx| {
    let client = ctx.handshake().await?;
    let suite = client
        .suite
        .ok_or_else(|| crate::stages::harness("no suite"))?;
    let mut transcript = Transcript::new(suite.hash);
    for (label, bytes) in [
        ("client_hello", &client.client_hello_bytes),
        ("server_hello", &client.server_hello_bytes),
        ("encrypted_extensions", &client.encrypted_extensions_bytes),
        ("certificate", &client.certificate_bytes),
        ("certificate_verify", &client.certificate_verify_bytes),
        ("server finished", &client.server_finished_bytes),
        ("client finished", &client.client_finished_bytes),
    ] {
        if !bytes.is_empty() {
            transcript.push(bytes, label);
        }
    }
    let mut c = Check::new("a transcript rebuilt from the messages alone");
    c.note_all(client.transcript_lines());
    c.note(
        "Rebuilt by concatenating the seven handshake messages, with no record framing \
         anywhere in sight.",
    );
    c.bytes_eq(
        "Transcript-Hash(everything)",
        &transcript.current(),
        &client.hash_after_client_finished,
    );
    c.finish()
});

fn examples() -> Vec<ExampleSpec> {
    vec![
        ExampleSpec::text("Which hash is used where")
            .request(
                "CH..SH                 -> client/server handshake traffic secrets\n\
                 CH..Certificate        -> the CertificateVerify signature\n\
                 CH..CertificateVerify  -> the server's Finished\n\
                 CH..server Finished    -> the application traffic secrets and the client's Finished\n\
                 CH..client Finished    -> resumption_master_secret",
            )
            .response(
                "Five snapshots of one running hash, taken in order. Nothing is ever hashed \
                 twice and nothing is ever removed.",
            )
            .note(
                "Take the snapshot before adding the message that consumes it. Adding the \
                 Finished and then computing its own verify_data is the classic off-by-one \
                 here, and it fails with an opaque 'bad Finished'.",
            ),
        ExampleSpec::text("What never goes in")
            .request(
                "Record headers; ChangeCipherSpec records; the AEAD tag and padding; the \
                 sequence numbers; anything read off the socket that is not a handshake \
                 message body",
            )
            .response(
                "Only `msg_type || uint24 length || body`, in the order the messages were \
                 sent, reassembled across records where they were fragmented.",
            )
            .note(
                "A handy check while building: hash the messages you *sent and received* at \
                 the message layer, never the bytes you read at the socket layer.",
            ),
    ]
}
