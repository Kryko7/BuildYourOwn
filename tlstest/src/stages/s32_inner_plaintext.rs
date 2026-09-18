//! Stage 32 — `TLSInnerPlaintext`: the hidden content type and the padding.

use crate::assert::{Check, Failure};
use crate::examples::ExampleSpec;
use crate::stages::{check_refused_with, reversed, Stage, Test};
use crate::tls::record::InnerPlaintext;
use crate::tls::{AlertDescription, ContentType};
use crate::tls_test;
use std::time::Duration;

/// Stage definition.
pub fn stage() -> Stage {
    Stage {
        number: 32,
        slug: "inner_plaintext",
        name: "TLSInnerPlaintext and padding",
        ext: false,
        hints: &[
            "Inside an encrypted record the plaintext is `content || content_type || zeros`; \
             the outer header always says application_data(23)",
            "To find the real type, scan back from the end past every zero byte — the first \
             non-zero byte is it",
            "Any amount of zero padding is legal, including none, and a receiver must accept \
             whatever it is sent without reading anything into it",
            "A decrypted record that is entirely zeros has no content type at all: that is \
             unexpected_message(10), not a record of type zero",
        ],
        examples,
        tests: vec![
            Test::new("a record with no padding is accepted", no_padding),
            Test::new(
                "a record with 16 bytes of padding is accepted",
                some_padding,
            ),
            Test::new(
                "a record with 1000 bytes of padding is accepted",
                lots_of_padding,
            ),
            Test::new(
                "padding changes the record length but not the answer",
                padding_is_invisible,
            ),
            Test::new(
                "the server's own records carry a valid inner content type",
                server_inner_type,
            ),
            Test::new("an all-zero decrypted record is refused", all_zero_record),
            Test::new(
                "a record whose inner type is a type TLS does not define is refused",
                undefined_inner_type,
            ),
        ],
    }
}

/// Send one line with `padding` zero bytes inside the record, and read the answer.
async fn echo_with_padding(
    ctx: &crate::stages::Ctx,
    line: &str,
    padding: usize,
) -> Result<(String, usize), Failure> {
    let mut client = ctx.handshake().await?;
    let before = client.conn.layer.write_seq;
    client
        .conn
        .write_record_padded(
            ContentType::ApplicationData,
            format!("{line}\n").as_bytes(),
            padding,
        )
        .await
        .map_err(Failure::tls)?;
    let answer = client.read_line().await.map_err(|e| {
        crate::stages::handshake_failure(e, &client).note(format!(
            "after a record carrying {padding} bytes of padding"
        ))
    })?;
    Ok((answer, (client.conn.layer.write_seq - before) as usize))
}

tls_test!(no_padding, |ctx| {
    let (answer, records) = echo_with_padding(ctx, "nopad", 0).await?;
    let mut c = Check::new("a record with no padding at all");
    c.note("plaintext = `nopad\\n` || 0x17 — the content type and nothing after it");
    c.eq("echo", "dapon".to_string(), answer);
    c.eq("records written", 1usize, records);
    c.finish()
});

tls_test!(some_padding, |ctx| {
    let (answer, _) = echo_with_padding(ctx, "pad16", 16).await?;
    let mut c = Check::new("a record with sixteen zero bytes of padding");
    c.note("plaintext = `pad16\\n` || 0x17 || 00 × 16");
    c.eq("echo", "61dap".to_string(), answer);
    c.finish()
});

tls_test!(lots_of_padding, |ctx| {
    let (answer, _) = echo_with_padding(ctx, "padlots", 1000).await?;
    let mut c = Check::new("a record with a kilobyte of padding");
    c.note(
        "Padding is how TLS 1.3 hides message lengths. A thousand zero bytes around seven of \
         content is unusual but entirely legal, and a receiver must not flinch.",
    );
    c.eq("echo", "stoldap".to_string(), answer);
    c.finish()
});

tls_test!(padding_is_invisible, |ctx| {
    let mut client = ctx.handshake().await?;
    let mut lengths = Vec::new();
    let mut answers = Vec::new();
    for padding in [0usize, 8, 64] {
        client
            .conn
            .write_record_padded(ContentType::ApplicationData, b"same\n", padding)
            .await
            .map_err(Failure::tls)?;
        let answer = client
            .read_line()
            .await
            .map_err(|e| crate::stages::handshake_failure(e, &client))?;
        answers.push(answer);
        lengths.push(padding);
    }
    let mut c = Check::new("the same line with three different amounts of padding");
    c.note(
        "The three records had different lengths on the wire and identical content. That is \
         the entire point of the feature.",
    );
    for (i, answer) in answers.iter().enumerate() {
        c.eq(
            &format!("echo with {} bytes of padding", lengths[i]),
            "emas".to_string(),
            answer.clone(),
        );
    }
    c.finish()
});

