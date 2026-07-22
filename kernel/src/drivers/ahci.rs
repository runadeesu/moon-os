//! Minimal AHCI (SATA host controller) driver.
//!
//! Brings up the HBA, initializes one command slot per implemented port with
//! an attached device, and can issue a single ATA or ATAPI command at a time
//! (synchronously -- we poll for completion, no interrupt-driven queueing
//! yet). That's enough to IDENTIFY a SATA disk, read a sector from the
//! SATAPI CD-ROM moon OS actually boots from, and -- via
//! `ata_read_sector`/`ata_write_sector` -- read and write real sectors on a
//! SATA hard disk, which `fs::persist` uses to survive a reboot.
//!
//! Every HBA/port register access goes through `vread`/`vwrite`: the
//! controller changes these registers on its own (command completion,
//! device status, ...), so a plain, non-volatile `(*ptr).field` read is
//! unsound here -- the optimizer is free to assume nothing external ever
//! changes it and hoist the read out of a polling loop entirely, which is
//! exactly the bug that made command-completion polling spin to a timeout
//! every time in an early version of this driver.

use super::pci;
use crate::memory::{self, mmio, pmm};
use alloc::vec::Vec;
use core::mem::size_of;

const PAGE_SIZE: u64 = 4096;

const SATA_SIG_ATA: u32 = 0x0000_0101;
const SATA_SIG_ATAPI: u32 = 0xEB14_0101;

const HBA_PORT_DET_PRESENT: u32 = 3;

const CMD_ST: u32 = 1 << 0;
const CMD_FRE: u32 = 1 << 4;
const CMD_FR: u32 = 1 << 14;
const CMD_CR: u32 = 1 << 15;

const GHC_AE: u32 = 1 << 31;

const TFD_ERR: u32 = 1 << 0;
const TFD_DRQ: u32 = 1 << 3;
const TFD_BSY: u32 = 1 << 7;

const SPIN_LIMIT: u32 = 1_000_000;

#[inline]
unsafe fn vread(ptr: *const u32) -> u32 {
    unsafe { core::ptr::read_volatile(ptr) }
}

#[inline]
unsafe fn vwrite(ptr: *mut u32, value: u32) {
    unsafe { core::ptr::write_volatile(ptr, value) }
}

#[repr(C)]
struct HbaPort {
    clb: u32,
    clbu: u32,
    fb: u32,
    fbu: u32,
    is: u32,
    ie: u32,
    cmd: u32,
    _reserved0: u32,
    tfd: u32,
    sig: u32,
    ssts: u32,
    sctl: u32,
    serr: u32,
    sact: u32,
    ci: u32,
    sntf: u32,
    fbs: u32,
    _reserved1: [u32; 11],
    _vendor: [u32; 4],
}

#[repr(C)]
struct HbaMem {
    cap: u32,
    ghc: u32,
    is: u32,
    pi: u32,
    vs: u32,
    _ccc_ctl: u32,
    _ccc_pts: u32,
    _em_loc: u32,
    _em_ctl: u32,
    _cap2: u32,
    _bohc: u32,
    _reserved: [u8; 0xA0 - 0x2C],
    _vendor: [u8; 0x100 - 0xA0],
    ports: [HbaPort; 32],
}

const _: () = assert!(size_of::<HbaMem>() == 0x1100);
const _: () = assert!(size_of::<HbaPort>() == 0x80);

#[repr(C)]
struct CmdHeader {
    flags: u16,
    prdtl: u16,
    prdbc: u32,
    ctba: u32,
    ctbau: u32,
    _reserved: [u32; 4],
}

#[repr(C)]
struct PrdtEntry {
    dba: u32,
    dbau: u32,
    _reserved: u32,
    dbc: u32,
}

/// Device type discovered on a live port.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PortKind {
    Sata,
    Atapi,
}

pub struct Port {
    regs: *mut HbaPort,
    pub index: u8,
    pub kind: PortKind,
    cmd_list_phys: u64,
    cmd_table_phys: u64,
    cmd_table_virt: *mut u8,
}

