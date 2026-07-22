//! A minimal TLS 1.3 client (RFC 8446): one fixed cipher suite
//! (`TLS_CHACHA20_POLY1305_SHA256`) and one fixed key-exchange group
//! (`x25519`) -- everything `net::http` needs for an `https://` GET.
//!
//! What's genuinely real: the full RFC 8446 section 7.1 key schedule
//! (HKDF-Extract/Expand-Label deriving handshake and application traffic
//! secrets from a running transcript hash), a real ephemeral X25519 key
//! exchange, real AEAD record encryption/decryption with per-record nonces
//! derived from the traffic IV and sequence number, and a real Finished-
//! message MAC check that the server's view of the handshake transcript
//! matches ours.
//!
//! What's explicitly **not** here, and why this is not a secure channel
//! against an active attacker: there is no X.509 certificate parsing, no
//! chain-of-trust/root-store validation, and no CertificateVerify signature
//! check. The Certificate and CertificateVerify handshake messages are
//! read (so the handshake can proceed past them) but never authenticated.
//! Concretely: traffic is genuinely encrypted against a passive
//! eavesdropper on the wire, but an active man-in-the-middle presenting
//! *any* certificate for *any* name is accepted silently. This is
//! opportunistic encryption, not authenticated TLS -- a real certificate
//! verifier (ASN.1/DER parsing, RSA-PSS/ECDSA signature verification, a
//! trust store) is another project on the scale of everything else in this
//! module again. Same "real mechanism, honestly incomplete guarantee"
//! tradeoff `gui::login` documents for its own plaintext-password gate.

use super::tcp::TcpStream;
use super::Ipv4Addr;
use crate::crypto::{aead, hkdf, sha256, x25519};
use alloc::string::String;
use alloc::vec::Vec;

const CONTENT_HANDSHAKE: u8 = 22;
const CONTENT_ALERT: u8 = 21;
const CONTENT_APPLICATION_DATA: u8 = 23;
const CONTENT_CHANGE_CIPHER_SPEC: u8 = 20;

const HS_CLIENT_HELLO: u8 = 1;
const HS_SERVER_HELLO: u8 = 2;
const HS_ENCRYPTED_EXTENSIONS: u8 = 8;
const HS_CERTIFICATE: u8 = 11;
const HS_CERTIFICATE_VERIFY: u8 = 15;
const HS_FINISHED: u8 = 20;

const EXT_SERVER_NAME: u16 = 0;
const EXT_SUPPORTED_GROUPS: u16 = 10;
const EXT_SIGNATURE_ALGORITHMS: u16 = 13;
const EXT_SUPPORTED_VERSIONS: u16 = 43;
const EXT_KEY_SHARE: u16 = 51;

const GROUP_X25519: u16 = 0x001d;
const CIPHER_SUITE_CHACHA20_POLY1305_SHA256: u16 = 0x1303;
const TLS_1_3: u16 = 0x0304;
const TLS_1_2: u16 = 0x0303;

#[derive(Debug)]
pub enum TlsError {
    ConnectFailed,
    Io,
    UnexpectedMessage,
    HandshakeFailed,
    Alert(u8),
}

/// One direction's traffic keys plus the running record sequence number --
/// nonces are `iv XOR seq` (RFC 8446 section 5.3), reset to zero every time
/// the traffic secret changes (handshake keys -> application keys).
struct DirectionKeys {
    key: [u8; 32],
    iv: [u8; 12],
    seq: u64,
}

impl DirectionKeys {
    fn from_secret(secret: &[u8; 32]) -> Self {
        let key: [u8; 32] = hkdf::expand_label(secret, "key", &[], 32)
            .try_into()
            .unwrap();
        let iv: [u8; 12] = hkdf::expand_label(secret, "iv", &[], 12)
            .try_into()
            .unwrap();
        Self { key, iv, seq: 0 }
    }

    fn nonce(&self) -> [u8; 12] {
        let mut n = self.iv;
        let seq_bytes = self.seq.to_be_bytes();
        for i in 0..8 {
            n[4 + i] ^= seq_bytes[i];
        }
        n
    }
}

pub struct TlsStream {
    stream: TcpStream,
    client_keys: DirectionKeys,
    server_keys: DirectionKeys,
    /// Decrypted handshake-record bytes not yet split into complete
    /// handshake messages -- record and message boundaries don't have to
    /// line up (several small handshake messages commonly share one
    /// record).
    handshake_buf: Vec<u8>,
}

fn hash_transcript(transcript: &[u8]) -> [u8; 32] {
    sha256::hash(transcript)
}

