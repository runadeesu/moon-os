//! IPv4: header build/parse, the one's-complement checksum every layer here
//! reuses, and outgoing-packet routing (resolving the destination MAC via
//! ARP, blocking on the resolution if necessary).

use super::Ipv4Addr;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU16, Ordering};

pub const PROTO_ICMP: u8 = 1;
pub const PROTO_TCP: u8 = 6;
pub const PROTO_UDP: u8 = 17;

const ARP_RESOLVE_SPIN_LIMIT: u32 = 500_000;

/// The standard Internet checksum: ones'-complement sum of 16-bit words,
/// folded and complemented. Used as-is for the IPv4 header, and (with a
/// pseudo-header prepended) for ICMP and UDP too.
pub fn checksum(data: &[u8]) -> u16 {
    let mut sum: u32 = 0;
    let mut chunks = data.chunks_exact(2);
    for chunk in &mut chunks {
        sum += u32::from(u16::from_be_bytes([chunk[0], chunk[1]]));
    }
    if let [last] = chunks.remainder() {
        sum += u32::from(*last) << 8;
    }
    while sum >> 16 != 0 {
        sum = (sum & 0xFFFF) + (sum >> 16);
    }
    !(sum as u16)
}

fn build_header(
    src: Ipv4Addr,
    dst: Ipv4Addr,
    protocol: u8,
    payload_len: u16,
    ident: u16,
) -> Vec<u8> {
    let total_len = 20 + payload_len;
    let mut header = Vec::with_capacity(20);
    header.push(0x45); // version 4, IHL 5 (20 bytes, no options)
    header.push(0x00); // DSCP/ECN
    header.extend_from_slice(&total_len.to_be_bytes());
    header.extend_from_slice(&ident.to_be_bytes());
    header.extend_from_slice(&0u16.to_be_bytes()); // flags/fragment offset: none, not fragmented
    header.push(64); // TTL
    header.push(protocol);
    header.extend_from_slice(&0u16.to_be_bytes()); // checksum placeholder
    header.extend_from_slice(&src);
    header.extend_from_slice(&dst);
    let csum = checksum(&header);
    header[10] = (csum >> 8) as u8;
    header[11] = csum as u8;
    header
}

pub struct ParsedHeader {
    pub src: Ipv4Addr,
    pub dst: Ipv4Addr,
    pub protocol: u8,
    pub header_len: usize,
    /// The header's own declared Total Length -- the real end of the IP
    /// packet. Ethernet pads short frames up to a 60-byte minimum, so
    /// `data.len()` alone can't be trusted for a short UDP/TCP payload
    /// (e.g. a bare SYN-ACK or FIN with no data): without truncating to
    /// this, trailing zero padding bytes get misread as real payload by
    /// whatever's above IP.
    pub total_len: usize,
}

pub fn parse_header(data: &[u8]) -> Option<ParsedHeader> {
    if data.len() < 20 {
        return None;
    }
    let ihl = (data[0] & 0x0F) as usize * 4;
    if data.len() < ihl {
        return None;
    }
    let mut src = [0u8; 4];
    src.copy_from_slice(&data[12..16]);
    let mut dst = [0u8; 4];
    dst.copy_from_slice(&data[16..20]);
    let total_len = u16::from_be_bytes([data[2], data[3]]) as usize;
    Some(ParsedHeader {
        src,
        dst,
        protocol: data[9],
        header_len: ihl,
        total_len: total_len.min(data.len()),
    })
}

pub fn handle(payload: &[u8]) {
    let Some(hdr) = parse_header(payload) else {
        return;
    };
    let our_ip = super::our_ip();
    // While still unconfigured (e.g. mid-DHCP), we can't yet know what's
    // "ours" -- accept everything the NIC's own MAC filter already let through.
    if our_ip != super::UNSPECIFIED_IP && hdr.dst != our_ip && hdr.dst != super::BROADCAST_IP {
        return;
    }
    if hdr.total_len < hdr.header_len {
        return;
    }
    let body = &payload[hdr.header_len..hdr.total_len];
    match hdr.protocol {
        PROTO_ICMP => super::icmp::handle(hdr.src, body),
        PROTO_TCP => super::tcp::handle(hdr.src, body),
        PROTO_UDP => super::udp::handle(hdr.src, body),
        _ => {}
    }
}

/// `true` if `ip` is on the same subnet as our own configured address --
/// the difference between "ARP the destination directly" and "ARP the
/// gateway and let it route" below.
fn same_subnet(ip: Ipv4Addr) -> bool {
    let our = super::our_ip();
    let mask = super::subnet_mask();
    (0..4).all(|i| (our[i] & mask[i]) == (ip[i] & mask[i]))
}

/// Builds and sends one IPv4 packet, resolving the next hop's MAC via ARP
/// first if it isn't already cached (blocking on the reply, bounded). The
/// next hop is `dst_ip` itself when it's on our subnet, or the default
/// gateway otherwise -- without this, anything addressed off-subnet (every
/// real Internet host `net::http` talks to) would ARP a destination that
/// has no reason to ever answer an ARP request on our local Ethernet
/// segment, and just time out. Returns whether the frame was actually sent.
pub fn send(dst_ip: Ipv4Addr, protocol: u8, payload: &[u8]) -> bool {
    static IDENT: AtomicU16 = AtomicU16::new(1);

    let next_hop = if dst_ip == super::BROADCAST_IP || same_subnet(dst_ip) {
        dst_ip
    } else {
        super::gateway()
    };

    let mac = if dst_ip == super::BROADCAST_IP {
        super::BROADCAST_MAC
    } else if let Some(mac) = super::arp::lookup(next_hop) {
        mac
    } else {
        super::arp::send_request(next_hop);
        let mut resolved = None;
        for _ in 0..ARP_RESOLVE_SPIN_LIMIT {
            super::poll_once();
            if let Some(mac) = super::arp::lookup(next_hop) {
                resolved = Some(mac);
                break;
            }
        }
        match resolved {
            Some(mac) => mac,
            None => {
                crate::serial_println!(
                    "ipv4: ARP resolution for next hop {} timed out",
                    super::format_ip(next_hop)
                );
                return false;
            }
        }
    };

    let ident = IDENT.fetch_add(1, Ordering::Relaxed);
    let mut packet = build_header(
        super::our_ip(),
        dst_ip,
        protocol,
        payload.len() as u16,
        ident,
    );
    packet.extend_from_slice(payload);
    super::send_frame(mac, 0x0800, &packet);
    true
}
