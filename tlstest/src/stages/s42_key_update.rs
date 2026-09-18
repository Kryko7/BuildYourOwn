//! Stage 42 — KeyUpdate in both directions.

use crate::assert::{Check, Failure};
use crate::examples::ExampleSpec;
use crate::stages::{check_refused_with, reversed, Stage, Test};
use crate::tls::msg::{encode_handshake, KeyUpdate};
use crate::tls::{hex, AlertDescription, ContentType, HandshakeType};
use crate::tls_test;

/// Stage definition.
pub fn stage() -> Stage {
    Stage {
        number: 42,
        slug: "key_update",
        name: "KeyUpdate",
        ext: true,
        hints: &[
            "The body is one byte: update_not_requested(0) or update_requested(1)",
            "The new secret is HKDF-Expand-Label(old secret, \"traffic upd\", \"\", \
             Hash.length); key and iv are expanded from it exactly as before",
            "Only the sender's own direction changes, and the sequence number goes back to \
             zero; the KeyUpdate itself is the last record under the old keys",
            "update_requested(1) obliges the peer to send its own KeyUpdate back — and that \
             answer must say update_not_requested, or two peers would ping-pong for ever",
        ],
        examples,
        tests: vec![
            Test::new(
                "data keeps flowing after update_not_requested",
                not_requested,
            ),
            Test::new("update_requested earns a KeyUpdate back", requested),
            Test::new("the write keys really change", keys_change),
            Test::new(
                "the sequence number resets on the updated direction",
                sequence_resets,
            ),
            Test::new("several updates in a row all work", repeated_updates),
            Test::new(
                "a KeyUpdate with an undefined request_update is refused",
                bad_request_update,
            ),
            Test::new(
                "a KeyUpdate with a body of the wrong length is refused",
                bad_length,
            ),
        ],
    }
}

tls_test!(not_requested, |ctx| {
    let mut client = ctx.handshake().await?;
    let before = client
        .echo_line("before")
        .await
        .map_err(|e| crate::stages::handshake_failure(e, &client))?;
    client
        .send_key_update(KeyUpdate::NOT_REQUESTED)
        .await
        .map_err(Failure::tls)?;
    let after = client.echo_line("after").await.map_err(|e| {
        crate::stages::handshake_failure(e, &client)
            .note("the client rotated its own write keys and kept writing")
    })?;
    let mut c = Check::new("data across an update_not_requested");
    c.note(
        "Only the client's write direction changed. The server's answers still come back \
         under the keys it has been using all along.",
    );
    c.eq("echo before the update", reversed("before"), before);
    c.eq("echo after the update", reversed("after"), after);
    c.finish()
});

tls_test!(requested, |ctx| {
    let mut client = ctx.handshake().await?;
    client
        .echo_line("pre")
        .await
        .map_err(|e| crate::stages::handshake_failure(e, &client))?;
    let secret_before = client
        .schedule
        .as_ref()
        .and_then(|s| s.server_application.as_ref().map(|k| k.secret.clone()))
        .unwrap_or_default();
    client
        .send_key_update(KeyUpdate::REQUESTED)
        .await
        .map_err(Failure::tls)?;
    // The echo forces the server's answer to arrive, and with it the KeyUpdate it owes us.
    let answer = client.echo_line("post").await.map_err(|e| {
        crate::stages::handshake_failure(e, &client)
            .note("after the client asked the server to update its keys too")
    })?;
    let secret_after = client
        .schedule
        .as_ref()
        .and_then(|s| s.server_application.as_ref().map(|k| k.secret.clone()))
        .unwrap_or_default();
    let mut c = Check::new("an update_requested and the answer to it");
    c.note(
        "RFC 8446 section 4.6.3: a peer that receives update_requested must answer with its \
         own KeyUpdate — and that one must say update_not_requested.",
    );
    c.eq("echo after the exchange", reversed("post"), answer);
    c.ne(
        "server_application_traffic_secret",
        hex(&secret_before),
        hex(&secret_after),
    );
    c.finish()
});

tls_test!(keys_change, |ctx| {
    let mut client = ctx.handshake().await?;
    let before = client
        .conn
        .layer
        .write
        .as_ref()
        .map(|k| (k.secret.clone(), k.key.clone(), k.iv.clone()))
        .ok_or_else(|| crate::stages::harness("no client write keys"))?;
    client
        .send_key_update(KeyUpdate::NOT_REQUESTED)
        .await
        .map_err(Failure::tls)?;
    let after = client
        .conn
        .layer
        .write
        .as_ref()
        .map(|k| (k.secret.clone(), k.key.clone(), k.iv.clone()))
        .ok_or_else(|| crate::stages::harness("no client write keys"))?;
    // And the derivation is the one the RFC names.
    let hash = client
        .suite
        .ok_or_else(|| crate::stages::harness("no suite"))?
        .hash;
    let expected =
        crate::tls::crypto::hkdf_expand_label(hash, &before.0, "traffic upd", &[], hash.len())
            .map_err(Failure::tls)?;
    let answer = client
        .echo_line("rotated")
        .await
        .map_err(|e| crate::stages::handshake_failure(e, &client))?;
    let mut c = Check::new("the new keys after a KeyUpdate");
    c.note("application_traffic_secret_N+1 = HKDF-Expand-Label(secret_N, \"traffic upd\", \"\", Hash.length)");
    c.bytes_eq("the new traffic secret", &expected, &after.0);
    c.ne("client_write_key", hex(&before.1), hex(&after.1));
    c.ne("client_write_iv", hex(&before.2), hex(&after.2));
    c.eq("echo under the new keys", reversed("rotated"), answer);
    c.finish()
});