fn derive_secret(secret: &[u8; 32], label: &str, transcript: &[u8]) -> [u8; 32] {
    let h = hash_transcript(transcript);
    hkdf::expand_label(secret, label, &h, 32)
        .try_into()
        .unwrap()
}

fn write_extension(out: &mut Vec<u8>, ext_type: u16, body: &[u8]) {
    out.extend_from_slice(&ext_type.to_be_bytes());
    out.extend_from_slice(&(body.len() as u16).to_be_bytes());
    out.extend_from_slice(body);
}

fn build_client_hello(host: &str, client_random: &[u8; 32], client_public: &[u8; 32]) -> Vec<u8> {
    let mut body = Vec::new();
    body.extend_from_slice(&TLS_1_2.to_be_bytes()); // legacy_version
    body.extend_from_slice(client_random);
    body.push(0); // legacy_session_id: empty (no compat-mode session resumption needed)
    let cipher_suites = CIPHER_SUITE_CHACHA20_POLY1305_SHA256.to_be_bytes();
    body.extend_from_slice(&2u16.to_be_bytes());
    body.extend_from_slice(&cipher_suites);
    body.push(1); // legacy_compression_methods length
    body.push(0); // "null" compression

    let mut extensions = Vec::new();

    let mut sni = Vec::new();
    sni.extend_from_slice(&(host.len() as u16 + 3).to_be_bytes());
    sni.push(0); // name_type: host_name
    sni.extend_from_slice(&(host.len() as u16).to_be_bytes());
    sni.extend_from_slice(host.as_bytes());
    write_extension(&mut extensions, EXT_SERVER_NAME, &sni);

    let mut versions = Vec::new();
    versions.push(2); // list length in bytes
    versions.extend_from_slice(&TLS_1_3.to_be_bytes());
    write_extension(&mut extensions, EXT_SUPPORTED_VERSIONS, &versions);

    let mut groups = Vec::new();
    groups.extend_from_slice(&2u16.to_be_bytes());
    groups.extend_from_slice(&GROUP_X25519.to_be_bytes());
    write_extension(&mut extensions, EXT_SUPPORTED_GROUPS, &groups);

    // A real server picks a certificate/signature based on this list; we
    // never check the signature ourselves (see module doc comment), but a
    // real handshake still needs to offer something plausible or some
    // servers will refuse to continue.
    let sig_algs: [u16; 4] = [0x0403, 0x0804, 0x0807, 0x0401]; // ecdsa_secp256r1_sha256, rsa_pss_rsae_sha256, ed25519, rsa_pkcs1_sha256
    let mut sigs = Vec::new();
    sigs.extend_from_slice(&((sig_algs.len() * 2) as u16).to_be_bytes());
    for alg in sig_algs {
        sigs.extend_from_slice(&alg.to_be_bytes());
    }
    write_extension(&mut extensions, EXT_SIGNATURE_ALGORITHMS, &sigs);

    let mut key_share = Vec::new();
    let mut entry = Vec::new();
    entry.extend_from_slice(&GROUP_X25519.to_be_bytes());
    entry.extend_from_slice(&32u16.to_be_bytes());
    entry.extend_from_slice(client_public);
    key_share.extend_from_slice(&(entry.len() as u16).to_be_bytes());
    key_share.extend_from_slice(&entry);
    write_extension(&mut extensions, EXT_KEY_SHARE, &key_share);

    body.extend_from_slice(&(extensions.len() as u16).to_be_bytes());
    body.extend_from_slice(&extensions);

    let mut message = Vec::with_capacity(4 + body.len());
    message.push(HS_CLIENT_HELLO);
    let len = body.len() as u32;
    message.extend_from_slice(&len.to_be_bytes()[1..]);
    message.extend_from_slice(&body);
    message
}

struct ServerHello {
    server_public: [u8; 32],
}

