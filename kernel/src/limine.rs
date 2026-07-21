//! Minimal Rust bindings for the Limine boot protocol.
//!
//! We hand-roll these instead of depending on the `limine` crate because that
//! crate requires the unstable `ptr_metadata` feature (nightly-only). Writing
//! our own bindings keeps moon OS buildable with stable Rust, and the layouts
//! here are a direct, minimal translation of the public protocol spec:
//! <https://github.com/limine-bootloader/limine-protocol/blob/trunk/PROTOCOL.md>
//!
//! Only the features moon OS currently needs are implemented: base revision,
//! request delimiters, bootloader info, HHDM, framebuffer, and the memory map.

use core::cell::UnsafeCell;
use core::ffi::c_char;

const COMMON_MAGIC: [u64; 2] = [0xc7b1dd30df4c8b88, 0x0a82e883a194f07b];

/// Marks the start of the region the bootloader scans for requests.
#[repr(C)]
pub struct RequestsStartMarker([u64; 4]);

impl RequestsStartMarker {
    pub const fn new() -> Self {
        Self([
            0xf6b8f4b39de7d1ae,
            0xfab91a6940fcb9cf,
            0x785c6ed015d3e316,
            0x181e920a7852b9d9,
        ])
    }
}

/// Marks the end of the region the bootloader scans for requests.
#[repr(C)]
pub struct RequestsEndMarker([u64; 2]);

impl RequestsEndMarker {
    pub const fn new() -> Self {
        Self([0xadc0e0531bb10d03, 0x9572709f31764c62])
    }
}

/// Tells the bootloader which base revision of the protocol we expect.
#[repr(C)]
pub struct BaseRevision {
    tag: UnsafeCell<[u64; 3]>,
}

unsafe impl Sync for BaseRevision {}

impl BaseRevision {
    pub const fn new(revision: u64) -> Self {
        Self {
            tag: UnsafeCell::new([0xf9562b2d5c95a6c8, 0x6a7b384944536bdc, revision]),
        }
    }

    /// True once the bootloader has confirmed it loaded us at the requested revision.
    pub fn is_supported(&self) -> bool {
        unsafe { (*self.tag.get())[2] == 0 }
    }
}

macro_rules! request_id {
    ($a:expr, $b:expr) => {
        [COMMON_MAGIC[0], COMMON_MAGIC[1], $a, $b]
    };
}

/// Common header shared by every Limine request structure.
#[repr(C)]
struct RequestHeader {
    id: [u64; 4],
    revision: u64,
}

macro_rules! define_request {
    ($name:ident, $id_a:expr, $id_b:expr, $response_ty:ty) => {
        #[repr(C)]
        pub struct $name {
            header: RequestHeader,
            response: UnsafeCell<*const $response_ty>,
        }

        unsafe impl Sync for $name {}

        impl $name {
            pub const fn new() -> Self {
                Self {
                    header: RequestHeader {
                        id: request_id!($id_a, $id_b),
                        revision: 0,
                    },
                    response: UnsafeCell::new(core::ptr::null()),
                }
            }

            pub fn response(&self) -> Option<&'static $response_ty> {
                let ptr = unsafe { *self.response.get() };
                if ptr.is_null() {
                    None
                } else {
                    Some(unsafe { &*ptr })
                }
            }
        }
    };
}

// ---- Bootloader Info ----------------------------------------------------

#[repr(C)]
pub struct BootloaderInfoResponse {
    pub revision: u64,
    name: *const c_char,
    version: *const c_char,
}

impl BootloaderInfoResponse {
    /// Reads the bootloader-provided name as a `&str`. Falls back to a
    /// placeholder if the string is somehow not valid UTF-8.
    pub fn name(&self) -> &str {
        unsafe { c_str_to_str(self.name) }
    }

    pub fn version(&self) -> &str {
        unsafe { c_str_to_str(self.version) }
    }
}

