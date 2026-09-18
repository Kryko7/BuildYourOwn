//! Stage 09 — A client that cannot do TLS 1.3 gets `protocol_version`.

use crate::assert::Check;
use crate::examples::{ExampleEnv, ExampleSpec, Expect};
use crate::stages::{check_refused_with, hello_message, provoke, Stage, Test};
use crate::tls::client::ClientConfig;
use crate::tls::msg::{find_extension, ClientHello};
use crate::tls::record::Record;
use crate::tls::{AlertDescription, EXT_SUPPORTED_VERSIONS, LEGACY_VERSION_TLS12};
use crate::tls_test;

/// Stage definition.
pub fn stage() -> Stage {
    Stage {
        number: 9,
        slug: "tls12_client",
        name: "A TLS 1.2-only client is refused",
        ext: false,
        hints: &[
            "A hello with no supported_versions extension is a pre-1.3 client: a 1.3-only \
             server answers protocol_version(70) and closes",
            "A hello whose supported_versions list does not contain 0x0304 gets the same \
             answer, however many other versions it lists",
            "Send the alert as a plaintext record — there are no keys yet, and there never \
             will be on this connection",
            "The check comes before cipher-suite selection: the version decides whether the \
             rest of the hello even means what you think it means",
        ],
        examples: examples,
        tests: vec![
            Test::new(
                "a hello with no supported_versions extension is refused",
                no_extension,
            ),
            Test::new(
                "a hello offering only TLS 1.2 is refused",
                only_tls12,
            ),
            Test::new(
                "a hello offering only TLS 1.0 and 1.1 is refused",
                only_old,
            ),
            Test::new(
                "the refusal is protocol_version(70) when an alert is sent",
                alert_description,
            ),
            Test::new(
                "a hello offering 1.2 and 1.3 is accepted",
                both_versions,
            ),
            Test::new(
                "the server still serves a 1.3 client afterwards",
                still_serving,
            ),
        ],
    }
}

/// A ClientHello with `supported_versions` removed or replaced.
fn hello_without_13(config: &ClientConfig, versions: Option<&[u16]>) -> Result<Vec<u8>, String> {
    let (mut hello, _) = crate::tls::client::build_hello(config).map_err(|e| e.to_string())?;
    hello
        .extensions
        .retain(|e| e.ext_type != EXT_SUPPORTED_VERSIONS);
    if let Some(list) = versions {
        hello
            .extensions
            .insert(1, crate::tls::msg::supported_versions_extension(list));
    }
    Ok(hello.encode())
}

async fn expect_refusal(
    ctx: &mut crate::stages::Ctx,
    versions: Option<&[u16]>,
    what: &str,
) -> Result<crate::tls::conn::Reaction, crate::assert::Failure> {
    let message =
        hello_without_13(&ctx.config(), versions).map_err(crate::stages::harness)?;
    let mut conn = ctx.connect().await?;
    let reaction = provoke(&mut conn, &Record::build(22, LEGACY_VERSION_TLS12, &message)).await;
    let mut c = Check::new(format!("what the server does with {what}"));
    c.block("client_hello", &message);
    check_refused_with(
        &mut c,
        "the server's reaction",
        &reaction,
        &[
            AlertDescription::PROTOCOL_VERSION,
            AlertDescription::HANDSHAKE_FAILURE,
            AlertDescription::ILLEGAL_PARAMETER,
            AlertDescription::INAPPROPRIATE_FALLBACK,
        ],
    );
    c.finish()?;
    Ok(reaction)
}

tls_test!(no_extension, |ctx| {
    expect_refusal(ctx, None, "a hello with no supported_versions extension").await?;
    Ok(())
});

tls_test!(only_tls12, |ctx| {
    expect_refusal(ctx, Some(&[0x0303]), "a hello offering only TLS 1.2").await?;
    Ok(())
});

tls_test!(only_old, |ctx| {
    expect_refusal(ctx, Some(&[0x0302, 0x0301]), "a hello offering only TLS 1.1 and 1.0").await?;
    Ok(())
});

