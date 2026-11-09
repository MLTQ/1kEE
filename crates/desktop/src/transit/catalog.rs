use crate::model::GeoPoint;

pub(super) enum Feed {
    Gtfs {
        schedule: &'static str,
        vehicles: &'static str,
        lookup_stop_sequences: bool,
    },
    NewYork,
    London,
    Finland,
}

pub(crate) struct Agency {
    pub name: &'static str,
    pub credit: &'static str,
    pub website: &'static str,
    pub focus: GeoPoint,
    pub(super) feed: Feed,
}

pub(crate) const AGENCIES: &[Agency] = &[
    Agency {
        name: "Seattle / Tacoma",
        credit: "Sound Transit",
        website: "https://www.soundtransit.org/help-contacts/business-information/open-transit-data-otd/otd-downloads",
        focus: GeoPoint {
            lat: 47.610,
            lon: -122.333,
        },
        feed: Feed::Gtfs {
            schedule: "https://www.soundtransit.org/GTFS-rail/40_gtfs.zip",
            vehicles: "https://feeds.sound-transit.continuousgtfs.io/vehicle_positions.pb",
            lookup_stop_sequences: false,
        },
    },
    Agency {
        name: "Boston",
        credit: "Massachusetts Bay Transportation Authority (MBTA)",
        website: "https://www.mbta.com/developers/gtfs-realtime",
        focus: GeoPoint {
            lat: 42.355,
            lon: -71.065,
        },
        feed: Feed::Gtfs {
            schedule: "https://cdn.mbta.com/MBTA_GTFS.zip",
            vehicles: "https://cdn.mbta.com/realtime/VehiclePositions.pb",
            lookup_stop_sequences: false,
        },
    },
    Agency {
        name: "New York subway",
        credit: "MTA subway data via Transiter",
        website: "https://demo.transiter.dev/systems/us-ny-subway",
        focus: GeoPoint {
            lat: 40.751,
            lon: -73.987,
        },
        feed: Feed::NewYork,
    },
    Agency {
        name: "London Underground",
        credit: "Powered by TfL Open Data",
        website: "https://tfl.gov.uk/info-for/open-data-users/",
        focus: GeoPoint {
            lat: 51.509,
            lon: -0.125,
        },
        feed: Feed::London,
    },
    Agency {
        name: "Tokyo Toei",
        credit: "Bureau of Transportation, Tokyo Metropolitan Government / Association for Open Data of Public Transportation (CC BY 4.0)",
        website: "https://ckan.odpt.org/en/dataset/r_train_gtfs_rt-odpt_train-toei",
        focus: GeoPoint {
            lat: 35.686,
            lon: 139.757,
        },
        feed: Feed::Gtfs {
            schedule: "https://api-public.odpt.org/api/v4/files/Toei/data/Toei-Train-GTFS.zip",
            vehicles: "https://api-public.odpt.org/api/v4/gtfs/realtime/toei_odpt_train_vehicle",
            lookup_stop_sequences: true,
        },
    },
    Agency {
        name: "Finland",
        credit: "Fintraffic / Digitraffic (CC BY 4.0)",
        website: "https://www.digitraffic.fi/en/railway-traffic/",
        focus: GeoPoint {
            lat: 60.172,
            lon: 24.941,
        },
        feed: Feed::Finland,
    },
    Agency {
        name: "Malaysia KTM",
        credit: "KTMB / data.gov.my (CC BY 4.0)",
        website: "https://developer.data.gov.my/realtime-api/gtfs-realtime",
        focus: GeoPoint {
            lat: 3.134,
            lon: 101.687,
        },
        feed: Feed::Gtfs {
            schedule: "https://api.data.gov.my/gtfs-static/ktmb",
            vehicles: "https://api.data.gov.my/gtfs-realtime/vehicle-position/ktmb",
            lookup_stop_sequences: false,
        },
    },
];
