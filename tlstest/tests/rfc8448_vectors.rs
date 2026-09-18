//! The RFC 8448 "Simple 1-RTT Handshake" trace, replayed through this crate's key schedule.
//!
//! This is the strongest evidence the suite's own maths is right before it judges anybody
//! else's: every secret, every `HkdfLabel`, every traffic key and both Finished messages
//! come from the published trace, and none of them was produced by this code.
//!
//! The trace uses TLS_AES_128_GCM_SHA256, so every value here is SHA-256-sized. Section 3
//! of RFC 8448 is the source; the labels in the comments are its own.

use tlstest::tls::crypto::{
    derive_secret, hkdf_expand_label, hkdf_extract, hkdf_label, HashAlg, KeySchedule, Suite,
    TrafficKeys, Transcript,
};
use tlstest::tls::{hex, unhex, TLS_AES_128_GCM_SHA256};

/// The ClientHello handshake message, header included.
const CLIENT_HELLO: &str =
    "010000c00303cb34ecb1e78163ba1c38c6dacb196a6dffa21a8d9912ec18a2ef6283024d\
     ece7000006130113031302010000910000000b0009000006736572766572ff0100010000\
     0a00140012001d0017001800190100010101020103010400230000003300260024001d00\
     2099381de560e4bd43d23d8e435a7dbafeb3c06e51c13cae4d5413691e529aaf2c002b00\
     03020304000d0020001e0403050306030203080408050806040105010601020104020502\
     06020202002d00020101001c00024001";
/// The ServerHello handshake message.
const SERVER_HELLO: &str =
    "020000560303a6af06a4121860dc5e6e60249cd34c95930c8ac5cb1434dac155772ed3e2\
     692800130100002e00330024001d0020c9828876112095fe66762bdbf7c672e156d6cc25\
     3b833df1dd69b1b04e751f0f002b00020304";
/// EncryptedExtensions, after decryption.
const ENCRYPTED_EXTENSIONS: &str =
    "080000240022000a00140012001d00170018001901000101010201030104001c00024001\
     00000000";
/// The server's Certificate message.
const CERTIFICATE: &str =
    "0b0001b9000001b50001b0308201ac30820115a003020102020102300d06092a864886f7\
     0d01010b0500300e310c300a06035504031303727361301e170d31363037333030313233\
     35395a170d3236303733303031323335395a300e310c300a060355040313037273613081\
     9f300d06092a864886f70d010101050003818d0030818902818100b4bb498f8279303d98\
     0836399b36c6988c0c68de55e1bdb826d3901a2461eafd2de49a91d015abbc9a95137ace\
     6c1af19eaa6af98c7ced43120998e187a80ee0ccb0524b1b018c3e0b63264d449a6d38e2\
     2a5fda430846748030530ef0461c8ca9d9efbfae8ea6d1d03e2bd193eff0ab9a8002c474\
     28a6d35a8d88d79f7f1e3f0203010001a31a301830090603551d1304023000300b060355\
     1d0f0404030205a0300d06092a864886f70d01010b05000381810085aad2a0e5b9276b90\
     8c65f73a7267170618a54c5f8a7b337d2df7a594365417f2eae8f8a58c8f8172f9319cf3\
     6b7fd6c55b80f21a03015156726096fd335e5e67f2dbf102702e608ccae6bec1fc63a42a\
     99be5c3eb7107c3c54e9b9eb2bd5203b1c3b84e0a8b2f759409ba3eac9d91d402dcc0cc8\
     f8961229ac9187b42b4de10000";
/// The server's CertificateVerify.
const CERTIFICATE_VERIFY: &str =
    "0f000084080400805a747c5d88fa9bd2e55ab085a61015b7211f824cd484145ab3ff52f1\
     fda8477b0b7abc90db78e2d33a5c141a078653fa6bef780c5ea248eeaaa785c4f394cab6\
     d30bbe8d4859ee511f602957b15411ac027671459e46445c9ea58c181e818e95b8c3fb0b\
     f3278409d3be152a3da5043e063dda65cdf5aea20d53dfacd42f74f3";
/// The server's Finished message.
const SERVER_FINISHED: &str =
    "140000209b9b141d906337fbd2cbdce71df4deda4ab42c309572cb7fffee5454b78f0718";
