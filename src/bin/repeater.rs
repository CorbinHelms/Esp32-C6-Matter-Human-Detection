#![no_std]
#![no_main]

//! Thread range extender — joins the existing Thread network as an always-on
//! **Full Thread Device** and lets the mesh promote it to a Router, so devices
//! out of the border router's radio range (like a sensor across the house)
//! attach through it. No Matter, no BLE, no sensor: plug it into USB power
//! roughly halfway between the border router and the far device.
//!
//! Thread does the actual "repeating": once this node is a Router it forwards
//! mesh traffic for any device that picked it as parent or next hop. Promotion
//! from child (REED) to Router is automatic — the leader grants a router ID
//! when another device tries to attach through us or the mesh wants more
//! routers — typically within a minute or two of joining.
//!
//! Build & flash (requires the `ftd` feature, which links the router-capable
//! OpenThread library; dataset hex is the same one the probe uses):
//! ```sh
//! THREAD_DATASET_HEX=$(cat .bringup/dataset.hex) \
//!     cargo build --release --features ftd --bin repeater
//! espflash flash --chip esp32c6 --port /dev/ttyACM0 --partition-table partitions.csv \
//!     target/riscv32imac-unknown-none-elf/release/repeater
//! ```
//!
//! LED (GPIO15) shows the role at a glance:
//! - fast blink: detached / joining
//! - slow blink: attached as child (not yet routing)
//! - solid with a short dip every 3 s: Router/Leader — repeating traffic

use embassy_time::{Duration, Timer};
use esp_alloc::heap_allocator;
use esp_backtrace as _;
use esp_hal::gpio::{Level, Output, OutputConfig};
use esp_hal::ram;
use esp_hal::timer::timg::TimerGroup;
use esp_metadata_generated::memory_range;
use log::info;
use tinyrlibc as _;

use openthread::esp::{EspRadio, Ieee802154};
use openthread::{DeviceRole, OpenThread, OtResources, SimpleRamSettings};

extern crate alloc;

esp_bootloader_esp_idf::esp_app_desc!();

/// Same static-allocation helper as the main firmware.
macro_rules! mk_static {
    ($t:ty, $val:expr) => {{
        static STATIC_CELL: static_cell::StaticCell<$t> = static_cell::StaticCell::new();
        STATIC_CELL.init($val)
    }};
}

/// Active dataset TLVs (hex) captured from a commissioning attempt.
/// Injected at build time so the (sensitive) network key never enters git.
const DATASET_HEX: Option<&str> = option_env!("THREAD_DATASET_HEX");

const HEAP_SIZE: usize = 128 * 1024;
const RECLAIMED_RAM: usize =
    memory_range!("DRAM2_UNINIT").end - memory_range!("DRAM2_UNINIT").start;

#[esp_rtos::main]
async fn main(_spawner: embassy_executor::Spawner) -> ! {
    esp_println::logger::init_logger_from_env();

    heap_allocator!(size: HEAP_SIZE - RECLAIMED_RAM);
    heap_allocator!(#[ram(reclaimed)] size: RECLAIMED_RAM);

    let peripherals = esp_hal::init(esp_hal::Config::default());

    let timg0 = TimerGroup::new(peripherals.TIMG0);
    let sw_interrupt =
        esp_hal::interrupt::software::SoftwareInterruptControl::new(peripherals.SW_INTERRUPT);
    esp_rtos::start(timg0.timer0, sw_interrupt.software_interrupt0);

    let Some(dataset_hex) = DATASET_HEX else {
        panic!("repeater was built without THREAD_DATASET_HEX — see the header of src/bin/repeater.rs");
    };
    let mut dataset = [0u8; 254];
    let dataset = decode_hex(dataset_hex, &mut dataset);

    // Per-chip EUI-64 from the factory MAC (EUI-48 + ff:fe midfix), so several
    // repeaters can coexist without hardcoded addresses colliding.
    let mac = esp_hal::efuse::base_mac_address();
    let m = mac.as_bytes();
    let eui64 = [m[0], m[1], m[2], 0xff, 0xfe, m[3], m[4], m[5]];
    info!("thread repeater booted, mac {mac}, eui64 {eui64:02x?}");

    let _trng_source = esp_hal::rng::TrngSource::new(peripherals.RNG, peripherals.ADC1);
    let mut rng = esp_hal::rng::Rng::new();

    let mut settings_buf = [0u8; 1024];
    let mut settings = SimpleRamSettings::new(&mut settings_buf);

    let resources = mk_static!(OtResources, OtResources::new());
    let ot = OpenThread::new(eui64, &mut rng, &mut settings, resources).unwrap();

    let radio = EspRadio::new(Ieee802154::new(peripherals.IEEE802154));

    // Full Thread Device, receiver always on, full network data — i.e.
    // router-eligible. The leader upgrades us from REED to Router on demand.
    ot.set_link_mode(true, true, true).unwrap();
    ot.set_active_dataset_tlv(dataset).unwrap();
    ot.enable_ipv6(true).unwrap();
    ot.enable_thread(true).unwrap();

    let led = Output::new(peripherals.GPIO15, Level::Low, OutputConfig::default());

    let mut runner = core::pin::pin!(ot.run(radio));
    let mut status = core::pin::pin!(status(ot.clone(), led));

    match embassy_futures::select::select(&mut runner, &mut status).await {
        embassy_futures::select::Either::First(_) | embassy_futures::select::Either::Second(_) => {
            unreachable!()
        }
    }
}

/// Logs role changes and drives the role-indicating LED pattern.
async fn status(ot: OpenThread<'_>, mut led: Output<'static>) -> ! {
    let mut last_role = None;
    loop {
        let role = ot.net_status().role;
        if last_role != Some(role) {
            info!("repeater: role -> {role:?}");
            if role.is_connected() && !matches!(last_role, Some(r) if r.is_connected()) {
                let _ = ot.ipv6_addrs(|addr| {
                    if let Some((addr, prefix)) = addr {
                        info!("repeater: addr {addr}/{prefix}");
                    }
                    Ok(())
                });
            }
            last_role = Some(role);
        }

        match role {
            DeviceRole::Router | DeviceRole::Leader => {
                led.set_high();
                Timer::after(Duration::from_millis(2900)).await;
                led.set_low();
                Timer::after(Duration::from_millis(100)).await;
            }
            DeviceRole::Child => {
                led.set_high();
                Timer::after(Duration::from_millis(500)).await;
                led.set_low();
                Timer::after(Duration::from_millis(500)).await;
            }
            _ => {
                led.set_high();
                Timer::after(Duration::from_millis(120)).await;
                led.set_low();
                Timer::after(Duration::from_millis(120)).await;
            }
        }
    }
}

fn decode_hex<'a>(hex: &str, out: &'a mut [u8]) -> &'a [u8] {
    let hex = hex.trim();
    let n = hex.len() / 2;
    assert!(n <= out.len(), "dataset hex too long");
    for i in 0..n {
        out[i] = u8::from_str_radix(&hex[2 * i..2 * i + 2], 16).expect("bad dataset hex");
    }
    &out[..n]
}
