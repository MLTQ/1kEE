//! Classify before coordinate I/O so unselected ways have no lookup cost.
use crate::args::PlanetAllCommand;
use crate::util::{
    canonical_aeroway_class, canonical_building_class, canonical_comm_class, canonical_govt_class,
    canonical_industrial_class, canonical_military_class, canonical_pipeline_class,
    canonical_port_class, canonical_power_class, canonical_railway_class, canonical_road_class,
    canonical_surv_class, canonical_tree_class, canonical_waterway_class, parse_voltage_kv,
};

pub(super) struct WaySelection {
    pub road_class: Option<&'static str>,
    pub waterway_class: Option<&'static str>,
    pub building_class: Option<&'static str>,
    pub tree_class: Option<&'static str>,
    pub military_class: Option<&'static str>,
    pub industrial_class: Option<&'static str>,
    pub port_class: Option<&'static str>,
    pub govt_class: Option<&'static str>,
    pub surv_class: Option<&'static str>,
    pub power_class: Option<&'static str>,
    pub raw_railway: Option<&'static str>,
    pub pipeline_class: Option<&'static str>,
    pub aeroway_class: Option<&'static str>,
    pub comm_class: Option<&'static str>,
    pub name: Option<String>,
}

pub(super) fn has_way_layers(cmd: &PlanetAllCommand) -> bool {
    [
        cmd.build_roads,
        cmd.build_waterways,
        cmd.build_buildings,
        cmd.build_trees,
        cmd.build_power,
        cmd.build_rail,
        cmd.build_pipeline,
        cmd.build_aeroway,
        cmd.build_military,
        cmd.build_comm,
        cmd.build_industrial,
        cmd.build_port,
        cmd.build_government,
        cmd.build_surveillance,
    ]
    .into_iter()
    .any(|enabled| enabled)
}

pub(super) fn classify(way: &osmpbf::Way<'_>, cmd: &PlanetAllCommand) -> Option<WaySelection> {
    let mut road_class: Option<&'static str> = None;
    let mut waterway_class: Option<&'static str> = None;
    let mut building_class: Option<&'static str> = None;
    let mut tree_class: Option<&'static str> = None;
    let mut military_class: Option<&'static str> = None;
    let mut industrial_class: Option<&'static str> = None;
    let mut port_class: Option<&'static str> = None;
    let mut govt_class: Option<&'static str> = None;
    let mut surv_class: Option<&'static str> = None;
    let mut name: Option<String> = None;

    let mut raw_power: Option<String> = None;
    let mut raw_voltage: Option<String> = None;
    let mut raw_railway: Option<&'static str> = None;
    let mut raw_pipeline = false;
    let mut raw_substance: Option<String> = None;
    let mut raw_aeroway: Option<String> = None;
    let mut raw_aerodrome_intl = false;
    let mut raw_man_made: Option<String> = None;
    let mut raw_tower_type: Option<String> = None;

    for (key, value) in way.tags() {
        match key {
            "highway" => road_class = canonical_road_class(value),
            "waterway" => waterway_class = canonical_waterway_class(value),
            "building" => building_class = canonical_building_class(value),
            "natural" | "landuse" => {
                if tree_class.is_none() {
                    tree_class = canonical_tree_class(key, value);
                }
                if industrial_class.is_none() {
                    industrial_class = canonical_industrial_class(key, value);
                }
            }
            "military" => military_class = canonical_military_class(key, value),
            "power" => raw_power = Some(value.to_owned()),
            "voltage" => raw_voltage = Some(value.to_owned()),
            "railway" => raw_railway = canonical_railway_class(value),
            "man_made" => {
                if value == "pipeline" {
                    raw_pipeline = true;
                } else {
                    if industrial_class.is_none() {
                        industrial_class = canonical_industrial_class(key, value);
                    }
                    raw_man_made = Some(value.to_owned());
                    if port_class.is_none() {
                        port_class = canonical_port_class(key, value);
                    }
                    if surv_class.is_none() {
                        surv_class = canonical_surv_class(key, value);
                    }
                }
            }
            "substance" => raw_substance = Some(value.to_owned()),
            "aeroway" => raw_aeroway = Some(value.to_owned()),
            "aerodrome:type" => {
                if value == "international" {
                    raw_aerodrome_intl = true;
                }
            }
            "tower:type" => raw_tower_type = Some(value.to_owned()),
            "amenity" => {
                if govt_class.is_none() {
                    govt_class = canonical_govt_class(key, value);
                }
                if port_class.is_none() {
                    port_class = canonical_port_class(key, value);
                }
            }
            "office" | "government" => {
                if govt_class.is_none() {
                    govt_class = canonical_govt_class(key, value);
                }
            }
            "harbour" | "leisure" => {
                if port_class.is_none() {
                    port_class = canonical_port_class(key, value);
                }
            }
            "surveillance" => {
                if surv_class.is_none() {
                    surv_class = canonical_surv_class(key, value);
                }
            }
            "name" if name.is_none() => name = Some(value.to_owned()),
            _ => {}
        }
    }

    let voltage_kv = raw_voltage.as_deref().and_then(parse_voltage_kv);
    let power_class = raw_power
        .as_deref()
        .and_then(|pt| canonical_power_class(pt, voltage_kv));
    let pipeline_class: Option<&'static str> = if raw_pipeline {
        Some(canonical_pipeline_class(
            raw_substance.as_deref().unwrap_or(""),
        ))
    } else {
        None
    };
    let aeroway_class = raw_aeroway
        .as_deref()
        .and_then(|a| canonical_aeroway_class(a, raw_aerodrome_intl));
    let comm_class = raw_man_made
        .as_deref()
        .and_then(|m| canonical_comm_class(m, raw_tower_type.as_deref()));

    let any_match = (cmd.build_roads && road_class.is_some())
        || (cmd.build_waterways && waterway_class.is_some())
        || (cmd.build_buildings && building_class.is_some())
        || (cmd.build_trees && tree_class.is_some())
        || (cmd.build_power && power_class.is_some())
        || (cmd.build_rail && raw_railway.is_some())
        || (cmd.build_pipeline && pipeline_class.is_some())
        || (cmd.build_aeroway && aeroway_class.is_some())
        || (cmd.build_military && military_class.is_some())
        || (cmd.build_comm && comm_class.is_some())
        || (cmd.build_industrial && industrial_class.is_some())
        || (cmd.build_port && port_class.is_some())
        || (cmd.build_government && govt_class.is_some())
        || (cmd.build_surveillance && surv_class.is_some());
    if !any_match {
        return None;
    }

    Some(WaySelection {
        road_class,
        waterway_class,
        building_class,
        tree_class,
        military_class,
        industrial_class,
        port_class,
        govt_class,
        surv_class,
        power_class,
        raw_railway,
        pipeline_class,
        aeroway_class,
        comm_class,
        name,
    })
}
