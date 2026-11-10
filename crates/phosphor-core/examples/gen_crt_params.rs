//! Generates `src/crt/shader_params.rs` from slang-shaders and copies the files the seven NTSCRT CRT
//! presets need into a bundle directory.
//!
//! cargo run -p phosphor-core --example gen_crt_params -- <slang-shaders root> <bundle out dir>

use std::collections::{BTreeSet, HashSet};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use librashader::presets::ShaderPreset;
use librashader_common::shader_features::ShaderFeatures;

const PRESETS: &[(&str, &str, &str)] = &[
    ("aperture", "CRT Aperture", "crt/crt-aperture.slangp"),
    ("easymode", "CRT Easymode", "crt/crt-easymode.slangp"),
    ("glow_gauss", "CRT Glow (Gaussian)", "crt/crtglow_gauss.slangp"),
    ("glow_lanczos", "CRT Glow (Lanczos)", "crt/crtglow_lanczos.slangp"),
    ("hyllian", "CRT Hyllian", "crt/crt-hyllian.slangp"),
    ("royale", "CRT Royale", "crt/crt-royale.slangp"),
    ("sim", "CRT Sim", "crt/crtsim.slangp"),
];

/// NTSCRT's house tweaks (AppState.appShaderDefaults).
const HOUSE: &[(&str, &str, f32)] = &[
    ("glow_gauss", "BOOST", 1.1),
    ("glow_gauss", "GLOW_ROLLOFF", 2.4),
    ("glow_gauss", "BLOOM_STRENGTH", 0.1),
    ("glow_lanczos", "BOOST", 1.1),
    ("glow_lanczos", "GLOW_ROLLOFF", 2.4),
    ("glow_lanczos", "BLOOM_STRENGTH", 0.1),
];

struct Param {
    name: String,
    desc: String,
    initial: f32,
    min: f32,
    max: f32,
    step: f32,
}

fn parse_pragma(line: &str) -> Option<Param> {
    let rest = line.trim().strip_prefix("#pragma parameter")?.trim();
    let (name, rest) = rest.split_once(char::is_whitespace)?;
    let rest = rest.trim();
    let rest = rest.strip_prefix('"')?;
    let (desc, rest) = rest.split_once('"')?;
    let nums: Vec<f32> = rest
        .split_whitespace()
        .map_while(|t| t.trim_end_matches('f').parse::<f32>().ok())
        .collect();
    if nums.len() < 3 {
        return None;
    }
    Some(Param {
        name: name.to_string(),
        desc: desc.trim().to_string(),
        initial: nums[0],
        min: nums[1],
        max: nums[2],
        step: nums.get(3).copied().unwrap_or(0.0),
    })
}

