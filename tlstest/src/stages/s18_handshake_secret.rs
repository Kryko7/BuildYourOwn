//! Stage 18 — The key schedule: Early, Handshake and Master secrets.

use crate::assert::Check;
use crate::examples::ExampleSpec;
use crate::stages::{Stage, Test};
use crate::tls::crypto::{derive_secret, hkdf_extract, hkdf_label, HashAlg, KeySchedule};
use crate::tls_test;

/// Stage definition.
pub fn stage() -> Stage {
    Stage {
        number: 18,
        slug: "handshake_secret",
        name: "Early, Handshake and Master secrets",
        ext: false,
        hints: &[
            "Early Secret = HKDF-Extract(salt = Hash.length zeros, IKM = PSK or Hash.length \
             zeros); with no PSK both inputs are zeros and the result is still not zero",
            "Between Extract steps comes Derive-Secret(secret, \"derived\", \"\") — over the \
             hash of the *empty* transcript, not the handshake so far",
            "Handshake Secret = HKDF-Extract(salt = that derived value, IKM = the (EC)DHE \
             shared secret); Master Secret = HKDF-Extract(salt = derived again, IKM = zeros)",
            "HKDF-Expand-Label's info is uint16 length, then \"tls13 \" + label as an \
             opaque<7..255>, then the context as an opaque<0..255> — the six-byte prefix \
             includes the space",
        ],
        examples,
        tests: vec![
            Test::new(
                "the Early Secret is HKDF-Extract of two zero blocks",
                early_secret,
            ),
            Test::new(
                "the Handshake Secret extracts the (EC)DHE secret under the derived salt",
                handshake_secret,
            ),
            Test::new(
                "the Master Secret extracts zeros under the second derived salt",
                master_secret,
            ),
            Test::new(
                "the handshake traffic secrets use the labels the RFC names",
                traffic_labels,
            ),
            Test::new(
                "every derivation is over the hash length of the negotiated suite",
                secret_lengths,
            ),
            Test::new(
                "getting the label wrong produces a different secret",
                wrong_label,
            ),
            Test::new(
                "the whole schedule is the one that decrypts the server's flight",
                schedule_works,
            ),
        ],
    }
}

tls_test!(early_secret, |ctx| {
    let client = ctx.handshake().await?;
    let schedule = client
        .schedule
        .as_ref()
        .ok_or_else(|| crate::stages::harness("no key schedule"))?;
    let hash = schedule.suite.hash;
    let expected = hkdf_extract(hash, &hash.zeros(), &hash.zeros());
    let mut c = Check::new("the Early Secret of a handshake with no PSK");
    c.note_all(client.schedule_lines());
    c.note(format!(
        "HKDF-Extract({} zero bytes as salt, {} zero bytes as IKM) under {}",
        hash.len(),
        hash.len(),
        hash.name()
    ));
    c.bytes_eq("Early Secret", &expected, &schedule.early_secret);
    c.ne(
        "Early Secret",
        crate::tls::hex(&hash.zeros()),
        crate::tls::hex(&schedule.early_secret),
    );
    c.finish()
});

tls_test!(handshake_secret, |ctx| {
    let client = ctx.handshake().await?;
    let schedule = client
        .schedule
        .as_ref()
        .ok_or_else(|| crate::stages::harness("no key schedule"))?;
    let hash = schedule.suite.hash;
    let derived = derive_secret(hash, &schedule.early_secret, "derived", &hash.empty_hash())
        .map_err(crate::assert::Failure::tls)?;
    let expected = hkdf_extract(hash, &derived, &client.shared_secret);
    let mut c = Check::new("the Handshake Secret");
    c.note_all(client.schedule_lines());
    c.note(
        "The salt is Derive-Secret(Early Secret, \"derived\", \"\") — the transcript argument \
         is the *empty* string, so its hash is Hash(\"\"), not the hash of the hellos.",
    );
    c.keying(&hash.empty_hash(), "derived → Derived Secret");
    c.bytes_eq("Handshake Secret", &expected, &schedule.handshake_secret);
    c.finish()
});

tls_test!(master_secret, |ctx| {
    let client = ctx.handshake().await?;
    let schedule = client
        .schedule
        .as_ref()
        .ok_or_else(|| crate::stages::harness("no key schedule"))?;
    let hash = schedule.suite.hash;
    let derived = derive_secret(
        hash,
        &schedule.handshake_secret,
        "derived",
        &hash.empty_hash(),
    )
    .map_err(crate::assert::Failure::tls)?;
    let expected = hkdf_extract(hash, &derived, &hash.zeros());
    let mut c = Check::new("the Master Secret");
    c.note_all(client.schedule_lines());
    c.note("The IKM here is zeros again; only the salt carries the (EC)DHE contribution.");
    c.bytes_eq("Master Secret", &expected, &schedule.master_secret);
    c.finish()
});

tls_test!(traffic_labels, |ctx| {
    let client = ctx.handshake().await?;
    let schedule = client
        .schedule
        .as_ref()
        .ok_or_else(|| crate::stages::harness("no key schedule"))?;
    let hash = schedule.suite.hash;
    let want_client = derive_secret(
        hash,
        &schedule.handshake_secret,
        "c hs traffic",
        &client.hash_after_server_hello,
    )
    .map_err(crate::assert::Failure::tls)?;
    let want_server = derive_secret(
        hash,
        &schedule.handshake_secret,
        "s hs traffic",
        &client.hash_after_server_hello,
    )
    .map_err(crate::assert::Failure::tls)?;
    let mut c = Check::new("the two handshake traffic secrets");
    c.keying(
        &client.hash_after_server_hello,
        "c hs traffic / s hs traffic",
    );
    c.note(
        "Both are taken over Transcript-Hash(ClientHello..ServerHello) — everything sent so \
         far, and nothing after.",
    );
    c.bytes_eq(
        "client_handshake_traffic_secret",
        &want_client,
        &schedule
            .client_handshake
            .as_ref()
            .map(|k| k.secret.clone())
            .unwrap_or_default(),
    );
    c.bytes_eq(
        "server_handshake_traffic_secret",
        &want_server,
        &schedule
            .server_handshake
            .as_ref()
            .map(|k| k.secret.clone())
            .unwrap_or_default(),
    );
    c.ne(
        "the two secrets",
        crate::tls::hex(&want_client),
        crate::tls::hex(&want_server),
    );
    c.finish()
});