tls_test!(alert_description, |ctx| {
    let reaction = expect_refusal(ctx, Some(&[0x0303]), "a TLS 1.2-only hello").await?;
    let mut c = Check::new("the alert a 1.3-only server sends a 1.2-only client");
    match reaction.alert() {
        Some(alert) => {
            c.note(
                "RFC 8446 appendix D.1 names protocol_version(70) for this; a server that \
                 closes without an alert is conformant too, which is why this check only \
                 looks at the description when there is one.",
            );
            c.eq(
                "alert.description",
                AlertDescription::PROTOCOL_VERSION.0,
                alert.description.0,
            );
            c.eq("alert.level", 2u8, alert.level.as_u8());
        }
        None => {
            c.note("the server closed without an alert, which RFC 8446 allows");
        }
    }
    c.finish()
});

tls_test!(both_versions, |ctx| {
    let mut config = ctx.config();
    config.versions = vec![crate::tls::TLS13_VERSION, 0x0303];
    let client = ctx.handshake_with(config).await?;
    let hello = client
        .server_hello
        .as_ref()
        .ok_or_else(|| crate::stages::harness("no ServerHello"))?;
    let mut c = Check::new("a hello offering both 1.2 and 1.3");
    c.block("server_hello", &client.server_hello_bytes);
    c.eq(
        "server_hello.supported_versions.selected_version",
        crate::tls::TLS13_VERSION,
        hello.selected_version().unwrap_or(0),
    );
    c.finish()
});

tls_test!(still_serving, |ctx| {
    let message =
        hello_without_13(&ctx.config(), Some(&[0x0303])).map_err(crate::stages::harness)?;
    let mut conn = ctx.connect().await?;
    let _ = provoke(&mut conn, &Record::build(22, LEGACY_VERSION_TLS12, &message)).await;
    drop(conn);
    ctx.expect_still_serving("a refused TLS 1.2 client").await
});

fn tls12_hello(env: &ExampleEnv) -> Result<Vec<u8>, String> {
    let message = hello_without_13(&env.config(), Some(&[0x0303]))?;
    Ok(Record::build(22, LEGACY_VERSION_TLS12, &message))
}

fn no_extension_hello(env: &ExampleEnv) -> Result<Vec<u8>, String> {
    let message = hello_without_13(&env.config(), None)?;
    // Prove the extension really is gone before the bytes are committed.
    let hello = ClientHello::parse_body(&message[4..]).map_err(|e| e.to_string())?;
    if find_extension(&hello.extensions, EXT_SUPPORTED_VERSIONS).is_some() {
        return Err("the example still carries supported_versions".to_string());
    }
    Ok(Record::build(22, LEGACY_VERSION_TLS12, &message))
}

fn examples() -> Vec<ExampleSpec> {
    let _ = hello_message;
    vec![
        ExampleSpec::raw(
            "A hello that offers only TLS 1.2",
            tls12_hello,
            Expect::UntilAlert,
        )
        .request(
            "A normal-looking ClientHello whose supported_versions list holds 0x0303 and \
             nothing else",
        )
        .response(
            "An alert record: level fatal(2), description protocol_version(70), sent in the \
             clear because no keys exist yet. Then the connection closes.",
        )
        .note(
            "Two bytes in the alert record's fragment, `02 46`. That is the whole message — \
             there is nothing else a 1.3-only server can say to a 1.2-only client.",
        ),
        ExampleSpec::raw(
            "A hello with no supported_versions extension at all",
            no_extension_hello,
            Expect::UntilAlert,
        )
        .request("A pre-1.3 ClientHello: legacy_version 0x0303 and no version extension")
        .response("The same protocol_version(70) alert.")
        .note(
            "An absent extension and a list without 0x0304 mean the same thing. Do not fall \
             back to legacy_version when the extension is missing — that field is 0x0303 in \
             every hello ever sent, including this one.",
        ),
    ]
}
