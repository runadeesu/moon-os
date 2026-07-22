//! Intel ICH AC97 audio driver: PCM-out playback plus mixer volume control,
//! enough to give the taskbar a real (not simulated) volume slider and play
//! a system beep for notifications/UI feedback. Polling-based, matching this
//! codebase's other early drivers (AHCI, RTL8139) -- no IRQ routing yet.
//! Register layout follows the standard Intel ICH AC97 host-controller
//! spec, which QEMU's `-device AC97` faithfully emulates.
//!
//! No floating point anywhere here, same constraint as the rest of the
//! kernel (the scheduler doesn't save/restore FPU/SSE state across a context
//! switch) -- the beep tone is a plain integer square wave.

use super::pci;
use crate::arch::x86_64::port::{inb, inl, inw, outb, outl, outw};
use crate::memory::{self, pmm};

const CLASS_MULTIMEDIA: u8 = 0x04;
const SUBCLASS_AUDIO: u8 = 0x01;

// NAM (Native Audio Mixer) register offsets.
const NAM_RESET: u16 = 0x00;
const NAM_MASTER_VOL: u16 = 0x02;
const NAM_PCM_OUT_VOL: u16 = 0x18;

// NABM (Native Audio Bus Master) register offsets, PCM OUT ("PO") channel.
const NABM_PO_BDBAR: u16 = 0x10;
const NABM_PO_LVI: u16 = 0x15;
const NABM_PO_SR: u16 = 0x16;
const NABM_PO_CR: u16 = 0x1B;
const NABM_GLOB_CNT: u16 = 0x2C;
const NABM_GLOB_STA: u16 = 0x30;

const PO_CR_RPBM: u8 = 1 << 0; // run/pause bus master
const PO_CR_RR: u8 = 1 << 1; // reset registers
const PO_SR_DCH: u16 = 1 << 0; // DMA controller halted

const SAMPLE_RATE: u32 = 48_000;
const BDL_ENTRIES: usize = 8;
/// Stereo frames per buffer; buffer is exactly one 4K page (1024 frames *
/// 2 channels * 2 bytes).
const SAMPLES_PER_BUFFER: usize = 1024;

pub struct Ac97 {
    nam: u16,
    nabm: u16,
    buffer_phys: u64,
    buffer: *mut u8,
    bdl_phys: u64,
    bdl: *mut u8,
}

unsafe impl Send for Ac97 {}

fn find_controller() -> Option<pci::PciDevice> {
    pci::enumerate()
        .into_iter()
        .find(|d| d.class == CLASS_MULTIMEDIA && d.subclass == SUBCLASS_AUDIO)
}

/// Finds the AC97 codec (if any) and brings it up. Returns `None` on
/// whatever machine has no audio device attached -- callers show an honest
/// "no audio device" state rather than a fake volume slider.
pub fn init() -> Option<Ac97> {
    let dev = find_controller()?;
    dev.enable_bus_mastering();
    let nam = dev.bar(0) as u16;
    let nabm = dev.bar(1) as u16;

    crate::serial_println!(
        "ac97: found controller {:04x}:{:04x} at {:02x}:{:02x}.{} (nam={:#x} nabm={:#x})",
        dev.vendor_id,
        dev.device_id,
        dev.bus,
        dev.device,
        dev.function,
        nam,
        nabm
    );

    unsafe {
        outl(nabm + NABM_GLOB_CNT, 0x0000_0002); // out of cold reset, no interrupts
        outw(nam + NAM_RESET, 0); // soft-reset the mixer
        for _ in 0..1_000_000 {
            if inl(nabm + NABM_GLOB_STA) & 0x100 != 0 {
                break; // primary codec ready
            }
        }

        // Unmuted, full volume by default -- `set_volume` scales this later
        // from a real Settings/taskbar control, not a cosmetic default.
        outw(nam + NAM_MASTER_VOL, 0x0000);
        outw(nam + NAM_PCM_OUT_VOL, 0x0000);

        let buffer_phys = pmm::alloc_frame().expect("out of memory for AC97 PCM buffer");
        let buffer = memory::phys_to_virt(buffer_phys);
        let bdl_phys = pmm::alloc_frame().expect("out of memory for AC97 BDL");
        let bdl = memory::phys_to_virt(bdl_phys);
        core::ptr::write_bytes(buffer, 0, 4096);
        core::ptr::write_bytes(bdl, 0, 4096);

        Some(Ac97 {
            nam,
            nabm,
            buffer_phys,
            buffer,
            bdl_phys,
            bdl,
        })
    }
}

impl Ac97 {
    /// Sets master output volume, 0 (silent) to 100 (loudest) -- a real
    /// attenuation value written to the codec's mixer register, not a
    /// cosmetic number with nothing behind it. `percent == 0` sets the
    /// mute bit outright.
    pub fn set_volume(&self, percent: u8) {
        let percent = percent.min(100);
        unsafe {
            if percent == 0 {
                outw(self.nam + NAM_MASTER_VOL, 0x8000);
                outw(self.nam + NAM_PCM_OUT_VOL, 0x8000);
                return;
            }
            // 6-bit attenuation field: 0 = loudest, 63 = quietest.
            let atten = (63 - (percent as u32 * 63 / 100)) as u16;
            let value = atten | (atten << 8);
            outw(self.nam + NAM_MASTER_VOL, value);
            outw(self.nam + NAM_PCM_OUT_VOL, value);
        }
    }

    /// Writes one square-wave tone into the PCM buffer and plays it through
    /// the bus-master DMA engine, blocking (polling `PO_SR`) until playback
    /// actually finishes. Used for the UI click sound and notification beep.
    pub fn beep(&mut self, freq_hz: u32, amplitude: i16) {
        let period = (SAMPLE_RATE / freq_hz).max(2) as usize;
        let half = period / 2;
        unsafe {
            let samples =
                core::slice::from_raw_parts_mut(self.buffer as *mut i16, SAMPLES_PER_BUFFER * 2);
            for frame in 0..SAMPLES_PER_BUFFER {
                let level = if frame % period < half {
                    amplitude
                } else {
                    -amplitude
                };
                samples[frame * 2] = level;
                samples[frame * 2 + 1] = level;
            }

            let bdl = core::slice::from_raw_parts_mut(self.bdl as *mut u32, BDL_ENTRIES * 2);
            for (i, entry) in bdl.chunks_exact_mut(2).enumerate().take(BDL_ENTRIES) {
                entry[0] = self.buffer_phys as u32;
                let ioc = if i == BDL_ENTRIES - 1 { 1 << 31 } else { 0 };
                entry[1] = ioc | (SAMPLES_PER_BUFFER as u32 * 2); // sample count (both channels)
            }

            outb(self.nabm + NABM_PO_CR, PO_CR_RR);
            for _ in 0..1_000_000 {
                if inb(self.nabm + NABM_PO_CR) & PO_CR_RR == 0 {
                    break;
                }
            }
            outl(self.nabm + NABM_PO_BDBAR, self.bdl_phys as u32);
            outb(self.nabm + NABM_PO_LVI, (BDL_ENTRIES - 1) as u8);
            outb(self.nabm + NABM_PO_CR, PO_CR_RPBM);

            for _ in 0..20_000_000 {
                if inw(self.nabm + NABM_PO_SR) & PO_SR_DCH != 0 {
                    break;
                }
            }
            outb(self.nabm + NABM_PO_CR, 0);
        }
    }
}