/// The client's Finished message.
const CLIENT_FINISHED: &str =
    "14000020a8ec436d677634ae525ac1fcebe11a039ec17694fac6e98527b642f2edd5ce61";
/// `Early Secret` = HKDF-Extract(0, 0).
const EARLY_SECRET: &str = "33ad0a1c607ec03b09e6cd9893680ce210adf300aa1f2660e1b22e10f170f92a";
/// SHA-256 of the empty string.
const EMPTY_HASH: &str = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";
/// The `HkdfLabel` for Derive-Secret(., "derived", "").
const DERIVED_INFO: &str =
    "00200d746c733133206465726976656420e3b0c44298fc1c149afbf4c8996fb92427ae41\
     e4649b934ca495991b7852b855";
/// Derive-Secret(Early Secret, "derived", "").
const DERIVED_FOR_HANDSHAKE: &str =
    "6f2615a108c702c5678f54fc9dbab69716c076189c48250cebeac3576c3611ba";
/// The X25519 shared secret of the trace.
const ECDHE: &str = "8bd4054fb55b9d63fdfbacf9f04b9f0d35e6d63f537563efd46272900f89492d";
/// `Handshake Secret` = HKDF-Extract(derived, (EC)DHE).
const HANDSHAKE_SECRET: &str = "1dc826e93606aa6fdc0aadc12f741b01046aa6b99f691ed221a9f0ca043fbeac";
/// Transcript-Hash(ClientHello, ServerHello).
const HASH_CH_SH: &str = "860c06edc07858ee8e78f0e7428c58edd6b43f2ca3e6e95f02ed063cf0e1cad8";
/// `client_handshake_traffic_secret`.
const C_HS_TRAFFIC: &str = "b3eddb126e067f35a780b3abf45e2d8f3b1a950738f52e9600746a0e27a55a21";
/// `server_handshake_traffic_secret`.
const S_HS_TRAFFIC: &str = "b67b7d690cc16c4e75e54213cb2d37b4e9c912bcded9105d42befd59d391ad38";
/// The `HkdfLabel` for the server's handshake write key.
const S_HS_KEY_INFO: &str = "001009746c733133206b657900";
/// The `HkdfLabel` for the server's handshake write IV.
const S_HS_IV_INFO: &str = "000c08746c73313320697600";
/// `server_write_key` for the handshake.
const S_HS_KEY: &str = "3fce516009c21727d0f2e4e86ee403bc";
/// `server_write_iv` for the handshake.
const S_HS_IV: &str = "5d313eb2671276ee13000b30";
/// Derive-Secret(Handshake Secret, "derived", "").
const DERIVED_FOR_MASTER: &str = "43de77e0c77713859a944db9db2590b53190a65b3ee2e4f12dd7a0bb7ce254b4";
/// `Master Secret` = HKDF-Extract(derived, 0).
const MASTER_SECRET: &str = "18df06843d13a08bf2a449844c5f8a478001bc4d4c627984d5a41da8d0402919";
/// The `HkdfLabel` for a finished key.
const S_FINISHED_INFO: &str = "00200e746c7331332066696e697368656400";
/// The server's `finished_key`.
const S_FINISHED_KEY: &str = "008d3b66f816ea559f96b537e885c31fc068bf492c652f01f288a1d8cdc19fc8";
/// The client's `finished_key`.
const C_FINISHED_KEY: &str = "b80ad01015fb2f0bd65ff7d4da5d6bf83f84821d1f87fdc7d3c75b5a7b42d9c4";
/// Transcript-Hash(ClientHello .. server Finished).
const HASH_THROUGH_SFIN: &str = "9608102a0f1ccc6db6250b7b7e417b1a000eaada3daae4777a7686c9ff83df13";
/// `client_application_traffic_secret_0`.
const C_AP_TRAFFIC: &str = "9e40646ce79a7f9dc05af8889bce6552875afa0b06df0087f792ebb7c17504a5";
/// `server_application_traffic_secret_0`.
const S_AP_TRAFFIC: &str = "a11af9f05531f856ad47116b45a950328204b4f44bfb6b3a4b4f1f3fcb631643";
/// `exporter_master_secret`.
const EXP_MASTER: &str = "fe22f881176eda18eb8f44529e6792c50c9a3f89452f68d8ae311b4309d3cf50";
/// Transcript-Hash(ClientHello .. client Finished).
const HASH_THROUGH_CFIN: &str = "209145a96ee8e2a122ff810047cc952684658d6049e86429426db87c54ad143d";
/// `resumption_master_secret`.
const RES_MASTER: &str = "7df235f2031d2a051287d02b0241b0bfdaf86cc856231f2d5aba46c434ec196c";
/// The `HkdfLabel` for the resumption PSK, nonce 00 00.
const RESUMPTION_INFO: &str = "002010746c73313320726573756d7074696f6e020000";
/// The PSK a ticket with nonce 00 00 stands for.
const RESUMPTION_PSK: &str = "4ecd0eb6ec3b4d87f5d6028f922ca4c5851a277fd41311c9e62d2c9492e1c4f3";

