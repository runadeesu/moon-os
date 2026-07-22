//! A minimal, client-only TCP: just enough of the real state machine (SYN,
//! SYN-ACK, ACK, PSH+ACK data transfer, FIN) to open one connection, send
//! one request, and read back one response -- what `net::http` needs for
//! the Browser app.
//!
//! What's genuinely real here: the three-way handshake with real sequence
//! numbers, a real pseudo-header checksum (peers drop segments with a bad
//! one, so this has to be right), in-order data delivery with real ACKs,
//! and a real four-way close. What's deliberately *not* here: retransmission
//! on packet loss, congestion/flow control, out-of-order reassembly, window
//! scaling, or a listening/server side -- a general-purpose TCP (RFC 9293)
//! is a project on the scale of this whole network stack again. This one
//! assumes a well-behaved peer and a lossless link (true for QEMU's SLIRP
//! loopback-ish NAT), same "poll-driven, bounded, good enough to prove the
//! stack works end to end" tradeoff as `icmp::ping`/`dns::query_a`. It also
//! only ever tracks one connection at a time -- fine for "the Browser app
//! fetches one page," not fine for anything wanting concurrent sockets.

use super::Ipv4Addr;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU16, Ordering};
use spin::Mutex;

const FLAG_FIN: u8 = 1 << 0;
const FLAG_SYN: u8 = 1 << 1;
const FLAG_ACK: u8 = 1 << 4;

const POLL_SPIN_LIMIT: u32 = 3_000_000;
const DEFAULT_WINDOW: u16 = 8192;

struct RecvSeg {
    flags: u8,
    seq: u32,
    payload: Vec<u8>,
}

/// The one connection we might currently be listening for -- keyed by our
/// own local port, since that's unique per `connect()` call and there's
/// only ever one active stream.
static LISTEN_PORT: Mutex<u16> = Mutex::new(0);
static INBOX: Mutex<Option<RecvSeg>> = Mutex::new(None);

/// The four addresses/ports identifying one end-to-end segment -- bundled
/// so `build_segment` stays under clippy's argument-count limit.
struct Endpoints {
    src_ip: Ipv4Addr,
    dst_ip: Ipv4Addr,
    src_port: u16,
    dst_port: u16,
}

fn build_segment(ep: &Endpoints, seq: u32, ack: u32, flags: u8, payload: &[u8]) -> Vec<u8> {
    let mut seg = Vec::with_capacity(20 + payload.len());
    seg.extend_from_slice(&ep.src_port.to_be_bytes());
    seg.extend_from_slice(&ep.dst_port.to_be_bytes());
    seg.extend_from_slice(&seq.to_be_bytes());
    seg.extend_from_slice(&ack.to_be_bytes());
    seg.push(5 << 4); // data offset: 5 words (20 bytes), no options
    seg.push(flags);
    seg.extend_from_slice(&DEFAULT_WINDOW.to_be_bytes());
    seg.extend_from_slice(&0u16.to_be_bytes()); // checksum placeholder
    seg.extend_from_slice(&0u16.to_be_bytes()); // urgent pointer
    seg.extend_from_slice(payload);

    let mut pseudo = Vec::with_capacity(12 + seg.len());
    pseudo.extend_from_slice(&ep.src_ip);
    pseudo.extend_from_slice(&ep.dst_ip);
    pseudo.push(0);
    pseudo.push(super::ipv4::PROTO_TCP);
    pseudo.extend_from_slice(&(seg.len() as u16).to_be_bytes());
    pseudo.extend_from_slice(&seg);
    let csum = super::ipv4::checksum(&pseudo);
    seg[16] = (csum >> 8) as u8;
    seg[17] = csum as u8;
    seg
}

/// Dispatched from `ipv4::handle`. Only keeps a segment if it's addressed
/// to whatever local port `connect`/the active stream is currently
/// listening on -- everything else (unrelated traffic, a stale connection's
/// leftovers) is silently dropped.
pub fn handle(_src_ip: Ipv4Addr, data: &[u8]) {
    if data.len() < 20 {
        return;
    }
    let dst_port = u16::from_be_bytes([data[2], data[3]]);
    if dst_port == 0 || dst_port != *LISTEN_PORT.lock() {
        return;
    }
    let seq = u32::from_be_bytes([data[4], data[5], data[6], data[7]]);
    let data_offset = ((data[12] >> 4) as usize) * 4;
    let flags = data[13];
    let payload = data.get(data_offset..).unwrap_or(&[]).to_vec();
    *INBOX.lock() = Some(RecvSeg {
        flags,
        seq,
        payload,
    });
}

fn alloc_local_port() -> u16 {
    static NEXT: AtomicU16 = AtomicU16::new(49152);
    let port = NEXT.fetch_add(1, Ordering::Relaxed);
    if port < 49152 {
        49152
    } else {
        port
    }
}

