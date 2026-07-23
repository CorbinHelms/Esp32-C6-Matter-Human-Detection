//! Seeed Studio XIAO ESP32-C6 board definition.
//!
//! ⚠️ The XIAO silkscreen `D`-labels do **not** map linearly to GPIO numbers.
//! This module is the single source of truth for the mapping so the wiring
//! footgun is documented in exactly one place. Verified against the Seeed wiki
//! and sigmdel.ca; **confirm against your board's silkscreen before wiring.**
//!
//! | Silkscreen | GPIO | Default function        |
//! |------------|------|-------------------------|
//! | D0 / A0    | 0    | ADC                     |
//! | D1 / A1    | 1    | ADC                     |
//! | D2 / A2    | 2    | ADC                     |
//! | D3         | 21   |                         |
//! | D4         | 22   | I2C SDA                 |
//! | D5         | 23   | I2C SCL                 |
//! | D6         | 16   | UART0 TX (console)      |
//! | D7         | 17   | UART0 RX (console)      |
//! | D8         | 19   | SPI SCK                 |
//! | D9         | 20   | SPI MISO                |
//! | D10        | 18   | SPI MOSI                |
//! | (LED)      | 15   | user/builtin LED        |
//!
//! Logs are emitted over **USB-Serial-JTAG** (esp-println `auto` selects it on
//! the C6), so they share the USB-C cable and consume no GPIO — leaving the
//! UART pins entirely free for the mmWave sensor.

/// User/builtin LED on the XIAO ESP32-C6.
pub const LED_GPIO: u8 = 15;

/// The user LED is **active-low**: drive GPIO15 LOW to light it (Seeed wiki;
/// also confirmed on hardware — a solid-ON pattern driven active-high showed
/// as dark-with-blips). A symmetric blink hides the inversion, so only
/// asymmetric patterns (solid/dip) ever expose this.
pub const LED_ACTIVE_LOW: bool = true;

// --- 24GHz mmWave sensor (HLK-LD2410), on a dedicated UART (NOT UART0) ---
//
// The Seeed "24GHz mmWave for XIAO" board routes its serial lines to D2/D3.
// Wiring (sensor side -> board side):
//   sensor TX  -> XIAO D2 (GPIO2)  == ESP RX
//   sensor RX  <- XIAO D3 (GPIO21) == ESP TX

/// ESP RX pin: receives the sensor's TX (XIAO D2).
pub const SENSOR_UART_RX_GPIO: u8 = 2;
/// ESP TX pin: drives the sensor's RX (XIAO D3).
pub const SENSOR_UART_TX_GPIO: u8 = 21;

/// LD2410 default UART baud rate (8N1).
pub const SENSOR_BAUD: u32 = 256_000;

/// Optional digital presence output (LD2410 `OUT` pin) used as a fallback when
/// the UART protocol can't be parsed (e.g. a board revision shipping the
/// Iclegend S3KM1110 instead of the LD2410). Wire the module's OUT pad here and
/// confirm the pin against your board before relying on it.
pub const SENSOR_PRESENCE_GPIO: u8 = 1;
