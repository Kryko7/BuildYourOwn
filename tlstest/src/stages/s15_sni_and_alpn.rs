//! Stage 15 — SNI and ALPN.

use crate::assert::Check;
use crate::examples::{ExampleEnv, ExampleSpec, Part};
use crate::stages::{Stage, Test};
use crate::tls::client::ClientConfig;
use crate::tls::msg::{alpn_extension, find_extension, Extension};
use crate::tls::{ext_name, EXT_ALPN, EXT_SERVER_NAME};
use crate::tls_test;

/// Stage definition.
pub fn stage() -> Stage {
    Stage {
        number: 15,
        slug: "sni_and_alpn",
        name: "SNI and ALPN",
        ext: false,
        hints: &[
            "server_name(0) is a list of (name_type, host_name) pairs; in practice it has \
             exactly one entry of type host_name(0)",
            "A server that accepted the name answers with an *empty* server_name extension in \
             EncryptedExtensions, or with nothing at all — never with the name echoed back",
            "ALPN lives in EncryptedExtensions too, and must name exactly one protocol, and \
             that protocol must be one the client offered",
            "A client that offers ALPN and a server that has no protocols configured is not \
             an error: send no ALPN extension and carry on",
        ],
        examples: examples,
        tests: vec![
            Test::new("a hello with SNI completes", sni_present),
            Test::new("a hello with no SNI completes", sni_absent),
            Test::new(
                "SNI is never echoed with the host name in it",
                sni_not_echoed,
            ),
            Test::new("an unusual but legal host name is accepted", odd_host_name),
            Test::new(
                "a hello offering ALPN completes either way",
                alpn_offered,
            ),
            Test::new(
                "if ALPN is selected it is one the client offered",
                alpn_selection,
            ),
            Test::new(
                "ALPN, if present in EncryptedExtensions, names exactly one protocol",
                alpn_single,
            ),
            Test::new(
                "a hello with neither SNI nor ALPN completes",
                neither,
            ),
        ],
    }
}

tls_test!(sni_present, |ctx| {
    let config = ctx.config().with_server_name(Some("localhost"));
    let mut client = ctx.handshake_with(config).await?;
    let answer = client
        .echo_line("sni")
        .await
        .map_err(|e| crate::stages::handshake_failure(e, &client))?;
    let mut c = Check::new("a handshake carrying server_name");
    c.block("client_hello", &client.client_hello_bytes);
    c.eq("echo", "ins".to_string(), answer);
    c.finish()
});

tls_test!(sni_absent, |ctx| {
    let config = ctx.config().with_server_name(None);
    let mut client = ctx.handshake_with(config).await?;
    let hello = client
        .client_hello
        .as_ref()
        .ok_or_else(|| crate::stages::harness("no ClientHello"))?;
    let has_sni = find_extension(&hello.extensions, EXT_SERVER_NAME).is_some();
    let answer = client
        .echo_line("nosni")
        .await
        .map_err(|e| crate::stages::handshake_failure(e, &client))?;
    let mut c = Check::new("a handshake with no server_name at all");
    c.block("client_hello", &client.client_hello_bytes);
    c.note(
        "SNI is optional. A server with one certificate has nothing to choose between, and a \
         hello without it must still work.",
    );
    c.eq("client_hello carries server_name", false, has_sni);
    c.eq("echo", "inson".to_string(), answer);
    c.finish()
});

tls_test!(sni_not_echoed, |ctx| {
    let client = ctx
        .handshake_with(ctx.config().with_server_name(Some("localhost")))
        .await?;
    let mut c = Check::new("what comes back for server_name");
    c.block("encrypted_extensions", &client.encrypted_extensions_bytes);
    c.note(
        "RFC 6066 as amended by RFC 8446 section 4.4.2.2: the server's server_name extension \
         is empty. Echoing the name back is a bug that no client will notice until it does.",
    );
    match find_extension(&client.encrypted_extensions, EXT_SERVER_NAME) {
        Some(e) => {
            c.eq("encrypted_extensions.server_name.length", 0usize, e.data.len());
        }
        None => {
            c.note("the server sent no server_name extension at all, which is also correct");
        }
    }
    c.finish()
});

tls_test!(odd_host_name, |ctx| {
    // A long label, a trailing hyphen-free IDN-ish label and mixed case: all legal DNS
    // presentation forms as far as TLS is concerned.
    let name = "A-very-long-but-legal.label.Example.Test";
    let config = ctx.config().with_server_name(Some(name));
    let client = ctx.handshake_with(config).await?;
    let mut c = Check::new("a hello whose server_name is unusual but legal");
    c.block("client_hello", &client.client_hello_bytes);
    c.note(format!("server_name was {name:?}"));
    c.note(
        "TLS does not validate the host name: it is an opaque byte string used to pick a \
         certificate. A server with one certificate should not care what it says.",
    );
    c.that(
        "the handshake",
        "completed",
        !client.server_finished_bytes.is_empty(),
        "did not complete",
    );
    c.finish()
});