/// A pseudo-random-enough initial sequence number: real TCP wants
/// unpredictability for security, but the only property that actually
/// matters for a single well-behaved local connection is "probably not the
/// same as last time," which the tick counter already gives us.
fn initial_seq() -> u32 {
    (crate::sched::ticks() as u32)
        .wrapping_mul(2_654_435_761)
        .wrapping_add(1)
}

pub struct TcpStream {
    local_port: u16,
    remote_ip: Ipv4Addr,
    remote_port: u16,
    seq: u32,
    ack: u32,
}

/// Opens a TCP connection via the real three-way handshake, blocking
/// (bounded) for the SYN-ACK. Returns `None` on any timeout/failure.
pub fn connect(remote_ip: Ipv4Addr, remote_port: u16) -> Option<TcpStream> {
    let local_port = alloc_local_port();
    *LISTEN_PORT.lock() = local_port;
    *INBOX.lock() = None;
    let our_ip = super::our_ip();

    let ep = Endpoints {
        src_ip: our_ip,
        dst_ip: remote_ip,
        src_port: local_port,
        dst_port: remote_port,
    };

    let isn = initial_seq();
    let syn = build_segment(&ep, isn, 0, FLAG_SYN, &[]);
    if !super::ipv4::send(remote_ip, super::ipv4::PROTO_TCP, &syn) {
        return None;
    }

    let mut synack = None;
    for _ in 0..POLL_SPIN_LIMIT {
        super::poll_once();
        if let Some(seg) = INBOX.lock().take() {
            if seg.flags & (FLAG_SYN | FLAG_ACK) == (FLAG_SYN | FLAG_ACK) {
                synack = Some(seg);
                break;
            }
        }
    }
    let synack = synack?;

    let seq = isn.wrapping_add(1);
    let ack = synack.seq.wrapping_add(1);
    let ack_seg = build_segment(&ep, seq, ack, FLAG_ACK, &[]);
    super::ipv4::send(remote_ip, super::ipv4::PROTO_TCP, &ack_seg);

    Some(TcpStream {
        local_port,
        remote_ip,
        remote_port,
        seq,
        ack,
    })
}

impl TcpStream {
    /// Sends `data` as one PSH+ACK segment. Fine for a single HTTP request
    /// (a few hundred bytes) -- there's no MSS-based segmentation for
    /// larger payloads.
    pub fn send(&mut self, data: &[u8]) -> bool {
        let ep = Endpoints {
            src_ip: super::our_ip(),
            dst_ip: self.remote_ip,
            src_port: self.local_port,
            dst_port: self.remote_port,
        };
        let seg = build_segment(&ep, self.seq, self.ack, FLAG_PSH_ACK, data);
        if !super::ipv4::send(self.remote_ip, super::ipv4::PROTO_TCP, &seg) {
            return false;
        }
        self.seq = self.seq.wrapping_add(data.len() as u32);
        true
    }

    /// Reads segments until the peer sends FIN (or we time out), ACKing
    /// each one and politely closing our side back once the peer's FIN
    /// arrives. Out-of-order segments are dropped rather than reassembled
    /// -- acceptable for a small HTTP response arriving over a local NAT
    /// link, not a general answer to real-world reordering/loss.
    pub fn read_to_end(&mut self) -> Vec<u8> {
        let mut body = Vec::new();
        for _ in 0..POLL_SPIN_LIMIT {
            super::poll_once();
            let Some(seg) = INBOX.lock().take() else {
                continue;
            };
            if seg.seq != self.ack {
                continue; // out-of-order or duplicate; no reorder buffer
            }
            let ep = Endpoints {
                src_ip: super::our_ip(),
                dst_ip: self.remote_ip,
                src_port: self.local_port,
                dst_port: self.remote_port,
            };
            if !seg.payload.is_empty() {
                body.extend_from_slice(&seg.payload);
                self.ack = self.ack.wrapping_add(seg.payload.len() as u32);
                let ack_seg = build_segment(&ep, self.seq, self.ack, FLAG_ACK, &[]);
                super::ipv4::send(self.remote_ip, super::ipv4::PROTO_TCP, &ack_seg);
            }
            if seg.flags & FLAG_FIN != 0 {
                self.ack = self.ack.wrapping_add(1);
                let close = build_segment(&ep, self.seq, self.ack, FLAG_FIN | FLAG_ACK, &[]);
                super::ipv4::send(self.remote_ip, super::ipv4::PROTO_TCP, &close);
                self.seq = self.seq.wrapping_add(1);
                break;
            }
        }
        body
    }
}

const FLAG_PSH_ACK: u8 = (1 << 3) | FLAG_ACK;
