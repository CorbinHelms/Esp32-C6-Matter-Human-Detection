//! HLK-LD2410 24GHz mmWave radar driver (async, `no_std`).
//!
//! The LD2410 free-runs after power-up, continuously emitting "report" frames
//! over UART (8N1 @ 256000 baud). We parse the *basic mode* (`0x02`) report to
//! extract whether a target is present (moving and/or stationary) plus distance
//! and energy for diagnostics, debounce it ([`crate::presence`]), and publish a
//! stable occupied/unoccupied bit on [`PRESENCE`] for the Matter layer.
//!
//! ## Report frame layout
//! ```text
//! F4 F3 F2 F1 | len(u16 LE) | <intra-frame data> | F8 F7 F6 F5
//! ```
//! Basic-mode intra-frame data (len = 13):
//! ```text
//! 02 AA | state | mov_dist(LE) mov_energy | stat_dist(LE) stat_energy | det_dist(LE) | 00 55
//! ```
//! `state`: 0x00 no target · 0x01 moving · 0x02 stationary · 0x03 both. Anything
//! non-zero counts as "someone present"; stationary is the key static-presence
//! signal for occupancy.
//!
//! We avoid the blocking `ld2410` crate on purpose: a blocking read would stall
//! the async executor. Hand-rolling the parser also lets us tolerate the minor
//! trailing-field differences seen across LD2410 firmware revisions.

use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::signal::Signal;
use embassy_time::{Duration, Instant, Timer};
use esp_hal::Async;
use esp_hal::uart::{RxError, Uart};
use log::{info, warn};

use crate::presence::PresenceDebouncer;

/// Stable, debounced occupancy state published by the sensor task and consumed
/// by the Matter task. `true` = occupied (someone present), `false` = vacant.
pub static PRESENCE: Signal<CriticalSectionRawMutex, bool> = Signal::new();

const REPORT_HEADER: [u8; 4] = [0xF4, 0xF3, 0xF2, 0xF1];
const REPORT_FOOTER: [u8; 4] = [0xF8, 0xF7, 0xF6, 0xF5];
const DATA_HEAD: u8 = 0xAA;
const DATA_TYPE_BASIC: u8 = 0x02;
const DATA_TYPE_ENGINEERING: u8 = 0x01;

/// Largest intra-frame data we accept (engineering-mode frames are ~45 bytes).
const MAX_FRAME_DATA: usize = 64;

/// How long the raw absence must persist before we report "vacant", on top of
/// the LD2410's own absence delay. Tunable to taste in [`crate::presence`].
const HOLD_MS: u64 = 2_000;

/// Reported target state (LD2410 basic-mode `state` byte).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TargetState {
    /// No target detected.
    None,
    /// A moving target only.
    Moving,
    /// A stationary (still) target only.
    Stationary,
    /// Both moving and stationary targets.
    Both,
    /// An unrecognized state byte (firmware/protocol mismatch).
    Unknown(u8),
}

impl TargetState {
    fn from_u8(b: u8) -> Self {
        match b {
            0x00 => Self::None,
            0x01 => Self::Moving,
            0x02 => Self::Stationary,
            0x03 => Self::Both,
            other => Self::Unknown(other),
        }
    }

    /// Whether this state means "someone is present".
    pub fn is_present(self) -> bool {
        matches!(self, Self::Moving | Self::Stationary | Self::Both)
    }
}

/// A parsed basic-mode presence report. Distances are in centimetres, energies
/// are 0..=255 confidence values.
#[derive(Debug, Clone, Copy)]
pub struct PresenceReport {
    pub state: TargetState,
    pub moving_distance_cm: u16,
    pub moving_energy: u8,
    pub stationary_distance_cm: u16,
    pub stationary_energy: u8,
    /// Detection (max) distance — absent on some firmware revisions.
    pub detection_distance_cm: Option<u16>,
}

/// Parses the intra-frame data of a report into a [`PresenceReport`].
///
/// Pure and tolerant of trailing-field variation: requires only the leading
/// fields (through stationary energy) and treats the detection distance as
/// optional. Returns `None` for non-report data types or malformed frames.
fn parse_report(data: &[u8]) -> Option<PresenceReport> {
    // data[0] = data type, data[1] = head marker 0xAA, data[2..] = payload.
    if data.len() < 9 || data[1] != DATA_HEAD {
        return None;
    }
    if data[0] != DATA_TYPE_BASIC && data[0] != DATA_TYPE_ENGINEERING {
        return None;
    }
    let le16 = |i: usize| u16::from_le_bytes([data[i], data[i + 1]]);
    Some(PresenceReport {
        state: TargetState::from_u8(data[2]),
        moving_distance_cm: le16(3),
        moving_energy: data[5],
        stationary_distance_cm: le16(6),
        stationary_energy: data[8],
        detection_distance_cm: (data.len() >= 11).then(|| le16(9)),
    })
}

