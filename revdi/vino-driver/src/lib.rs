//! Userspace libusb transport for DisplayLink DL3 devices.
//!
//! Session state, wire framing, authentication, and orchestration belong to
//! Chimera. This crate contains only USB ownership and transfer operations.

pub mod usb;

pub use usb::{Dock, Error};

/// DisplayLink vendor and known DL3-family products.
pub const VID: u16 = 0x17e9;
pub const PID_D6000: u16 = 0x6006;
pub const PID_THINKPAD_USB3_PRO: u16 = 0x433f;

/// Bulk endpoints we use.
pub const EP_OUT_CTRL: u8 = 0x02;
pub const EP_IN_CTRL: u8 = 0x84;
pub const EP_OUT_VIDEO: u8 = 0x08;