/// Allocates one physical frame and returns both addresses, zeroed.
fn alloc_dma_page() -> (u64, *mut u8) {
    let phys = pmm::alloc_frame().expect("out of memory for AHCI DMA buffer");
    let virt = memory::phys_to_virt(phys);
    unsafe { core::ptr::write_bytes(virt, 0, PAGE_SIZE as usize) };
    (phys, virt)
}

fn find_controller() -> Option<pci::PciDevice> {
    pci::enumerate()
        .into_iter()
        .find(|d| d.class == 0x01 && d.subclass == 0x06 && d.prog_if == 0x01)
}

/// Finds the AHCI controller (if any), brings up the HBA, and initializes
/// every port with a device physically attached. Returns the live ports.
pub fn init() -> Vec<Port> {
    let Some(dev) = find_controller() else {
        crate::serial_println!("ahci: no AHCI controller found on the PCI bus");
        return Vec::new();
    };

    dev.enable_bus_mastering();
    let abar_phys = dev.bar(5);
    crate::serial_println!(
        "ahci: found controller {:04x}:{:04x} at {:02x}:{:02x}.{} (ABAR={:#x})",
        dev.vendor_id,
        dev.device_id,
        dev.bus,
        dev.device,
        dev.function,
        abar_phys
    );

    let hba = mmio::map(abar_phys, size_of::<HbaMem>() as u64) as *mut HbaMem;
    unsafe {
        let ghc = core::ptr::addr_of_mut!((*hba).ghc);
        vwrite(ghc, vread(ghc) | GHC_AE);
    }

    let pi = unsafe { vread(core::ptr::addr_of!((*hba).pi)) };
    let mut ports = Vec::new();

    for index in 0..32u8 {
        if pi & (1 << index) == 0 {
            continue;
        }
        let port_regs = unsafe { core::ptr::addr_of_mut!((*hba).ports[index as usize]) };
        let ssts = unsafe { vread(core::ptr::addr_of!((*port_regs).ssts)) };
        if ssts & 0xF != HBA_PORT_DET_PRESENT {
            continue;
        }

        let sig = unsafe { vread(core::ptr::addr_of!((*port_regs).sig)) };
        let kind = match sig {
            SATA_SIG_ATA => PortKind::Sata,
            SATA_SIG_ATAPI => PortKind::Atapi,
            other => {
                crate::serial_println!(
                    "ahci: port {} has unrecognized signature {:#x}",
                    index,
                    other
                );
                continue;
            }
        };

        let port = init_port(port_regs, index, kind);
        crate::serial_println!("ahci: port {} live, device = {:?}", index, kind);
        ports.push(port);
    }

    ports
}

fn stop_command_engine(port: *mut HbaPort) {
    unsafe {
        let cmd = core::ptr::addr_of_mut!((*port).cmd);
        vwrite(cmd, vread(cmd) & !CMD_ST);
        for _ in 0..SPIN_LIMIT {
            if vread(cmd) & CMD_CR == 0 {
                break;
            }
        }
        vwrite(cmd, vread(cmd) & !CMD_FRE);
        for _ in 0..SPIN_LIMIT {
            if vread(cmd) & CMD_FR == 0 {
                break;
            }
        }
    }
}

fn start_command_engine(port: *mut HbaPort) {
    unsafe {
        let cmd = core::ptr::addr_of_mut!((*port).cmd);
        vwrite(cmd, vread(cmd) | CMD_FRE);
        vwrite(cmd, vread(cmd) | CMD_ST);
    }
}

fn init_port(regs: *mut HbaPort, index: u8, kind: PortKind) -> Port {
    stop_command_engine(regs);

    let (cl_phys, _cl_virt) = alloc_dma_page(); // 32 * 32-byte command headers, fits in one page
    let (fb_phys, _fb_virt) = alloc_dma_page(); // FIS receive area, 256 bytes, fits in one page
    let (ct_phys, ct_virt) = alloc_dma_page(); // command table for slot 0

    unsafe {
        vwrite(core::ptr::addr_of_mut!((*regs).clb), cl_phys as u32);
        vwrite(
            core::ptr::addr_of_mut!((*regs).clbu),
            (cl_phys >> 32) as u32,
        );
        vwrite(core::ptr::addr_of_mut!((*regs).fb), fb_phys as u32);
        vwrite(core::ptr::addr_of_mut!((*regs).fbu), (fb_phys >> 32) as u32);
        vwrite(core::ptr::addr_of_mut!((*regs).serr), 0xFFFF_FFFF); // clear stale errors
        vwrite(core::ptr::addr_of_mut!((*regs).is), 0xFFFF_FFFF);

        // The command list and its headers are memory only we (and, once a
        // command is issued, the controller via DMA) touch -- no volatility
        // concerns for this plain write.
        let cmd_list = memory::phys_to_virt(cl_phys) as *mut CmdHeader;
        (*cmd_list).ctba = ct_phys as u32;
        (*cmd_list).ctbau = (ct_phys >> 32) as u32;
    }

    start_command_engine(regs);

    Port {
        regs,
        index,
        kind,
        cmd_list_phys: cl_phys,
        cmd_table_phys: ct_phys,
        cmd_table_virt: ct_virt,
    }
}