/// Errors from reading a frame off the UART.
// Fields are surfaced through `Debug` in the warn log; rustc's dead-code pass
// doesn't count `{:?}` formatting as a read, hence the allow.
#[derive(Debug)]
#[allow(dead_code)]
enum FrameError {
    Uart(RxError),
    /// Length field exceeds our buffer (likely desync / wrong baud).
    TooLong(usize),
    /// Footer bytes did not match — frame boundary lost.
    BadFooter,
}

impl From<RxError> for FrameError {
    fn from(e: RxError) -> Self {
        FrameError::Uart(e)
    }
}

/// Reads one complete report frame into `buf`, returning the intra-frame data
/// length. Resynchronizes on the 4-byte header so a mid-stream start recovers.
async fn read_frame(uart: &mut Uart<'static, Async>, buf: &mut [u8]) -> Result<usize, FrameError> {
    sync_to_header(uart).await?;

    let mut len_bytes = [0u8; 2];
    uart.read_exact_async(&mut len_bytes).await?;
    let len = u16::from_le_bytes(len_bytes) as usize;
    if len == 0 || len > buf.len() {
        return Err(FrameError::TooLong(len));
    }

    uart.read_exact_async(&mut buf[..len]).await?;

    let mut footer = [0u8; 4];
    uart.read_exact_async(&mut footer).await?;
    if footer != REPORT_FOOTER {
        return Err(FrameError::BadFooter);
    }
    Ok(len)
}

/// Consumes bytes until the 4-byte report header has been matched.
async fn sync_to_header(uart: &mut Uart<'static, Async>) -> Result<(), FrameError> {
    let mut matched = 0usize;
    let mut b = [0u8; 1];
    while matched < REPORT_HEADER.len() {
        uart.read_exact_async(&mut b).await?;
        if b[0] == REPORT_HEADER[matched] {
            matched += 1;
        } else {
            // Restart the match; the mismatched byte may itself be a header start.
            matched = usize::from(b[0] == REPORT_HEADER[0]);
        }
    }
    Ok(())
}

/// Sensor task: reads report frames, debounces presence, and publishes
/// transitions on [`PRESENCE`]. Logs every change plus diagnostics.
#[embassy_executor::task]
pub async fn sensor_task(mut uart: Uart<'static, Async>) {
    info!("LD2410 sensor task started (8N1 @ 256000 baud)");
    let mut debouncer = PresenceDebouncer::new(HOLD_MS);
    let mut buf = [0u8; MAX_FRAME_DATA];
    // Publish the initial (vacant) state so consumers have a defined value.
    PRESENCE.signal(debouncer.is_present());

    let mut consecutive_errors: u32 = 0;
    loop {
        match read_frame(&mut uart, &mut buf).await {
            Ok(len) => {
                consecutive_errors = 0;
                let Some(report) = parse_report(&buf[..len]) else {
                    continue;
                };
                let now_ms = Instant::now().as_millis();
                if let Some(occupied) = debouncer.update(report.state.is_present(), now_ms) {
                    PRESENCE.signal(occupied);
                    info!(
                        "presence -> {} | state={:?} stationary={}cm/{} moving={}cm/{}",
                        if occupied { "OCCUPIED" } else { "vacant" },
                        report.state,
                        report.stationary_distance_cm,
                        report.stationary_energy,
                        report.moving_distance_cm,
                        report.moving_energy,
                    );
                }
            }
            Err(e) => {
                consecutive_errors += 1;
                // Don't spam: warn on the first error of a burst, then back off
                // briefly so a desync/disconnect doesn't tight-loop the CPU.
                if consecutive_errors == 1 {
                    warn!("sensor frame error: {e:?} (resyncing)");
                }
                Timer::after(Duration::from_millis(20)).await;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_basic_present_frame() {
        // 02 AA | state=02 (stationary) | mov 0,0 e0 | stat 150cm(0x96,0x00) e80 | det 200cm | 00 55
        let data = [
            0x02, 0xAA, 0x02, 0x00, 0x00, 0x00, 0x96, 0x00, 0x50, 0xC8, 0x00, 0x00, 0x55,
        ];
        let r = parse_report(&data).expect("should parse");
        assert_eq!(r.state, TargetState::Stationary);
        assert!(r.state.is_present());
        assert_eq!(r.stationary_distance_cm, 150);
        assert_eq!(r.stationary_energy, 80);
        assert_eq!(r.detection_distance_cm, Some(200));
    }

    #[test]
    fn parses_absent_frame_and_rejects_garbage() {
        let absent = [
            0x02, 0xAA, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x55,
        ];
        assert_eq!(parse_report(&absent).unwrap().state, TargetState::None);
        assert!(!parse_report(&absent).unwrap().state.is_present());

        assert!(parse_report(&[0x00, 0x11, 0x22]).is_none()); // wrong head/type
        assert!(parse_report(&[0x02, 0xAA]).is_none()); // too short
    }
}
