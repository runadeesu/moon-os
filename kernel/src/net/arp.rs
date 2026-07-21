//! Address Resolution Protocol: resolves IPv4 addresses to Ethernet MAC
//! addresses, and answers other hosts' requests for our own address.

use super::{Ipv4Addr, MacAddr};
use alloc::collections::BTreeMap;
use alloc::vec::Vec;
use spin::Mutex;

const HTYPE_ETHERNET: u16 = 1;
const PTYPE_IPV4: u16 = 0x0800;
const OP_REQUEST: u16 = 1;
const OP_REPLY: u16 = 2;

static CACHE: Mutex<BTreeMap<Ipv4Addr, MacAddr>> = Mutex::new(BTreeMap::new());

pub fn lookup(ip: Ipv4Addr) -> Option<MacAddr> {
    CACHE.lock().get(&ip).copied()
}

fn build(
    op: u16,
    sender_mac: MacAddr,
    sender_ip: Ipv4Addr,
    target_mac: MacAddr,
    target_ip: Ipv4Addr,
) -> Vec<u8> {
    let mut packet = Vec::with_capacity(28);
    packet.extend_from_slice(&HTYPE_ETHERNET.to_be_bytes());
    packet.extend_from_slice(&PTYPE_IPV4.to_be_bytes());
    packet.push(6); // hardware address length
    packet.push(4); // protocol address length
    packet.extend_from_slice(&op.to_be_bytes());
    packet.extend_from_slice(&sender_mac);
    packet.extend_from_slice(&sender_ip);
    packet.extend_from_slice(&target_mac);
    packet.extend_from_slice(&target_ip);
    packet
}

pub fn send_request(target_ip: Ipv4Addr) {
    let packet = build(OP_REQUEST, super::mac(), super::our_ip(), [0; 6], target_ip);
    super::send_frame(super::BROADCAST_MAC, 0x0806, &packet);
}

pub fn handle(payload: &[u8]) {
    if payload.len() < 28 {
        return;
    }
    let op = u16::from_be_bytes([payload[6], payload[7]]);
    let mut sender_mac = [0u8; 6];
    sender_mac.copy_from_slice(&payload[8..14]);
    let mut sender_ip = [0u8; 4];
    sender_ip.copy_from_slice(&payload[14..18]);
    let mut target_ip = [0u8; 4];
    target_ip.copy_from_slice(&payload[24..28]);

    CACHE.lock().insert(sender_ip, sender_mac);

    let our_ip = super::our_ip();
    if op == OP_REQUEST && our_ip != super::UNSPECIFIED_IP && target_ip == our_ip {
        let reply = build(OP_REPLY, super::mac(), our_ip, sender_mac, sender_ip);
        super::send_frame(sender_mac, 0x0806, &reply);
    }
}