tls_test!(secret_lengths, |ctx| {
    for suite in crate::tls::ALL_SUITES {
        let config = ctx.config().with_suites(&[suite]);
        let client = ctx.handshake_with(config).await?;
        let schedule = client
            .schedule
            .as_ref()
            .ok_or_else(|| crate::stages::harness("no key schedule"))?;
        let n = schedule.suite.hash.len();
        let mut c = Check::new(format!(
            "secret lengths under {}",
            crate::tls::suite_name(suite)
        ));
        c.note(format!(
            "{} means {}, so every secret and every transcript hash is {n} bytes",
            crate::tls::suite_name(suite),
            schedule.suite.hash.name()
        ));
        c.eq("Early Secret length", n, schedule.early_secret.len());
        c.eq(
            "Handshake Secret length",
            n,
            schedule.handshake_secret.len(),
        );
        c.eq("Master Secret length", n, schedule.master_secret.len());
        c.eq(
            "transcript hash length",
            n,
            client.hash_after_server_hello.len(),
        );
        c.finish()?;
    }
    Ok(())
});

tls_test!(wrong_label, |ctx| {
    let client = ctx.handshake().await?;
    let schedule = client
        .schedule
        .as_ref()
        .ok_or_else(|| crate::stages::harness("no key schedule"))?;
    let hash = schedule.suite.hash;
    let right = derive_secret(
        hash,
        &schedule.handshake_secret,
        "s hs traffic",
        &client.hash_after_server_hello,
    )
    .map_err(crate::assert::Failure::tls)?;
    // Two plausible mistakes: the client's label, and the prefix without its space.
    let wrong_side = derive_secret(
        hash,
        &schedule.handshake_secret,
        "c hs traffic",
        &client.hash_after_server_hello,
    )
    .map_err(crate::assert::Failure::tls)?;
    let mut c = Check::new("what a wrong label costs");
    c.note(
        "Both of these derive cleanly and produce a perfectly good-looking secret. The \
         handshake then fails at the server's Finished, several messages later, with no clue \
         pointing back here — which is why the key schedule is worth testing directly.",
    );
    c.ne(
        "Derive-Secret with \"c hs traffic\" instead of \"s hs traffic\"",
        crate::tls::hex(&right),
        crate::tls::hex(&wrong_side),
    );
    let info_right = hkdf_label("s hs traffic", &client.hash_after_server_hello, hash.len());
    c.eq(
        "HkdfLabel.label[0..6]",
        "tls13 ".to_string(),
        String::from_utf8_lossy(&info_right[3..9]).to_string(),
    );
    c.eq(
        "HkdfLabel.length",
        hash.len() as u16,
        u16::from_be_bytes([info_right[0], info_right[1]]),
    );
    c.finish()
});

tls_test!(schedule_works, |ctx| {
    let mut client = ctx.handshake().await?;
    let answer = client
        .echo_line("schedule")
        .await
        .map_err(|e| crate::stages::handshake_failure(e, &client))?;
    let mut c = Check::new("the schedule that actually decrypted the handshake");
    c.note_all(client.schedule_lines());
    c.note(
        "Reaching this line at all means every secret above matched the server's: the flight \
         decrypted, the signature verified and the Finished checked out.",
    );
    c.eq("echo", "eludehcs".to_string(), answer);
    c.finish()
});

fn examples() -> Vec<ExampleSpec> {
    let _ = (HashAlg::Sha256, KeySchedule::last_label);
    vec![
        ExampleSpec::text("The schedule, top to bottom")
            .request(
                "0 -> HKDF-Extract(salt=0, IKM=PSK|0) -> Early Secret\n\
                 Early Secret -> Derive-Secret(., \"derived\", \"\") -> salt\n\
                 salt + (EC)DHE -> HKDF-Extract -> Handshake Secret\n\
                 Handshake Secret -> Derive-Secret(., \"c hs traffic\"|\"s hs traffic\", CH..SH)\n\
                 Handshake Secret -> Derive-Secret(., \"derived\", \"\") -> salt\n\
                 salt + 0 -> HKDF-Extract -> Master Secret",
            )
            .response(
                "Master Secret -> Derive-Secret(., \"c ap traffic\"|\"s ap traffic\", CH..server \
                 Finished), \"exp master\" likewise, and \"res master\" over CH..client Finished.",
            )
            .note(
                "Every arrow is one HMAC call. The two `\"derived\"` steps are over the hash of \
                 the empty string, and every other Derive-Secret is over the transcript so far.",
            ),
        ExampleSpec::text("HkdfLabel on the wire")
            .request("HKDF-Expand-Label(secret, \"key\", \"\", 16)")
            .response(
                "info = 00 10 | 09 | 74 6c 73 31 33 20 6b 65 79 | 00\n\
                 that is: length 16, a 9-byte label \"tls13 key\", and a zero-length context.",
            )
            .note(
                "Six bytes of prefix, and the sixth is a space. Getting that wrong gives keys \
                 that are self-consistent and interoperate with nothing.",
            ),
    ]
}
