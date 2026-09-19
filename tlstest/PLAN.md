# tlstest stage plan

Tick a stage when `tlstest --server my_server --stage N` is green. `tlstest --list` reads
these boxes. Stages marked **[ext]** go beyond the core track (`--skip-ext` hides them).
All 45 stages are implemented — 321 tests — so no entry says **(planned)**; each one
names its source file and its test count. See README.md, "Adding a stage".

Run one stage: `tlstest --server my_server --stage 5` — everything so far: `--until 12` —
the lot: `--all`. Prove the suite itself: `tlstest --server openssl --validate --all`.

Every stage also carries worked examples — 91 of them — the exact bytes the reference
answered, annotated field by field. They live in the stage's own file, are recaptured with
`tlstest --capture-examples examples/captured.json --server openssl`, and reach the site
through `catalog.json`. See README.md, "Examples".

Build order: the sections are in dependency order, and a stage only assumes what the stages
before it establish. Sections A and B check liveness by sending a fresh ClientHello and
requiring a handshake record back, not a completed handshake, so the record layer can be
finished before the key schedule is started.

## A. TCP & the record layer

- [ ] **Stage 01** — Accept a connection and let the client speak first (`src/stages/s01_accept.rs`, 7 tests)
  - Bind a TCP listener on the port `-accept` names and accept in a loop; the port is chosen by the harness, never hardcoded
  - Say nothing on accept: TLS is client-speaks-first, and the ClientHello is the first byte either side sends
  - Set SO_REUSEADDR so a restart does not hit 'address already in use'
  - A client that connects and vanishes without a ClientHello is normal; close that socket and go back to accepting
- [ ] **Stage 02** — TLSPlaintext framing (`src/stages/s02_record_framing.rs`, 7 tests)
  - Every record is five header bytes — type(1), legacy_record_version(2), length(2) — and then exactly `length` fragment bytes
  - Read the five bytes first, then read `length` more; one read() is never one record, and one record is not always one message
  - legacy_record_version is 0x0303 everywhere, with one exception: the record carrying the first ClientHello may say 0x0301, and a server must accept it
  - The server's own records carry 0x0303; the real version is negotiated inside the ServerHello's supported_versions extension
- [ ] **Stage 03** — Record size limits (`src/stages/s03_record_limits.rs`, 6 tests)
  - TLSPlaintext.length may not exceed 2^14 (16384); a longer one is a record_overflow alert, not a bigger buffer
  - Check the length field before allocating: a two-byte length can ask for 65535 bytes that will never arrive
  - TLSCiphertext may be 2^14 + 256, which leaves room for the content type, the padding and the AEAD tag
  - A record that promises more bytes than the client ever sends must time out or close, never spin or wedge the accept loop
- [ ] **Stage 04** — Garbage on connect (`src/stages/s04_garbage_on_connect.rs`, 7 tests)
  - Validate the record's content type before anything else: a TLS 1.3 stream only ever starts with handshake(22)
  - A client that sends HTTP, SSH or noise gets an alert or a close — never a stack trace, never a hang, and never a dead accept loop
  - One bad connection must not take the others with it: handle each socket's errors where they happen
  - Whatever you decide, decide it quickly; a stranger must not be able to hold a connection open for ever by sending four bytes
- [ ] **Stage 05** — A ClientHello split across segments and records (`src/stages/s05_split_client_hello.rs`, 6 tests)
  - TCP has no message boundaries: keep a buffer per connection and only act when a whole record is in it
  - A handshake message may span several records, so keep a second buffer for handshake bytes and only parse when msg_type's uint24 length is satisfied
  - Reassemble across records before parsing, never the other way round: the record boundaries are not part of the message
  - Only the handshake bytes go into the transcript — never the five-byte record headers, however the message was fragmented
- [ ] **Stage 06** — Coalesced handshake messages (`src/stages/s06_coalesced_handshake.rs`, 6 tests)
  - One record may hold several handshake messages end to end; loop over the fragment until it is empty instead of parsing it once
  - The server's own flight is usually coalesced: EncryptedExtensions, Certificate, CertificateVerify and Finished often arrive in one or two records
  - Each message still gets its own four-byte header, and each goes into the transcript separately, in order
  - A record must never end in the middle of a message *and* start a new one — but a message may start in one record and finish in the next
