//! Matter-over-Thread stack assembly for the occupancy sensor.
//!
//! Adapted from rs-matter-embassy's `light_thread` example: an
//! `EmbassyThreadMatterStack` using the ESP32-C6 802.15.4 radio (via
//! `EspThreadDriver`) for Thread operation and BLE for non-concurrent
//! commissioning. The On/Off "light" endpoint is replaced with an Occupancy
//! Sensor endpoint ([`occupancy`]) driven by the mmWave [`PresenceState`].
//!
//! Commissioning uses bundled **test** attestation (VID `0xFFF1`, PID `0x8001`)
//! — register that VID/PID in the Google Home Developer Console to pair. The
//! stack prints the QR/manual pairing code over the log at startup.

pub mod occupancy;

use core::borrow::BorrowMut;
use core::pin::pin;

use embassy_embedded_hal::adapter::BlockingAsync;
use embassy_futures::select::{select, Either};
use esp_bootloader_esp_idf::partitions::{
    read_partition_table, DataPartitionSubType, PartitionType, PARTITION_TABLE_MAX_LEN,
};
use esp_hal::gpio::{Input, InputConfig, Pull};
use esp_hal::peripherals::{ADC1, BT, FLASH, GPIO9, IEEE802154, RNG};
use esp_storage::FlashStorage;
use log::{info, warn};

use rs_matter_embassy::matter::crypto::{default_crypto, Crypto, RngCore};
use rs_matter_embassy::matter::dm::clusters::basic_info::BasicInfoConfig;
use rs_matter_embassy::matter::dm::clusters::desc::{self, ClusterHandler as _};
use rs_matter_embassy::matter::dm::devices::test::{
    DAC_PRIVKEY, TEST_DEV_ATT, TEST_DEV_COMM, TEST_DEV_DET,
};
use rs_matter_embassy::matter::dm::{
    Async, Dataver, EmptyHandler, Endpoint, EpClMatcher, Node,
};
use rs_matter_embassy::matter::error::Error;
use rs_matter_embassy::matter::persist::KvBlobStore;
use rs_matter_embassy::matter::utils::init::InitMaybeUninit;
use rs_matter_embassy::matter::utils::select::Coalesce;
use rs_matter_embassy::matter::{clusters, devices, BasicCommData};
use rs_matter_embassy::persist::SeqMapKvBlobStore;
use rs_matter_embassy::stack::rand::reseeding_csprng;
use rs_matter_embassy::wireless::esp::EspThreadDriver;
use rs_matter_embassy::wireless::{EmbassyThread, EmbassyThreadMatterStack};

use occupancy::{OccupancySensingHandler, DEV_TYPE_OCCUPANCY_SENSOR};

use crate::sensor::PRESENCE;

/// Bump-allocator budget for the rs-matter-stack `run` futures. Increase if the
/// stack panics during initialization for lack of memory.
const BUMP_SIZE: usize = 25000;

/// Endpoint 0 hosts the hidden system clusters, so the sensor lives on EP 1.
const OCC_ENDPOINT_ID: u16 = 1;

/// Seconds the BOOT pin (GPIO9) must be held low to factory-reset the fabric.
const RESET_SECS: u64 = 3;

/// Allocates a `'static`, zeroed `$t` from a `StaticCell` (avoids blowing the
/// program stack with the ~35-50KB Matter stack).
macro_rules! mk_static {
    ($t:ty) => {{
        static STATIC_CELL: static_cell::StaticCell<$t> = static_cell::StaticCell::new();
        STATIC_CELL.uninit()
    }};
}

/// Device identity. Test VID/PID; override the names so the device is
/// recognizable in the commissioning app.
const BASIC_INFO: BasicInfoConfig = BasicInfoConfig {
    sai: Some(500),
    product_name: "mmWave Occupancy",
    vendor_name: "DIY Matter",
    ..TEST_DEV_DET
};

/// The Matter node: root endpoint + our Occupancy Sensor endpoint.
const NODE: Node = Node {
    endpoints: &[
        EmbassyThreadMatterStack::<0, ()>::root_endpoint(),
        Endpoint::new(
            OCC_ENDPOINT_ID,
            devices!(DEV_TYPE_OCCUPANCY_SENSOR),
            clusters!(
                desc::DescHandler::CLUSTER,
                <OccupancySensingHandler as rs_matter_embassy::matter::dm::clusters::decl::occupancy_sensing::ClusterHandler>::CLUSTER
            ),
        ),
    ],
};