fn scan(path: &Path, out: &mut Vec<Param>, seen: &mut HashSet<String>, files: &mut BTreeSet<PathBuf>) {
    let path = std::fs::canonicalize(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    if files.contains(&path) {
        return;
    }
    files.insert(path.clone());
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    for line in text.lines() {
        let t = line.trim();
        if let Some(inc) = t.strip_prefix("#include") {
            let inc = inc.trim().trim_matches('"');
            scan(&path.parent().unwrap().join(inc), out, seen, files);
        } else if let Some(p) = parse_pragma(t) {
            if seen.insert(p.name.clone()) {
                out.push(p);
            }
        }
    }
}

fn gate(preset: &str, name: &str, desc: &str) -> Option<(&'static str, &'static str)> {
    // (condition, hint). Condition syntax: "ge:NAME:V", "lt:NAME:V", "height:N", "never", "info".
    let glow = match name {
        "warpX" | "warpY" | "cornersize" | "cornersmooth" | "noise_amt" => Some(("ge:CURVATURE:0.5", "Requires Curvature")),
        "maskDark" | "maskLight" => Some(("ge:shadowMask:0.5", "Requires Mask Effect > 0")),
        _ => None,
    };
    match preset {
        "glow_gauss" | "glow_lanczos" => glow,
        "hyllian" => match name {
            "GLOW_WHITEPOINT" | "GLOW_ROLLOFF" | "GLOW_RADIUS" | "GLOW_STRENGTH" => Some(("ge:GLOW_ENABLE:0.5", "Requires Enable Glow")),
            "h_shape" | "h_radius" | "h_cornersize" | "h_cornersmooth" => Some(("ge:h_curvature:0.5", "Requires Curvature")),
            "DISPLAY_RES" => Some(("ge:PRESET_OPTION:0.5", "Only used by non-CUSTOM mask presets")),
            "MASK_STRENGTH" => Some(("lt:PRESET_OPTION:0.5", "Overridden unless Mask Preset is CUSTOM")),
            "MONITOR_SUBPIXELS" => Some(("info", "Only affects RGB-triad mask layouts")),
            _ if desc.trim_start().starts_with('*') && !desc.trim_start().starts_with("**") => {
                Some(("lt:PRESET_OPTION:0.5", "Overridden unless Mask Preset is CUSTOM"))
            }
            _ => None,
        },
        "royale" => match name {
            "geom_tilt_angle_x" | "geom_tilt_angle_y" | "geom_view_dist" | "geom_radius" | "aa_cubic_c" => {
                Some(("ge:geom_mode_runtime:0.5", "Requires Geometry Mode != flat"))
            }
            "mask_num_triads_desired" => Some(("ge:mask_specify_num_triads:0.5", "Requires Specify Number of Triads")),
            "interlace_detect_toggle" | "interlace_bff" => Some(("height:480", "Needs a 480-line input")),
            "interlace_1080i" => Some(("height:1080", "Needs a 1080-line input")),
            "aa_subpixel_r_offset_x_runtime" | "aa_subpixel_r_offset_y_runtime" | "aa_gauss_sigma" | "beam_horiz_sigma" => {
                Some(("never", "Static in this shader build"))
            }
            _ => None,
        },
        "easymode" if name == "MASK_DOT_HEIGHT" => Some(("ge:MASK_STAGGER:1", "Requires Mask Stagger > 0")),
        "aperture" if name == "SCANLINE_OFFSET" => Some(("info", "Only shifts at even output/input scale factors")),
        "sim" if name == "animate_artifacts" => Some(("info", "Visible with Animate on")),
        _ => None,
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let root = PathBuf::from(&args[1]);
    let bundle = PathBuf::from(&args[2]);
    let root = std::fs::canonicalize(&root).unwrap();

    let mut code = String::new();
    writeln!(code, "// @generated by examples/gen_crt_params.rs from libretro/slang-shaders. Do not edit.").unwrap();
    writeln!(code, "use super::{{CrtParam, CrtPreset, Gate}};\n").unwrap();
    let mut all_files = BTreeSet::new();

    for (id, _, rel) in PRESETS {
        let preset_path = root.join(rel);
        let preset = ShaderPreset::try_parse(&preset_path, ShaderFeatures::NONE)
            .unwrap_or_else(|e| panic!("{rel}: {e}"));
        all_files.insert(std::fs::canonicalize(&preset_path).unwrap());

        let mut params = Vec::new();
        let mut seen = HashSet::new();
        let mut preset_files = BTreeSet::new();
        for pass in &preset.passes {
            scan(&pass.path, &mut params, &mut seen, &mut preset_files);
        }
        all_files.extend(preset_files);
        for tex in &preset.textures {
            all_files.insert(std::fs::canonicalize(&tex.path).unwrap());
        }
        for p in &mut params {
            if let Some(o) = preset.parameters.iter().find(|o| o.name.as_str() == p.name) {
                p.initial = o.value;
            }
            if let Some((_, _, v)) = HOUSE.iter().find(|(pid, n, _)| pid == id && *n == p.name) {
                p.initial = *v;
            }
        }

        let ident = id.to_uppercase();
        writeln!(code, "const {ident}: &[CrtParam] = &[").unwrap();
        for p in &params {
            let g = match gate(id, &p.name, &p.desc) {
                None => "None".to_string(),
                Some((cond, hint)) => {
                    let cond = match cond.split(':').collect::<Vec<_>>().as_slice() {
                        ["ge", n, v] => format!("Gate::AtLeast({n:?}, {:?})", v.parse::<f32>().unwrap()),
                        ["lt", n, v] => format!("Gate::Below({n:?}, {:?})", v.parse::<f32>().unwrap()),
                        ["height", h] => format!("Gate::InputHeight({h})"),
                        ["never"] => "Gate::Never".to_string(),
                        ["info"] => "Gate::Info".to_string(),
                        other => panic!("bad gate {other:?}"),
                    };
                    format!("Some(({cond}, {hint:?}))")
                }
            };
            writeln!(
                code,
                "    CrtParam {{ name: {:?}, desc: {:?}, initial: {:?}, min: {:?}, max: {:?}, step: {:?}, gate: {g} }},",
                p.name, p.desc, p.initial, p.min, p.max, p.step
            )
            .unwrap();
        }
        writeln!(code, "];\n").unwrap();
        eprintln!("{id}: {} params, {} passes", params.len(), preset.passes.len());
    }

    writeln!(code, "pub const CRT_PRESETS: &[CrtPreset] = &[").unwrap();
    for (id, display, rel) in PRESETS {
        writeln!(
            code,
            "    CrtPreset {{ id: {id:?}, name: {display:?}, path: {rel:?}, params: {} }},",
            id.to_uppercase()
        )
        .unwrap();
    }
    writeln!(code, "];").unwrap();

    let out_rs = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/crt/shader_params.rs");
    std::fs::write(&out_rs, code).unwrap();

    // Copy every file the presets touch, keeping paths relative to the slang-shaders root.
    let mut copied = 0;
    for f in &all_files {
        let rel = f.strip_prefix(&root).unwrap_or_else(|_| panic!("{} outside root", f.display()));
        let dst = bundle.join(rel);
        std::fs::create_dir_all(dst.parent().unwrap()).unwrap();
        std::fs::copy(f, &dst).unwrap();
        copied += 1;
    }
    eprintln!("wrote {} and copied {copied} shader files to {}", out_rs.display(), bundle.display());
}
