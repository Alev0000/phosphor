//! Port of NTSCRT's `GlitchParams.swift`.

use std::collections::BTreeMap;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GlitchGroup {
    Reception,
    Tape,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum GlitchKind {
    Slider {
        min: f64,
        max: f64,
        percent: bool,
        unit: &'static str,
    },
    Toggle,
}

#[derive(Clone, Copy, Debug)]
pub struct GlitchParam {
    pub id: &'static str,
    pub label: &'static str,
    pub group: GlitchGroup,
    pub kind: GlitchKind,
    pub default: f64,
}

const fn slider(
    id: &'static str,
    label: &'static str,
    group: GlitchGroup,
    min: f64,
    max: f64,
    percent: bool,
    unit: &'static str,
    default: f64,
) -> GlitchParam {
    GlitchParam {
        id,
        label,
        group,
        kind: GlitchKind::Slider {
            min,
            max,
            percent,
            unit,
        },
        default,
    }
}

const fn toggle(id: &'static str, label: &'static str, group: GlitchGroup, default: f64) -> GlitchParam {
    GlitchParam {
        id,
        label,
        group,
        kind: GlitchKind::Toggle,
        default,
    }
}

use GlitchGroup::{Reception, Tape};

/// Defaults describe a healthy set on a clean signal: enabling the stage changes nothing.
pub const GLITCH_PARAMS: &[GlitchParam] = &[
    slider("signal_strength", "Signal strength", Reception, 0.0, 1.0, true, "", 1.0),
    slider("vertical_hold", "Vertical hold", Reception, -1.0, 1.0, true, "", 0.0),
    slider("horizontal_hold", "Horizontal hold", Reception, -1.0, 1.0, true, "", 0.0),
    slider("afc_speed", "AFC response", Reception, 0.0, 1.0, true, "", 0.5),
    slider("brightness", "Brightness", Reception, 0.0, 1.0, true, "", 0.0),
    slider("ghost_level", "Ghost", Reception, -1.0, 1.0, true, "", 0.0),
    slider("ghost_delay", "Ghost delay", Reception, 0.2, 12.0, false, "µs", 2.0),
    slider("hum", "Hum", Reception, 0.0, 1.0, true, "", 0.0),
    toggle("closed_captions", "Caption data", Reception, 1.0),
    slider("head_switch", "Head switching", Tape, 0.0, 8.0, false, "µs", 0.0),
    slider("timebase_jitter", "Time-base jitter", Tape, 0.0, 3.0, false, "µs", 0.0),
    slider("crinkle", "Tape crinkle", Tape, 0.0, 1.0, true, "", 0.0),
    slider("head_clog", "Head clog", Tape, 0.0, 1.0, true, "", 0.0),
    slider("tracking", "Tracking error", Tape, 0.0, 1.0, true, "", 0.0),
    slider("search_speed", "Search", Tape, 1.0, 9.0, false, "×", 1.0),
    slider("dropouts", "Dropouts", Tape, 0.0, 1.0, true, "", 0.0),
    toggle("dropout_compensation", "Dropout compensation", Tape, 1.0),
];

pub fn param(id: &str) -> Option<&'static GlitchParam> {
    GLITCH_PARAMS.iter().find(|p| p.id == id)
}

/// Every knob's value at one moment, keyed by param id.
#[derive(Clone, Debug, PartialEq)]
pub struct GlitchSettings {
    pub values: BTreeMap<&'static str, f64>,
}

impl Default for GlitchSettings {
    fn default() -> Self {
        Self {
            values: GLITCH_PARAMS.iter().map(|p| (p.id, p.default)).collect(),
        }
    }
}

impl GlitchSettings {
    pub fn get(&self, id: &str) -> f64 {
        self.values
            .get(id)
            .copied()
            .or_else(|| param(id).map(|p| p.default))
            .unwrap_or(0.0)
    }

    pub fn set(&mut self, id: &str, value: f64) {
        if let Some(p) = param(id) {
            self.values.insert(p.id, value);
        }
    }

    pub fn flag(&self, id: &str) -> bool {
        self.get(id) >= 0.5
    }

    /// Knobs that change the receiver's evolving state; the rest only affect drawing.
    pub const TIMING_IDS: &'static [&'static str] = &[
        "signal_strength",
        "vertical_hold",
        "horizontal_hold",
        "afc_speed",
        "hum",
        "head_switch",
        "timebase_jitter",
        "crinkle",
        "head_clog",
        "search_speed",
        "tracking",
    ];

    pub fn timing_equal(&self, other: &Self) -> bool {
        Self::TIMING_IDS.iter().all(|id| self.get(id) == other.get(id))
    }
}
