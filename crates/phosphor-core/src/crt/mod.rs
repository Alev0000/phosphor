//! The CRT stage: NTSCRT's seven RetroArch slang presets, run through librashader (Direct3D 11).

mod shader_params;
#[cfg(windows)]
mod compute;
#[cfg(windows)]
mod d3d11;

pub use shader_params::CRT_PRESETS;
#[cfg(windows)]
pub use d3d11::CrtRenderer;

/// When a shader parameter does anything (NTSCRT's ParamGates, verified there with crt-sweep).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Gate {
    AtLeast(&'static str, f32),
    Below(&'static str, f32),
    InputHeight(usize),
    /// Compile-time static in this shader build: never does anything.
    Never,
    /// Always active; the hint is informational.
    Info,
}

#[derive(Clone, Copy, Debug)]
pub struct CrtParam {
    pub name: &'static str,
    pub desc: &'static str,
    pub initial: f32,
    pub min: f32,
    pub max: f32,
    pub step: f32,
    pub gate: Option<(Gate, &'static str)>,
}

impl CrtParam {
    /// Whether the control is live given the preset's other values and the chain input height.
    pub fn active(&self, value_of: impl Fn(&str) -> Option<f32>, input_height: usize) -> bool {
        match self.gate {
            None | Some((Gate::Info, _)) => true,
            Some((Gate::AtLeast(n, v), _)) => value_of(n).unwrap_or(0.0) >= v,
            Some((Gate::Below(n, v), _)) => value_of(n).unwrap_or(0.0) < v,
            Some((Gate::InputHeight(h), _)) => input_height >= h,
            Some((Gate::Never, _)) => false,
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct CrtPreset {
    pub id: &'static str,
    pub name: &'static str,
    /// Relative to the slang-shaders root.
    pub path: &'static str,
    pub params: &'static [CrtParam],
}

impl CrtPreset {
    pub fn by_id(id: &str) -> Option<&'static CrtPreset> {
        CRT_PRESETS.iter().find(|p| p.id == id)
    }

    pub fn param(&self, name: &str) -> Option<&'static CrtParam> {
        self.params.iter().find(|p| p.name == name)
    }
}

/// NTSCRT's default preset.
pub const DEFAULT_PRESET: &str = "glow_gauss";
