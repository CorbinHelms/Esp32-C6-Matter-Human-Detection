//! Occupancy Sensing (Matter cluster `0x0406`) server, driven by the mmWave
//! radar's debounced presence state.
//!
//! rs-matter generates the cluster *scaffolding* (`decl::occupancy_sensing`:
//! the `Cluster` metadata, attribute/enum/bitmap types, and the strongly-typed
//! `ClusterHandler` trait) from the bundled Matter IDL — but ships no handler
//! for it. We implement that generated trait here (the three mandatory
//! read-only attributes) and add a background `run` task that pushes a
//! subscription update whenever presence flips, so Google Home reacts promptly.
//!
//! The pattern mirrors rs-matter's own `identify`/`desc` cluster handlers:
//! implement the typed `ClusterHandler`, delegate attribute encoding to the
//! generated `HandlerAdaptor`, and call `ctx.notify_attr_changed(..)` from
//! `run` (which both bumps the cluster data-version and notifies subscribers).

use rs_matter_embassy::matter::dm::clusters::decl::occupancy_sensing::{
    AttributeId, ClusterHandler as OccupancyClusterHandler, Feature, HandlerAdaptor,
    OccupancyBitmap, OccupancySensorTypeBitmap, OccupancySensorTypeEnum, FULL_CLUSTER,
};
use rs_matter_embassy::matter::dm::{
    Cluster, Dataver, DeviceType, EndptId, Handler, HandlerContext, MatchContext,
    NonBlockingHandler, ReadContext, ReadReply,
};
use rs_matter_embassy::matter::error::Error;
use rs_matter_embassy::matter::with;

use crate::sensor::PresenceState;

/// Matter Device Library "Occupancy Sensor" device type (`0x0107`). Not
/// predefined in rs-matter, so we declare it. Revision 4 matches the current
/// device-library revision (how rs-matter claims conformance for its own types).
pub const DEV_TYPE_OCCUPANCY_SENSOR: DeviceType = DeviceType {
    dtype: 0x0107,
    drev: 4,
};

/// Occupancy Sensing cluster server backed by a shared [`PresenceState`].
pub struct OccupancySensingHandler {
    dataver: Dataver,
    endpoint_id: EndptId,
    presence: &'static PresenceState,
}

impl OccupancySensingHandler {
    /// Creates the handler for `endpoint_id`, reading presence from `presence`.
    pub const fn new(
        dataver: Dataver,
        endpoint_id: EndptId,
        presence: &'static PresenceState,
    ) -> Self {
        Self {
            dataver,
            endpoint_id,
            presence,
        }
    }

    fn occupancy_bitmap(&self) -> OccupancyBitmap {
        if self.presence.occupied() {
            OccupancyBitmap::OCCUPIED
        } else {
            OccupancyBitmap::empty()
        }
    }
}

impl OccupancyClusterHandler for OccupancySensingHandler {
    // Keep only the mandatory attributes (Occupancy, OccupancySensorType,
    // OccupancySensorTypeBitmap) plus the global ones; drop the optional
    // PIR/ultrasonic delay/threshold attributes we don't implement. Advertise
    // the PIR feature so the FeatureMap, sensor type, and type bitmap below all
    // agree — radar has no legacy enum value, and reporting as a plain PIR
    // occupancy sensor is the most widely-compatible choice for controllers.
    const CLUSTER: Cluster<'static> = FULL_CLUSTER
        .with_attrs(with!(required))
        .with_features(Feature::PASSIVE_INFRARED.bits());

    fn dataver(&self) -> u32 {
        self.dataver.get()
    }

    fn dataver_changed(&self) {
        self.dataver.changed();
    }

    fn occupancy(&self, _ctx: impl ReadContext) -> Result<OccupancyBitmap, Error> {
        Ok(self.occupancy_bitmap())
    }

    fn occupancy_sensor_type(
        &self,
        _ctx: impl ReadContext,
    ) -> Result<OccupancySensorTypeEnum, Error> {
        // The legacy enum has no "radar" value; report PIR (matching the
        // FeatureMap) for maximum controller compatibility. Controllers key off
        // the Occupancy bit, so the sensor-type is informational.
        Ok(OccupancySensorTypeEnum::PIR)
    }

    fn occupancy_sensor_type_bitmap(
        &self,
        _ctx: impl ReadContext,
    ) -> Result<OccupancySensorTypeBitmap, Error> {
        Ok(OccupancySensorTypeBitmap::PIR)
    }
}

impl Handler for OccupancySensingHandler {
    fn read(&self, ctx: impl ReadContext, reply: impl ReadReply) -> Result<(), Error> {
        // Reuse the generated adaptor's attribute encoding (incl. the global
        // attributes). `&Self: ClusterHandler` via the generated blanket impl.
        HandlerAdaptor(self).read(ctx, reply)
    }

    fn bump_dataver(&self, ctx: impl MatchContext) {
        HandlerAdaptor(self).bump_dataver(ctx);
    }

    async fn run(&self, ctx: impl HandlerContext) -> Result<(), Error> {
        loop {
            self.presence.wait_changed().await;
            let occupied = self.presence.occupied();
            log::info!(
                "matter: occupancy attribute -> {}",
                if occupied { "OCCUPIED" } else { "vacant" }
            );
            // Bumps the cluster data-version (via the handler chain) and flags
            // subscribers, so Google Home gets the new occupancy value.
            ctx.notify_attr_changed(
                self.endpoint_id,
                <Self as OccupancyClusterHandler>::CLUSTER.id,
                AttributeId::Occupancy as _,
            );
        }
    }
}

impl NonBlockingHandler for OccupancySensingHandler {}
