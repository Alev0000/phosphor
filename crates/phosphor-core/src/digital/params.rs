//! Port of NTSCRT's `DigitalVideoParams.swift`.

use std::collections::BTreeMap;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DigitalGroup {
    Capture,
    Picture,
    Compression,
    Errors,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum DigitalKind {
    Slider {
        min: f64,
        max: f64,
        percent: bool,
        unit: &'static str,
    },
    Choice(&'static [&'static str]),
}

#[derive(Clone, Copy, Debug)]
pub struct DigitalParam {
    pub id: &'static str,
    pub label: &'static str,
    pub group: DigitalGroup,
    pub kind: DigitalKind,
    pub default: f64,
}

use DigitalGroup::*;

const fn s(
    id: &'static str,
    label: &'static str,
    group: DigitalGroup,
    min: f64,
    max: f64,
    percent: bool,
    unit: &'static str,
    default: f64,
) -> DigitalParam {
    DigitalParam {
        id,
        label,
        group,
        kind: DigitalKind::Slider {
            min,
            max,
            percent,
            unit,
        },
        default,
    }
}

pub const FIELDS_CHOICES: &[&str] = &["Progressive", "One field (line-doubled)"];
pub const CHROMA_CHOICES: &[&str] = &["4:4:4 (full)", "4:2:2 (half)", "4:2:0 (quarter)", "4:1:1 (DV)"];

pub const DIGITAL_PARAMS: &[DigitalParam] = &[
    s("frame_rate", "Frame rate", Capture, 1.0, 60.0, false, "fps", 15.0),
    DigitalParam {
        id: "fields",
        label: "Fields",
        group: Capture,
        kind: DigitalKind::Choice(FIELDS_CHOICES),
        default: 0.0,
    },
    s("lens_blur", "Lens softness", Capture, 0.0, 1.0, true, "", 0.0),
    s("aliasing", "Aliasing", Capture, 0.0, 1.0, true, "", 0.0),
    s("sensor_noise", "Sensor noise", Capture, 0.0, 1.0, true, "", 0.4),
    s("noise_reduction", "Noise reduction", Capture, 0.0, 1.0, true, "", 0.5),
    s("exposure", "Exposure", Picture, -2.0, 2.0, false, "stops", 0.0),
    s("white_balance", "White balance", Picture, -1.0, 1.0, true, "", 0.0),
    s("tint", "Tint", Picture, -1.0, 1.0, true, "", 0.0),
    s("saturation", "Saturation", Picture, 0.0, 2.0, true, "", 1.0),
    s("black_level", "Black level", Picture, 0.0, 0.25, true, "", 0.0),
    s("sharpening", "Sharpening", Picture, 0.0, 1.0, true, "", 0.0),
    DigitalParam {
        id: "chroma",
        label: "Color",
        group: Compression,
        kind: DigitalKind::Choice(CHROMA_CHOICES),
        default: 2.0,
    },
    s("quality", "Quality", Compression, 0.0, 1.0, true, "", 0.22),
    s("keyframe_interval", "Keyframe interval", Compression, 0.0, 10.0, false, "s", 0.0),
    s("fixed_bitrate", "Fixed bitrate", Compression, 0.0, 1.0, true, "", 0.0),
    s("deblocking", "Deblocking", Compression, 0.0, 1.0, true, "", 0.0),
    s("data_loss", "Data loss", Errors, 0.0, 1.0, true, "", 0.0),
    s("datamosh", "Datamosh", Errors, 0.0, 1.0, true, "", 0.0),
];

/// Controls that set the stream's structure; they hold for a whole render.
pub const STRUCTURE_IDS: &[&str] = &[
    "frame_rate",
    "keyframe_interval",
    "chroma",
    "fields",
    "lens_blur",
    "aliasing",
];

pub fn param(id: &str) -> Option<&'static DigitalParam> {
    DIGITAL_PARAMS.iter().find(|p| p.id == id)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ChromaSubsampling {
    Full,
    HalfWide,
    HalfBoth,
    QuarterWide,
}