/// Brings up Matter-over-Thread and runs it forever. Consumes the radios, the
/// RNG/ADC1 used to seed the crypto CSPRNG, the flash (for fabric persistence),
/// and the BOOT pin (GPIO9, used for factory reset).
pub async fn run(
    ieee802154: IEEE802154<'static>,
    bt: BT<'static>,
    rng: RNG<'static>,
    adc1: ADC1<'static>,
    flash: FLASH<'static>,
    boot_pin: GPIO9<'static>,
) -> ! {
    // Seed a reseeding CSPRNG from the hardware TRNG; the source guard must
    // outlive every use of the RNG, so keep it bound for the whole function.
    let _trng_source = esp_hal::rng::TrngSource::new(rng, adc1);
    let crypto = default_crypto(
        reseeding_csprng(esp_hal::rng::Trng::try_new().unwrap(), 1000).unwrap(),
        DAC_PRIVKEY,
    );
    let mut weak_rand = crypto.weak_rand().unwrap();

    // A random EUI-64 for the Thread interface.
    let mut ieee_eui64 = [0u8; 8];
    weak_rand.fill_bytes(&mut ieee_eui64);

    // Allocate the (large) Matter stack statically.
    let stack = mk_static!(EmbassyThreadMatterStack::<BUMP_SIZE, ()>).init_with(
        EmbassyThreadMatterStack::init(
            &BASIC_INFO,
            BasicCommData {
                password: TEST_DEV_COMM.password,
                discriminator: TEST_DEV_COMM.discriminator,
            },
            &TEST_DEV_ATT,
        ),
    );

    // Occupancy Sensor cluster handler, driven by the shared presence state.
    let occupancy =
        OccupancySensingHandler::new(Dataver::new_rand(&mut weak_rand), OCC_ENDPOINT_ID, &PRESENCE);

    // Chain the endpoint's cluster handlers: Occupancy + the per-endpoint
    // Descriptor cluster rs-matter provides out of the box.
    let handler = EmptyHandler
        .chain(
            EpClMatcher::new(
                Some(OCC_ENDPOINT_ID),
                Some(<OccupancySensingHandler as rs_matter_embassy::matter::dm::clusters::decl::occupancy_sensing::ClusterHandler>::CLUSTER.id),
            ),
            Async(occupancy),
        )
        .chain(
            EpClMatcher::new(Some(OCC_ENDPOINT_ID), Some(desc::DescHandler::CLUSTER.id)),
            Async(desc::DescHandler::new(Dataver::new_rand(&mut weak_rand)).adapt()),
        );

    // Flash-backed KV store: fabric/ACL state persists across reboots so the
    // device does not need re-commissioning. Requires an NVS partition (see
    // partitions.csv).
    let mut pt_buf = [0u8; PARTITION_TABLE_MAX_LEN];
    let mut store = persistent_store(flash, &mut pt_buf[..]);
    stack.startup(&crypto, &mut store).await.unwrap();
    let kv = stack.matter().kv(store);

    if stack.is_commissioned() {
        info!(
            "Already commissioned. To factory-reset, hold BOOT (GPIO9) low for {RESET_SECS}+ seconds."
        );
    }

    {
        // Run Matter; concurrently watch the BOOT pin for a factory-reset request.
        let mut matter = pin!(stack.run(
            EmbassyThread::new(
                EspThreadDriver::new(ieee802154, bt),
                crypto.rand().unwrap(),
                ieee_eui64,
                &kv,
                stack,
                true, // randomize the BLE address
            ),
            &crypto,
            (NODE, handler),
            &kv,
            (),
        ));
        let mut wait_reset = pin!(wait_factory_reset(Input::new(
            boot_pin,
            InputConfig::default().with_pull(Pull::Down)
        )));
        select(&mut matter, &mut wait_reset).coalesce().await.unwrap();
    }

    // Reached only when the user requested a factory reset.
    warn!("Factory reset: clearing Matter fabric storage");
    stack.matter().reset_persist(kv).await.unwrap();
    warn!("Rebooting...");
    esp_hal::system::software_reset()
}

/// Returns a flash-backed [`KvBlobStore`] persisting to the first NVS partition
/// in the chip's partition table. Panics if no NVS partition exists — provide
/// one via `partitions.csv`.
fn persistent_store<'d>(
    flash: FLASH<'d>,
    mut buf: impl BorrowMut<[u8]>,
) -> impl KvBlobStore + 'd {
    let mut flash = FlashStorage::new(flash);
    let pt_buf = &mut buf.borrow_mut()[..PARTITION_TABLE_MAX_LEN];
    let pt = read_partition_table(&mut flash, pt_buf).unwrap();
    let nvs = pt
        .find_partition(PartitionType::Data(DataPartitionSubType::Nvs))
        .unwrap()
        .expect("no NVS partition found — see partitions.csv");

    let range = nvs.offset()..nvs.offset() + nvs.len();
    info!(
        "Persisting Matter state to NVS partition \"{}\" at {:#x}..{:#x}",
        nvs.label_as_str(),
        range.start,
        range.end
    );
    SeqMapKvBlobStore::new(BlockingAsync::new(flash), range)
}

/// Resolves once the BOOT pin (GPIO9) has been held low for [`RESET_SECS`].
async fn wait_factory_reset(mut pin: Input<'_>) -> Result<(), Error> {
    loop {
        pin.wait_for_low().await;
        embassy_time::Timer::after_millis(50).await; // debounce
        if pin.is_low() {
            warn!("BOOT held low — keep holding {RESET_SECS}s to factory-reset");
            if matches!(
                select(
                    pin.wait_for_high(),
                    embassy_time::Timer::after_secs(RESET_SECS),
                )
                .await,
                Either::Second(())
            ) {
                return Ok(());
            }
        }
    }
}
