#![no_std]
#![no_main]

//! Standalone Thread/SRP probe — measures SRP registration reliability against
//! the real border router WITHOUT commissioning (no BLE, no Matter, no phone).
//!
//! Joins the Thread network whose **active dataset TLV hex** is baked in at
//! build time, then loops forever registering SRP services of increasing size
//! (mimicking the Matter commissionable/operational registrations) and logs
//! how many attempts / how long each takes. This isolates the lossy
//! BR→device path that blocks M2 and gives an A/B harness for radio fixes.
//!
//! Build & flash (dataset hex comes from a commissioning-attempt log line
//! "Connecting to Thread network, dataset: ..."):
//! ```sh
//! THREAD_DATASET_HEX=$(cat /path/to/dataset.hex) cargo build --release --bin srp-probe
//! espflash flash --chip esp32c6 --port /dev/ttyACM0 --partition-table partitions.csv \
//!     target/riscv32imac-unknown-none-elf/release/srp-probe
//! ```

use embassy_futures::select::select;
use embassy_time::{Duration, Instant, Timer};
use esp_alloc::heap_allocator;
use esp_backtrace as _;
use esp_hal::ram;
use esp_hal::timer::timg::TimerGroup;
use esp_metadata_generated::memory_range;
use log::{info, warn};
use tinyrlibc as _;

use openthread::esp::{EspRadio, Ieee802154};
use openthread::{
    OpenThread, OtResources, OtSrpResources, SimpleRamSettings, SrpConf, SrpService, SrpState,
};

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

/// Give each registration attempt the same budget a Matter commissioner's
/// fail-safe would (Google armed 120s).
const PHASE_DEADLINE: Duration = Duration::from_secs(120);

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

    info!("srp-probe booted");

    let Some(dataset_hex) = DATASET_HEX else {
        panic!("srp-probe was built without THREAD_DATASET_HEX — see the header of src/bin/srp-probe.rs");
    };
    let mut dataset = [0u8; 254];
    let dataset = decode_hex(dataset_hex, &mut dataset);

    // True RNG: while the TrngSource guard lives, `Rng` yields true random
    // numbers (and it implements rand_core 0.9's `RngCore` = `OtRngCore`).
    let _trng_source = esp_hal::rng::TrngSource::new(peripherals.RNG, peripherals.ADC1);
    let mut rng = esp_hal::rng::Rng::new();

    let mut settings_buf = [0u8; 1024];
    let mut settings = SimpleRamSettings::new(&mut settings_buf);

    let resources = mk_static!(OtResources, OtResources::new());
    let srp_resources = mk_static!(OtSrpResources<4, 1024>, OtSrpResources::new());

    let ot = OpenThread::new_with_srp(
        [0x58, 0xe6, 0xc5, 0xff, 0xfe, 0x15, 0x3a, 0xa8],
        &mut rng,
        &mut settings,
        resources,
        srp_resources,
    )
    .unwrap();

    let radio = EspRadio::new(Ieee802154::new(peripherals.IEEE802154));

    ot.srp_autostart().unwrap();
    ot.set_active_dataset_tlv(dataset).unwrap();
    ot.enable_ipv6(true).unwrap();
    ot.enable_thread(true).unwrap();

    let mut runner = core::pin::pin!(ot.run(radio));
    let mut probe = core::pin::pin!(probe(ot.clone()));
    let mut diag = core::pin::pin!(dump_radio_diag());

    match embassy_futures::select::select3(&mut runner, &mut probe, &mut diag).await {
        embassy_futures::select::Either3::First(_)
        | embassy_futures::select::Either3::Second(_)
        | embassy_futures::select::Either3::Third(_) => unreachable!(),
    }
}