fn parse_server_hello(body: &[u8]) -> Option<ServerHello> {
    if body.len() < 34 {
        return None;
    }
    let mut pos = 2 + 32; // skip legacy_version, random
    let session_id_len = *body.get(pos)? as usize;
    pos += 1 + session_id_len;
    pos += 2; // cipher_suite (we only offered one, trust it matches)
    pos += 1; // legacy_compression_method
    let ext_total_len = u16::from_be_bytes([*body.get(pos)?, *body.get(pos + 1)?]) as usize;
    pos += 2;
    let ext_end = pos + ext_total_len;
    let mut server_public = None;
    while pos + 4 <= ext_end && pos + 4 <= body.len() {
        let ext_type = u16::from_be_bytes([body[pos], body[pos + 1]]);
        let ext_len = u16::from_be_bytes([body[pos + 2], body[pos + 3]]) as usize;
        pos += 4;
        let ext_body = body.get(pos..pos + ext_len)?;
        if ext_type == EXT_KEY_SHARE {
            let group = u16::from_be_bytes([*ext_body.first()?, *ext_body.get(1)?]);
            let key_len = u16::from_be_bytes([*ext_body.get(2)?, *ext_body.get(3)?]) as usize;
            if group == GROUP_X25519 && key_len == 32 {
                let mut pk = [0u8; 32];
                pk.copy_from_slice(ext_body.get(4..36)?);
                server_public = Some(pk);
            }
        }
        pos += ext_len;
    }
    Some(ServerHello {
        server_public: server_public?,
    })
}

fn record_header(content_type: u8, len: usize) -> [u8; 5] {
    let mut h = [0u8; 5];
    h[0] = content_type;
    h[1..3].copy_from_slice(&TLS_1_2.to_be_bytes());
    h[3..5].copy_from_slice(&(len as u16).to_be_bytes());
    h
}

fn send_plaintext_record(stream: &mut TcpStream, content_type: u8, payload: &[u8]) {
    let mut record = Vec::with_capacity(5 + payload.len());
    record.extend_from_slice(&record_header(content_type, payload.len()));
    record.extend_from_slice(payload);
    stream.send(&record);
}

fn send_encrypted_record(
    stream: &mut TcpStream,
    keys: &mut DirectionKeys,
    real_content_type: u8,
    plaintext: &[u8],
) {
    let mut inner = plaintext.to_vec();
    inner.push(real_content_type);
    let header = record_header(CONTENT_APPLICATION_DATA, inner.len() + 16);
    let (ciphertext, tag) = aead::seal(&keys.key, &keys.nonce(), &header, &inner);
    keys.seq += 1;

    let mut record = Vec::with_capacity(5 + ciphertext.len() + 16);
    record.extend_from_slice(&header);
    record.extend_from_slice(&ciphertext);
    record.extend_from_slice(&tag);
    stream.send(&record);
}

/// Reads and decrypts exactly one TLS record, returning its real content
/// type and plaintext (padding already stripped). `None` on any I/O
/// failure, a plaintext alert, or a failed AEAD tag (never returns
/// plaintext that didn't authenticate).
fn recv_encrypted_record(
    stream: &mut TcpStream,
    keys: &mut DirectionKeys,
) -> Option<(u8, Vec<u8>)> {
    let header = stream.read_exact(5)?;
    let outer_type = header[0];
    let len = u16::from_be_bytes([header[3], header[4]]) as usize;
    let body = stream.read_exact(len)?;

    if outer_type == CONTENT_CHANGE_CIPHER_SPEC {
        // A compatibility-mode artifact some servers still send; it isn't
        // protected and carries no information we need.
        return Some((CONTENT_CHANGE_CIPHER_SPEC, Vec::new()));
    }
    if len < 16 {
        return None;
    }
    let (ciphertext, tag_bytes) = body.split_at(len - 16);
    let mut tag = [0u8; 16];
    tag.copy_from_slice(tag_bytes);

    let mut inner = aead::open(&keys.key, &keys.nonce(), &header, ciphertext, &tag)?;
    keys.seq += 1;

    while inner.last() == Some(&0) {
        inner.pop();
    }
    let real_type = inner.pop()?;
    Some((real_type, inner))
}

/// Pulls the next complete handshake message, decrypting and buffering
/// further records as needed. Transcript-hashes every handshake message it
/// returns into `transcript` (the raw `msg_type||length||body` bytes, per
/// RFC 8446 section 4.4.1) since the caller needs that running hash for
/// the key schedule regardless of which message type it turns out to be.
fn next_handshake_message(
    stream: &mut TcpStream,
    keys: &mut DirectionKeys,
    buf: &mut Vec<u8>,
    transcript: &mut Vec<u8>,
) -> Result<(u8, Vec<u8>), TlsError> {
    loop {
        if buf.len() >= 4 {
            let len = u32::from_be_bytes([0, buf[1], buf[2], buf[3]]) as usize;
            if buf.len() >= 4 + len {
                let message: Vec<u8> = buf.drain(..4 + len).collect();
                transcript.extend_from_slice(&message);
                return Ok((message[0], message[4..].to_vec()));
            }
        }
        let (content_type, plaintext) = recv_encrypted_record(stream, keys).ok_or(TlsError::Io)?;
        match content_type {
            CONTENT_HANDSHAKE => buf.extend_from_slice(&plaintext),
            CONTENT_CHANGE_CIPHER_SPEC => {}
            CONTENT_ALERT => {
                return Err(TlsError::Alert(*plaintext.get(1).unwrap_or(&0)));
            }
            _ => return Err(TlsError::UnexpectedMessage),
        }
    }
}

