#![no_std]
#![no_main]
#![deny(
    clippy::mem_forget,
    reason = "mem::forget is generally not safe to do with esp_hal types, especially those \
    holding buffers for the duration of a data transfer."
)]

//! Firmware entry point.
//!
//! M0 (current): brings up the `esp-rtos`/embassy runtime, blinks the builtin
//! LED, and emits a heartbeat log over USB-Serial-JTAG to prove the toolchain,
//! flashing, and logging path. Sensor and Matter tasks are spawned here in
//! later milestones.

use embassy_executor::Spawner;
use embassy_time::{Duration, Timer};
use esp32c6_matter_human_detection::board;
use esp_backtrace as _;
use esp_hal::clock::CpuClock;
use esp_hal::gpio::{Level, Output, OutputConfig};
use esp_hal::timer::timg::TimerGroup;
use log::info;

extern crate alloc;

// Creates the app-descriptor required by the esp-idf 2nd-stage bootloader.
esp_bootloader_esp_idf::esp_app_desc!();

#[esp_rtos::main]
async fn main(spawner: Spawner) -> ! {
    esp_println::logger::init_logger_from_env();

    let config = esp_hal::Config::default().with_cpu_clock(CpuClock::max());
    let peripherals = esp_hal::init(config);

    // Heap for the allocator-backed parts of the stack (Matter needs a large
    // heap later; 64 KiB is plenty for the sensor-only milestones).
    esp_alloc::heap_allocator!(#[esp_hal::ram(reclaimed)] size: 65536);

    // Start the esp-rtos scheduler that backs the embassy executor.
    let timg0 = TimerGroup::new(peripherals.TIMG0);
    let sw_interrupt =
        esp_hal::interrupt::software::SoftwareInterruptControl::new(peripherals.SW_INTERRUPT);
    esp_rtos::start(timg0.timer0, sw_interrupt.software_interrupt0);

    info!("esp32-c6 human-detection firmware booted (M0)");

    // Builtin LED — see `board::LED_GPIO` (GPIO15) for the verified mapping.
    let led = Output::new(peripherals.GPIO15, Level::Low, OutputConfig::default());
    spawner.spawn(blink(led).expect("failed to create blink task"));

    let mut beats: u32 = 0;
    loop {
        info!("alive — heartbeat {beats}");
        beats = beats.wrapping_add(1);
        Timer::after(Duration::from_secs(5)).await;
    }
}

/// Blinks the builtin LED so the board shows a visible sign of life independent
/// of the serial console.
#[embassy_executor::task]
async fn blink(mut led: Output<'static>) {
    let _ = board::LED_GPIO; // documents which pin this task drives
    loop {
        led.toggle();
        Timer::after(Duration::from_millis(500)).await;
    }
}