/// The bytes of a constant, or a panic naming it — these are fixtures, not inputs.
fn b(name: &str, value: &str) -> Vec<u8> {
    unhex(value).unwrap_or_else(|| panic!("{name} is not valid hex"))
}

/// The suite the trace uses.
fn suite() -> Suite {
    Suite::from_code(TLS_AES_128_GCM_SHA256).expect("TLS_AES_128_GCM_SHA256 is implemented")
}

#[test]
fn the_empty_transcript_hash_matches() {
    assert_eq!(hex(&HashAlg::Sha256.empty_hash()), EMPTY_HASH);
    assert_eq!(hex(&HashAlg::Sha256.digest(&[])), EMPTY_HASH);
}

#[test]
fn hkdf_label_structures_match_the_trace() {
    // Derive-Secret(., "derived", "") over the empty transcript hash.
    assert_eq!(
        hex(&hkdf_label("derived", &b("EMPTY_HASH", EMPTY_HASH), 32)),
        DERIVED_INFO
    );
    // The key and IV labels, whose context is empty.
    assert_eq!(hex(&hkdf_label("key", &[], 16)), S_HS_KEY_INFO);
    assert_eq!(hex(&hkdf_label("iv", &[], 12)), S_HS_IV_INFO);
    // And the finished key's.
    assert_eq!(hex(&hkdf_label("finished", &[], 32)), S_FINISHED_INFO);
}

#[test]
fn the_early_secret_is_extract_of_two_zero_blocks() {
    let zeros = HashAlg::Sha256.zeros();
    assert_eq!(
        hex(&hkdf_extract(HashAlg::Sha256, &zeros, &zeros)),
        EARLY_SECRET
    );
}

#[test]
fn the_handshake_secret_matches() {
    let early = b("EARLY_SECRET", EARLY_SECRET);
    let derived = derive_secret(
        HashAlg::Sha256,
        &early,
        "derived",
        &b("EMPTY_HASH", EMPTY_HASH),
    )
    .expect("derive");
    assert_eq!(hex(&derived), DERIVED_FOR_HANDSHAKE);
    let handshake = hkdf_extract(HashAlg::Sha256, &derived, &b("ECDHE", ECDHE));
    assert_eq!(hex(&handshake), HANDSHAKE_SECRET);
}

#[test]
fn the_transcript_hash_after_the_hellos_matches() {
    let mut transcript = Transcript::new(HashAlg::Sha256);
    transcript.push(&b("CLIENT_HELLO", CLIENT_HELLO), "client_hello");
    transcript.push(&b("SERVER_HELLO", SERVER_HELLO), "server_hello");
    assert_eq!(hex(&transcript.current()), HASH_CH_SH);
}

#[test]
fn the_handshake_traffic_secrets_match() {
    let handshake = b("HANDSHAKE_SECRET", HANDSHAKE_SECRET);
    let hash = b("HASH_CH_SH", HASH_CH_SH);
    assert_eq!(
        hex(&derive_secret(HashAlg::Sha256, &handshake, "c hs traffic", &hash).expect("derive")),
        C_HS_TRAFFIC
    );
    assert_eq!(
        hex(&derive_secret(HashAlg::Sha256, &handshake, "s hs traffic", &hash).expect("derive")),
        S_HS_TRAFFIC
    );
}

