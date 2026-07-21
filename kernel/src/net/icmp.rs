//! ICMP: echo request/reply (ping), both directions -- we answer pings
//! aimed at us and can send our own to verify the stack end to end.

use super::Ipv4Addr;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU16, Ordering};

const TYPE_ECHO_REPLY: u8 = 0;
const TYPE_ECHO_REQUEST: u8 = 8;

const PING_SPIN_LIMIT: u32 = 1_000_000;

/// Sequence number of the last echo reply we saw, so `ping` can tell its own
/// reply apart from an unrelated one. `None` means no reply seen yet.
static LAST_REPLY: AtomicU16 = AtomicU16::new(u16::MAX);

fn build_echo(ty: u8, id: u16, seq: u16, payload: &[u8]) -> Vec<u8> {
    let mut packet = Vec::with_capacity(8 + payload.len());
    packet.push(ty);
    packet.push(0); // code
    packet.extend_from_slice(&0u16.to_be_bytes()); // checksum placeholder
    packet.extend_from_slice(&id.to_be_bytes());
    packet.extend_from_slice(&seq.to_be_bytes());
    packet.extend_from_slice(payload);
    let csum = super::ipv4::checksum(&packet);
    packet[2] = (csum >> 8) as u8;
    packet[3] = csum as u8;
    packet
}

pub fn handle(src: Ipv4Addr, data: &[u8]) {
    if data.len() < 8 {
        return;
    }
    match data[0] {
        TYPE_ECHO_REQUEST => {
            let id = u16::from_be_bytes([data[4], data[5]]);
            let seq = u16::from_be_bytes([data[6], data[7]]);
            let reply = build_echo(TYPE_ECHO_REPLY, id, seq, &data[8..]);
            super::ipv4::send(src, super::ipv4::PROTO_ICMP, &reply);
        }
        TYPE_ECHO_REPLY => {
            let seq = u16::from_be_bytes([data[6], data[7]]);
            crate::serial_println!(
                "icmp: echo reply from {} seq={}",
                super::format_ip(src),
                seq
            );
            LAST_REPLY.store(seq, Ordering::Relaxed);
        }
        _ => {}
    }
}

/// Sends one echo request and blocks (bounded) for the matching reply.
pub fn ping(dst: Ipv4Addr, id: u16, seq: u16) -> bool {
    LAST_REPLY.store(u16::MAX, Ordering::Relaxed);
    let packet = build_echo(TYPE_ECHO_REQUEST, id, seq, b"moonOS-ping");
    if !super::ipv4::send(dst, super::ipv4::PROTO_ICMP, &packet) {
        return false;
    }
    for _ in 0..PING_SPIN_LIMIT {
        super::poll_once();
        if LAST_REPLY.load(Ordering::Relaxed) == seq {
            return true;
        }
    }
    false
}