fn wait_not_busy(port: &Port) -> bool {
    for _ in 0..SPIN_LIMIT {
        let tfd = unsafe { vread(core::ptr::addr_of!((*port.regs).tfd)) };
        if tfd & (TFD_BSY | TFD_DRQ) == 0 {
            return true;
        }
    }
    false
}

/// A single ATA or ATAPI command to issue in slot 0.
struct AtaCommand<'a> {
    ata_command: u8,
    lba: u64,
    sector_count: u16,
    /// SCSI CDB (up to 16 bytes): present for an ATA PACKET (ATAPI) command,
    /// absent for a plain ATA command.
    atapi_cdb: Option<&'a [u8]>,
    buffer_phys: u64,
    buffer_len: u32,
    write: bool,
}

/// Builds slot 0's command header + Register H2D FIS, points its PRDT at
/// the command's buffer, and issues it.
fn issue_command(port: &Port, command: &AtaCommand) -> bool {
    if !wait_not_busy(port) {
        crate::serial_println!("ahci: port {} busy, giving up", port.index);
        return false;
    }

    let is_atapi = command.atapi_cdb.is_some();
    let cmd_table = port.cmd_table_virt;
    unsafe { core::ptr::write_bytes(cmd_table, 0, PAGE_SIZE as usize) };

    // Register H2D FIS at offset 0. Plain writes: this is our own command
    // table memory, not a live HBA register.
    unsafe {
        let fis = cmd_table;
        let lba = command.lba;
        *fis = 0x27; // FIS_TYPE_REG_H2D
        *fis.add(1) = 0x80; // C bit: this is a command
        *fis.add(2) = command.ata_command;
        *fis.add(3) = if is_atapi { 0x01 } else { 0x00 }; // features: DMA for PACKET
        *fis.add(4) = lba as u8;
        *fis.add(5) = (lba >> 8) as u8;
        *fis.add(6) = (lba >> 16) as u8;
        *fis.add(7) = 0x40; // device: LBA mode
        *fis.add(8) = (lba >> 24) as u8;
        *fis.add(9) = (lba >> 32) as u8;
        *fis.add(10) = (lba >> 40) as u8;
        *fis.add(12) = command.sector_count as u8;
        *fis.add(13) = (command.sector_count >> 8) as u8;
    }

    if let Some(cdb) = command.atapi_cdb {
        unsafe {
            let acmd = cmd_table.add(64);
            core::ptr::copy_nonoverlapping(cdb.as_ptr(), acmd, cdb.len());
        }
    }

    // Single PRDT entry right after the fixed 128-byte header region.
    unsafe {
        let prdt = cmd_table.add(128) as *mut PrdtEntry;
        (*prdt).dba = command.buffer_phys as u32;
        (*prdt).dbau = (command.buffer_phys >> 32) as u32;
        (*prdt).dbc = command.buffer_len.saturating_sub(1);
    }

    let flags: u16 = 5 // CFL: Register H2D FIS is 5 dwords
        | if is_atapi { 1 << 5 } else { 0 } // A bit
        | if command.write { 1 << 6 } else { 0 }; // W bit

    unsafe {
        let cmd_list = memory::phys_to_virt(port.cmd_list_phys) as *mut CmdHeader;
        (*cmd_list).flags = flags;
        (*cmd_list).prdtl = 1;
        (*cmd_list).prdbc = 0;
        (*cmd_list).ctba = port.cmd_table_phys as u32;
        (*cmd_list).ctbau = (port.cmd_table_phys >> 32) as u32;

        vwrite(core::ptr::addr_of_mut!((*port.regs).ci), 1);
    }

    for _ in 0..SPIN_LIMIT {
        let ci = unsafe { vread(core::ptr::addr_of!((*port.regs).ci)) };
        let tfd = unsafe { vread(core::ptr::addr_of!((*port.regs).tfd)) };
        if tfd & TFD_ERR != 0 {
            crate::serial_println!("ahci: port {} command errored (tfd={:#x})", port.index, tfd);
            return false;
        }
        if ci & 1 == 0 {
            return true;
        }
    }

    crate::serial_println!("ahci: port {} command timed out", port.index);
    false
}