tls_test!(alpn_offered, |ctx| {
    let config = ctx.config().with_alpn(&["http/1.1", "h2"]);
    let mut client = ctx.handshake_with(config).await?;
    let selected = client.alpn_selected.clone();
    let answer = client
        .echo_line("alpn")
        .await
        .map_err(|e| crate::stages::handshake_failure(e, &client))?;
    let mut c = Check::new("a hello offering two ALPN protocols");
    c.block("encrypted_extensions", &client.encrypted_extensions_bytes);
    c.note(
        "A server with no protocols configured — which is what `-rev` is — sends no ALPN \
         extension and carries on. Refusing with no_application_protocol(120) would also be \
         conformant for a server that requires ALPN, but it must not simply ignore the \
         extension and then behave as though one had been selected.",
    );
    c.observe("encrypted_extensions.alpn", &selected);
    c.eq("echo", "npla".to_string(), answer);
    c.finish()
});

tls_test!(alpn_selection, |ctx| {
    let offered = ["http/1.1", "h2", "my-protocol"];
    let client = ctx.handshake_with(ctx.config().with_alpn(&offered)).await?;
    let mut c = Check::new("the ALPN protocol the server selected, if it selected one");
    c.block("encrypted_extensions", &client.encrypted_extensions_bytes);
    match &client.alpn_selected {
        Some(p) => {
            c.that(
                "encrypted_extensions.alpn.protocol_name_list[0]",
                "one of the protocols the client offered",
                offered.contains(&p.as_str()),
                p.clone(),
            );
        }
        None => {
            c.note("no ALPN extension came back, which is correct for a server with none configured");
        }
    }
    c.finish()
});

tls_test!(alpn_single, |ctx| {
    let client = ctx
        .handshake_with(ctx.config().with_alpn(&["http/1.1"]))
        .await?;
    let mut c = Check::new("the shape of the server's ALPN extension");
    c.block("encrypted_extensions", &client.encrypted_extensions_bytes);
    match find_extension(&client.encrypted_extensions, EXT_ALPN) {
        Some(e) => {
            // 2 bytes of list length, 1 byte of name length, then the name.
            let list_len = if e.data.len() >= 2 {
                u16::from_be_bytes([e.data[0], e.data[1]]) as usize
            } else {
                0
            };
            let name_len = e.data.get(2).copied().unwrap_or(0) as usize;
            c.note(
                "RFC 7301 as used by TLS 1.3: the server's list has exactly one entry, so the \
                 list length is always one more than the name length.",
            );
            c.eq(
                "encrypted_extensions.alpn.protocol_name_list.length",
                name_len + 1,
                list_len,
            );
            c.eq(
                "encrypted_extensions.alpn.extension_data.len()",
                name_len + 3,
                e.data.len(),
            );
        }
        None => {
            c.note("no ALPN extension came back, which is correct for a server with none configured");
        }
    }
    c.finish()
});

tls_test!(neither, |ctx| {
    let config = ctx.config().with_server_name(None);
    let mut client = ctx.handshake_with(config).await?;
    let answer = client
        .echo_line("plain")
        .await
        .map_err(|e| crate::stages::handshake_failure(e, &client))?;
    let mut c = Check::new("a hello with neither SNI nor ALPN");
    c.eq("echo", "nialp".to_string(), answer);
    let names: Vec<String> = client
        .encrypted_extensions
        .iter()
        .map(|e| ext_name(e.ext_type))
        .collect();
    c.observe("encrypted_extensions", names);
    c.finish()
});

fn sni_config(env: &ExampleEnv) -> ClientConfig {
    env.config().with_server_name(Some("localhost"))
}

fn alpn_config(env: &ExampleEnv) -> ClientConfig {
    env.config().with_alpn(&["http/1.1", "h2"])
}

fn examples() -> Vec<ExampleSpec> {
    let _ = (alpn_extension, Extension::new(0, Vec::new()));
    vec![
        ExampleSpec::handshake(
            "A ClientHello carrying server_name",
            sni_config,
            Part::ClientHello,
            Part::EncryptedExtensions,
        )
        .request(
            "extension 0: a two-byte list length, name_type host_name(0), a two-byte length, \
             then the ASCII name",
        )
        .response(
            "EncryptedExtensions, which is where every extension that is not needed to derive \
             keys lives. The server_name answer, when there is one, is empty.",
        )
        .note(
            "Three nested lengths for one string. The outer one is the extension's, then the \
             list's, then the name's — the shape is RFC 6066's and TLS 1.3 kept it.",
        ),
        ExampleSpec::handshake(
            "A ClientHello offering ALPN",
            alpn_config,
            Part::ClientHello,
            Part::EncryptedExtensions,
        )
        .request("extension 16 offering `http/1.1` and `h2`, each with a one-byte length")
        .response(
            "EncryptedExtensions. A server with no protocols configured — like `s_server -rev` \
             — sends no ALPN extension back, and that is correct.",
        )
        .note(
            "If you do select a protocol, the answer is a list of exactly one, and it has to \
             be one the client offered. Selecting something else is worse than selecting \
             nothing.",
        ),
    ]
}
