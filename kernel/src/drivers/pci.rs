//! PCI configuration space access via the legacy 0xCF8/0xCFC I/O ports
//! (mechanism #1, supported by every x86 chipset including QEMU's q35).

use crate::arch::x86_64::port::{inl, outl};
use alloc::vec::Vec;

const CONFIG_ADDRESS: u16 = 0xCF8;
const CONFIG_DATA: u16 = 0xCFC;

#[derive(Clone, Copy)]
pub struct PciDevice {
    pub bus: u8,
    pub device: u8,
    pub function: u8,
    pub vendor_id: u16,
    pub device_id: u16,
    pub class: u8,
    pub subclass: u8,
    pub prog_if: u8,
}

fn address(bus: u8, device: u8, function: u8, offset: u8) -> u32 {
    (1 << 31)
        | (u32::from(bus) << 16)
        | (u32::from(device) << 11)
        | (u32::from(function) << 8)
        | u32::from(offset & 0xFC)
}

pub fn config_read32(bus: u8, device: u8, function: u8, offset: u8) -> u32 {
    unsafe {
        outl(CONFIG_ADDRESS, address(bus, device, function, offset));
        inl(CONFIG_DATA)
    }
}

pub fn config_write32(bus: u8, device: u8, function: u8, offset: u8, value: u32) {
    unsafe {
        outl(CONFIG_ADDRESS, address(bus, device, function, offset));
        outl(CONFIG_DATA, value);
    }
}

impl PciDevice {
    /// Reads a Base Address Register. Only handles 32-bit (non-prefetchable
    /// memory or I/O) BARs, which is all moon OS's drivers need so far.
    pub fn bar(&self, index: u8) -> u64 {
        let raw = config_read32(self.bus, self.device, self.function, 0x10 + index * 4);
        if raw & 0x1 != 0 {
            u64::from(raw & 0xFFFF_FFFC) // I/O space BAR
        } else {
            u64::from(raw & 0xFFFF_FFF0) // 32-bit memory space BAR
        }
    }

    /// Sets the Memory Space Enable and Bus Master Enable bits in the
    /// command register, needed before a device's BARs or DMA will work.
    pub fn enable_bus_mastering(&self) {
        let command = config_read32(self.bus, self.device, self.function, 0x04);
        config_write32(
            self.bus,
            self.device,
            self.function,
            0x04,
            command | 0x0002 | 0x0004,
        );
    }
}

/// Scans every bus/device/function and returns every present device found.
pub fn enumerate() -> Vec<PciDevice> {
    let mut devices = Vec::new();
    for bus in 0..=255u16 {
        let bus = bus as u8;
        for device in 0..32u8 {
            for function in 0..8u8 {
                let id = config_read32(bus, device, function, 0x00);
                let vendor_id = (id & 0xFFFF) as u16;
                if vendor_id == 0xFFFF {
                    if function == 0 {
                        break; // no device at all in this slot
                    }
                    continue;
                }
                let device_id = (id >> 16) as u16;

                let class_reg = config_read32(bus, device, function, 0x08);
                let class = (class_reg >> 24) as u8;
                let subclass = (class_reg >> 16) as u8;
                let prog_if = (class_reg >> 8) as u8;

                devices.push(PciDevice {
                    bus,
                    device,
                    function,
                    vendor_id,
                    device_id,
                    class,
                    subclass,
                    prog_if,
                });

                if function == 0 {
                    let header_type = config_read32(bus, device, 0, 0x0C) >> 16 & 0xFF;
                    if header_type & 0x80 == 0 {
                        break; // not multi-function, skip remaining functions
                    }
                }
            }
        }
    }
    devices
}