impl ChromaSubsampling {
    pub fn factors(self) -> (usize, usize) {
        match self {
            Self::Full => (1, 1),
            Self::HalfWide => (2, 1),
            Self::HalfBoth => (2, 2),
            Self::QuarterWide => (4, 1),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct DigitalSettings {
    pub values: BTreeMap<&'static str, f64>,
}

impl Default for DigitalSettings {
    fn default() -> Self {
        Self {
            values: DIGITAL_PARAMS.iter().map(|p| (p.id, p.default)).collect(),
        }
    }
}

impl DigitalSettings {
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

    pub fn chroma(&self) -> ChromaSubsampling {
        match self.get("chroma").round() as i64 {
            0 => ChromaSubsampling::Full,
            1 => ChromaSubsampling::HalfWide,
            3 => ChromaSubsampling::QuarterWide,
            _ => ChromaSubsampling::HalfBoth,
        }
    }

    /// The same settings with the structure controls taken from `base`.
    pub fn holding_structure(&self, base: &DigitalSettings) -> DigitalSettings {
        let mut out = self.clone();
        for id in STRUCTURE_IDS {
            out.set(id, base.get(id));
        }
        out
    }

    pub fn hash_into(&self, h: &mut impl std::hash::Hasher) {
        for p in DIGITAL_PARAMS {
            h.write_u64(self.get(p.id).to_bits());
        }
    }
}

/// A named starting point: settings plus the capture width (which becomes the Downscale width).
#[derive(Clone, Copy, Debug)]
pub struct DigitalFormat {
    pub id: &'static str,
    pub name: &'static str,
    pub width: usize,
    pub values: &'static [(&'static str, f64)],
}

pub const DIGITAL_FORMATS: &[DigitalFormat] = &[
    DigitalFormat {
        id: "webcam",
        name: "Webcam (2005)",
        width: 320,
        values: &[
            ("frame_rate", 15.0),
            ("lens_blur", 0.35),
            ("aliasing", 0.15),
            ("sensor_noise", 0.4),
            ("noise_reduction", 0.5),
            ("exposure", 0.4),
            ("white_balance", -0.15),
            ("saturation", 0.85),
            ("black_level", 0.04),
            ("sharpening", 0.25),
            ("chroma", 2.0),
            ("quality", 0.22),
            ("keyframe_interval", 0.0),
            ("fixed_bitrate", 0.0),
            ("deblocking", 0.0),
            ("data_loss", 0.0),
            ("datamosh", 0.0),
        ],
    },
    DigitalFormat {
        id: "video_call",
        name: "Video call",
        width: 320,
        values: &[
            ("frame_rate", 15.0),
            ("lens_blur", 0.3),
            ("sensor_noise", 0.25),
            ("noise_reduction", 0.6),
            ("exposure", 0.2),
            ("saturation", 0.9),
            ("chroma", 2.0),
            ("quality", 0.22),
            ("keyframe_interval", 4.0),
            ("fixed_bitrate", 0.8),
            ("deblocking", 0.6),
            ("data_loss", 0.15),
            ("datamosh", 0.0),
        ],
    },
    DigitalFormat {
        id: "minidv",
        name: "MiniDV",
        width: 720,
        values: &[
            ("frame_rate", 30.0),
            ("fields", 1.0),
            ("lens_blur", 0.25),
            ("aliasing", 0.6),
            ("sensor_noise", 0.12),
            ("noise_reduction", 0.0),
            ("exposure", 0.3),
            ("white_balance", 0.4),
            ("tint", 0.2),
            ("saturation", 1.15),
            ("black_level", 0.08),
            ("sharpening", 0.6),
            ("chroma", 3.0),
            ("quality", 0.7),
            ("keyframe_interval", 0.0),
            ("fixed_bitrate", 0.3),
            ("deblocking", 0.0),
            ("data_loss", 0.0),
            ("datamosh", 0.0),
        ],
    },
    DigitalFormat {
        id: "early_web",
        name: "Early web video",
        width: 320,
        values: &[
            ("frame_rate", 15.0),
            ("sensor_noise", 0.0),
            ("noise_reduction", 0.0),
            ("chroma", 2.0),
            ("quality", 0.12),
            ("keyframe_interval", 5.0),
            ("fixed_bitrate", 0.7),
            ("deblocking", 0.0),
            ("data_loss", 0.0),
            ("datamosh", 0.0),
        ],
    },
    DigitalFormat {
        id: "datamosh",
        name: "Datamosh",
        width: 480,
        values: &[
            ("frame_rate", 30.0),
            ("sensor_noise", 0.0),
            ("noise_reduction", 0.0),
            ("chroma", 2.0),
            ("quality", 0.4),
            ("keyframe_interval", 1.0),
            ("fixed_bitrate", 0.3),
            ("deblocking", 0.0),
            ("data_loss", 0.0),
            ("datamosh", 1.0),
        ],
    },
];

impl DigitalFormat {
    pub fn settings(&self) -> DigitalSettings {
        let mut s = DigitalSettings::default();
        for (id, v) in self.values {
            s.set(id, *v);
        }
        s
    }
}
