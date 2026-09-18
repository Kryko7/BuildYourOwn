//! Stage 35 — A tampered ciphertext is `bad_record_mac`, and the connection ends.

use crate::assert::{Check, Failure};
use crate::examples::ExampleSpec;
use crate::stages::{check_refused_with, Stage, Test};
use crate::tls::client::Client;
use crate::tls::{AlertDescription, ContentType};
use crate::tls_test;

/// Stage definition.
pub fn stage() -> Stage {
    Stage {
        number: 35,
        slug: "bad_record_mac",
        name: "A tampered record is bad_record_mac",
        ext: false,
        hints: &[
            "A record whose AEAD tag does not verify is bad_record_mac(20), fatal, and the \
             connection is over — there is no retry and no resynchronisation",
            "The additional data is the record header, so changing the length or the type \
             byte breaks the tag just as surely as changing the ciphertext",
            "Do not leak *why* it failed: a bad tag, a bad length and a bad type must all \
             produce the same alert at the same moment",
            "The alert goes out under the keys that are still in force; the connection is \
             closed straight after it",
        ],
        examples,
        tests: vec![
            Test::new(
                "a flipped bit in the ciphertext is bad_record_mac",
                flipped_ciphertext,
            ),
            Test::new("a flipped bit in the tag is bad_record_mac", flipped_tag),
            Test::new(
                "a record whose header length was changed is bad_record_mac",
                changed_length,
            ),
            Test::new(
                "a record replayed a second time is bad_record_mac",
                replayed_record,
            ),
            Test::new(
                "a record sealed with the wrong key is bad_record_mac",
                wrong_key,
            ),
            Test::new(
                "the connection is over afterwards, not merely unhappy",
                connection_ends,
            ),
            Test::new("the server still serves a new connection", still_serving),
        ],
    }
}

/// Complete a handshake, echo one line so the data path is known good, then write `bytes`
/// verbatim and see what the server does.
async fn tamper(
    ctx: &mut crate::stages::Ctx,
    what: &str,
    make: impl FnOnce(&mut Client) -> Result<Vec<u8>, Failure>,
) -> Result<(), Failure> {
    let mut client = ctx.handshake().await?;
    let answer = client
        .echo_line("good")
        .await
        .map_err(|e| crate::stages::handshake_failure(e, &client))?;
    if answer != "doog" {
        return Err(crate::stages::harness(format!(
            "the data path was already wrong before the tampering: got {answer:?}"
        )));
    }
    let bytes = make(&mut client)?;
    client.conn.write_raw(&bytes).await.map_err(Failure::tls)?;
    let reaction = crate::stages::reaction_past_tickets(&mut client.conn).await;
    let mut c = Check::new(format!("what the server does with {what}"));
    c.block("the record that was sent", &bytes);
    c.note(
        "RFC 8446 section 5.2: 'If the decryption fails, the receiver MUST terminate the \
         connection with a bad_record_mac alert'.",
    );
    check_refused_with(
        &mut c,
        "the server's reaction",
        &reaction,
        &[
            AlertDescription::BAD_RECORD_MAC,
            AlertDescription::DECRYPT_ERROR,
            AlertDescription::UNEXPECTED_MESSAGE,
        ],
    );
    c.finish()?;
    drop(client);
    ctx.expect_still_serving(what).await
}

/// Seal `content` as an application-data record and hand back the bytes.
fn seal(client: &mut Client, content: &[u8]) -> Result<Vec<u8>, Failure> {
    client
        .conn
        .layer
        .seal(ContentType::ApplicationData, content, 0)
        .map_err(Failure::tls)
}

tls_test!(flipped_ciphertext, |ctx| {
    tamper(ctx, "a record with one ciphertext bit flipped", |client| {
        let mut bytes = seal(client, b"tampered\n")?;
        bytes[7] ^= 0x01;
        Ok(bytes)
    })
    .await
});

tls_test!(flipped_tag, |ctx| {
    tamper(ctx, "a record with one tag bit flipped", |client| {
        let mut bytes = seal(client, b"tampered\n")?;
        let last = bytes.len() - 1;
        bytes[last] ^= 0x80;
        Ok(bytes)
    })
    .await
});

tls_test!(changed_length, |ctx| {
    tamper(
        ctx,
        "a record whose header length no longer matches the tag",
        |client| {
            // Seal a longer payload, then claim a shorter one and truncate to match: the
            // bytes are internally consistent, but the additional data has changed.
            let mut bytes = seal(client, b"a longer line to trim\n")?;
            let new_len = bytes.len() - 5 - 4;
            bytes[3] = (new_len >> 8) as u8;
            bytes[4] = new_len as u8;
            bytes.truncate(5 + new_len);
            Ok(bytes)
        },
    )
    .await
});