/// ATA IDENTIFY DEVICE (0xEC): reads 512 bytes of drive info into `out`.
pub fn identify(port: &Port, out: &mut [u8; 512]) -> bool {
    let (phys, virt) = alloc_dma_page();
    let ok = issue_command(
        port,
        &AtaCommand {
            ata_command: 0xEC,
            lba: 0,
            sector_count: 1,
            atapi_cdb: None,
            buffer_phys: phys,
            buffer_len: 512,
            write: false,
        },
    );
    if ok {
        unsafe { core::ptr::copy_nonoverlapping(virt, out.as_mut_ptr(), 512) };
    }
    pmm::free_frame(phys);
    ok
}

/// ATA READ DMA EXT (0x25): reads one 512-byte sector from a SATA disk at
/// 48-bit LBA `lba`. This is real disk I/O -- the same command path
/// `identify` already proved works, just with a data-transfer command
/// instead of IDENTIFY.
pub fn ata_read_sector(port: &Port, lba: u64, out: &mut [u8; 512]) -> bool {
    let (phys, virt) = alloc_dma_page();
    let ok = issue_command(
        port,
        &AtaCommand {
            ata_command: 0x25,
            lba,
            sector_count: 1,
            atapi_cdb: None,
            buffer_phys: phys,
            buffer_len: 512,
            write: false,
        },
    );
    if ok {
        unsafe { core::ptr::copy_nonoverlapping(virt, out.as_mut_ptr(), 512) };
    }
    pmm::free_frame(phys);
    ok
}

/// ATA WRITE DMA EXT (0x35): writes one 512-byte sector to a SATA disk at
/// 48-bit LBA `lba`. Genuinely persists to the backing disk image -- data
/// written here is still there after a full QEMU restart, verified in
/// `fs::persist`.
pub fn ata_write_sector(port: &Port, lba: u64, data: &[u8; 512]) -> bool {
    let (phys, virt) = alloc_dma_page();
    unsafe { core::ptr::copy_nonoverlapping(data.as_ptr(), virt, 512) };
    let ok = issue_command(
        port,
        &AtaCommand {
            ata_command: 0x35,
            lba,
            sector_count: 1,
            atapi_cdb: None,
            buffer_phys: phys,
            buffer_len: 512,
            write: true,
        },
    );
    pmm::free_frame(phys);
    ok
}

/// SCSI READ(10) via an ATA PACKET command: reads one 2048-byte sector from
/// an ATAPI device (e.g. the CD/DVD-ROM moon OS itself booted from).
pub fn atapi_read_sector(port: &Port, lba: u32, out: &mut [u8; 2048]) -> bool {
    let mut cdb = [0u8; 12];
    cdb[0] = 0x28; // READ(10)
    cdb[2] = (lba >> 24) as u8;
    cdb[3] = (lba >> 16) as u8;
    cdb[4] = (lba >> 8) as u8;
    cdb[5] = lba as u8;
    cdb[8] = 1; // transfer length: 1 block

    let (phys, virt) = alloc_dma_page();
    let ok = issue_command(
        port,
        &AtaCommand {
            ata_command: 0xA0,
            lba: 0,
            sector_count: 0,
            atapi_cdb: Some(&cdb),
            buffer_phys: phys,
            buffer_len: 2048,
            write: false,
        },
    );
    if ok {
        unsafe { core::ptr::copy_nonoverlapping(virt, out.as_mut_ptr(), 2048) };
    }
    pmm::free_frame(phys);
    ok
}
