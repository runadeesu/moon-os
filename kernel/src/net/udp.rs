//! UDP: header build/parse and a small inbox keyed by destination port, so
//! anything waiting on a reply (DHCP, DNS) can poll for it by the port it
//! sent from. The checksum is left zero (optional over IPv4) to keep this
//! simple; every peer we talk to (SLIRP's DHCP/DNS servers) accepts that.

use super::Ipv4Addr;
use alloc::collections::BTreeMap;
use alloc::vec::Vec;
use spin::Mutex;

static INBOX: Mutex<BTreeMap<u16, Vec<u8>>> = Mutex::new(BTreeMap::new());

fn build(src_port: u16, dst_port: u16, payload: &[u8]) -> Vec<u8> {
    let len = 8 + payload.len();
    let mut packet = Vec::with_capacity(len);
    packet.extend_from_slice(&src_port.to_be_bytes());
    packet.extend_from_slice(&dst_port.to_be_bytes());
    packet.extend_from_slice(&(len as u16).to_be_bytes());
    packet.extend_from_slice(&0u16.to_be_bytes()); // checksum: unused
    packet.extend_from_slice(payload);
    packet
}

pub fn handle(_src_ip: Ipv4Addr, data: &[u8]) {
    if data.len() < 8 {
        return;
    }
    let dst_port = u16::from_be_bytes([data[2], data[3]]);
    let payload = data[8..].to_vec();
    if dst_port == 68 {
        super::dhcp::handle(&payload);
    }
    INBOX.lock().insert(dst_port, payload);
}

/// Takes (and clears) whatever was last received addressed to `port`.
pub fn take(port: u16) -> Option<Vec<u8>> {
    INBOX.lock().remove(&port)
}

pub fn send(src_port: u16, dst_ip: Ipv4Addr, dst_port: u16, payload: &[u8]) -> bool {
    let packet = build(src_port, dst_port, payload);
    super::ipv4::send(dst_ip, super::ipv4::PROTO_UDP, &packet)
}
