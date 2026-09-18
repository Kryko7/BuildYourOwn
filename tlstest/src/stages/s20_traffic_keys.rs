//! Stage 20 — Traffic keys, IVs and the per-record nonce.

use crate::assert::Check;
use crate::examples::ExampleSpec;
use crate::stages::{Stage, Test};
use crate::tls::crypto::{hkdf_expand_label, TrafficKeys};
use crate::tls::record::{InnerPlaintext, RecordLayer};
use crate::tls::{hex, ContentType, ALL_SUITES};
use crate::tls_test;

/// Stage definition.
pub fn stage() -> Stage {
    Stage {
        number: 20,
        slug: "traffic_keys",
        name: "Traffic keys, IVs and the record nonce",
        ext: false,
        hints: &[
            "key = HKDF-Expand-Label(secret, \"key\", \"\", key_length) and iv = \
             HKDF-Expand-Label(secret, \"iv\", \"\", 12); the context is empty for both",
            "The nonce is the static IV with the 64-bit sequence number XORed into its right \
             end — the number itself is never sent",
            "The AEAD's additional data is the five bytes of the record header, exactly as \
             written, with the *ciphertext* length in it",
            "Every TLS 1.3 AEAD uses a 12-byte nonce and a 16-byte tag; only the key length \
             changes between suites",
        ],
        examples: examples,
        tests: vec![
            Test::new(
                "key and iv have the lengths the suite dictates",
                key_lengths,
            ),
            Test::new(
                "key and iv are HKDF-Expand-Label of the traffic secret",
                derived_from_secret,
            ),
            Test::new(
                "the nonce is the IV XOR the sequence number",
                nonce_rule,
            ),
            Test::new(
                "the server's first encrypted record opens with the derived keys",
                opens_first_record,
            ),
            Test::new(
                "the additional data is the record header with the ciphertext length",
                additional_data,
            ),
            Test::new(
                "the client's and server's keys are different",
                directions_differ,
            ),
            Test::new(
                "every suite's keys open that suite's records",
                all_suites,
            ),
        ],
    }
}

tls_test!(key_lengths, |ctx| {
    for suite in ALL_SUITES {
        let client = ctx.handshake_with(ctx.config().with_suites(&[suite])).await?;
        let schedule = client
            .schedule
            .as_ref()
            .ok_or_else(|| crate::stages::harness("no key schedule"))?;
        let keys = schedule
            .server_handshake
            .as_ref()
            .ok_or_else(|| crate::stages::harness("no server handshake keys"))?;
        let mut c = Check::new(format!(
            "key and iv lengths under {}",
            crate::tls::suite_name(suite)
        ));
        c.note(format!(
            "{} uses {}",
            crate::tls::suite_name(suite),
            schedule.suite.aead.name()
        ));
        c.eq(
            "server_write_key length",
            schedule.suite.aead.key_len(),
            keys.key.len(),
        );
        c.eq("server_write_iv length", 12usize, keys.iv.len());
        c.eq("AEAD tag length", 16usize, schedule.suite.aead.tag_len());
        c.finish()?;
    }
    Ok(())
});

tls_test!(derived_from_secret, |ctx| {
    let client = ctx.handshake().await?;
    let schedule = client
        .schedule
        .as_ref()
        .ok_or_else(|| crate::stages::harness("no key schedule"))?;
    let keys = schedule
        .server_handshake
        .as_ref()
        .ok_or_else(|| crate::stages::harness("no server handshake keys"))?;
    let hash = schedule.suite.hash;
    let want_key = hkdf_expand_label(hash, &keys.secret, "key", &[], schedule.suite.aead.key_len())
        .map_err(crate::assert::Failure::tls)?;
    let want_iv = hkdf_expand_label(hash, &keys.secret, "iv", &[], 12)
        .map_err(crate::assert::Failure::tls)?;
    let mut c = Check::new("key and iv as HKDF-Expand-Label of the traffic secret");
    c.keying(&client.hash_after_server_hello, "s hs traffic → key / iv");
    c.note("The context argument is empty for both: the transcript is already baked into the secret.");
    c.bytes_eq("server_write_key", &want_key, &keys.key);
    c.bytes_eq("server_write_iv", &want_iv, &keys.iv);
    c.finish()
});

