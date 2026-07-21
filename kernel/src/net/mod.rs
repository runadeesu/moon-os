//! A from-scratch, minimal network stack: Ethernet framing, ARP, IPv4,
//! ICMP, UDP, a DHCP client, and a best-effort DNS resolver. No TCP yet --
//! a real state machine with retransmission and congestion control is a
//! much larger project, left for a follow-up milestone (see ROADMAP.md).
//!
//! Everything here is poll-driven rather than interrupt-driven, same choice
//! as the AHCI driver: `poll_once()` tries to receive and dispatch a single
//! frame, and anything waiting on a reply (ARP resolution, a DHCP lease, a
//! DNS answer) just calls it in a bounded loop. That keeps the whole stack
//! synchronous and easy to reason about, at the cost of not being able to
//! do anything else while waiting -- fine for what this milestone needs to
//! prove.

pub mod arp;
pub mod dhcp;
pub mod dns;
pub mod icmp;
pub mod ipv4;
pub mod udp;

use crate::drivers::rtl8139::Rtl8139;
use alloc::vec::Vec;
use spin::Mutex;

pub type MacAddr = [u8; 6];
pub type Ipv4Addr = [u8; 4];

pub const BROADCAST_MAC: MacAddr = [0xFF; 6];
pub const UNSPECIFIED_IP: Ipv4Addr = [0, 0, 0, 0];
pub const BROADCAST_IP: Ipv4Addr = [255, 255, 255, 255];

struct NetState {
    nic: Option<Rtl8139>,
    mac: MacAddr,
    ip: Ipv4Addr,
    gateway: Ipv4Addr,
    subnet_mask: Ipv4Addr,
    dns: Ipv4Addr,
}

static NET: Mutex<NetState> = Mutex::new(NetState {
    nic: None,
    mac: [0; 6],
    ip: UNSPECIFIED_IP,
    gateway: UNSPECIFIED_IP,
    subnet_mask: UNSPECIFIED_IP,
    dns: UNSPECIFIED_IP,
});

/// Finds and brings up the NIC, if present. Safe to call even with none --
/// everything else in this module just becomes a no-op.
pub fn init() {
    if let Some(nic) = crate::drivers::rtl8139::init() {
        let mut state = NET.lock();
        state.mac = nic.mac;
        state.nic = Some(nic);
    } else {
        crate::serial_println!("net: no NIC found, networking disabled");
    }
}

pub fn is_up() -> bool {
    NET.lock().nic.is_some()
}

pub fn mac() -> MacAddr {
    NET.lock().mac
}

pub fn our_ip() -> Ipv4Addr {
    NET.lock().ip
}

pub fn gateway() -> Ipv4Addr {
    NET.lock().gateway
}

pub fn dns_server() -> Ipv4Addr {
    NET.lock().dns
}

pub fn set_ip_config(ip: Ipv4Addr, subnet_mask: Ipv4Addr, gateway: Ipv4Addr, dns: Ipv4Addr) {
    let mut state = NET.lock();
    state.ip = ip;
    state.subnet_mask = subnet_mask;
    state.gateway = gateway;
    state.dns = dns;
}

pub fn send_frame(dst_mac: MacAddr, ethertype: u16, payload: &[u8]) {
    let mut state = NET.lock();
    let src_mac = state.mac;
    if let Some(nic) = state.nic.as_mut() {
        let mut frame = Vec::with_capacity(14 + payload.len());
        frame.extend_from_slice(&dst_mac);
        frame.extend_from_slice(&src_mac);
        frame.extend_from_slice(&ethertype.to_be_bytes());
        frame.extend_from_slice(payload);
        nic.send(&frame);
    }
}

/// Tries to receive and dispatch a single frame. A no-op if nothing is
/// waiting, so it's cheap to call in a tight polling loop.
pub fn poll_once() {
    let frame = {
        let mut state = NET.lock();
        let Some(nic) = state.nic.as_mut() else {
            return;
        };
        nic.receive()
    };
    let Some(frame) = frame else {
        return;
    };
    if frame.len() < 14 {
        return;
    }
    let ethertype = u16::from_be_bytes([frame[12], frame[13]]);
    let payload = &frame[14..];
    match ethertype {
        0x0806 => arp::handle(payload),
        0x0800 => ipv4::handle(payload),
        _ => {}
    }
}

/// Runs DHCP to get an address, then pings the gateway and attempts a DNS
/// lookup as an end-to-end proof the stack actually works. Best-effort:
/// logs and moves on if any step fails (e.g. DNS needs the sandbox's host
/// to have real internet access, which DHCP/ping to the virtual gateway do
/// not).
pub fn run_demo() {
    if !is_up() {
        return;
    }

    crate::serial_println!("net: mac={}", format_mac(mac()));

    let Some(lease) = dhcp::run() else {
        crate::serial_println!("net: DHCP failed, skipping ping/DNS demo");
        return;
    };
    set_ip_config(lease.ip, lease.subnet, lease.router, lease.dns);
    crate::serial_println!(
        "net: configured ip={} mask={} gateway={} dns={}",
        format_ip(lease.ip),
        format_ip(lease.subnet),
        format_ip(lease.router),
        format_ip(lease.dns)
    );

    if icmp::ping(lease.router, 1, 1) {
        crate::serial_println!("net: ping to gateway {} succeeded", format_ip(lease.router));
    } else {
        crate::serial_println!("net: ping to gateway {} timed out", format_ip(lease.router));
    }

    match dns::query_a("example.com", lease.dns) {
        Some(ip) => crate::serial_println!("net: DNS example.com -> {}", format_ip(ip)),
        None => crate::serial_println!(
            "net: DNS query timed out (needs real internet from the host; DHCP/ping above don't)"
        ),
    }
}

pub fn format_mac(mac: MacAddr) -> alloc::string::String {
    alloc::format!(
        "{:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
        mac[0],
        mac[1],
        mac[2],
        mac[3],
        mac[4],
        mac[5]
    )
}

pub fn format_ip(ip: Ipv4Addr) -> alloc::string::String {
    alloc::format!("{}.{}.{}.{}", ip[0], ip[1], ip[2], ip[3])
}

/// A short "NET ..." label for the desktop top bar: no NIC, no lease yet, or
/// the actual configured address.
pub fn status_summary() -> alloc::string::String {
    if !is_up() {
        return alloc::string::String::from("NET --");
    }
    let ip = our_ip();
    if ip == UNSPECIFIED_IP {
        alloc::string::String::from("NET (no lease)")
    } else {
        alloc::format!("NET {}", format_ip(ip))
    }
}