define_request!(
    BootloaderInfoRequest,
    0xf55038d8e2a1202f,
    0x279426fcf5f59740,
    BootloaderInfoResponse
);

// ---- HHDM (Higher Half Direct Map) --------------------------------------

#[repr(C)]
pub struct HhdmResponse {
    pub revision: u64,
    pub offset: u64,
}

define_request!(
    HhdmRequest,
    0x48dcf1cb8ad2b852,
    0x63984e959a98244b,
    HhdmResponse
);

// ---- Framebuffer ---------------------------------------------------------

#[repr(C)]
pub struct Framebuffer {
    pub address: *mut u8,
    pub width: u64,
    pub height: u64,
    pub pitch: u64,
    pub bpp: u16,
    pub memory_model: u8,
    pub red_mask_size: u8,
    pub red_mask_shift: u8,
    pub green_mask_size: u8,
    pub green_mask_shift: u8,
    pub blue_mask_size: u8,
    pub blue_mask_shift: u8,
    unused: [u8; 7],
    pub edid_size: u64,
    pub edid: *mut u8,
    pub mode_count: u64,
    pub modes: *mut *mut u8,
}

#[repr(C)]
pub struct FramebufferResponse {
    pub revision: u64,
    pub framebuffer_count: u64,
    framebuffers: *const *const Framebuffer,
}

impl FramebufferResponse {
    pub fn framebuffers(&self) -> &'static [*const Framebuffer] {
        if self.framebuffer_count == 0 {
            &[]
        } else {
            unsafe {
                core::slice::from_raw_parts(self.framebuffers, self.framebuffer_count as usize)
            }
        }
    }
}

define_request!(
    FramebufferRequest,
    0x9d5827dcd881dd75,
    0xa3148604f6fab11b,
    FramebufferResponse
);

// ---- Memory Map -----------------------------------------------------------

#[repr(u64)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemmapEntryType {
    Usable = 0,
    Reserved = 1,
    AcpiReclaimable = 2,
    AcpiNvs = 3,
    BadMemory = 4,
    BootloaderReclaimable = 5,
    ExecutableAndModules = 6,
    Framebuffer = 7,
    ReservedMapped = 8,
    Unknown = u64::MAX,
}

impl From<u64> for MemmapEntryType {
    fn from(value: u64) -> Self {
        match value {
            0 => Self::Usable,
            1 => Self::Reserved,
            2 => Self::AcpiReclaimable,
            3 => Self::AcpiNvs,
            4 => Self::BadMemory,
            5 => Self::BootloaderReclaimable,
            6 => Self::ExecutableAndModules,
            7 => Self::Framebuffer,
            8 => Self::ReservedMapped,
            _ => Self::Unknown,
        }
    }
}

#[repr(C)]
pub struct MemmapEntry {
    pub base: u64,
    pub length: u64,
    ty: u64,
}

impl MemmapEntry {
    pub fn entry_type(&self) -> MemmapEntryType {
        self.ty.into()
    }
}

#[repr(C)]
pub struct MemmapResponse {
    pub revision: u64,
    pub entry_count: u64,
    entries: *const *const MemmapEntry,
}

impl MemmapResponse {
    pub fn entries(&self) -> &'static [*const MemmapEntry] {
        if self.entry_count == 0 {
            &[]
        } else {
            unsafe { core::slice::from_raw_parts(self.entries, self.entry_count as usize) }
        }
    }
}

define_request!(
    MemmapRequest,
    0x67cf3d9d378a806f,
    0xe304acdfc50c3c62,
    MemmapResponse
);

unsafe fn c_str_to_str<'a>(ptr: *const c_char) -> &'a str {
    if ptr.is_null() {
        return "";
    }
    let mut len = 0usize;
    while *ptr.add(len) != 0 {
        len += 1;
    }
    let bytes = core::slice::from_raw_parts(ptr as *const u8, len);
    core::str::from_utf8(bytes).unwrap_or("")
}