/// Prints the vendored esp-radio ISR counters every 10s (deltas since the
/// previous line), so the radio-level fate of every frame is visible without
/// any logging from the ISR itself.
async fn dump_radio_diag() -> ! {
    let mut prev: ([u32; 12], [u32; 32], [u32; 16]) = Default::default();
    loop {
        Timer::after(Duration::from_secs(10)).await;
        let cur = esp_radio::ieee802154::diag::snapshot();
        let d: heapless::String<256> = {
            let mut s = heapless::String::new();
            let n = &cur.0;
            let p = &prev.0;
            let _ = core::fmt::write(
                &mut s,
                format_args!(
                    "sfd {} rxevt {} rxq {} qfull {} ack_imm {} ack_enh {} noack {} acktx {} txdone {} ackrx {} ackto {} rearm {} | rxab",
                    n[0] - p[0], n[11] - p[11], n[1] - p[1], n[2] - p[2], n[3] - p[3], n[4] - p[4],
                    n[5] - p[5], n[6] - p[6], n[7] - p[7], n[8] - p[8], n[9] - p[9],
                    n[10] - p[10],
                ),
            );
            for (i, (c, pc)) in cur.1.iter().zip(prev.1.iter()).enumerate() {
                if c - pc > 0 {
                    let _ = core::fmt::write(&mut s, format_args!(" {}:{}", i, c - pc));
                }
            }
            let _ = core::fmt::write(&mut s, format_args!(" | txab"));
            for (i, (c, pc)) in cur.2.iter().zip(prev.2.iter()).enumerate() {
                if c - pc > 0 {
                    let _ = core::fmt::write(&mut s, format_args!(" {}:{}", i, c - pc));
                }
            }
            s
        };
        info!("RADIO DIAG {}", d.as_str());
        prev = cur;
    }
}