- [ ] **Stage 07** — Half-close and abandoned handshakes (`src/stages/s07_half_close.rs`, 6 tests)
  - read() returning 0 is the peer's FIN: a normal end of stream, not an error and not a reason to abort the process
  - A client that shuts down its write half mid-handshake will never send more; drop that connection's state and carry on
  - Never let one abandoned handshake hold a resource for ever — the accept loop has to come back to accepting
  - A write to a socket the peer has closed is EPIPE or ECONNRESET; handle it where it happens rather than letting it end the process

## B. ClientHello, extensions, negotiation

- [ ] **Stage 08** — legacy_version and supported_versions (`src/stages/s08_supported_versions.rs`, 7 tests)
  - ClientHello.legacy_version is always 0x0303 and means nothing; never negotiate from it
  - The real version is in supported_versions(43): a one-byte-counted list in the ClientHello, a single uint16 in the ServerHello
  - ServerHello.legacy_version is 0x0303 too — a TLS 1.3 server puts 0x0304 in its own supported_versions extension and nowhere else
  - A ClientHello with no supported_versions extension is a pre-1.3 client; a 1.3-only server answers protocol_version(70)
- [ ] **Stage 09** — A TLS 1.2-only client is refused (`src/stages/s09_tls12_client.rs`, 6 tests)
  - A hello with no supported_versions extension is a pre-1.3 client: a 1.3-only server answers protocol_version(70) and closes
  - A hello whose supported_versions list does not contain 0x0304 gets the same answer, however many other versions it lists
  - Send the alert as a plaintext record — there are no keys yet, and there never will be on this connection
  - The check comes before cipher-suite selection: the version decides whether the rest of the hello even means what you think it means
- [ ] **Stage 10** — Cipher-suite negotiation (`src/stages/s10_cipher_suites.rs`, 8 tests)
  - TLS 1.3 has three suites worth implementing: 0x1301 AES-128-GCM-SHA256, 0x1302 AES-256-GCM-SHA384, 0x1303 ChaCha20-Poly1305-SHA256
  - A suite names a hash *and* an AEAD; the hash drives the whole key schedule and the transcript, so picking 0x1302 means SHA-384 everywhere
  - The chosen suite must be one the ClientHello offered — a server that picks its own favourite regardless will fail every client's check
  - TLS 1.2 suite numbers may appear in the list; ignore them rather than choking on them
- [ ] **Stage 11** — No acceptable parameters: handshake_failure (`src/stages/s11_handshake_failure.rs`, 6 tests)
  - When no offered cipher suite is acceptable, the answer is a fatal handshake_failure(40) — not a ServerHello with a suite the client never offered
  - The same alert covers a client with no acceptable group and no acceptable signature algorithm
  - Send the alert in the clear: there are no handshake keys on a connection that never got past the hello
  - Close the connection after a fatal alert; there is no state left worth keeping
- [ ] **Stage 12** — key_share: x25519 and secp256r1 (`src/stages/s12_key_share_groups.rs`, 8 tests)
  - The ClientHello's key_share carries a list of (group, key_exchange) pairs; the ServerHello's carries exactly one
  - x25519 key_exchange is 32 raw bytes; secp256r1 is a 65-byte uncompressed point starting with 0x04
  - The shared secret is the raw X coordinate — 32 bytes for both of these groups — and goes straight into HKDF-Extract as the IKM
  - Reject an all-zero x25519 result: it means the peer sent a small-order point (RFC 8446 section 7.4.2, RFC 7748 section 6.1)
- [ ] **Stage 13** — signature_algorithms and the server's key (`src/stages/s13_signature_algorithms.rs`, 8 tests)
  - signature_algorithms(13) is mandatory in a TLS 1.3 ClientHello; a hello without it is missing_extension(109)
  - Pick a scheme the client offered *and* the server's own key can produce: an ECDSA P-256 key cannot sign rsa_pss_rsae_sha256
  - An RSA key in TLS 1.3 signs with RSA-PSS (0x0804/5/6), never PKCS#1 v1.5 — those code points exist only for certificate signatures
  - When nothing in the list fits the key, the answer is handshake_failure(40), not a signature the client cannot check