#[test]
fn the_handshake_traffic_keys_match() {
    let keys = TrafficKeys::derive(suite(), &b("S_HS_TRAFFIC", S_HS_TRAFFIC)).expect("derive");
    assert_eq!(hex(&keys.key), S_HS_KEY);
    assert_eq!(hex(&keys.iv), S_HS_IV);
    // And the nonce rule, which the trace exercises one record at a time.
    assert_eq!(hex(&keys.nonce(0)), S_HS_IV);
    let mut expected = b("S_HS_IV", S_HS_IV);
    expected[11] ^= 1;
    assert_eq!(hex(&keys.nonce(1)), hex(&expected));
}

#[test]
fn the_master_secret_matches() {
    let handshake = b("HANDSHAKE_SECRET", HANDSHAKE_SECRET);
    let derived = derive_secret(
        HashAlg::Sha256,
        &handshake,
        "derived",
        &b("EMPTY_HASH", EMPTY_HASH),
    )
    .expect("derive");
    assert_eq!(hex(&derived), DERIVED_FOR_MASTER);
    let master = hkdf_extract(HashAlg::Sha256, &derived, &HashAlg::Sha256.zeros());
    assert_eq!(hex(&master), MASTER_SECRET);
}

#[test]
fn the_finished_keys_match() {
    let server = TrafficKeys::derive(suite(), &b("S_HS_TRAFFIC", S_HS_TRAFFIC)).expect("derive");
    assert_eq!(
        hex(&server.finished_key().expect("finished key")),
        S_FINISHED_KEY
    );
    let client = TrafficKeys::derive(suite(), &b("C_HS_TRAFFIC", C_HS_TRAFFIC)).expect("derive");
    assert_eq!(
        hex(&client.finished_key().expect("finished key")),
        C_FINISHED_KEY
    );
}

#[test]
fn the_server_finished_verifies_over_the_right_transcript() {
    let mut transcript = Transcript::new(HashAlg::Sha256);
    for (bytes, label) in [
        (CLIENT_HELLO, "client_hello"),
        (SERVER_HELLO, "server_hello"),
        (ENCRYPTED_EXTENSIONS, "encrypted_extensions"),
        (CERTIFICATE, "certificate"),
        (CERTIFICATE_VERIFY, "certificate_verify"),
    ] {
        transcript.push(&b(label, bytes), label);
    }
    let schedule = KeySchedule::new(suite(), None);
    let keys = TrafficKeys::derive(suite(), &b("S_HS_TRAFFIC", S_HS_TRAFFIC)).expect("derive");
    let verify_data = schedule
        .verify_data(&keys, &transcript.current())
        .expect("verify data");
    let message = b("SERVER_FINISHED", SERVER_FINISHED);
    assert_eq!(message[0], 20, "finished(20)");
    assert_eq!(hex(&verify_data), hex(&message[4..]));
}

#[test]
fn the_client_finished_verifies_over_the_right_transcript() {
    let mut transcript = Transcript::new(HashAlg::Sha256);
    for (bytes, label) in [
        (CLIENT_HELLO, "client_hello"),
        (SERVER_HELLO, "server_hello"),
        (ENCRYPTED_EXTENSIONS, "encrypted_extensions"),
        (CERTIFICATE, "certificate"),
        (CERTIFICATE_VERIFY, "certificate_verify"),
        (SERVER_FINISHED, "server finished"),
    ] {
        transcript.push(&b(label, bytes), label);
    }
    assert_eq!(hex(&transcript.current()), HASH_THROUGH_SFIN);
    let schedule = KeySchedule::new(suite(), None);
    let keys = TrafficKeys::derive(suite(), &b("C_HS_TRAFFIC", C_HS_TRAFFIC)).expect("derive");
    let verify_data = schedule
        .verify_data(&keys, &transcript.current())
        .expect("verify data");
    assert_eq!(
        hex(&verify_data),
        hex(&b("CLIENT_FINISHED", CLIENT_FINISHED)[4..])
    );
}

#[test]
fn the_application_secrets_match() {
    let master = b("MASTER_SECRET", MASTER_SECRET);
    let hash = b("HASH_THROUGH_SFIN", HASH_THROUGH_SFIN);
    assert_eq!(
        hex(&derive_secret(HashAlg::Sha256, &master, "c ap traffic", &hash).expect("derive")),
        C_AP_TRAFFIC
    );
    assert_eq!(
        hex(&derive_secret(HashAlg::Sha256, &master, "s ap traffic", &hash).expect("derive")),
        S_AP_TRAFFIC
    );
    assert_eq!(
        hex(&derive_secret(HashAlg::Sha256, &master, "exp master", &hash).expect("derive")),
        EXP_MASTER
    );
}

