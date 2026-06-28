#![no_std]
//! Baremetal (`no_std`) firmware for a Seeed XIAO ESP32-C6 driving a 24GHz
//! mmWave human-presence sensor and exposing it to Google Home as a Matter
//! Occupancy Sensor over Thread.
//!
//! Module map:
//! - [`board`] — XIAO ESP32-C6 pin assignments (the non-linear D-label map).
//!
//! Further modules (`presence`, `sensor`, `matter`) are added in later
//! milestones as the sensor and Matter/Thread layers come online.

pub mod board;