tls_test!(nonce_rule, |ctx| {
    let client = ctx.handshake().await?;
    let schedule = client
        .schedule
        .as_ref()
        .ok_or_else(|| crate::stages::harness("no key schedule"))?;
    let keys = schedule
        .server_handshake
        .as_ref()
        .ok_or_else(|| crate::stages::harness("no server handshake keys"))?;
    let mut c = Check::new("nonce = static_iv XOR sequence_number");
    c.note(format!("server_write_iv = {}", hex(&keys.iv)));
    c.bytes_eq("nonce for record 0", &keys.iv, &keys.nonce(0));
    let mut want = keys.iv.clone();
    want[11] ^= 1;
    c.bytes_eq("nonce for record 1", &want, &keys.nonce(1));
    let mut want = keys.iv.clone();
    want[10] ^= 1;
    c.bytes_eq("nonce for record 256", &want, &keys.nonce(256));
    c.note(
        "The sequence number is written as a 64-bit big-endian value right-aligned in the \
         12-byte IV, so only the last eight bytes ever change.",
    );
    c.finish()
});

tls_test!(opens_first_record, |ctx| {
    // Do the handshake up to the ServerHello with the driver, then open the very next
    // record by hand with keys derived here, and compare with what the driver read.
    let mut client = ctx.client().await?;
    client
        .send_client_hello()
        .await
        .map_err(crate::assert::Failure::tls)?;
    client.maybe_send_ccs().await.map_err(crate::assert::Failure::tls)?;
    if let Err(e) = client.read_server_hello().await {
        return Err(crate::stages::handshake_failure(e, &client));
    }
    let schedule = client
        .schedule
        .as_ref()
        .ok_or_else(|| crate::stages::harness("no key schedule"))?;
    let keys = schedule
        .server_handshake
        .clone()
        .ok_or_else(|| crate::stages::harness("no server handshake keys"))?;
    // Skip the compatibility ChangeCipherSpec, if one comes.
    let mut record = client
        .conn
        .read_record()
        .await
        .map_err(crate::assert::Failure::tls)?;
    while record.content_type == ContentType::ChangeCipherSpec {
        record = client
            .conn
            .read_record()
            .await
            .map_err(crate::assert::Failure::tls)?;
    }
    let mut layer = RecordLayer::new();
    layer.set_read(keys.clone());
    let opened = layer
        .open(&record)
        .map_err(|e| crate::assert::Failure::tls(e).note("opening the first encrypted record by hand"))?;
    let mut c = Check::new("the server's first encrypted record, opened by hand");
    c.block("the record as it arrived", &record.raw);
    c.block("what it decrypted to", &opened.content);
    c.keying(&client.hash_after_server_hello, "s hs traffic → key / iv");
    c.note(format!("nonce used: {}", hex(&keys.nonce(0))));
    c.eq("record.type (outer)", 23u8, record.content_type.as_u8());
    c.eq(
        "TLSInnerPlaintext.type",
        ContentType::Handshake.as_u8(),
        opened.content_type.as_u8(),
    );
    c.eq(
        "the first decrypted message",
        crate::tls::HandshakeType::ENCRYPTED_EXTENSIONS.0,
        opened.content.first().copied().unwrap_or(0),
    );
    c.finish()
});

tls_test!(additional_data, |ctx| {
    let client = ctx.handshake().await?;
    let record = client
        .conn
        .records_in
        .iter()
        .find(|r| r.content_type == ContentType::ApplicationData)
        .ok_or_else(|| crate::stages::harness("the server sent no encrypted record"))?;
    let aad = record.header();
    let mut c = Check::new("the AEAD additional data of an encrypted record");
    c.block("the record as it arrived", &record.raw);
    c.mark(0..5);
    c.note(
        "The additional data is the five header bytes verbatim, so the length in it is the \
         *ciphertext* length — content plus the type byte plus padding plus the 16-byte tag.",
    );
    c.eq("additional_data[0]", 23u8, aad[0]);
    c.eq("additional_data[1..3]", 0x0303u16, u16::from_be_bytes([aad[1], aad[2]]));
    c.eq(
        "additional_data[3..5]",
        record.fragment.len(),
        u16::from_be_bytes([aad[3], aad[4]]) as usize,
    );
    c.at_least(
        "the ciphertext length",
        17usize,
        record.fragment.len(),
        );
    c.finish()
});