tls_test!(server_inner_type, |ctx| {
    let mut client = ctx.handshake().await?;
    let answer = client
        .echo_line("inner")
        .await
        .map_err(|e| crate::stages::handshake_failure(e, &client))?;
    // Re-open the server's last record by hand to look at its inner plaintext.
    let record = client
        .conn
        .records_in
        .iter()
        .rev()
        .find(|r| r.content_type == ContentType::ApplicationData)
        .cloned()
        .ok_or_else(|| crate::stages::harness("no application record came back"))?;
    let keys = client
        .schedule
        .as_ref()
        .and_then(|s| s.server_application.clone())
        .ok_or_else(|| crate::stages::harness("no server application keys"))?;
    let seq = client.conn.layer.read_seq.saturating_sub(1);
    let mut layer = crate::tls::record::RecordLayer::new();
    layer.set_read(keys);
    layer.read_seq = seq;
    let inner = layer
        .open(&record)
        .map_err(|e| Failure::tls(e).note("re-opening the server's answer by hand"))?;
    let mut c = Check::new("the inner plaintext of the server's answer");
    c.block("the record as it arrived", &record.raw);
    c.block("what it decrypted to", &inner.content);
    c.eq("echo", "renni".to_string(), answer);
    c.eq("record.type (outer)", 23u8, record.content_type.as_u8());
    c.eq(
        "TLSInnerPlaintext.type",
        ContentType::ApplicationData.as_u8(),
        inner.content_type.as_u8(),
    );
    c.observe("TLSInnerPlaintext.padding", inner.padding);
    c.eq(
        "TLSInnerPlaintext.content",
        "renni\n".to_string(),
        String::from_utf8_lossy(&inner.content).to_string(),
    );
    c.finish()
});

tls_test!(all_zero_record, |ctx| {
    let mut client = ctx.handshake().await?;
    // Seal a plaintext that is nothing but zeros: there is no content type byte in it.
    let keys = client
        .conn
        .layer
        .write
        .clone()
        .ok_or_else(|| crate::stages::harness("no client write keys"))?;
    let nonce = keys.nonce(client.conn.layer.write_seq);
    let inner = vec![0u8; 16];
    let length = inner.len() + keys.aead.tag_len();
    let aad = [23u8, 0x03, 0x03, (length >> 8) as u8, length as u8];
    let sealed = keys
        .aead
        .seal(&keys.key, &nonce, &aad, &inner)
        .map_err(Failure::tls)?;
    let mut bytes = aad.to_vec();
    bytes.extend_from_slice(&sealed);
    client.conn.layer.write_seq += 1;
    client.conn.write_raw(&bytes).await.map_err(Failure::tls)?;
    let reaction = crate::stages::reaction_past_tickets(&mut client.conn).await;
    let mut c = Check::new("a record that decrypts to nothing but zeros");
    c.block("the record that was sent", &bytes);
    c.note(
        "RFC 8446 section 5.4: 'If a receiving implementation does not find a non-zero octet \
         in the cleartext, it MUST terminate the connection with an unexpected_message alert'.",
    );
    check_refused_with(
        &mut c,
        "the server's reaction",
        &reaction,
        &[
            AlertDescription::UNEXPECTED_MESSAGE,
            AlertDescription::DECODE_ERROR,
            AlertDescription::BAD_RECORD_MAC,
            AlertDescription::ILLEGAL_PARAMETER,
        ],
    );
    c.finish()?;
    drop(client);
    ctx.expect_still_serving("an all-zero record").await
});

tls_test!(undefined_inner_type, |ctx| {
    let mut client = ctx.handshake().await?;
    client
        .conn
        .write_record_padded(ContentType::Other(99), b"what type is this", 4)
        .await
        .map_err(Failure::tls)?;
    let reaction = crate::stages::reaction_past_tickets(&mut client.conn).await;
    let mut c = Check::new("a record whose inner content type is 99");
    c.note(
        "The record decrypts perfectly; it is the type byte inside it that TLS does not \
         define. RFC 8446 section 5.4 makes that unexpected_message(10).",
    );
    check_refused_with(
        &mut c,
        "the server's reaction",
        &reaction,
        &[
            AlertDescription::UNEXPECTED_MESSAGE,
            AlertDescription::DECODE_ERROR,
            AlertDescription::ILLEGAL_PARAMETER,
        ],
    );
    c.finish()?;
    drop(client);
    ctx.expect_still_serving("a record with an undefined inner content type")
        .await
});

fn examples() -> Vec<ExampleSpec> {
    let _ = (
        InnerPlaintext::build(ContentType::Alert, &[], 0),
        Duration::from_millis(0),
        reversed("x"),
    );
    vec![
        ExampleSpec::echo("What is really inside an encrypted record", "pad")
            .request(
                "`pad\\n` as application data. Inside the AEAD the plaintext is `pad\\n` \
                 followed by the byte 0x17, and then as many zero bytes as the sender likes.",
            )
            .response(
                "The answer's plaintext is `dap\\n` || 0x17, with whatever padding the server \
                 chose. Both records say application_data(23) in the clear.",
            )
            .note(
                "The outer content type of every protected record is 23, always — even for a \
                 handshake message or an alert. The real type is the last non-zero byte of the \
                 decrypted fragment.",
            ),
        ExampleSpec::text("Finding the content type")
            .request(
                "decrypted = 61 62 63 0a 17 00 00 00 00\n\
                 Scan back from the end past the zeros.",
            )
            .response(
                "The first non-zero byte is 0x17 at index 4: content is `abc\\n`, type is \
                 application_data(23), padding is four bytes.",
            )
            .note(
                "Scan, do not assume. A fixed `plaintext[len-1]` works right up until the \
                 first peer that pads — and TLS 1.3 pads to hide traffic patterns, so that \
                 peer will turn up.",
            ),
    ]
}