tls_test!(replayed_record, |ctx| {
    let mut client = ctx.handshake().await?;
    // Capture the bytes of a legitimate record, then send them a second time.
    let record = seal(&mut client, b"replay\n")?;
    client.conn.write_raw(&record).await.map_err(Failure::tls)?;
    let answer = client
        .read_line()
        .await
        .map_err(|e| crate::stages::handshake_failure(e, &client))?;
    client.conn.write_raw(&record).await.map_err(Failure::tls)?;
    let reaction = crate::stages::reaction_past_tickets(&mut client.conn).await;
    let mut c = Check::new("the same record sent twice");
    c.block("the record that was replayed", &record);
    c.eq("the first copy's echo", "yalper".to_string(), answer);
    c.note(
        "The replay is not detected by remembering anything: the second copy is opened with \
         the next sequence number, so the nonce is different and the tag simply fails.",
    );
    check_refused_with(
        &mut c,
        "the server's reaction",
        &reaction,
        &[
            AlertDescription::BAD_RECORD_MAC,
            AlertDescription::DECRYPT_ERROR,
            AlertDescription::UNEXPECTED_MESSAGE,
        ],
    );
    c.finish()?;
    drop(client);
    ctx.expect_still_serving("a replayed record").await
});

tls_test!(wrong_key, |ctx| {
    tamper(ctx, "a record sealed with the wrong key", |client| {
        let keys = client
            .conn
            .layer
            .write
            .clone()
            .ok_or_else(|| crate::stages::harness("no client write keys"))?;
        let mut wrong = keys.clone();
        wrong.key[0] ^= 0xff;
        let nonce = wrong.nonce(client.conn.layer.write_seq);
        let inner = crate::tls::record::InnerPlaintext::build(
            ContentType::ApplicationData,
            b"wrong key\n",
            0,
        );
        let length = inner.len() + wrong.aead.tag_len();
        let aad = [23u8, 0x03, 0x03, (length >> 8) as u8, length as u8];
        let sealed = wrong
            .aead
            .seal(&wrong.key, &nonce, &aad, &inner)
            .map_err(Failure::tls)?;
        client.conn.layer.write_seq += 1;
        let mut bytes = aad.to_vec();
        bytes.extend_from_slice(&sealed);
        Ok(bytes)
    })
    .await
});

tls_test!(connection_ends, |ctx| {
    let mut client = ctx.handshake().await?;
    client
        .echo_line("before")
        .await
        .map_err(|e| crate::stages::handshake_failure(e, &client))?;
    let mut bytes = seal(&mut client, b"broken\n")?;
    bytes[6] ^= 0x40;
    client.conn.write_raw(&bytes).await.map_err(Failure::tls)?;
    let reaction = crate::stages::reaction_past_tickets(&mut client.conn).await;
    // Now try to keep using the connection: it must be finished.
    let after = client
        .conn
        .write_record(ContentType::ApplicationData, b"after\n")
        .await;
    let follow_up = crate::stages::reaction_past_tickets(&mut client.conn).await;
    let mut c = Check::new("whether the connection survives a bad record");
    c.note(format!(
        "the tampered record earned: {}",
        reaction.describe()
    ));
    c.note(
        "RFC 8446 section 5.2 makes this fatal: there is no way to resynchronise a record \
         stream whose sequence numbers may now disagree.",
    );
    c.that(
        "the connection after a bad record",
        "closed, or refusing to carry anything further",
        after.is_err() || !matches!(follow_up, crate::tls::conn::Reaction::Message(_)),
        format!(
            "a further record was accepted and answered: {}",
            follow_up.describe()
        ),
    );
    c.finish()?;
    drop(client);
    ctx.expect_still_serving("a fatal bad_record_mac").await
});

tls_test!(still_serving, |ctx| {
    let mut client = ctx.handshake().await?;
    let mut bytes = seal(&mut client, b"noise\n")?;
    let at = bytes.len() / 2;
    bytes[at] ^= 0xff;
    client.conn.write_raw(&bytes).await.map_err(Failure::tls)?;
    let _ = crate::stages::reaction_past_tickets(&mut client.conn).await;
    drop(client);
    let mut fresh = ctx.handshake_with(ctx.config_n(7)).await?;
    let answer = fresh
        .echo_line("recovered")
        .await
        .map_err(|e| crate::stages::handshake_failure(e, &fresh))?;
    let mut c = Check::new("a new connection after a fatal alert");
    c.eq("echo", "derevocer".to_string(), answer);
    c.finish()
});

fn examples() -> Vec<ExampleSpec> {
    vec![
        ExampleSpec::echo("A record that is not tampered with, for comparison", "good")
            .request("`good\\n` sealed with the client's application keys")
            .response("`doog\\n` back. Flip any bit of that ciphertext and this stops happening.")
            .note(
                "AEAD is all-or-nothing: one changed bit anywhere in the ciphertext, the tag, \
                 or the five header bytes that make up the additional data, and decryption \
                 fails outright. There is no partially valid record.",
            ),
        ExampleSpec::text("What must not differ")
            .request(
                "A flipped ciphertext bit; a flipped tag bit; a header length that no longer \
                 matches; a record replayed at the wrong sequence number.",
            )
            .response(
                "All four: bad_record_mac(20), fatal, connection closed. Same alert, same \
                 moment, no extra information.",
            )
            .note(
                "Distinguishing them would be an oracle. The reason TLS 1.3 has one alert here \
                 rather than several is a decade of padding-oracle attacks on the versions \
                 that had more.",
            ),
    ]
}
