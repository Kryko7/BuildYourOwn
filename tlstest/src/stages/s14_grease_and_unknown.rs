//! Stage 14 — GREASE and unknown extensions must be ignored.

use crate::assert::Check;
use crate::examples::{ExampleEnv, ExampleSpec, Part};
use crate::stages::{handshake_edited, Stage, Test};
use crate::tls::client::ClientConfig;
use crate::tls::msg::{client_key_share_extension, Extension};
use crate::tls::{ext_name, grease_values, is_grease, ALL_SUITES, GROUP_X25519, TLS13_VERSION};
use crate::tls_test;

/// Stage definition.
pub fn stage() -> Stage {
    Stage {
        number: 14,
        slug: "grease_and_unknown",
        name: "GREASE and unknown extensions are ignored",
        ext: false,
        hints: &[
            "An extension type you do not know is skipped, not an error: read its two-byte \
             length and step over the body",
            "The same goes for unknown cipher suites, groups, versions and signature schemes \
             — skip the entry, keep reading the list",
            "GREASE (RFC 8701) is the sixteen values 0x0a0a, 0x1a1a … 0xfafa; real clients \
             send them deliberately to catch servers that do not skip",
            "Never echo an extension back that the client did not send, and never send one in \
             a ServerHello that RFC 8446 section 4.1.3 does not allow there",
        ],
        examples,
        tests: vec![
            Test::new(
                "a GREASE extension in the hello is ignored",
                grease_extension,
            ),
            Test::new(
                "several GREASE extensions at once are ignored",
                many_grease_extensions,
            ),
            Test::new("a GREASE cipher suite is ignored", grease_suite),
            Test::new(
                "a GREASE group and a GREASE key share are ignored",
                grease_group_and_share,
            ),
            Test::new(
                "a GREASE version in supported_versions is ignored",
                grease_version,
            ),
            Test::new(
                "an unknown extension carrying data is ignored",
                unknown_extension_with_data,
            ),
            Test::new(
                "the ServerHello carries only extensions RFC 8446 allows there",
                server_hello_extensions,
            ),
            Test::new(
                "the server never echoes an extension the client did not send",
                no_unsolicited_extensions,
            ),
        ],
    }
}

tls_test!(grease_extension, |ctx| {
    let grease = grease_values()[3];
    let client = handshake_edited(ctx, ctx.config(), |hello| {
        hello
            .extensions
            .insert(0, Extension::new(grease, Vec::new()));
    })
    .await?;
    let mut c = Check::new("a hello whose first extension is a GREASE flag");
    c.block("client_hello", &client.client_hello_bytes);
    c.note(format!(
        "extension type 0x{grease:04x} with an empty body, sent first so a server that reads \
         extensions positionally trips over it"
    ));
    c.that(
        "the handshake",
        "completed",
        !client.server_finished_bytes.is_empty(),
        "did not complete",
    );
    c.finish()
});

tls_test!(many_grease_extensions, |ctx| {
    let values = grease_values();
    let client = handshake_edited(ctx, ctx.config(), |hello| {
        hello
            .extensions
            .insert(0, Extension::new(values[0], Vec::new()));
        hello
            .extensions
            .push(Extension::new(values[7], vec![0x00; 8]));
        hello
            .extensions
            .push(Extension::new(values[15], vec![0xff; 1]));
    })
    .await?;
    let mut c = Check::new("a hello carrying three GREASE extensions");
    c.block("client_hello", &client.client_hello_bytes);
    c.that(
        "the handshake",
        "completed",
        !client.server_finished_bytes.is_empty(),
        "did not complete",
    );
    c.finish()
});

tls_test!(grease_suite, |ctx| {
    let mut suites = vec![grease_values()[2]];
    suites.extend_from_slice(&ALL_SUITES);
    let client = ctx
        .handshake_with(ctx.config().with_suites(&suites))
        .await?;
    let hello = client
        .server_hello
        .as_ref()
        .ok_or_else(|| crate::stages::harness("no ServerHello"))?;
    let mut c = Check::new("a GREASE value at the head of cipher_suites");
    c.block("client_hello", &client.client_hello_bytes);
    c.that(
        "server_hello.cipher_suite",
        "a real TLS 1.3 suite, not the GREASE value",
        ALL_SUITES.contains(&hello.cipher_suite),
        crate::tls::suite_name(hello.cipher_suite),
    );
    c.that(
        "server_hello.cipher_suite",
        "not a GREASE value",
        !is_grease(hello.cipher_suite),
        crate::tls::suite_name(hello.cipher_suite),
    );
    c.finish()
});

tls_test!(grease_group_and_share, |ctx| {
    let grease = grease_values()[5];
    let client = handshake_edited(ctx, ctx.config(), move |hello| {
        // A GREASE group in supported_groups, and a one-byte GREASE key share in front of
        // the real one — exactly what Chrome does.
        for e in &mut hello.extensions {
            if e.ext_type == crate::tls::EXT_SUPPORTED_GROUPS && e.data.len() >= 2 {
                let mut data = vec![0u8, 0u8];
                data.extend_from_slice(&grease.to_be_bytes());
                data.extend_from_slice(&e.data[2..]);
                let len = (data.len() - 2) as u16;
                data[0..2].copy_from_slice(&len.to_be_bytes());
                e.data = data;
            }
        }
        let real = hello
            .extensions
            .iter()
            .find(|e| e.ext_type == crate::tls::EXT_KEY_SHARE)
            .map(|e| e.data.clone());
        if let Some(existing) = real {
            // Rebuild the key_share list with a GREASE entry first.
            let mut shares = vec![(grease, vec![0u8])];
            if existing.len() > 6 {
                let group = u16::from_be_bytes([existing[2], existing[3]]);
                let len = u16::from_be_bytes([existing[4], existing[5]]) as usize;
                if existing.len() >= 6 + len {
                    shares.push((group, existing[6..6 + len].to_vec()));
                }
            }
            for e in &mut hello.extensions {
                if e.ext_type == crate::tls::EXT_KEY_SHARE {
                    e.data = client_key_share_extension(&shares).data;
                }
            }
        }
    })
    .await?;
    let mut c = Check::new("a hello with a GREASE group and a GREASE key share");
    c.block("client_hello", &client.client_hello_bytes);
    c.eq(
        "server_hello.key_share.group",
        GROUP_X25519,
        client.selected_group.unwrap_or(0),
    );
    c.finish()
});

