//! Stage 08 — `legacy_version` is a lie; the real version is in `supported_versions`.

use crate::assert::Check;
use crate::examples::{ExampleEnv, ExampleSpec, Part};
use crate::stages::{Stage, Test};
use crate::tls::client::{negotiated_version, ClientConfig};
use crate::tls::msg::find_extension;
use crate::tls::{
    ext_name, EXT_SUPPORTED_VERSIONS, LEGACY_VERSION_TLS12, TLS13_VERSION,
};
use crate::tls_test;

/// Stage definition.
pub fn stage() -> Stage {
    Stage {
        number: 8,
        slug: "supported_versions",
        name: "legacy_version and supported_versions",
        ext: false,
        hints: &[
            "ClientHello.legacy_version is always 0x0303 and means nothing; never negotiate \
             from it",
            "The real version is in supported_versions(43): a one-byte-counted list in the \
             ClientHello, a single uint16 in the ServerHello",
            "ServerHello.legacy_version is 0x0303 too — a TLS 1.3 server puts 0x0304 in its \
             own supported_versions extension and nowhere else",
            "A ClientHello with no supported_versions extension is a pre-1.3 client; a \
             1.3-only server answers protocol_version(70)",
        ],
        examples: examples,
        tests: vec![
            Test::new(
                "the ClientHello's legacy_version is 0x0303 and is ignored",
                legacy_version_ignored,
            ),
            Test::new(
                "the ServerHello's legacy_version is 0x0303",
                server_legacy_version,
            ),
            Test::new(
                "the ServerHello carries supported_versions with 0x0304",
                server_supported_versions,
            ),
            Test::new(
                "supported_versions in the ServerHello is exactly two bytes",
                supported_versions_shape,
            ),
            Test::new(
                "a hello offering 1.3 among several versions still selects 1.3",
                several_versions,
            ),
            Test::new(
                "an unrecognised legacy_version does not change the outcome",
                odd_legacy_version,
            ),
            Test::new(
                "the negotiated version is 0x0304 and the handshake completes",
                completes,
            ),
        ],
    }
}

tls_test!(legacy_version_ignored, |ctx| {
    let client = ctx.handshake().await?;
    let hello = client
        .client_hello
        .as_ref()
        .ok_or_else(|| crate::stages::harness("no ClientHello was built"))?;
    let mut c = Check::new("what the client put in legacy_version");
    c.block("client_hello", &client.client_hello_bytes);
    c.mark(4..6);
    c.eq(
        "client_hello.legacy_version",
        LEGACY_VERSION_TLS12,
        hello.legacy_version,
    );
    c.note(
        "This is the suite checking itself: RFC 8446 section 4.1.2 pins the field at 0x0303 \
         and a server must not read a version out of it.",
    );
    c.finish()
});

tls_test!(server_legacy_version, |ctx| {
    let client = ctx.handshake().await?;
    let hello = client
        .server_hello
        .as_ref()
        .ok_or_else(|| crate::stages::harness("no ServerHello"))?;
    let mut c = Check::new("the ServerHello's legacy_version");
    c.block("server_hello", &client.server_hello_bytes);
    c.mark(4..6);
    c.eq(
        "server_hello.legacy_version",
        LEGACY_VERSION_TLS12,
        hello.legacy_version,
    );
    c.finish()
});

tls_test!(server_supported_versions, |ctx| {
    let client = ctx.handshake().await?;
    let hello = client
        .server_hello
        .as_ref()
        .ok_or_else(|| crate::stages::harness("no ServerHello"))?;
    let mut c = Check::new("the ServerHello's supported_versions extension");
    c.block("server_hello", &client.server_hello_bytes);
    c.that(
        &format!("server_hello.extensions[{}]", ext_name(EXT_SUPPORTED_VERSIONS)),
        "present",
        find_extension(&hello.extensions, EXT_SUPPORTED_VERSIONS).is_some(),
        "missing — without it a client cannot tell 1.3 from 1.2",
    );
    if c.ok() {
        c.eq(
            "server_hello.supported_versions.selected_version",
            TLS13_VERSION,
            hello.selected_version().unwrap_or(0),
        );
    }
    c.finish()
});

