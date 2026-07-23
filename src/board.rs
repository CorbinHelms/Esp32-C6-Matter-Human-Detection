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

// --- Antenna selection (XIAO ESP32-C6 FM8625H RF switch, internal nets) ---
//
// The board has an onboard ceramic antenna (hardware default) plus a U.FL
// connector, selected by an RF switch on two GPIOs that are NOT broken out
// on the header: GPIO3 low powers the switch, GPIO14 selects the port
// (low = ceramic, high = U.FL). Selecting U.FL with no antenna attached
// makes range dramatically WORSE, so this is a per-unit build-time opt-in:
// build with EXTERNAL_ANTENNA=1 (provision.py/flash_repeater.py
// --external-antenna, or the GUI checkbox).

/// RF switch power enable (active low). Internal net, not on the header.
pub const RF_SWITCH_EN_GPIO: u8 = 3;
/// RF switch port select: low = onboard ceramic, high = U.FL. Internal net.
pub const RF_SWITCH_SEL_GPIO: u8 = 14;

/// Drive the user LED; `on = true` means *visibly lit* (handles the
/// active-low wiring so callers never think in pin levels).
pub fn led_set(led: &mut esp_hal::gpio::Output<'_>, on: bool) {
    use esp_hal::gpio::Level;

    let high = on != LED_ACTIVE_LOW;
    led.set_level(if high { Level::High } else { Level::Low });
}

/// Set at build time via the `EXTERNAL_ANTENNA` env var ("", "0" and
/// "false" count as unset).
pub const EXTERNAL_ANTENNA: bool = env_flag(option_env!("EXTERNAL_ANTENNA"));

const fn env_flag(v: Option<&str>) -> bool {
    match v {
        None => false,
        Some(s) => !matches!(s.as_bytes(), b"" | b"0" | b"false"),
    }
}

/// Apply the build-time antenna selection. Call once at boot, before radio
/// bring-up. With `EXTERNAL_ANTENNA` unset this touches nothing and the
/// hardware default (ceramic antenna) stays in effect.
pub fn select_antenna(
    rf_switch_en: esp_hal::peripherals::GPIO3<'static>,
    rf_switch_sel: esp_hal::peripherals::GPIO14<'static>,
) {
    use esp_hal::gpio::{Level, Output, OutputConfig};

    if EXTERNAL_ANTENNA {
        // `Output` has no Drop impl, so the pin configuration persists after
        // these bindings go out of scope.
        Output::new(rf_switch_en, Level::Low, OutputConfig::default());
        Output::new(rf_switch_sel, Level::High, OutputConfig::default());
        log::info!("antenna: external (U.FL) selected");
    }
}

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
