#![no_std]
#![allow(async_fn_in_trait)]
//! Baremetal (`no_std`) firmware for a Seeed XIAO ESP32-C6 driving a 24GHz
//! mmWave human-presence sensor and exposing it to Google Home as a Matter
//! Occupancy Sensor over Thread.
//!
//! Module map:
//! - [`board`]    — XIAO ESP32-C6 pin assignments (the non-linear D-label map).
//! - [`presence`] — debounces the raw radar bit into a stable occupancy state.
//! - [`sensor`]   — async HLK-LD2410 mmWave driver + the shared presence signal.
//! - [`matter`]   — Matter Occupancy Sensor over Thread → Google Home.

pub mod board;
pub mod matter;
pub mod presence;
pub mod sensor;