tls_test!(supported_versions_shape, |ctx| {
    let client = ctx.handshake().await?;
    let hello = client
        .server_hello
        .as_ref()
        .ok_or_else(|| crate::stages::harness("no ServerHello"))?;
    let e = find_extension(&hello.extensions, EXT_SUPPORTED_VERSIONS)
        .ok_or_else(|| crate::stages::harness("no supported_versions extension"))?;
    let mut c = Check::new("the shape of the server's supported_versions");
    c.block("server_hello", &client.server_hello_bytes);
    c.note(
        "A client sends a *list* here (one length byte, then the versions); a server sends a \
         single uint16 with no list around it.",
    );
    c.eq("server_hello.supported_versions.length", 2usize, e.data.len());
    c.finish()
});

tls_test!(several_versions, |ctx| {
    let mut config = ctx.config();
    // A real browser offers a GREASE value, 1.3 and 1.2 in that order.
    config.versions = vec![0x0a0a, TLS13_VERSION, 0x0303, 0x0302];
    let client = ctx.handshake_with(config).await?;
    let hello = client
        .server_hello
        .as_ref()
        .ok_or_else(|| crate::stages::harness("no ServerHello"))?;
    let mut c = Check::new("the version chosen from a list of four");
    c.block("client_hello", &client.client_hello_bytes);
    c.block("server_hello", &client.server_hello_bytes);
    c.eq(
        "server_hello.supported_versions.selected_version",
        TLS13_VERSION,
        negotiated_version(hello),
    );
    c.finish()
});

tls_test!(odd_legacy_version, |ctx| {
    let mut config = ctx.config();
    // 0x0301 in legacy_version, 1.3 in supported_versions: the extension wins.
    config.legacy_version = 0x0301;
    let client = ctx.handshake_with(config).await?;
    let hello = client
        .server_hello
        .as_ref()
        .ok_or_else(|| crate::stages::harness("no ServerHello"))?;
    let mut c = Check::new("a hello whose legacy_version says TLS 1.0");
    c.block("client_hello", &client.client_hello_bytes);
    c.note(
        "legacy_version is not a version. A server that reads one out of it will refuse this \
         hello, and will refuse a lot of real clients too.",
    );
    c.eq(
        "server_hello.supported_versions.selected_version",
        TLS13_VERSION,
        negotiated_version(hello),
    );
    c.finish()
});

tls_test!(completes, |ctx| {
    let mut client = ctx.handshake().await?;
    let answer = client
        .echo_line("version")
        .await
        .map_err(|e| crate::stages::handshake_failure(e, &client))?;
    let mut c = Check::new("the handshake that version negotiation produced");
    c.eq("echo", "noisrev".to_string(), answer);
    c.finish()
});

fn config(env: &ExampleEnv) -> ClientConfig {
    env.config()
}

fn examples() -> Vec<ExampleSpec> {
    vec![
        ExampleSpec::handshake(
            "Where the version really lives",
            config,
            Part::ClientHello,
            Part::ServerHello,
        )
        .request(
            "legacy_version 0x0303, and a supported_versions extension whose list holds \
             0x0304",
        )
        .response(
            "legacy_version 0x0303 again, and a supported_versions extension holding the two \
             bytes 0x0304.",
        )
        .note(
            "Both messages say 0x0303 in the field called `version`, and neither of them means \
             it. Negotiate from the extension; the field is there so a 1990s middlebox does \
             not panic.",
        ),
        ExampleSpec::text("The two shapes of supported_versions")
            .request(
                "ClientHello: extension_data is a one-byte count followed by that many uint16 \
                 versions — `02 03 04` for a 1.3-only client.",
            )
            .response(
                "ServerHello: extension_data is a bare uint16 — `03 04`. No list, no count.",
            )
            .note(
                "The same extension code point, two different bodies, depending on which \
                 message it is in. That asymmetry is real and it is easy to miss.",
            ),
    ]
}
