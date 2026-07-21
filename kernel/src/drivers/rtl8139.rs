//! Realtek RTL8139 NIC driver: poll-based (no IRQ routing yet, same pragmatic
//! choice as the AHCI driver), just enough to send and receive raw Ethernet
//! frames. Chosen over e1000/virtio-net for a first NIC driver because its
//! register set is small and its RX mechanism -- one big ring buffer the
//! card DMAs into, no descriptor rings -- is the simplest to get right.

use super::pci;
use crate::arch::x86_64::port::{inb, inl, outb, outl, outw};
use crate::memory::{self, pmm};
use alloc::vec::Vec;

const VENDOR_REALTEK: u16 = 0x10EC;
const DEVICE_RTL8139: u16 = 0x8139;

const REG_MAC: u16 = 0x00;
const REG_TSAD: [u16; 4] = [0x20, 0x24, 0x28, 0x2C];
const REG_TSD: [u16; 4] = [0x10, 0x14, 0x18, 0x1C];
const REG_RBSTART: u16 = 0x30;
const REG_CR: u16 = 0x37;
const REG_CAPR: u16 = 0x38;
const REG_IMR: u16 = 0x3C;
const REG_RCR: u16 = 0x44;
const REG_CONFIG1: u16 = 0x52;

const CR_BUFE: u8 = 1 << 0;
const CR_TE: u8 = 1 << 2;
const CR_RE: u8 = 1 << 3;
const CR_RST: u8 = 1 << 4;

const TSD_TOK: u32 = 1 << 15;
const TSD_TABT: u32 = 1 << 14;

/// Nominal ring size the read/write pointers wrap within. The physical
/// buffer is bigger (below) so the card can write a packet that straddles
/// the "end" without splitting it.
const RX_RING_SIZE: usize = 8192;
const RX_BUFFER_SIZE: usize = RX_RING_SIZE + 16 + 1500;
const RX_BUFFER_PAGES: u64 = (RX_BUFFER_SIZE as u64).div_ceil(4096) + 1;

const SPIN_LIMIT: u32 = 1_000_000;

pub struct Rtl8139 {
    io_base: u16,
    pub mac: [u8; 6],
    rx_buffer: *mut u8,
    rx_offset: usize,
    tx_buffers_phys: [u64; 4],
    tx_buffers_virt: [*mut u8; 4],
    tx_next: usize,
}

unsafe impl Send for Rtl8139 {}

fn find_controller() -> Option<pci::PciDevice> {
    pci::enumerate()
        .into_iter()
        .find(|d| d.vendor_id == VENDOR_REALTEK && d.device_id == DEVICE_RTL8139)
}

/// Finds the RTL8139 (if any) and brings it up: reset, MAC read, RX ring and
/// TX buffers allocated and registered, receiver and transmitter enabled.
pub fn init() -> Option<Rtl8139> {
    let dev = find_controller()?;
    dev.enable_bus_mastering();
    let io_base = dev.bar(0) as u16; // BAR0 is I/O space for this card

    crate::serial_println!(
        "rtl8139: found controller {:04x}:{:04x} at {:02x}:{:02x}.{} (io_base={:#x})",
        dev.vendor_id,
        dev.device_id,
        dev.bus,
        dev.device,
        dev.function,
        io_base
    );

    unsafe {
        outb(io_base + REG_CONFIG1, 0x00); // power on

        outb(io_base + REG_CR, CR_RST);
        for _ in 0..SPIN_LIMIT {
            if inb(io_base + REG_CR) & CR_RST == 0 {
                break;
            }
        }

        let mut mac = [0u8; 6];
        for (i, byte) in mac.iter_mut().enumerate() {
            *byte = inb(io_base + REG_MAC + i as u16);
        }

        let rx_phys = pmm::alloc_contiguous(RX_BUFFER_PAGES)
            .expect("out of contiguous memory for RTL8139 RX ring");
        let rx_virt = memory::phys_to_virt(rx_phys);
        core::ptr::write_bytes(rx_virt, 0, RX_BUFFER_PAGES as usize * 4096);
        outl(io_base + REG_RBSTART, rx_phys as u32);

        let mut tx_buffers_phys = [0u64; 4];
        let mut tx_buffers_virt = [core::ptr::null_mut(); 4];
        for i in 0..4 {
            let phys = pmm::alloc_frame().expect("out of memory for RTL8139 TX buffer");
            tx_buffers_phys[i] = phys;
            tx_buffers_virt[i] = memory::phys_to_virt(phys);
        }

        outw(io_base + REG_IMR, 0x0005); // ROK|TOK -- unused while polling, harmless to set
        outl(io_base + REG_RCR, 0x0000_008E); // APM | AM | AB | WRAP

        outb(io_base + REG_CR, CR_RE | CR_TE);

        crate::serial_println!(
            "rtl8139: mac={:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
            mac[0],
            mac[1],
            mac[2],
            mac[3],
            mac[4],
            mac[5]
        );

        Some(Rtl8139 {
            io_base,
            mac,
            rx_buffer: rx_virt,
            rx_offset: 0,
            tx_buffers_phys,
            tx_buffers_virt,
            tx_next: 0,
        })
    }
}

