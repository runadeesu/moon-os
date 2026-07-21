//! A minimal DHCP client: DISCOVER -> OFFER -> REQUEST -> ACK, just enough
//! to get a lease from QEMU's SLIRP (or any standard DHCPv4 server). No
//! renewal/rebinding -- this runs once at boot.

use super::Ipv4Addr;
use alloc::vec::Vec;
use spin::Mutex;

const CLIENT_PORT: u16 = 68;
const SERVER_PORT: u16 = 67;

const MAGIC_COOKIE: [u8; 4] = [99, 130, 83, 99];

const MSG_DISCOVER: u8 = 1;
const MSG_OFFER: u8 = 2;
const MSG_REQUEST: u8 = 3;
const MSG_ACK: u8 = 5;

const WAIT_SPIN_LIMIT: u32 = 2_000_000;

pub struct Lease {
    pub ip: Ipv4Addr,
    pub subnet: Ipv4Addr,
    pub router: Ipv4Addr,
    pub dns: Ipv4Addr,
}

#[derive(Clone, Copy)]
struct Pending {
    msg_type: u8,
    yiaddr: Ipv4Addr,
    subnet: Ipv4Addr,
    router: Ipv4Addr,
    dns: Ipv4Addr,
    server_id: Ipv4Addr,
}

static PENDING: Mutex<Option<Pending>> = Mutex::new(None);

fn build_packet(
    msg_type: u8,
    xid: u32,
    requested_ip: Option<Ipv4Addr>,
    server_id: Option<Ipv4Addr>,
) -> Vec<u8> {
    let mac = super::mac();
    let mut p = Vec::with_capacity(300);
    p.push(1); // op: BOOTREQUEST
    p.push(1); // htype: Ethernet
    p.push(6); // hlen
    p.push(0); // hops
    p.extend_from_slice(&xid.to_be_bytes());
    p.extend_from_slice(&0u16.to_be_bytes()); // secs
    p.extend_from_slice(&0x8000u16.to_be_bytes()); // flags: broadcast (we have no IP yet)
    p.extend_from_slice(&[0; 4]); // ciaddr
    p.extend_from_slice(&[0; 4]); // yiaddr
    p.extend_from_slice(&[0; 4]); // siaddr
    p.extend_from_slice(&[0; 4]); // giaddr
    p.extend_from_slice(&mac);
    p.extend_from_slice(&[0u8; 10]); // chaddr padding (16 bytes total)
    p.extend_from_slice(&[0u8; 64]); // sname
    p.extend_from_slice(&[0u8; 128]); // file
    p.extend_from_slice(&MAGIC_COOKIE);

    p.push(53);
    p.push(1);
    p.push(msg_type);
    if let Some(ip) = requested_ip {
        p.push(50);
        p.push(4);
        p.extend_from_slice(&ip);
    }
    if let Some(ip) = server_id {
        p.push(54);
        p.push(4);
        p.extend_from_slice(&ip);
    }
    p.push(55); // parameter request list
    p.push(3);
    p.push(1); // subnet mask
    p.push(3); // router
    p.push(6); // DNS server
    p.push(255); // end
    p
}

fn parse_options(data: &[u8]) -> Pending {
    let mut result = Pending {
        msg_type: 0,
        yiaddr: super::UNSPECIFIED_IP,
        subnet: super::UNSPECIFIED_IP,
        router: super::UNSPECIFIED_IP,
        dns: super::UNSPECIFIED_IP,
        server_id: super::UNSPECIFIED_IP,
    };
    let mut i = 0;
    while i + 1 < data.len() {
        let code = data[i];
        if code == 255 {
            break;
        }
        if code == 0 {
            i += 1;
            continue;
        }
        let len = data[i + 1] as usize;
        if i + 2 + len > data.len() {
            break;
        }
        let val = &data[i + 2..i + 2 + len];
        match (code, len) {
            (53, 1) => result.msg_type = val[0],
            (1, 4) => result.subnet.copy_from_slice(val),
            (3, 4) => result.router.copy_from_slice(val),
            (6, 4) => result.dns.copy_from_slice(&val[..4]),
            (54, 4) => result.server_id.copy_from_slice(val),
            _ => {}
        }
        i += 2 + len;
    }
    result
}

pub fn handle(payload: &[u8]) {
    if payload.len() < 240 || payload[236..240] != MAGIC_COOKIE {
        return;
    }
    let mut yiaddr = super::UNSPECIFIED_IP;
    yiaddr.copy_from_slice(&payload[16..20]);
    let mut pending = parse_options(&payload[240..]);
    pending.yiaddr = yiaddr;
    if pending.msg_type != 0 {
        *PENDING.lock() = Some(pending);
    }
}

fn wait_for(msg_type: u8) -> Option<Pending> {
    for _ in 0..WAIT_SPIN_LIMIT {
        super::poll_once();
        if let Some(pending) = *PENDING.lock() {
            if pending.msg_type == msg_type {
                return Some(pending);
            }
        }
    }
    None
}

/// Runs the full DISCOVER/OFFER/REQUEST/ACK exchange, blocking (bounded) at
/// each step. Returns `None` if any step times out.
pub fn run() -> Option<Lease> {
    let xid: u32 = 0x6D6F_6F6E; // "moon" in hex-ish, fixed is fine for a one-shot client

    *PENDING.lock() = None;
    super::udp::send(
        CLIENT_PORT,
        super::BROADCAST_IP,
        SERVER_PORT,
        &build_packet(MSG_DISCOVER, xid, None, None),
    );
    crate::serial_println!("dhcp: DISCOVER sent");

    let offer = wait_for(MSG_OFFER).or_else(|| {
        crate::serial_println!("dhcp: no OFFER received (timeout)");
        None
    })?;
    crate::serial_println!("dhcp: OFFER {}", super::format_ip(offer.yiaddr));

    *PENDING.lock() = None;
    super::udp::send(
        CLIENT_PORT,
        super::BROADCAST_IP,
        SERVER_PORT,
        &build_packet(MSG_REQUEST, xid, Some(offer.yiaddr), Some(offer.server_id)),
    );
    crate::serial_println!("dhcp: REQUEST sent");

    let ack = wait_for(MSG_ACK).or_else(|| {
        crate::serial_println!("dhcp: no ACK received (timeout)");
        None
    })?;
    crate::serial_println!("dhcp: ACK, lease = {}", super::format_ip(ack.yiaddr));

    Some(Lease {
        ip: ack.yiaddr,
        subnet: ack.subnet,
        router: ack.router,
        dns: ack.dns,
    })
}