tls_test!(sequence_resets, |ctx| {
    let mut client = ctx.handshake().await?;
    for i in 0..3 {
        client
            .echo_line(&format!("line{i}"))
            .await
            .map_err(|e| crate::stages::handshake_failure(e, &client))?;
    }
    let before = client.conn.layer.write_seq;
    client
        .send_key_update(KeyUpdate::NOT_REQUESTED)
        .await
        .map_err(Failure::tls)?;
    let after = client.conn.layer.write_seq;
    let answer = client
        .echo_line("reset")
        .await
        .map_err(|e| crate::stages::handshake_failure(e, &client))?;
    let mut c = Check::new("the sequence number across a KeyUpdate");
    c.at_least("write sequence number before the update", 3u64, before);
    c.note(
        "The KeyUpdate goes out under the old keys, at the next sequence number; the new keys \
         then start again at zero.",
    );
    c.eq("write sequence number after the update", 0u64, after);
    c.eq(
        "write sequence number after one more record",
        1u64,
        client.conn.layer.write_seq,
    );
    c.eq("echo", reversed("reset"), answer);
    c.finish()
});

tls_test!(repeated_updates, |ctx| {
    let mut client = ctx.handshake().await?;
    let mut c = Check::new("five KeyUpdates in a row");
    for i in 0..5 {
        client
            .send_key_update(KeyUpdate::NOT_REQUESTED)
            .await
            .map_err(Failure::tls)?;
        let line = format!("gen{i}");
        let answer = client.echo_line(&line).await.map_err(|e| {
            crate::stages::handshake_failure(e, &client)
                .note(format!("after key generation {}", i + 1))
        })?;
        c.eq(
            &format!("echo under generation {}", i + 1),
            reversed(&line),
            answer,
        );
    }
    c.note(
        "Each generation is expanded from the one before it, so a peer that missed an update \
         cannot catch up — which is why the message is mandatory rather than advisory.",
    );
    c.finish()
});

tls_test!(bad_request_update, |ctx| {
    let mut client = ctx.handshake().await?;
    client
        .echo_line("before")
        .await
        .map_err(|e| crate::stages::handshake_failure(e, &client))?;
    // request_update = 2 is not a value RFC 8446 defines.
    client
        .conn
        .write_record(
            ContentType::Handshake,
            &encode_handshake(HandshakeType::KEY_UPDATE, &[2]),
        )
        .await
        .map_err(Failure::tls)?;
    let reaction = crate::stages::reaction_past_tickets(&mut client.conn).await;
    let mut c = Check::new("a KeyUpdate whose request_update is 2");
    c.note(
        "RFC 8446 section 4.6.3: 'If an implementation receives any other value, it MUST \
         terminate the connection with an illegal_parameter alert'.",
    );
    check_refused_with(
        &mut c,
        "the server's reaction",
        &reaction,
        &[
            AlertDescription::ILLEGAL_PARAMETER,
            AlertDescription::DECODE_ERROR,
            AlertDescription::UNEXPECTED_MESSAGE,
        ],
    );
    c.finish()?;
    drop(client);
    ctx.expect_still_serving("a KeyUpdate with an undefined request_update")
        .await
});

tls_test!(bad_length, |ctx| {
    let mut client = ctx.handshake().await?;
    client
        .echo_line("before")
        .await
        .map_err(|e| crate::stages::handshake_failure(e, &client))?;
    client
        .conn
        .write_record(
            ContentType::Handshake,
            &encode_handshake(HandshakeType::KEY_UPDATE, &[0, 0, 0]),
        )
        .await
        .map_err(Failure::tls)?;
    let reaction = crate::stages::reaction_past_tickets(&mut client.conn).await;
    let mut c = Check::new("a KeyUpdate with a three-byte body");
    c.note("The body is one byte. A message of any other length does not decode.");
    check_refused_with(
        &mut c,
        "the server's reaction",
        &reaction,
        &[
            AlertDescription::DECODE_ERROR,
            AlertDescription::ILLEGAL_PARAMETER,
            AlertDescription::UNEXPECTED_MESSAGE,
        ],
    );
    c.finish()?;
    drop(client);
    ctx.expect_still_serving("a malformed KeyUpdate").await
});

fn examples() -> Vec<ExampleSpec> {
    vec![
        ExampleSpec::echo("A line before the keys rotate", "rotate")
            .request("`rotate\\n` under application_traffic_secret_0")
            .response(
                "`etator\\n` back. A `18 00 00 01 00` KeyUpdate follows, and then the same \
                 exchange works under secret_1.",
            )
            .note(
                "The KeyUpdate message is five bytes: type 24, a uint24 length of 1, and the \
                 request_update byte. Everything else about it is derivation.",
            ),
        ExampleSpec::text("What a KeyUpdate changes, and what it does not")
            .request(
                "secret_(N+1) = HKDF-Expand-Label(secret_N, \"traffic upd\", \"\", Hash.length)\n\
                 key, iv      = HKDF-Expand-Label(secret_(N+1), \"key\"/\"iv\", \"\", ...)\n\
                 sequence number -> 0",
            )
            .response(
                "Only the sender's direction moves. The transcript is untouched, the master \
                 secret is untouched, and no new key exchange happens.",
            )
            .note(
                "update_requested(1) asks the peer to rotate too, and its answer must say \
                 update_not_requested(0). Two peers that both say 1 would never stop.",
            ),
    ]
}
