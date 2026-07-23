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
//! - fast blink: searching — cannot find/join the network from here (out of
//!   range or 2.4 GHz interference; USB-3 ports and busy WiFi are classic).
//!   The device stays child-only while searching, so it can never fracture
//!   the home network by forming a partition of its own.
//! - slow blink: attached as child (not yet routing)
//! - solid with a short dip every 3 s: Router (or Leader of a shared
//!   partition) — meshed and repeating traffic. Extra quick dips right
//!   after the main one count attached children (double dip = one device
//!   is parented through this repeater)
//! - double-flash then pause: Leader of a **singleton** partition (should no
//!   longer happen with the eligibility gating; kept as a safety net)

use embassy_time::{Duration, Instant, Timer};
use esp_alloc::heap_allocator;
use esp_backtrace as _;
use esp_hal::gpio::{Level, Output, OutputConfig};
use esp_hal::ram;
use esp_hal::timer::timg::TimerGroup;
use esp_metadata_generated::memory_range;
use log::{info, warn};
use tinyrlibc as _;

use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::channel::Channel;
use esp32c6_matter_human_detection::board::{self, led_set};
use openthread::esp::{EspRadio, Ieee802154};
use openthread::{DeviceRole, OpenThread, OtResources, OtUdpResources, SimpleRamSettings, UdpSocket};

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

/// Optional remote log sink: an IPv6 address (typically the dev PC's global
/// address) that receives every log line — including OpenThread's internal
/// debug stream — as UDP datagrams on port 9999, routed out through the
/// border router. Gives full radio-stack visibility while the repeater sits
/// deployed at a wall, no serial cable involved.
const TELEMETRY_DEST: Option<&str> = option_env!("TELEMETRY_DEST");
/// The dev PC's IPv4 (dotted quad), reached as a second sink via the border
/// router's NAT64 prefix **discovered from Thread network data** at runtime.
/// (Google's Nest BR publishes a ULA-derived /96 — observed
/// fd6b:588a:7d6a:2::/96 — NOT the well-known 64:ff9b::/96 that v2 assumed;
/// the well-known prefix is kept only as a fallback guess if no /96 route is
/// published.) On this mesh the netdata routes are fc00::/7 + the NAT64 /96
/// only — no default route — so this is the ONLY sink that can work.
const TELEMETRY_DEST_V4: Option<&str> = option_env!("TELEMETRY_DEST_V4");
const TELEMETRY_PORT: u16 = 9999;

/// Log lines waiting for the UDP telemetry task (drop-on-full).
static TELEMETRY: Channel<CriticalSectionRawMutex, heapless::String<160>, 64> = Channel::new();

/// Global logger: mirrors everything to the USB console and queues it for
/// UDP telemetry. OpenThread's own debug stream (target "openthread…") is
/// enabled; everything else logs at info.
///
/// Feedback guard: sending telemetry makes OpenThread log its own UDP/IP6
/// transmissions, which would re-enter the queue and self-amplify forever —
/// any line mentioning the telemetry port/socket is dropped from the queue
/// (still printed to the console).
struct TeleLogger;
static TELE_LOGGER: TeleLogger = TeleLogger;

impl log::Log for TeleLogger {
    fn enabled(&self, m: &log::Metadata) -> bool {
        if m.target().starts_with("openthread") {
            m.level() <= log::Level::Debug
        } else {
            m.level() <= log::Level::Info
        }
    }

    fn log(&self, r: &log::Record) {
        if !self.enabled(r.metadata()) {
            return;
        }
        esp_println::println!("{} - {}", r.level(), r.args());
        let mut s = heapless::String::<160>::new();
        let _ = core::fmt::write(&mut s, format_args!("{}", r.args())); // truncates
        // "Failed to find valid route": OT's Ip6 error for an unroutable sink,
        // logged once PER SEND with no port in the line — it slipped the two
        // port filters and self-amplified into a 100+ line/s storm (observed
        // 2026-07-23; also starved the heartbeat).
        // "MeshForwarder": every per-packet TX/RX/drop summary from that
        // module. Each telemetry send generates at least one such port-free
        // line ("Sent IPv6 UDP msg" on success, "Dropping ... NoRoute" on
        // route failure — both observed echoing at ratio 1.0, ~235
        // datagrams/s), so the module's chatter can never be queue-safe.
        // Console still shows it all; MLE/RouterTable lines still stream.
        // The circuit breaker in telemetry() is the backstop for whatever
        // port-free line another module invents next.
        if s.contains(":9999")
            || s.contains("12424")
            || s.contains("Failed to find valid route")
            || s.contains("MeshForwarder")
        {
            return;
        }
        let _ = TELEMETRY.try_send(s);
    }

