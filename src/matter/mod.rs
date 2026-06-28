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

use core::pin::pin;

use esp_hal::peripherals::{ADC1, BT, IEEE802154, RNG};

use rs_matter_embassy::matter::crypto::{default_crypto, Crypto, RngCore};
use rs_matter_embassy::matter::dm::clusters::basic_info::BasicInfoConfig;
use rs_matter_embassy::matter::dm::clusters::desc::{self, ClusterHandler as _};
use rs_matter_embassy::matter::dm::devices::test::{
    DAC_PRIVKEY, TEST_DEV_ATT, TEST_DEV_COMM, TEST_DEV_DET,
};
use rs_matter_embassy::matter::dm::{
    Async, Dataver, EmptyHandler, Endpoint, EpClMatcher, Node,
};
use rs_matter_embassy::matter::persist::DummyKvBlobStore;
use rs_matter_embassy::matter::utils::init::InitMaybeUninit;
use rs_matter_embassy::matter::{clusters, devices, BasicCommData};
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

/// Brings up Matter-over-Thread and runs it forever. Consumes the radios and
/// the RNG/ADC1 used to seed the crypto CSPRNG.
pub async fn run(ieee802154: IEEE802154<'static>, bt: BT<'static>, rng: RNG<'static>, adc1: ADC1<'static>) -> ! {
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

    // Non-persistent KV store (fabric does not survive reboot yet — that's M3).
    let mut store = DummyKvBlobStore;
    stack.startup(&crypto, &mut store).await.unwrap();
    let kv = stack.matter().kv(store);

    let matter = pin!(stack.run(
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

    matter.await.unwrap();
    // `stack.run` only returns on a fatal error; surface it as a panic above.
    unreachable!("matter stack exited")
}