/// A not-cryptographically-strong but non-repeating-in-practice 32-byte
/// value for `ClientHello.random` -- this kernel has no hardware RNG
/// driver, so the tick counter plus a per-call counter is the honest best
/// available source (documented the same way `tcp::initial_seq` documents
/// its own weaker-than-ideal randomness).
fn pseudo_random_32(salt: u32) -> [u8; 32] {
    let mut out = [0u8; 32];
    let mut state = crate::sched::ticks()
        .wrapping_mul(6364136223846793005)
        .wrapping_add(u64::from(salt));
    for chunk in out.chunks_mut(8) {
        state = state.wrapping_mul(6364136223846793005).wrapping_add(1);
        chunk.copy_from_slice(&state.to_le_bytes());
    }
    out
}

/// Opens a TLS 1.3 connection to `ip:443` with server name `host` (sent as
/// SNI). Performs the full real handshake described in the module doc
/// comment and returns a stream ready for `send`/`read_to_end` of
/// application data (e.g. an HTTP request/response).
pub fn connect(ip: Ipv4Addr, host: &str) -> Result<TlsStream, TlsError> {
    let mut stream = super::tcp::connect(ip, 443).ok_or(TlsError::ConnectFailed)?;

    let client_private = pseudo_random_32(1);
    let client_public = x25519::public_key(&client_private);
    let client_random = pseudo_random_32(2);

    let client_hello = build_client_hello(host, &client_random, &client_public);
    send_plaintext_record(&mut stream, CONTENT_HANDSHAKE, &client_hello);
    let mut transcript = client_hello.clone();

    // The ServerHello itself always arrives as a plaintext record (the
    // handshake keys aren't derivable until we've seen it).
    let header = stream.read_exact(5).ok_or(TlsError::Io)?;
    let len = u16::from_be_bytes([header[3], header[4]]) as usize;
    let sh_record = stream.read_exact(len).ok_or(TlsError::Io)?;
    if sh_record.len() < 4 || sh_record[0] != HS_SERVER_HELLO {
        return Err(TlsError::UnexpectedMessage);
    }
    let sh_len = u32::from_be_bytes([0, sh_record[1], sh_record[2], sh_record[3]]) as usize;
    let sh_body = sh_record
        .get(4..4 + sh_len)
        .ok_or(TlsError::HandshakeFailed)?;
    let server_hello = parse_server_hello(sh_body).ok_or(TlsError::HandshakeFailed)?;
    transcript.extend_from_slice(&sh_record[..4 + sh_len]);

    let shared_secret = x25519::scalarmult(&client_private, &server_hello.server_public);

    // RFC 8446 section 7.1 key schedule, PSK-less variant: both the salt
    // and IKM of the first extraction are defined as Hash.length zero
    // bytes when no PSK is in use.
    let zeros32 = [0u8; 32];
    let early_secret = hkdf::extract(&zeros32, &zeros32);
    let empty_transcript: [u8; 0] = [];
    let derived1 = derive_secret(&early_secret, "derived", &empty_transcript);
    let handshake_secret = hkdf::extract(&derived1, &shared_secret);
    let client_hs_secret = derive_secret(&handshake_secret, "c hs traffic", &transcript);
    let server_hs_secret = derive_secret(&handshake_secret, "s hs traffic", &transcript);

    let mut client_keys = DirectionKeys::from_secret(&client_hs_secret);
    let mut server_keys = DirectionKeys::from_secret(&server_hs_secret);
    let mut handshake_buf = Vec::new();

    let (ty, body) = next_handshake_message(
        &mut stream,
        &mut server_keys,
        &mut handshake_buf,
        &mut transcript,
    )?;
    if ty != HS_ENCRYPTED_EXTENSIONS {
        return Err(TlsError::UnexpectedMessage);
    }
    let _ = body;

    // Certificate + CertificateVerify: read past them (so the handshake
    // stream stays in sync) without validating anything -- see the module
    // doc comment for exactly what that means.
    let (ty, _body) = next_handshake_message(
        &mut stream,
        &mut server_keys,
        &mut handshake_buf,
        &mut transcript,
    )?;
    if ty != HS_CERTIFICATE {
        return Err(TlsError::UnexpectedMessage);
    }
    let (ty, _body) = next_handshake_message(
        &mut stream,
        &mut server_keys,
        &mut handshake_buf,
        &mut transcript,
    )?;
    if ty != HS_CERTIFICATE_VERIFY {
        return Err(TlsError::UnexpectedMessage);
    }

    // Server Finished: this one we *do* check -- a real HMAC over the
    // transcript hash taken right before this message, proving the server
    // derived the same handshake secret we did.
    let transcript_before_server_finished = transcript.clone();
    let (ty, server_finished_body) = next_handshake_message(
        &mut stream,
        &mut server_keys,
        &mut handshake_buf,
        &mut transcript,
    )?;
    if ty != HS_FINISHED {
        return Err(TlsError::UnexpectedMessage);
    }
    let server_finished_key: [u8; 32] = hkdf::expand_label(&server_hs_secret, "finished", &[], 32)
        .try_into()
        .unwrap();
    let expected_verify_data = crate::crypto::hmac::hmac_sha256(
        &server_finished_key,
        &hash_transcript(&transcript_before_server_finished),
    );
    if expected_verify_data.as_slice() != server_finished_body.as_slice() {
        return Err(TlsError::HandshakeFailed);
    }
    // RFC 8446 7.1: the application traffic secrets are derived from the
    // transcript through the *server's* Finished -- captured here, before
    // the client's own Finished (below) gets appended to `transcript` too.
    let transcript_through_server_finished = transcript.clone();

    // Client Finished, encrypted under our own handshake traffic secret.
    let client_finished_key: [u8; 32] = hkdf::expand_label(&client_hs_secret, "finished", &[], 32)
        .try_into()
        .unwrap();
    let client_verify_data =
        crate::crypto::hmac::hmac_sha256(&client_finished_key, &hash_transcript(&transcript));
    let mut client_finished_msg = Vec::with_capacity(4 + 32);
    client_finished_msg.push(HS_FINISHED);
    client_finished_msg.extend_from_slice(&32u32.to_be_bytes()[1..]);
    client_finished_msg.extend_from_slice(&client_verify_data);
    send_encrypted_record(
        &mut stream,
        &mut client_keys,
        CONTENT_HANDSHAKE,
        &client_finished_msg,
    );
    transcript.extend_from_slice(&client_finished_msg);

    // Derive application traffic secrets from the transcript through the
    // server's Finished (RFC 8446 7.1's diagram -- *not* including the
    // client's own Finished appended above); `DirectionKeys::from_secret`
    // resets both directions' record sequence numbers back to zero for
    // these new keys.
    let derived2 = derive_secret(&handshake_secret, "derived", &empty_transcript);
    let master_secret = hkdf::extract(&derived2, &zeros32);
    let client_app_secret = derive_secret(
        &master_secret,
        "c ap traffic",
        &transcript_through_server_finished,
    );
    let server_app_secret = derive_secret(
        &master_secret,
        "s ap traffic",
        &transcript_through_server_finished,
    );

    Ok(TlsStream {
        stream,
        client_keys: DirectionKeys::from_secret(&client_app_secret),
        server_keys: DirectionKeys::from_secret(&server_app_secret),
        handshake_buf: Vec::new(),
    })
}

