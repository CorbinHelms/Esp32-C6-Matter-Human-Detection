#![no_std]
//! Baremetal (`no_std`) firmware for a Seeed XIAO ESP32-C6 driving a 24GHz
//! mmWave human-presence sensor and exposing it to Google Home as a Matter
//! Occupancy Sensor over Thread.
//!
//! Module map:
//! - [`board`]    — XIAO ESP32-C6 pin assignments (the non-linear D-label map).
//! - [`presence`] — debounces the raw radar bit into a stable occupancy state.
//! - [`sensor`]   — async HLK-LD2410 mmWave driver + the shared presence signal.
//!
//! The `matter` module (Matter Occupancy Sensor over Thread) is added in the
//! next milestone.

pub mod board;
pub mod presence;
pub mod sensor;
