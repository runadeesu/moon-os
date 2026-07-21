//! A minimal, one-shot DNS resolver: builds a single-question A-record
//! query, sends it over UDP, and parses just enough of the response to
//! pull out the first address. No caching, no retries -- this exists to
//! prove UDP round-trips work end to end, not to be a real resolver.

use super::Ipv4Addr;
use alloc::vec::Vec;

const DNS_PORT: u16 = 53;
const QUERY_PORT: u16 = 50000;
const QUERY_SPIN_LIMIT: u32 = 2_000_000;

fn build_query(id: u16, name: &str) -> Vec<u8> {
    let mut packet = Vec::with_capacity(32 + name.len());
    packet.extend_from_slice(&id.to_be_bytes());
    packet.extend_from_slice(&0x0100u16.to_be_bytes()); // standard query, recursion desired
    packet.extend_from_slice(&1u16.to_be_bytes()); // QDCOUNT
    packet.extend_from_slice(&0u16.to_be_bytes()); // ANCOUNT
    packet.extend_from_slice(&0u16.to_be_bytes()); // NSCOUNT
    packet.extend_from_slice(&0u16.to_be_bytes()); // ARCOUNT
    for label in name.split('.') {
        packet.push(label.len() as u8);
        packet.extend_from_slice(label.as_bytes());
    }
    packet.push(0); // root label
    packet.extend_from_slice(&1u16.to_be_bytes()); // QTYPE A
    packet.extend_from_slice(&1u16.to_be_bytes()); // QCLASS IN
    packet
}

/// Skips one DNS name (a sequence of length-prefixed labels, a compression
/// pointer, or a mix), returning the offset just past it.
fn skip_name(data: &[u8], mut i: usize) -> usize {
    while i < data.len() {
        let len = data[i];
        if len == 0 {
            return i + 1;
        }
        if len & 0xC0 == 0xC0 {
            return i + 2; // compression pointer: always exactly 2 bytes here
        }
        i += 1 + len as usize;
    }
    i
}

fn parse_response(data: &[u8], expected_id: u16) -> Option<Ipv4Addr> {
    if data.len() < 12 {
        return None;
    }
    if u16::from_be_bytes([data[0], data[1]]) != expected_id {
        return None;
    }
    let ancount = u16::from_be_bytes([data[6], data[7]]);
    if ancount == 0 {
        return None;
    }

    let mut i = skip_name(data, 12);
    i += 4; // QTYPE + QCLASS

    for _ in 0..ancount {
        if i >= data.len() {
            return None;
        }
        i = skip_name(data, i);
        if i + 10 > data.len() {
            return None;
        }
        let rtype = u16::from_be_bytes([data[i], data[i + 1]]);
        let rdlength = u16::from_be_bytes([data[i + 8], data[i + 9]]) as usize;
        i += 10;
        if rtype == 1 && rdlength == 4 && i + 4 <= data.len() {
            let mut ip = [0u8; 4];
            ip.copy_from_slice(&data[i..i + 4]);
            return Some(ip);
        }
        i += rdlength;
    }
    None
}

pub fn query_a(name: &str, dns_server: Ipv4Addr) -> Option<Ipv4Addr> {
    const QUERY_ID: u16 = 0xBEEF;
    let query = build_query(QUERY_ID, name);
    if !super::udp::send(QUERY_PORT, dns_server, DNS_PORT, &query) {
        return None;
    }

    for _ in 0..QUERY_SPIN_LIMIT {
        super::poll_once();
        if let Some(response) = super::udp::take(QUERY_PORT) {
            if let Some(ip) = parse_response(&response, QUERY_ID) {
                return Some(ip);
            }
        }
    }
    None
}