    fn flush(&self) {}
}

const HEAP_SIZE: usize = 128 * 1024;
const RECLAIMED_RAM: usize =
    memory_range!("DRAM2_UNINIT").end - memory_range!("DRAM2_UNINIT").start;

#[esp_rtos::main]
async fn main(_spawner: embassy_executor::Spawner) -> ! {
    log::set_logger(&TELE_LOGGER).unwrap();
    log::set_max_level(log::LevelFilter::Debug);

    heap_allocator!(size: HEAP_SIZE - RECLAIMED_RAM);
    heap_allocator!(#[ram(reclaimed)] size: RECLAIMED_RAM);

    let peripherals = esp_hal::init(esp_hal::Config::default());
    board::select_antenna(peripherals.GPIO3, peripherals.GPIO14);

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
    let udp_resources = mk_static!(OtUdpResources<2, 1024>, OtUdpResources::new());
    let ot =
        OpenThread::new_with_udp(eui64, &mut rng, &mut settings, resources, udp_resources).unwrap();

    let radio = EspRadio::new(Ieee802154::new(peripherals.IEEE802154));

    // Full Thread Device, receiver always on, full network data.
    ot.set_link_mode(true, true, true).unwrap();
    // Never form our own partition: stay ineligible for Router/Leader until
    // we have actually attached as a child of the existing network (status()
    // flips it). A repeater that can't join must keep searching — a stray
    // partition from a half-jammed radio pulls real routers away from the
    // home network (observed: the border router migrated to ours once).
    ot.set_router_eligible(false).unwrap();
    ot.set_active_dataset_tlv(dataset).unwrap();
    ot.enable_ipv6(true).unwrap();
    ot.enable_thread(true).unwrap();

    let led_off = if board::LED_ACTIVE_LOW { Level::High } else { Level::Low };
    let led = Output::new(peripherals.GPIO15, led_off, OutputConfig::default());

    let mut runner = core::pin::pin!(ot.run(radio));
    let mut status = core::pin::pin!(status(ot.clone(), led));
    let mut diag = core::pin::pin!(dump_radio_diag());
    let mut tele = core::pin::pin!(telemetry(ot.clone()));

    match embassy_futures::select::select4(&mut runner, &mut status, &mut diag, &mut tele).await {
        embassy_futures::select::Either4::First(_)
        | embassy_futures::select::Either4::Second(_)
        | embassy_futures::select::Either4::Third(_)
        | embassy_futures::select::Either4::Fourth(_) => unreachable!(),
    }
}

/// Streams queued log lines to `TELEMETRY_DEST` as UDP datagrams, batched a
/// few lines per packet (batching also keeps the OT-logs-about-telemetry
/// amplification ratio below 1, alongside the logger's content filter).
async fn telemetry(ot: OpenThread<'_>) -> ! {
    use core::net::{Ipv6Addr, SocketAddrV6};

    loop {
        if ot.net_status().role.is_connected() {
            break;
        }
        ot.wait_changed().await;
    }
    Timer::after(Duration::from_secs(3)).await;

    let local = SocketAddrV6::new(Ipv6Addr::UNSPECIFIED, 12424, 0, 0);
    let sock = loop {
        match UdpSocket::bind(ot.clone(), &local) {
            Ok(s) => break s,
            Err(_) => Timer::after(Duration::from_secs(5)).await,
        }
    };

    // What can the mesh route to? (Answers "why doesn't a sink work".) The
    // /96 external route is the border router's NAT64 prefix — harvest it —
    // and every entry goes into the coverage table used to vet sinks below.
    let mut nat64: Option<Ipv6Addr> = None;
    let mut coverage: heapless::Vec<(Ipv6Addr, u8), 8> = heapless::Vec::new();
    ot.netdata_dump(|is_route, octets, len| {
        let ip = Ipv6Addr::from(*octets);
        info!(
            "netdata {}: {ip}/{len}",
            if is_route { "route" } else { "prefix" }
        );
        if is_route && len == 96 && nat64.is_none() {
            nat64 = Some(ip);
        }
        let _ = coverage.push((ip, len));
    });

    // Sink list: the PC's global IPv6, plus the PC's IPv4 embedded in the
    // discovered NAT64 prefix. A sink no published route covers is disabled
    // up front: send() reports Ok even for those (the drop happens later, in
    // forwarding — observed 2026-07-23), so its ok-counter would just lie.
    // On a mesh that publishes a default route the GUA sink auto-enables.
    let mut sinks: heapless::Vec<SocketAddrV6, 2> = heapless::Vec::new();
    if let Some(Ok(ip)) = TELEMETRY_DEST.map(|s| s.parse::<Ipv6Addr>()) {
        if covered(&ip, &coverage) {
            let _ = sinks.push(SocketAddrV6::new(ip, TELEMETRY_PORT, 0, 0));
        } else {
            info!("telemetry: sink {ip} not covered by any mesh route, disabled");
        }
    }
    if let Some(Ok(v4)) = TELEMETRY_DEST_V4.map(|s| s.parse::<core::net::Ipv4Addr>()) {
        let p = nat64
            .unwrap_or(Ipv6Addr::new(0x64, 0xff9b, 0, 0, 0, 0, 0, 0))
            .segments();
        let [a, b, c, d] = v4.octets();
        let ip = Ipv6Addr::new(
            p[0], p[1], p[2], p[3], p[4], p[5],
            u16::from_be_bytes([a, b]),
            u16::from_be_bytes([c, d]),
        );
        if covered(&ip, &coverage) {
            info!(
                "telemetry: nat64 sink {ip} (prefix {} netdata)",
                if nat64.is_some() { "from" } else { "NOT in" }
            );
            let _ = sinks.push(SocketAddrV6::new(ip, TELEMETRY_PORT, 0, 0));
        } else {
            info!("telemetry: nat64 sink {ip} not covered by any mesh route, disabled");
        }
    }
    if sinks.is_empty() {
        loop {
            Timer::after(Duration::from_secs(3600)).await;
        }
    }
    info!("telemetry: streaming to {} sink(s)", sinks.len());

    // Consecutive-error backoff per sink: an unroutable sink makes OpenThread
    // log an Ip6 error PER SEND; without a bound, those lines re-enter the
    // queue and sustain a log storm (observed: 100+ lines/s for 8 min).
    let mut ok = [0u32; 2];
    let mut err = [0u32; 2];
    let mut consec = [0u8; 2];
    let mut bench_until = [Instant::from_ticks(0); 2];
    let mut last_hb = Instant::now();
    let mut window_batches: u32 = 0;
    let mut batch = heapless::String::<1400>::new();
    loop {
        // Heartbeat is emitted here, NOT in the timer branch of the select:
        // a busy channel wins the select every time, and v2's timer-branch
        // heartbeat never ran once during the storm.
        if Instant::now() >= last_hb + Duration::from_secs(10) {
            last_hb = Instant::now();
            // Queued via info!, rides the next batch to every sink.
            info!(
                "tele hb sinks ok={},{} err={},{}",
                ok[0], ok[1], err[0], err[1]
            );
            // Circuit breaker: legit debug traffic is tens of batches per
            // window; hundreds means some new OT log line is echoing our own
            // sends. Bench everything briefly — the echo dies with the sends.
            if window_batches > 300 {
                warn!("telemetry: {window_batches} batches/10s — echo storm, pausing 10s");
                let until = Instant::now() + Duration::from_secs(10);
                bench_until = [until; 2];
            }
            window_batches = 0;
        }
        match embassy_futures::select::select(
            TELEMETRY.receive(),
            Timer::at(last_hb + Duration::from_secs(10)),
        )
        .await
        {
            embassy_futures::select::Either::First(first) => {
                batch.clear();
                let _ = batch.push_str(first.as_str());
                for _ in 0..7 {
                    let Ok(next) = TELEMETRY.try_receive() else {
                        break;
                    };
                    if batch.len() + next.len() + 1 > batch.capacity() {
                        break;
                    }
                    let _ = batch.push('\n');
                    let _ = batch.push_str(next.as_str());
                }
                let mut sent_any = false;
                for (i, dest) in sinks.iter().enumerate() {
                    if Instant::now() < bench_until[i] {
                        continue;
                    }
                    sent_any = true;
                    match sock.send(batch.as_bytes(), None, dest).await {
                        Ok(()) => {
                            ok[i] += 1;
                            consec[i] = 0;
                        }
                        Err(_) => {
                            err[i] += 1;
                            consec[i] = consec[i].saturating_add(1);
                            if consec[i] >= 5 {
                                bench_until[i] = Instant::now() + Duration::from_secs(30);
                                consec[i] = 0;
                            }
                        }
                    }
                }
                if sent_any {
                    window_batches += 1;
                }
            }
            embassy_futures::select::Either::Second(()) => {}
        }
    }
}

/// Longest-prefix containment check against the netdata coverage table: is
/// `dest` inside any published on-mesh prefix or external route? (A /0 route
/// — a real default route — covers everything.)
fn covered(dest: &core::net::Ipv6Addr, table: &[(core::net::Ipv6Addr, u8)]) -> bool {
    table.iter().any(|(prefix, len)| {
        let (d, p) = (dest.octets(), prefix.octets());
        let whole = (*len / 8) as usize;
        let rem = *len % 8;
        d[..whole] == p[..whole]
            && (rem == 0 || (d[whole] ^ p[whole]) >> (8 - rem) == 0)
    })
}

/// Prints the vendored esp-radio ISR counters every 15 s (deltas since the
/// previous line) — same x-ray as srp-probe, so a bench session shows the
/// radio-level fate of every frame (e.g. CCA-busy TX aborts under
/// interference) without logging from the ISR.
async fn dump_radio_diag() -> ! {
    let mut prev: ([u32; 12], [u32; 32], [u32; 16]) = Default::default();
    loop {
        Timer::after(Duration::from_secs(15)).await;
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

/// Logs role changes and drives the role-indicating LED pattern.
///
/// Also nudges the stack toward the Router role: the autonomous REED→Router
/// upgrade rides a randomized jitter and was observed taking 10+ minutes (or
/// stalling outright) on the real Nest-led network, so while attached as a
/// child we explicitly solicit a router ID every 30 s.
async fn status(ot: OpenThread<'_>, mut led: Output<'static>) -> ! {
    let mut last_role = None;
    let mut child_secs = 0u32;
    let mut last_children = 0u16;
    let mut last_lonely = false;
    loop {
        let role = ot.net_status().role;
        // A Leader that stays singleton didn't join anything — it gave up and
        // formed its own partition. Treat that as "not working".
        let lonely = role == DeviceRole::Leader && ot.is_singleton();
        if lonely != last_lonely {
            if lonely {
                warn!(
                    "repeater: became leader of a SINGLETON partition — the join \
                     handshake with the existing network is failing (interference \
                     or out of range); nothing is being repeated"
                );
            } else {
                info!("repeater: partition is shared now (no longer singleton)");
            }
            last_lonely = lonely;
        }
        if last_role != Some(role) {
            info!("repeater: role -> {role:?}");
            match role {
                // Attached: now it's safe to be promotable — promotion can
                // only mean joining the existing partition's router set.
                DeviceRole::Child => {
                    if ot.set_router_eligible(true).is_ok() {
                        info!("repeater: attached, router eligibility enabled");
                    }
                }
                // Lost the network: back to child-only so a jammed/isolated
                // radio can't spin up a partition of its own.
                DeviceRole::Detached | DeviceRole::Disabled => {
                    let _ = ot.set_router_eligible(false);
                }
                _ => {}
            }
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
            DeviceRole::Leader if lonely => {
                child_secs = 0;
                last_children = 0;
                // Double-flash + long pause: "radio is fine, but I'm alone".
                for _ in 0..2 {
                    led_set(&mut led, true);
                    Timer::after(Duration::from_millis(120)).await;
                    led_set(&mut led, false);
                    Timer::after(Duration::from_millis(120)).await;
                }
                Timer::after(Duration::from_millis(1520)).await;
            }
            DeviceRole::Router | DeviceRole::Leader => {
                child_secs = 0;
                // Devices attaching *through us* show up here — the proof the
                // far sensor is actually using the repeater.
                let children = ot.child_count();
                if children != last_children {
                    info!("repeater: attached children -> {children}");
                    last_children = children;
                }
                led_set(&mut led, true);
                Timer::after(Duration::from_millis(2900)).await;
                led_set(&mut led, false);
                Timer::after(Duration::from_millis(100)).await;
                // One extra quick dip per attached child (capped at 5), so
                // "is the far sensor using me?" is visible without serial:
                // single dip = routing but no children; double dip = 1 child.
                for _ in 0..children.min(5) {
                    led_set(&mut led, true);
                    Timer::after(Duration::from_millis(180)).await;
                    led_set(&mut led, false);
                    Timer::after(Duration::from_millis(180)).await;
                }
            }
            DeviceRole::Child => {
                child_secs += 1;
                last_children = 0;
                if child_secs % 30 == 10 {
                    match ot.become_router() {
                        Ok(()) => info!("repeater: soliciting router role"),
                        Err(e) => warn!("repeater: router solicit failed: {e:?}"),
                    }
                }
                led_set(&mut led, true);
                Timer::after(Duration::from_millis(500)).await;
                led_set(&mut led, false);
                Timer::after(Duration::from_millis(500)).await;
            }
            _ => {
                child_secs = 0;
                last_children = 0;
                led_set(&mut led, true);
                Timer::after(Duration::from_millis(120)).await;
                led_set(&mut led, false);
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