impl Rtl8139 {
    /// Sends one Ethernet frame, padding up to the minimum frame size and
    /// blocking (via polling) until the card reports it went out.
    pub fn send(&mut self, frame: &[u8]) {
        let slot = self.tx_next;
        self.tx_next = (self.tx_next + 1) % 4;
        let len = frame.len().max(60);

        unsafe {
            let virt = self.tx_buffers_virt[slot];
            core::ptr::write_bytes(virt, 0, len);
            core::ptr::copy_nonoverlapping(frame.as_ptr(), virt, frame.len());
            outl(
                self.io_base + REG_TSAD[slot],
                self.tx_buffers_phys[slot] as u32,
            );
            outl(self.io_base + REG_TSD[slot], len as u32);

            for _ in 0..SPIN_LIMIT {
                let tsd = inl(self.io_base + REG_TSD[slot]);
                if tsd & TSD_TOK != 0 {
                    return;
                }
                if tsd & TSD_TABT != 0 {
                    crate::serial_println!("rtl8139: TX abort on slot {}", slot);
                    return;
                }
            }
        }
        crate::serial_println!("rtl8139: TX timed out on slot {}", slot);
    }

    /// Pops one received frame (payload only, CRC stripped), if any is
    /// waiting. Safe to call frequently -- returns `None` immediately when
    /// the ring is empty.
    pub fn receive(&mut self) -> Option<Vec<u8>> {
        unsafe {
            if inb(self.io_base + REG_CR) & CR_BUFE != 0 {
                return None;
            }

            let header_ptr = self.rx_buffer.add(self.rx_offset) as *const u16;
            let status = core::ptr::read_volatile(header_ptr);
            let length = core::ptr::read_volatile(header_ptr.add(1)) as usize;

            if status & 0x1 == 0 || !(4..=RX_BUFFER_SIZE).contains(&length) {
                // Corrupt state (shouldn't happen under normal operation);
                // resetting the read pointer to the write pointer would need
                // CBR, which we don't track -- just stop reading this round.
                crate::serial_println!(
                    "rtl8139: bad RX header (status={:#x} length={}), skipping",
                    status,
                    length
                );
                return None;
            }

            let data_ptr = self.rx_buffer.add(self.rx_offset + 4);
            let payload_len = length - 4; // exclude the trailing CRC
            let data = core::slice::from_raw_parts(data_ptr, payload_len).to_vec();

            let mut new_offset = self.rx_offset + 4 + length;
            new_offset = new_offset.div_ceil(4) * 4; // 4-byte alignment, per spec
            new_offset %= RX_RING_SIZE;
            self.rx_offset = new_offset;

            outw(
                self.io_base + REG_CAPR,
                (self.rx_offset as u16).wrapping_sub(16),
            );

            Some(data)
        }
    }
}