- [ ] **Stage 14** — GREASE and unknown extensions are ignored (`src/stages/s14_grease_and_unknown.rs`, 8 tests)
  - An extension type you do not know is skipped, not an error: read its two-byte length and step over the body
  - The same goes for unknown cipher suites, groups, versions and signature schemes — skip the entry, keep reading the list
  - GREASE (RFC 8701) is the sixteen values 0x0a0a, 0x1a1a … 0xfafa; real clients send them deliberately to catch servers that do not skip
  - Never echo an extension back that the client did not send, and never send one in a ServerHello that RFC 8446 section 4.1.3 does not allow there
- [ ] **Stage 15** — SNI and ALPN (`src/stages/s15_sni_and_alpn.rs`, 8 tests)
  - server_name(0) is a list of (name_type, host_name) pairs; in practice it has exactly one entry of type host_name(0)
  - A server that accepted the name answers with an *empty* server_name extension in EncryptedExtensions, or with nothing at all — never with the name echoed back
  - ALPN lives in EncryptedExtensions too, and must name exactly one protocol, and that protocol must be one the client offered
  - A client that offers ALPN and a server that has no protocols configured is not an error: send no ALPN extension and carry on
- [ ] **Stage 16** — Session id echo, duplicate extensions, lying lengths (`src/stages/s16_bad_extensions.rs`, 8 tests)
  - legacy_session_id is echoed into the ServerHello byte for byte, whatever the client put there — 32 bytes, zero bytes, or anything between
  - A ClientHello with the same extension type twice is illegal_parameter(47); checking for it means keeping a set of the types you have seen
  - Every length must be checked against the bytes you actually have before you index; an extension claiming 400 bytes inside a 40-byte block is a decode_error
  - legacy_compression_methods must be exactly `01 00`; anything else is illegal_parameter

## C. Key schedule & handshake encryption

- [ ] **Stage 17** — The ServerHello, field by field (`src/stages/s17_server_hello.rs`, 8 tests)
  - The body is legacy_version(2), random(32), legacy_session_id_echo(vec8), cipher_suite(2), legacy_compression_method(1), extensions(vec16) — in that order
  - random is 32 fresh random bytes; it is not a timestamp and not derived from anything the client sent
  - A TLS 1.3 server must not put the downgrade sentinels of RFC 8446 section 4.1.3 in the last eight bytes of random — those mean 'I negotiated 1.2 on purpose'
  - key_share and supported_versions are the two extensions that make this a TLS 1.3 ServerHello at all; everything else belongs in EncryptedExtensions
- [ ] **Stage 18** — Early, Handshake and Master secrets (`src/stages/s18_handshake_secret.rs`, 7 tests)
  - Early Secret = HKDF-Extract(salt = Hash.length zeros, IKM = PSK or Hash.length zeros); with no PSK both inputs are zeros and the result is still not zero
  - Between Extract steps comes Derive-Secret(secret, "derived", "") — over the hash of the *empty* transcript, not the handshake so far
  - Handshake Secret = HKDF-Extract(salt = that derived value, IKM = the (EC)DHE shared secret); Master Secret = HKDF-Extract(salt = derived again, IKM = zeros)
  - HKDF-Expand-Label's info is uint16 length, then "tls13 " + label as an opaque<7..255>, then the context as an opaque<0..255> — the six-byte prefix includes the space
- [ ] **Stage 19** — The transcript hash at every boundary (`src/stages/s19_transcript_hash.rs`, 7 tests)
  - The transcript is the concatenation of complete handshake messages — msg_type, the uint24 length and the body — and nothing else
  - Record headers, ChangeCipherSpec records and the AEAD's own bytes never enter it; a message that arrived in three records is hashed once
  - Keep a running hash context and take a snapshot at each boundary: four different hashes are needed at four different moments
  - The hash a signature or a MAC covers is the one *before* that message was added — CertificateVerify covers up to Certificate, Finished covers up to CertificateVerify