async fn probe(ot: OpenThread<'_>) -> ! {
    // Wait for attach.
    loop {
        let status = ot.net_status();
        if status.role.is_connected() {
            info!("PROBE: attached, role {:?}", status.role);
            break;
        }
        ot.wait_changed().await;
    }

    // Small per-boot discriminator so re-flashes don't collide with stale
    // server-side registrations under a different ECDSA key.
    let seed = (Instant::now().as_ticks() as u16) ^ 0x5aa5;
    info!("PROBE: seed {seed:04X}");

    let mut hostname = heapless::String::<24>::new();
    let _ = core::fmt::write(&mut hostname, format_args!("PROBE-{seed:04X}"));

    ot.srp_set_conf(&SrpConf {
        host_name: hostname.as_str(),
        ..Default::default()
    })
    .unwrap();

    info!(
        "PROBE: SRP host {} conf set, server: {:?}",
        hostname.as_str(),
        ot.srp_server_addr()
    );

    // Matter-shaped payloads. Phase A ~ a minimal single service; phase B ~ the
    // commissionable service (4 subtypes, 6 TXT entries); phase C ~ both
    // services Matter registers during commissioning (the ~728B update that
    // never lands within the fail-safe).
    let mut inst_c = heapless::String::<20>::new();
    let _ = core::fmt::write(&mut inst_c, format_args!("{seed:04X}73CAD5831857"));
    let mut inst_o = heapless::String::<40>::new();
    let _ = core::fmt::write(
        &mut inst_o,
        format_args!("F143A01EB67C{seed:04X}-00000000C4280E34"),
    );

    let svc_small = SrpService {
        name: "_probe._udp",
        instance_name: inst_c.as_str(),
        port: 5540,
        subtype_labels: ([] as [&str; 0]).into_iter(),
        txt_entries: [("A", &b"1"[..])].into_iter(),
        priority: 0,
        weight: 0,
        lease_secs: 0,
        key_lease_secs: 0,
    };
    let svc_commissionable = SrpService {
        name: "_matterc._udp",
        instance_name: inst_c.as_str(),
        port: 5540,
        subtype_labels: ["_L3840", "_S15", "_V65521", "_CM"].into_iter(),
        txt_entries: [
            ("D", &b"3840"[..]),
            ("CM", &b"1"[..]),
            ("DT", &b"263"[..]),
            ("DN", &b"mmWave Occupancy"[..]),
            ("VP", &b"65521+32769"[..]),
            ("SII", &b"5000"[..]),
        ]
        .into_iter(),
        priority: 0,
        weight: 0,
        lease_secs: 0,
        key_lease_secs: 0,
    };
    let svc_operational = SrpService {
        name: "_matter._tcp",
        instance_name: inst_o.as_str(),
        port: 5540,
        subtype_labels: ["_IF143A01EB67C63E2"].into_iter(),
        txt_entries: [("SII", &b"5000"[..]), ("SAI", &b"500"[..])].into_iter(),
        priority: 0,
        weight: 0,
        lease_secs: 0,
        key_lease_secs: 0,
    };

    let mut stats = [(0u32, 0u32, 0u64); 4]; // (ok, fail, total_ok_ms)

    loop {
        for (idx, name) in [
            "A/small-1svc",
            "B/matterc-1svc",
            "C/matter-2svc",
            "D/churn-2svc",
        ]
        .iter()
        .enumerate()
        {
            // (Re-)register from a clean local state. The immediate clear also
            // wipes the host name, so the conf must be re-set each cycle
            // (mirrors rs-matter-embassy's OtMdns::run_register).
            ot.srp_remove_all(true).unwrap();
            ot.srp_set_conf(&SrpConf {
                host_name: hostname.as_str(),
                ..Default::default()
            })
            .unwrap();
            Timer::after(Duration::from_secs(2)).await;

            match idx {
                0 => {
                    ot.srp_add_service(&svc_small).unwrap();
                }
                1 => {
                    ot.srp_add_service(&svc_commissionable).unwrap();
                }
                2 => {
                    ot.srp_add_service(&svc_commissionable).unwrap();
                    ot.srp_add_service(&svc_operational).unwrap();
                }
                _ => {
                    // Replicate rs-matter-embassy OtMdns::run_register churn:
                    // repeated *immediate* clear + full re-register while the
                    // previous SRP update transaction is still in flight (the
                    // main firmware does this 2-3x within the first seconds
                    // after attach). Suspected to wedge the SRP client.
                    for _ in 0..2 {
                        ot.srp_add_service(&svc_commissionable).unwrap();
                        ot.srp_add_service(&svc_operational).unwrap();
                        // Let the client actually transmit (tx jitter is
                        // 10-700ms), then yank everything mid-transaction.
                        Timer::after(Duration::from_millis(800)).await;
                        ot.srp_remove_all(true).unwrap();
                        ot.srp_set_conf(&SrpConf {
                            host_name: hostname.as_str(),
                            ..Default::default()
                        })
                        .unwrap();
                    }
                    // Final (real) registration, same as phase C.
                    ot.srp_add_service(&svc_commissionable).unwrap();
                    ot.srp_add_service(&svc_operational).unwrap();
                }
            }

            let t0 = Instant::now();
            let deadline = t0 + PHASE_DEADLINE;
            let ok = loop {
                if all_registered(&ot) {
                    break true;
                }
                if Instant::now() >= deadline {
                    break false;
                }
                let _ = select(ot.srp_wait_changed(), Timer::at(deadline)).await;
            };

            let (oks, fails, ms) = &mut stats[idx];
            if ok {
                *oks += 1;
                *ms += t0.elapsed().as_millis();
                info!(
                    "PROBE RESULT {name}: REGISTERED in {} ms",
                    t0.elapsed().as_millis()
                );
            } else {
                *fails += 1;
                warn!(
                    "PROBE RESULT {name}: TIMEOUT after {} s",
                    PHASE_DEADLINE.as_secs()
                );
            }

            let summary: heapless::String<160> = {
                let mut s = heapless::String::new();
                for (i, (o, f, ms)) in stats.iter().enumerate() {
                    let avg = if *o > 0 { ms / (*o as u64) } else { 0 };
                    let _ = core::fmt::write(
                        &mut s,
                        format_args!("[{}: {}ok/{}fail avg {}ms] ", i, o, f, avg),
                    );
                }
                s
            };
            info!("PROBE SUMMARY {}", summary.as_str());
        }
    }
}

fn all_registered(ot: &OpenThread<'_>) -> bool {
    let mut any = false;
    let mut all = true;
    let _ = ot.srp_services(|svc| {
        if let Some((_, state, _)) = svc {
            any = true;
            if !matches!(state, SrpState::Registered) {
                all = false;
            }
        }
    });
    any && all
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
