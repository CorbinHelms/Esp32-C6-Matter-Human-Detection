#![no_std]
#![no_main]
#![deny(
    clippy::mem_forget,
    reason = "mem::forget is generally not safe to do with esp_hal types, especially those \
    holding buffers for the duration of a data transfer."
)]
#![recursion_limit = "256"]

//! Firmware entry point.
//!
//! Brings up the `esp-rtos`/embassy runtime, blinks the builtin LED for
//! liveness, runs the LD2410 mmWave sensor task (publishing debounced presence
//! on [`sensor::PRESENCE`]), then runs the Matter-over-Thread stack exposing an
//! Occupancy Sensor to Google Home.

use embassy_executor::Spawner;
use embassy_time::{Duration, Timer};
use esp32c6_matter_human_detection::{board, matter, sensor};
use esp_alloc::heap_allocator;
use esp_backtrace as _;
use esp_hal::gpio::{Level, Output, OutputConfig};
use esp_hal::interrupt::Priority;
use esp_hal::ram;
use esp_hal::timer::timg::TimerGroup;
use esp_hal::uart::{Config as UartConfig, Uart};
use esp_metadata_generated::memory_range;
use esp_rtos::embassy::InterruptExecutor;
use log::info;
use openthread::esp::{EspRadio, Ieee802154};
use openthread::{EmbassyTimeTimer, PhyRadioRunner, ProxyRadio, ProxyRadioResources};
use static_cell::StaticCell;
use tinyrlibc as _;

extern crate alloc;

// Creates the app-descriptor required by the esp-idf 2nd-stage bootloader.
esp_bootloader_esp_idf::esp_app_desc!();

/// Heap for the Matter stack (x509/crypto) and the radios. Matter needs a large
/// heap; the rs-matter-embassy Thread example uses 100KB — we add headroom for
/// the deepened 802.15.4 RX queue (200 frames ≈ 26KB, see vendor/openthread).
const HEAP_SIZE: usize = 128 * 1024;
/// Reclaimable RAM region folded into the heap.
const RECLAIMED_RAM: usize =
    memory_range!("DRAM2_UNINIT").end - memory_range!("DRAM2_UNINIT").start;

#[esp_rtos::main]
async fn main(spawner: Spawner) -> ! {
    esp_println::logger::init_logger_from_env();

    heap_allocator!(size: HEAP_SIZE - RECLAIMED_RAM);
    heap_allocator!(#[ram(reclaimed)] size: RECLAIMED_RAM);

    let peripherals = esp_hal::init(esp_hal::Config::default());

    // Start the esp-rtos scheduler that backs the embassy executor.
    let timg0 = TimerGroup::new(peripherals.TIMG0);
    let sw_interrupt =
        esp_hal::interrupt::software::SoftwareInterruptControl::new(peripherals.SW_INTERRUPT);
    esp_rtos::start(timg0.timer0, sw_interrupt.software_interrupt0);

    info!("esp32-c6 human-detection firmware booted (M2: Matter over Thread)");

    // Builtin LED liveness — see `board::LED_GPIO` (GPIO15).
    let led = Output::new(peripherals.GPIO15, Level::Low, OutputConfig::default());
    spawner.spawn(blink(led).expect("failed to create blink task"));

    // LD2410 mmWave sensor on UART1 (RX = GPIO2/D2, TX = GPIO21/D3), 8N1 @ 256000.
    let sensor_uart = Uart::new(
        peripherals.UART1,
        UartConfig::default().with_baudrate(board::SENSOR_BAUD),
    )
    .expect("LD2410 UART config")
    .with_rx(peripherals.GPIO2)
    .with_tx(peripherals.GPIO21)
    .into_async();
    spawner.spawn(sensor::sensor_task(sensor_uart).expect("failed to create sensor task"));

    // Split the 802.15.4 radio: the PHY half runs on a high-priority interrupt
    // executor so radio timing (ACKs, MRP-critical RX) survives the multi-
    // second mbedtls operations Matter commissioning runs on this executor
    // (PASE/SPAKE2p, attestation, CASE). The Matter stack gets the proxy half.
    static RADIO_EXECUTOR: StaticCell<InterruptExecutor<1>> = StaticCell::new();
    let radio_spawner = RADIO_EXECUTOR
        .init(InterruptExecutor::new(sw_interrupt.software_interrupt1))
        .start(Priority::Priority2);

    static PROXY_RADIO_RESOURCES: StaticCell<ProxyRadioResources> = StaticCell::new();
    let (radio_proxy, phy_runner) = ProxyRadio::<{ matter::RADIO_CAPS }>::new(
        PROXY_RADIO_RESOURCES.init(ProxyRadioResources::new()),
    );
    radio_spawner.spawn(
        phy_radio_task(
            phy_runner,
            EspRadio::new(Ieee802154::new(peripherals.IEEE802154)),
        )
        .expect("failed to create PHY radio task"),
    );

    // Run Matter-over-Thread forever (consumes the radio proxy, BT, RNG/ADC1,
    // flash for fabric persistence, and the BOOT pin for factory reset).
    matter::run(
        radio_proxy,
        peripherals.BT,
        peripherals.RNG,
        peripherals.ADC1,
        peripherals.FLASH,
        peripherals.GPIO9,
    )
    .await
}

/// Services the 802.15.4 PHY (TX/RX/ACK timing) on the interrupt executor.
#[embassy_executor::task]
async fn phy_radio_task(mut runner: PhyRadioRunner<'static>, radio: EspRadio<'static>) -> ! {
    runner.run(radio, EmbassyTimeTimer).await
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