- [ ] **Stage 20** — Traffic keys, IVs and the record nonce (`src/stages/s20_traffic_keys.rs`, 7 tests)
  - key = HKDF-Expand-Label(secret, "key", "", key_length) and iv = HKDF-Expand-Label(secret, "iv", "", 12); the context is empty for both
  - The nonce is the static IV with the 64-bit sequence number XORed into its right end — the number itself is never sent
  - The AEAD's additional data is the five bytes of the record header, exactly as written, with the *ciphertext* length in it
  - Every TLS 1.3 AEAD uses a 12-byte nonce and a 16-byte tag; only the key length changes between suites
- [ ] **Stage 21** — The ChangeCipherSpec compatibility record (`src/stages/s21_change_cipher_spec.rs`, 7 tests)
  - TLS 1.3 has no ChangeCipherSpec message; the record survives only so the handshake looks like TLS 1.2 to a middlebox
  - Accept one (or several) at any point after the first ClientHello, ignore it, and keep the transcript and the sequence numbers untouched
  - It is always plaintext, even after keys are in force: content type 20, one fragment byte 0x01
  - Sending one is optional both ways; a client that sends none, and a client that sends three, must both get the same handshake
- [ ] **Stage 22** — EncryptedExtensions (`src/stages/s22_encrypted_extensions.rs`, 7 tests)
  - EncryptedExtensions is always the first message the server sends under the handshake traffic keys, and it is always sent — even when it is empty
  - Its body is one extensions block and nothing else: `08 00 00 02 00 00` is a perfectly good empty one
  - Only extensions that are *not* needed to establish keys go here: server_name, ALPN, max_fragment_length, and so on
  - key_share, pre_shared_key and supported_versions belong in the ServerHello; sending them here is unsupported_extension(110)
- [ ] **Stage 23** — Record sequence numbers and key changes (`src/stages/s23_sequence_numbers.rs`, 6 tests)
  - Each direction keeps a 64-bit counter of records written under the current keys; it is never sent, both sides just count
  - The counter resets to zero every time the keys change — handshake keys to application keys, and again after every KeyUpdate
  - ChangeCipherSpec records are not encrypted and do not advance either counter
  - A record that arrives out of order simply will not authenticate: the nonce is wrong, so the tag is wrong, and that is bad_record_mac

## D. Authentication

- [ ] **Stage 24** — The Certificate message (`src/stages/s24_certificate.rs`, 8 tests)
  - The body is certificate_request_context (an opaque<0..255>, always empty from a server) and then certificate_list, an opaque<0..2^24-1>
  - Each entry is cert_data (uint24-length DER) followed by its own extensions block — that per-entry block is new in TLS 1.3 and is usually two zero bytes
  - The end-entity certificate comes first, and each later entry should certify the one before it; the self-signed root may be left out
  - The certificate is sent encrypted, under the server handshake traffic keys, so it is not visible to a passive observer
- [ ] **Stage 25** — CertificateVerify: context and transcript (`src/stages/s25_certificate_verify.rs`, 7 tests)
  - The signed content is 64 bytes of 0x20, then "TLS 1.3, server CertificateVerify", then a single 0x00, then Transcript-Hash(CH..Certificate)
  - The 64 spaces and the context string exist so a signature made here can never be replayed as a TLS 1.2 one, or as a client's
  - The body is algorithm (a uint16 SignatureScheme) and signature (an opaque<0..2^16-1>) — nothing else
  - Sign the transcript hash as it stood *before* this message; the message cannot cover itself
- [ ] **Stage 26** — RSA-PSS, ECDSA and Ed25519 signatures (`src/stages/s26_signature_schemes.rs`, 7 tests)
  - rsa_pss_rsae_sha256 is RSA-PSS with MGF1-SHA-256 and a salt as long as the hash; the signature is exactly the modulus size
  - ecdsa_secp256r1_sha256 signs SHA-256 of the content and the signature is DER: a SEQUENCE of two INTEGERs, so 70-72 bytes and never fixed
  - Ed25519 is PureEdDSA: it signs the content itself, unhashed, and the signature is always 64 bytes
  - Whatever the scheme, the content is the same 64 spaces + context + 0x00 + transcript hash