#[test]
fn the_resumption_secrets_match() {
    let mut transcript = Transcript::new(HashAlg::Sha256);
    for (bytes, label) in [
        (CLIENT_HELLO, "client_hello"),
        (SERVER_HELLO, "server_hello"),
        (ENCRYPTED_EXTENSIONS, "encrypted_extensions"),
        (CERTIFICATE, "certificate"),
        (CERTIFICATE_VERIFY, "certificate_verify"),
        (SERVER_FINISHED, "server finished"),
        (CLIENT_FINISHED, "client finished"),
    ] {
        transcript.push(&b(label, bytes), label);
    }
    assert_eq!(hex(&transcript.current()), HASH_THROUGH_CFIN);
    let master = b("MASTER_SECRET", MASTER_SECRET);
    let res = derive_secret(
        HashAlg::Sha256,
        &master,
        "res master",
        &b("HASH_THROUGH_CFIN", HASH_THROUGH_CFIN),
    )
    .expect("derive");
    assert_eq!(hex(&res), RES_MASTER);
    // The ticket in the trace carries a two-byte nonce of 00 00.
    let nonce = [0u8, 0];
    assert_eq!(hex(&hkdf_label("resumption", &nonce, 32)), RESUMPTION_INFO);
    let psk = hkdf_expand_label(HashAlg::Sha256, &res, "resumption", &nonce, 32).expect("expand");
    assert_eq!(hex(&psk), RESUMPTION_PSK);
}

#[test]
fn the_whole_schedule_matches_when_driven_through_key_schedule() {
    // The same trace, this time through the type a stage test uses rather than the free
    // functions, so the two paths cannot drift apart.
    let mut schedule = KeySchedule::new(suite(), None);
    assert_eq!(hex(&schedule.early_secret), EARLY_SECRET);
    schedule
        .enter_handshake(&b("ECDHE", ECDHE), &b("HASH_CH_SH", HASH_CH_SH))
        .expect("enter handshake");
    assert_eq!(hex(&schedule.handshake_secret), HANDSHAKE_SECRET);
    assert_eq!(hex(&schedule.master_secret), MASTER_SECRET);
    let server = schedule
        .server_handshake
        .as_ref()
        .expect("server handshake keys");
    assert_eq!(hex(&server.secret), S_HS_TRAFFIC);
    assert_eq!(hex(&server.key), S_HS_KEY);
    assert_eq!(hex(&server.iv), S_HS_IV);
    let client = schedule
        .client_handshake
        .as_ref()
        .expect("client handshake keys");
    assert_eq!(hex(&client.secret), C_HS_TRAFFIC);

    schedule
        .enter_application(&b("HASH_THROUGH_SFIN", HASH_THROUGH_SFIN))
        .expect("enter application");
    assert_eq!(
        hex(&schedule
            .client_application
            .as_ref()
            .expect("client application keys")
            .secret),
        C_AP_TRAFFIC
    );
    assert_eq!(
        hex(&schedule
            .server_application
            .as_ref()
            .expect("server application keys")
            .secret),
        S_AP_TRAFFIC
    );
    assert_eq!(
        hex(&schedule.exporter_master.clone().expect("exporter secret")),
        EXP_MASTER
    );

    schedule
        .derive_resumption_master(&b("HASH_THROUGH_CFIN", HASH_THROUGH_CFIN))
        .expect("resumption master");
    assert_eq!(
        hex(&schedule
            .resumption_master
            .clone()
            .expect("resumption secret")),
        RES_MASTER
    );
    assert_eq!(
        hex(&schedule.resumption_psk(&[0, 0]).expect("resumption psk")),
        RESUMPTION_PSK
    );

    // Every derivation the trace names is recorded, in order, with its label.
    let labels: Vec<&str> = schedule.steps.iter().map(|s| s.label.as_str()).collect();
    assert!(labels.contains(&"c hs traffic"));
    assert!(labels.contains(&"s hs traffic"));
    assert!(labels.contains(&"c ap traffic"));
    assert!(labels.contains(&"res master"));
}