tls_test!(grease_version, |ctx| {
    let mut config = ctx.config();
    config.versions = vec![grease_values()[9], TLS13_VERSION];
    let client = ctx.handshake_with(config).await?;
    let hello = client
        .server_hello
        .as_ref()
        .ok_or_else(|| crate::stages::harness("no ServerHello"))?;
    let mut c = Check::new("a GREASE value at the head of supported_versions");
    c.block("client_hello", &client.client_hello_bytes);
    c.eq(
        "server_hello.supported_versions.selected_version",
        TLS13_VERSION,
        hello.selected_version().unwrap_or(0),
    );
    c.finish()
});

tls_test!(unknown_extension_with_data, |ctx| {
    // 0x7a7a is not GREASE and is not assigned; a server must skip it by its length.
    let client = handshake_edited(ctx, ctx.config(), |hello| {
        hello
            .extensions
            .insert(1, Extension::new(0x7a7a, (0u8..64).collect()));
    })
    .await?;
    let mut c = Check::new("an unassigned extension carrying 64 bytes");
    c.block("client_hello", &client.client_hello_bytes);
    c.note(
        "The rule is mechanical: two bytes of type, two bytes of length, then skip that many \
         bytes. Nothing about the body matters when the type is unknown.",
    );
    c.that(
        "the handshake",
        "completed",
        !client.server_finished_bytes.is_empty(),
        "did not complete",
    );
    c.finish()
});

tls_test!(server_hello_extensions, |ctx| {
    let client = ctx.handshake().await?;
    let hello = client
        .server_hello
        .as_ref()
        .ok_or_else(|| crate::stages::harness("no ServerHello"))?;
    let mut c = Check::new("which extensions the ServerHello carries");
    c.block("server_hello", &client.server_hello_bytes);
    c.note(
        "RFC 8446 section 4.1.3 allows exactly three in a ServerHello: key_share(51), \
         pre_shared_key(41) and supported_versions(43). Everything else belongs in \
         EncryptedExtensions.",
    );
    let allowed = [
        crate::tls::EXT_KEY_SHARE,
        crate::tls::EXT_PRE_SHARED_KEY,
        crate::tls::EXT_SUPPORTED_VERSIONS,
    ];
    for e in &hello.extensions {
        c.that(
            &format!("server_hello.extensions[{}]", ext_name(e.ext_type)),
            "one of key_share, pre_shared_key, supported_versions",
            allowed.contains(&e.ext_type),
            ext_name(e.ext_type),
        );
    }
    c.observe(
        "server_hello.extensions",
        hello
            .extensions
            .iter()
            .map(|e| ext_name(e.ext_type))
            .collect::<Vec<_>>(),
    );
    c.finish()
});

tls_test!(no_unsolicited_extensions, |ctx| {
    let client = ctx.handshake().await?;
    let sent: Vec<u16> = client
        .client_hello
        .as_ref()
        .map(|h| h.extensions.iter().map(|e| e.ext_type).collect())
        .unwrap_or_default();
    let mut c = Check::new("that every extension the server sent was asked for");
    c.block("client_hello", &client.client_hello_bytes);
    c.block("encrypted_extensions", &client.encrypted_extensions_bytes);
    c.note(
        "RFC 8446 section 4.2: a server may only send an extension the client offered. The \
         exceptions are cookie and, in a HelloRetryRequest, key_share.",
    );
    for e in &client.encrypted_extensions {
        c.that(
            &format!("encrypted_extensions[{}]", ext_name(e.ext_type)),
            "an extension the ClientHello offered",
            sent.contains(&e.ext_type),
            format!("{} was never offered", ext_name(e.ext_type)),
        );
    }
    c.finish()
});

fn config(env: &ExampleEnv) -> ClientConfig {
    env.config()
}

fn examples() -> Vec<ExampleSpec> {
    vec![
        ExampleSpec::handshake(
            "A GREASE extension, and the ServerHello that ignores it",
            config,
            Part::ClientHello,
            Part::ServerHello,
        )
        .request(
            "An ordinary ClientHello. A real browser also puts a GREASE extension type first \
             and a GREASE cipher suite at the head of the list.",
        )
        .response("An ordinary ServerHello. Nothing GREASE-shaped comes back.")
        .note(
            "GREASE values are reserved forever and will never mean anything. Their entire \
             purpose is to break servers that do not skip what they do not recognise — which \
             is how the ecosystem stays extensible.",
        ),
        ExampleSpec::text("The sixteen GREASE values")
            .request(
                "0x0a0a, 0x1a1a, 0x2a2a, 0x3a3a, 0x4a4a, 0x5a5a, 0x6a6a, 0x7a7a, 0x8a8a, \
                 0x9a9a, 0xaaaa, 0xbaba, 0xcaca, 0xdada, 0xeaea, 0xfafa",
            )
            .response(
                "They appear as extension types, cipher suites, named groups, signature \
                 schemes, versions and ALPN protocols. Skip them everywhere.",
            )
            .note(
                "You do not need to recognise them as GREASE. If unknown values are skipped \
                 the way the format says, GREASE takes care of itself.",
            ),
    ]
}