- [ ] **Stage 27** — The server's Finished (`src/stages/s27_server_finished.rs`, 7 tests)
  - finished_key = HKDF-Expand-Label(server_handshake_traffic_secret, "finished", "", Hash.length) — the context is empty
  - verify_data = HMAC(finished_key, Transcript-Hash(everything before this message))
  - The whole body is the verify_data: no length prefix, no algorithm field, just Hash.length bytes
  - Finished is the last message of the server's flight, and the transcript it covers is the one right after CertificateVerify
- [ ] **Stage 28** — The client's Finished, and what a wrong one costs (`src/stages/s28_client_finished.rs`, 7 tests)
  - The client's verify_data uses the *client* handshake traffic secret's finished key, over Transcript-Hash(everything up to and including the server's Finished)
  - It arrives encrypted under the client handshake keys, not the application keys — those only come into force once it has been sent
  - A wrong verify_data is decrypt_error(51), fatal, and the connection ends there
  - Only after this message is the handshake complete: application data written before it is early data, and that is a different feature
- [ ] **Stage 29** — Wrong-order flights (`src/stages/s29_flight_order.rs`, 8 tests)
  - A TLS server is a state machine: decide what message is legal *next*, and refuse everything else with unexpected_message(10)
  - The client's whole first flight is one ClientHello; a second one is only ever legal as the answer to a HelloRetryRequest
  - Application data before the client's Finished is early data, and without an early_data extension there is nowhere for it to be decrypted
  - The check belongs before the parse: a message type that cannot occur here should never reach the code that decodes its body

## E. Application data & AEAD

- [ ] **Stage 30** — The -rev echo (`src/stages/s30_echo.rs`, 8 tests)
  - `-rev` is line-based: buffer bytes until a newline arrives, then answer that one line and keep the rest
  - Strip the trailing CR and LF, reverse the bytes that are left, and write them back followed by exactly one LF
  - An empty line — a lone newline — answers with a lone newline; there is nothing to reverse
  - Only complete lines are answered: a write with no newline in it gets nothing back until one arrives
- [ ] **Stage 31** — The data path under every suite (`src/stages/s31_echo_all_suites.rs`, 7 tests)
  - The record layer does not care which AEAD it is using: same framing, same inner content type, same nonce rule
  - What does change is the key length (16 or 32 bytes) and, for the SHA-384 suite, every secret and transcript hash in the schedule
  - ChaCha20-Poly1305 has no block size, so its ciphertext is exactly the plaintext length plus the 16-byte tag — the same as GCM here
  - Test the data path under each suite separately: a key-schedule bug that only affects SHA-384 is invisible under the other two
- [ ] **Stage 32** — TLSInnerPlaintext and padding (`src/stages/s32_inner_plaintext.rs`, 7 tests)
  - Inside an encrypted record the plaintext is `content || content_type || zeros`; the outer header always says application_data(23)
  - To find the real type, scan back from the end past every zero byte — the first non-zero byte is it
  - Any amount of zero padding is legal, including none, and a receiver must accept whatever it is sent without reading anything into it
  - A decrypted record that is entirely zeros has no content type at all: that is unexpected_message(10), not a record of type zero
- [ ] **Stage 33** — Record sizes and volume (`src/stages/s33_record_sizes.rs`, 7 tests)
  - A sender may split application data into records however it likes; a receiver must reassemble the stream and never assume one record is one message
  - Keep each protected record's length within 2^14 + 256; when the data is bigger, write several records
  - A megabyte of traffic is a few dozen records, not one: the size limit is on the record, not on the connection
  - Many tiny records are legal and expensive — 17 bytes of overhead each — but they must still all arrive, in order
- [ ] **Stage 34** — close_notify in both directions (`src/stages/s34_close_notify.rs`, 7 tests)
  - close_notify is an alert record — level warning(1), description 0 — sent encrypted under the current application keys
  - It says 'I will write no more'; the other direction stays open until that side sends its own, which is why it is a warning and not fatal
  - A server that receives one should stop reading that connection and may answer with its own before closing the socket
  - A TCP close with no close_notify is a truncation: legal to tolerate, worth telling apart from a clean end of stream
- [ ] **Stage 35** — A tampered record is bad_record_mac (`src/stages/s35_bad_record_mac.rs`, 7 tests)
  - A record whose AEAD tag does not verify is bad_record_mac(20), fatal, and the connection is over — there is no retry and no resynchronisation
  - The additional data is the record header, so changing the length or the type byte breaks the tag just as surely as changing the ciphertext
  - Do not leak *why* it failed: a bad tag, a bad length and a bad type must all produce the same alert at the same moment
  - The alert goes out under the keys that are still in force; the connection is closed straight after it

## F. Alerts, errors, robustness

- [ ] **Stage 36** — The alert record (`src/stages/s36_alerts.rs`, 8 tests)
  - An alert is a record of type 21 whose fragment is exactly two bytes: level, then description
  - In TLS 1.3 the level is decoration — every alert except close_notify and user_canceled is fatal whatever the byte says, and a fatal alert always closes the connection
  - Before keys exist alerts go out in the clear; afterwards they are encrypted like anything else, with an inner content type of 21
  - An alert record must never be split across records or coalesced with another: two bytes, one record
- [ ] **Stage 37** — Unknown and truncated handshake messages (`src/stages/s37_bad_handshake.rs`, 7 tests)
  - Check the uint24 length against the bytes you actually have before you index into the body
  - A handshake type you do not know is unexpected_message(10) — a server cannot skip it the way it skips an unknown extension, because it has no idea what state it would be in afterwards
  - A message whose body does not decode is decode_error(50); a message that decodes but is not legal here is illegal_parameter(47)
  - A truncated message is not an error until the connection ends: more bytes may still be coming, so wait, then fail cleanly
- [ ] **Stage 38** — Renegotiation is gone (`src/stages/s38_no_renegotiation.rs`, 7 tests)
  - TLS 1.3 removed renegotiation: there is no HelloRequest, and a ClientHello after the handshake is unexpected_message(10)
  - Key rotation is KeyUpdate and nothing else; a peer asking for new keys is not asking to renegotiate anything
  - The renegotiation_info extension and the SCSV may still arrive from old clients — ignore them, do not act on them
  - Post-handshake authentication exists, but only when the client offered post_handshake_auth in its hello; without it a CertificateRequest is illegal
- [ ] **Stage 39** — Fifty connections at once **[ext]** (`src/stages/s39_concurrency.rs`, 6 tests)
  - Every connection needs its own state: keys, sequence numbers, transcript and read buffer. Anything shared between them is a bug waiting for load
  - A server may serve connections one at a time or in parallel; both are conformant, and both must finish all fifty
  - Never let one slow or stuck client stop the accept loop — a listen backlog is not a queue you can ignore
  - Close each connection's socket when it ends rather than when the process does, or a long-lived server runs out of file descriptors
- [ ] **Stage 40** — Fuzz: four hundred mutated hellos **[ext]** (`src/stages/s40_fuzz.rs`, 6 tests)
  - Validate every length against the bytes you actually have before you index — that one rule survives most of this stage
  - A malformed input is at worst a closed connection: never a panic, never an unbounded allocation, never a loop that does not end
  - The accept loop has to outlive every bad connection; test that by sending a clean handshake afterwards, which is what this stage does
  - Everything here comes from --seed, so a failure is reproducible: the report names the round and the mutation that broke it

## G. Advanced

- [ ] **Stage 41** — HelloRetryRequest **[ext]** (`src/stages/s41_hello_retry_request.rs`, 8 tests)
  - A HelloRetryRequest *is* a ServerHello: same message type, same fields, and a random fixed at SHA-256("HelloRetryRequest")
  - Its key_share carries only a selected_group and no share; the client answers with a second ClientHello carrying a share for that group
  - Before the retry goes into the transcript, ClientHello1 is replaced by the synthetic `message_hash` message of RFC 8446 section 4.4.1 — this is the one rule nobody guesses
  - A cookie, if one is sent, must be echoed back verbatim in the second hello and nowhere else
- [ ] **Stage 42** — KeyUpdate **[ext]** (`src/stages/s42_key_update.rs`, 7 tests)
  - The body is one byte: update_not_requested(0) or update_requested(1)
  - The new secret is HKDF-Expand-Label(old secret, "traffic upd", "", Hash.length); key and iv are expanded from it exactly as before
  - Only the sender's own direction changes, and the sequence number goes back to zero; the KeyUpdate itself is the last record under the old keys
  - update_requested(1) obliges the peer to send its own KeyUpdate back — and that answer must say update_not_requested, or two peers would ping-pong for ever
- [ ] **Stage 43** — NewSessionTicket and resumption **[ext]** (`src/stages/s43_resumption.rs`, 8 tests)
  - A ticket's PSK is HKDF-Expand-Label(resumption_master_secret, "resumption", ticket_nonce, Hash.length) — the nonce is what makes two tickets from one connection different
  - pre_shared_key must be the *last* extension of the ClientHello, because the binder is computed over everything before it
  - binder = HMAC(finished_key(binder_key), Transcript-Hash(truncated ClientHello)), where the truncation stops just before the binder list's own length
  - A resumed handshake sends no Certificate and no CertificateVerify: the PSK is the authentication
- [ ] **Stage 44** — Early data, offered and refused **[ext]** (`src/stages/s44_early_data.rs`, 7 tests)
  - Early data is only on offer when the *ticket* said so: a NewSessionTicket with no early_data extension means 0-RTT is not available on it
  - A client may send early_data in its hello anyway; a server that does not want it simply leaves early_data out of EncryptedExtensions
  - When early data is refused there is no EndOfEarlyData message — that only exists on a connection where the server accepted it
  - Refusing is always safe and often right: 0-RTT data is replayable, and RFC 8446 appendix E.5 spends several pages on why
- [ ] **Stage 45** — Real-client interop **[ext]** (`src/stages/s45_interop.rs`, 9 tests)
  - The suite's own client is one implementation; a second, independent one is the only way to find the places where both of yours agree and the RFC does not
  - `openssl s_client -tls1_3 -connect host:port` is the shortest full TLS 1.3 client there is, and it prints the negotiated parameters
  - Its `-ciphersuites` and `-groups` flags pin the negotiation, so each of the three suites and both groups can be exercised from the outside
  - `-sess_out` then `-sess_in` proves resumption end to end: the second run prints `Reused` when the ticket was accepted

## H. Client authentication [ext]

A TLS server may ask the client to prove who it is, and RFC 8446 turns the handshake around
to do it: the same Certificate and CertificateVerify messages, sent by the client, signed
under a different context string. These three stages are that mirror image, and the policy
decision behind it — request, or require.

- [ ] **Stage 46** — CertificateRequest **[ext]** (`src/stages/s46_client_certificate_request.rs`, 8 tests)
  - CertificateRequest is handshake type 13 and belongs in the encrypted flight, after EncryptedExtensions and before the server's own Certificate
  - Its body is certificate_request_context (opaque<0..255>, empty during a handshake) followed by an extensions block — there is no certificate_types or supported_signature_algorithms list any more, those were TLS 1.2
  - signature_algorithms is mandatory in it: without it the client has no way to know what it may sign with, so an empty extensions block is an illegal message
  - Sending it at all is a choice; a server that never wants client certificates simply omits the message and nothing else about the handshake changes
- [ ] **Stage 47** — The client's Certificate and CertificateVerify **[ext]** (`src/stages/s47_client_certificate.rs`, 8 tests)
  - Both messages go out under the *client* handshake traffic keys, after the server's Finished and before the client's own
  - The client's Certificate echoes the certificate_request_context byte for byte — empty during a handshake, but echoing is the rule
  - CertificateVerify signs 64 spaces, then "TLS 1.3, client CertificateVerify", then a zero byte, then the transcript hash up to and including the client's Certificate — a different context string from the server's, on purpose
  - The client's Certificate and CertificateVerify are in the transcript, so the Finished that follows covers them and its verify_data differs from an unauthenticated handshake's
- [ ] **Stage 48** — Declining, and requesting against requiring **[ext]** (`src/stages/s48_client_auth_policy.rs`, 6 tests)
  - Declining is an empty Certificate, not silence: the client still sends the message, with a zero-length certificate_list, and no CertificateVerify after it
  - A server that only *requests* must carry on with an unauthenticated connection — the handshake completes and application data flows
  - A server that *requires* must fail the handshake, and certificate_required(116) is the alert that says why; handshake_failure loses that information
  - An empty Certificate is followed by Finished and nothing else — a CertificateVerify with no certificate to verify is an illegal message