tls_test!(directions_differ, |ctx| {
    let client = ctx.handshake().await?;
    let schedule = client
        .schedule
        .as_ref()
        .ok_or_else(|| crate::stages::harness("no key schedule"))?;
    let (c_keys, s_keys) = (
        schedule
            .client_handshake
            .as_ref()
            .ok_or_else(|| crate::stages::harness("no client handshake keys"))?,
        schedule
            .server_handshake
            .as_ref()
            .ok_or_else(|| crate::stages::harness("no server handshake keys"))?,
    );
    let mut c = Check::new("that the two directions have different keys");
    c.note(
        "One shared secret, two labels, four values: a key and an IV each way. Reusing one \
         key in both directions would let either side's records be replayed at the other.",
    );
    c.ne("server_write_key", hex(&c_keys.key), hex(&s_keys.key));
    c.ne("server_write_iv", hex(&c_keys.iv), hex(&s_keys.iv));
    let ap = (
        schedule.client_application.as_ref(),
        schedule.server_application.as_ref(),
    );
    if let (Some(ca), Some(sa)) = ap {
        c.ne(
            "client_application key vs client_handshake key",
            hex(&c_keys.key),
            hex(&ca.key),
        );
        c.ne("server_application key", hex(&ca.key), hex(&sa.key));
    }
    c.finish()
});

tls_test!(all_suites, |ctx| {
    for suite in ALL_SUITES {
        let mut client = ctx.handshake_with(ctx.config().with_suites(&[suite])).await?;
        let line = format!("keys{:x}", suite & 0xff);
        let answer = client.echo_line(&line).await.map_err(|e| {
            crate::stages::handshake_failure(e, &client)
                .note(format!("under {}", crate::tls::suite_name(suite)))
        })?;
        let mut c = Check::new(format!(
            "application records under {}",
            crate::tls::suite_name(suite)
        ));
        c.eq("echo", crate::stages::reversed(&line), answer);
        c.finish()?;
    }
    Ok(())
});

fn examples() -> Vec<ExampleSpec> {
    let _ = (TrafficKeys::finished_key, InnerPlaintext::parse);
    vec![
        ExampleSpec::text("From a traffic secret to a record")
            .request(
                "key = HKDF-Expand-Label(secret, \"key\", \"\", key_length)\n\
                 iv  = HKDF-Expand-Label(secret, \"iv\",  \"\", 12)\n\
                 nonce = iv XOR seq (right-aligned, 64-bit big-endian)\n\
                 aad = the five record header bytes, ciphertext length included",
            )
            .response(
                "ciphertext = AEAD-Encrypt(key, nonce, aad, content || content_type || zeros), \
                 and the record on the wire is `17 03 03 <len> <ciphertext||tag>`.",
            )
            .note(
                "Four inputs, and three of them are derived rather than sent. The sequence \
                 number is the only state either side keeps, and it resets to zero whenever \
                 the keys change.",
            ),
        ExampleSpec::text("Key and IV lengths per suite")
            .request(
                "TLS_AES_128_GCM_SHA256: 16-byte key, 12-byte IV, 16-byte tag\n\
                 TLS_AES_256_GCM_SHA384: 32-byte key, 12-byte IV, 16-byte tag\n\
                 TLS_CHACHA20_POLY1305_SHA256: 32-byte key, 12-byte IV, 16-byte tag",
            )
            .response(
                "Only the key length changes. The nonce is 12 bytes and the tag 16 bytes in \
                 every TLS 1.3 suite.",
            )
            .note(
                "The hash changes too, and that is the bigger difference: SHA-384 makes every \
                 secret, transcript hash and finished key 48 bytes instead of 32.",
            ),
    ]
}