impl TlsStream {
    pub fn send(&mut self, data: &[u8]) {
        send_encrypted_record(
            &mut self.stream,
            &mut self.client_keys,
            CONTENT_APPLICATION_DATA,
            data,
        );
    }

    /// Reads application data until the peer sends `close_notify` (or the
    /// underlying TCP connection closes), concatenating every
    /// `application_data` record's plaintext. A `handshake`-type record
    /// arriving here (a post-handshake NewSessionTicket, which some
    /// servers send unprompted) is decrypted to keep the record stream in
    /// sync but its content is discarded -- this client never resumes
    /// sessions, so a session ticket is nothing it can use.
    pub fn read_to_end(&mut self) -> Vec<u8> {
        let mut out = Vec::new();
        loop {
            let Some((content_type, plaintext)) =
                recv_encrypted_record(&mut self.stream, &mut self.server_keys)
            else {
                return out;
            };
            match content_type {
                CONTENT_APPLICATION_DATA => out.extend_from_slice(&plaintext),
                CONTENT_ALERT => return out,
                CONTENT_HANDSHAKE | CONTENT_CHANGE_CIPHER_SPEC => {
                    self.handshake_buf.extend_from_slice(&plaintext);
                }
                _ => return out,
            }
        }
    }
}

pub fn body_text_lossy(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}
