//! NTSCRT look presets (the app's `.json` files) and the app's house defaults.

use ntsc_rs::settings::standard::NtscEffectFullSettings;
use ntsc_rs::settings::SettingsList;
use serde_json::{Map, Value};

use crate::digital::DigitalSettings;
use crate::downscale::DownscaleMethod;
use crate::glitch::GlitchSettings;
use crate::howlaround::HowlSettings;

/// NTSCRT's house VHS look overlaid on the ntsc-rs defaults (AppState.appNtscDefaults).
pub const HOUSE_NTSC: &[(&str, f64)] = &[
    ("filter_type", 0.0),
    ("composite_preemphasis", 0.2753769054878048),
    ("composite_noise_intensity", 0.1391983173076923),
    ("composite_noise_frequency", 0.8576),
    ("composite_noise_detail", 2.0),
    ("snow_intensity", 0.0),
    ("video_scanline_phase_shift_offset", 3.0),
    ("luma_smear", 0.6692),
    ("head_switching_height", 8.0),
    ("head_switching_offset", 3.0),
    ("head_switching_horizontal_shift", 41.57),
    ("head_switching_mid_line_jitter", 0.181),
    ("tracking_noise_height", 63.0),
    ("ringing_power", 5.674),
    ("ringing_scale", 4.935),
    ("luma_noise_intensity", 0.153),
    ("chroma_noise_intensity", 0.13054193376068376),
    ("chroma_noise_frequency", 0.0777),
    ("chroma_phase_error", 0.016),
    ("chroma_phase_noise_intensity", 0.029),
    ("chroma_delay_horizontal", 1.7279639582876793),
    ("chroma_delay_vertical", 2.0),
    ("vhs_chroma_loss", 0.003639155982905983),
    ("vhs_edge_wave", 0.34226190476190477),
    ("vhs_tape_speed", 3.0),
    ("bandwidth_scale", 0.3),
    ("vertical_scale", 0.3),
];
/// Booleans in the house look.
pub const HOUSE_NTSC_FLAGS: &[(&str, bool)] = &[("scale_with_video_size", true)];

/// NTSCRT's Downscale default: VGA, Chunky.
pub const DEFAULT_DOWNSCALE_WIDTH: usize = 320;

pub type Error = Box<dyn std::error::Error + Send + Sync>;

fn ntsc_list() -> SettingsList<NtscEffectFullSettings> {
    SettingsList::<NtscEffectFullSettings>::new()
}

fn ntsc_to_map(s: &NtscEffectFullSettings) -> Map<String, Value> {
    let json = ntsc_list().to_json_string(s).expect("ntsc settings serialize");
    match serde_json::from_str::<Value>(&json) {
        Ok(Value::Object(m)) => m,
        _ => Map::new(),
    }
}

fn ntsc_from_map(m: &Map<String, Value>) -> Result<NtscEffectFullSettings, Error> {
    let mut m = m.clone();
    m.entry("version").or_insert(Value::from(1));
    Ok(ntsc_list().from_json(&Value::Object(m).to_string())?)
}

/// The ntsc-rs settings NTSCRT starts from.
pub fn house_ntsc() -> NtscEffectFullSettings {
    let mut m = ntsc_to_map(&NtscEffectFullSettings::default());
    for (k, v) in HOUSE_NTSC {
        let value = if v.fract() == 0.0 && m.get(*k).is_some_and(|e| e.is_i64() || e.is_u64()) {
            Value::from(*v as i64)
        } else {
            Value::from(*v)
        };
        m.insert((*k).to_string(), value);
    }
    for (k, v) in HOUSE_NTSC_FLAGS {
        m.insert((*k).to_string(), Value::from(*v));
    }
    ntsc_from_map(&m).expect("house ntsc settings parse")
}


/// How a timeline segment eases, carried by the keyframe it leaves (CSS semantics).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Easing {
    Linear,
    EaseIn,
    EaseOut,
    EaseInOut,
    Hold,
}

impl Easing {
    fn parse(s: &str) -> Self {
        match s {
            "Ease in" => Easing::EaseIn,
            "Ease out" => Easing::EaseOut,
            "Ease in-out" => Easing::EaseInOut,
            "Hold" => Easing::Hold,
            _ => Easing::Linear,
        }
    }

    /// Shape a segment's progress 0…1.
    pub fn apply(self, u: f64) -> f64 {
        match self {
            Easing::Linear => u,
            Easing::EaseIn => u * u,
            Easing::EaseOut => 1.0 - (1.0 - u) * (1.0 - u),
            Easing::EaseInOut => {
                if u < 0.5 { 2.0 * u * u } else { 1.0 - (-2.0 * u + 2.0).powi(2) / 2.0 }
            }
            Easing::Hold => 0.0,
        }
    }
}

/// One timeline keyframe: every stage's settings at that moment (a key's values override the
/// look's base, as in NTSCRT).
#[derive(Clone, Debug)]
pub struct LookKey {
    /// Seconds from the start.
    pub time: f64,
    pub easing: Easing,
    pub glitch: GlitchSettings,
    pub digital: DigitalSettings,
    pub ntsc: NtscEffectFullSettings,
    pub shader: Vec<(String, f32)>,
}

#[derive(Clone, Debug)]
pub struct Look {
    pub digital: Option<(bool, DigitalSettings)>,
    pub ntsc: Option<(bool, NtscEffectFullSettings)>,
    pub downscale: Option<(bool, usize, DownscaleMethod)>,
    pub glitch: Option<(bool, GlitchSettings)>,
    pub shader: Option<(bool, String, Vec<(String, f32)>)>,
    pub keys: Vec<LookKey>,
    pub timeline_fps: f64,
    pub duration: f64,
}

fn values(v: &Value) -> Vec<(String, f64)> {
    v.as_object()
        .map(|o| {
            o.iter()
                .filter_map(|(k, v)| v.as_f64().or_else(|| v.as_bool().map(|b| b as u8 as f64)).map(|f| (k.clone(), f)))
                .collect()
        })
        .unwrap_or_default()
}

fn enabled(v: &Value) -> bool {
    v.get("enabled").and_then(Value::as_bool).unwrap_or(true)
}

impl Look {
    pub fn parse(json: &str) -> Result<Look, Error> {
        let root: Value = serde_json::from_str(json)?;
        // A bare ntsc-rs preset is a look with only the NTSC section.
        if root.get("ntsc").is_none() && root.get("version").is_some() && root.get("luma_smear").is_some() {
            let ntsc = ntsc_from_map(root.as_object().unwrap())?;
            return Ok(Look {
                digital: None,
                ntsc: Some((true, ntsc)),
                downscale: None,
                glitch: None,
                shader: None,
                keys: Vec::new(),
                timeline_fps: 30.0,
                duration: 5.0,
            });
        }
        if root.get("kind").and_then(Value::as_str) == Some(SCREEN_LOOP_KIND) {
            return Err("this is a Screen Loop preset, not a look".into());
        }

        let ntsc_base: Option<(bool, Map<String, Value>)> = match root.get("ntsc") {
            Some(n) => {
                let settings = n.get("settings").and_then(Value::as_object).ok_or("ntsc.settings missing")?;
                Some((enabled(n), settings.clone()))
            }
            None => None,
        };
        let ntsc = match &ntsc_base {
            Some((on, m)) => Some((*on, ntsc_from_map(m)?)),
            None => None,
        };
        let downscale = root.get("downscale").map(|d| {
            let method = match d.get("method").and_then(Value::as_str) {
                Some("area") => DownscaleMethod::Area,
                _ => DownscaleMethod::Nearest,
            };
            let width = d.get("width").and_then(Value::as_u64).map_or(DEFAULT_DOWNSCALE_WIDTH, |w| w as usize);
            (enabled(d), width, method)
        });
        let glitch_base = root.get("glitch").map(|g| (enabled(g), values(g.get("values").unwrap_or(&Value::Null))));
        let digital_base = root.get("digital").map(|g| (enabled(g), values(g.get("values").unwrap_or(&Value::Null))));
        let shader_base = root.get("shader").map(|s| {
            (
                enabled(s),
                s.get("preset").and_then(Value::as_str).unwrap_or("glow_gauss").to_string(),
                values(s.get("params").unwrap_or(&Value::Null)),
            )
        });

        let glitch_from = |over: &[(String, f64)]| {
            let mut s = GlitchSettings::default();
            for (k, f) in glitch_base.iter().flat_map(|(_, b)| b.iter()).chain(over) {
                s.set(k, *f);
            }
            s
        };
        let digital_from = |over: &[(String, f64)]| {
            let mut s = DigitalSettings::default();
            for (k, f) in digital_base.iter().flat_map(|(_, b)| b.iter()) {
                s.set(k, *f);
            }
            let base = s.clone();
            for (k, f) in over {
                s.set(k, *f);
            }
            // The stream's structure holds for a whole render.
            s.holding_structure(&base)
        };
        let shader_from = |over: &[(String, f64)]| -> Vec<(String, f32)> {
            let mut out: Vec<(String, f32)> = shader_base
                .iter()
                .flat_map(|(_, _, b)| b.iter())
                .map(|(k, f)| (k.clone(), *f as f32))
                .collect();
            for (k, f) in over {
                match out.iter_mut().find(|(n, _)| n == k) {
                    Some(e) => e.1 = *f as f32,
                    None => out.push((k.clone(), *f as f32)),
                }
            }
            out
        };

        let mut keys = Vec::new();
        let mut timeline_fps = 30.0;
        let mut duration = 5.0;
        if let Some(tl) = root.get("timeline") {
            timeline_fps = tl.get("fps").and_then(Value::as_f64).unwrap_or(30.0);
            duration = tl.get("duration").and_then(Value::as_f64).unwrap_or(5.0).max(0.1);
            let timeline_on = tl.get("enabled").and_then(Value::as_bool).unwrap_or(true);
            if let Some(arr) = tl.get("keys").and_then(Value::as_array).filter(|_| timeline_on) {
                for k in arr {
                    let over = |name: &str| k.get(name).map(values).unwrap_or_default();
                    let ntsc = match &ntsc_base {
                        Some((_, base)) => {
                            let mut m = base.clone();
                            if let Some(o) = k.get("ntsc").and_then(Value::as_object) {
                                for (key, v) in o {
                                    m.insert(key.clone(), v.clone());
                                }
                            }
                            ntsc_from_map(&m)?
                        }
                        None => house_ntsc(),
                    };
                    keys.push(LookKey {
                        time: k.get("t").and_then(Value::as_f64).unwrap_or(0.0).clamp(0.0, 1.0) * duration,
                        easing: Easing::parse(k.get("easing").and_then(Value::as_str).unwrap_or("Linear")),
                        glitch: glitch_from(&over("glitch")),
                        digital: digital_from(&over("digital")),
                        ntsc,
                        shader: shader_from(&over("shader")),
                    });
                }
                keys.sort_by(|a, b| a.time.total_cmp(&b.time));
            }
        }

        Ok(Look {
            digital: digital_base.as_ref().map(|(on, _)| (*on, digital_from(&[]))),
            ntsc,
            downscale,
            glitch: glitch_base.as_ref().map(|(on, _)| (*on, glitch_from(&[]))),
            shader: shader_base.as_ref().map(|(on, id, _)| (*on, id.clone(), shader_from(&[]))),
            keys,
            timeline_fps,
            duration,
        })
    }
}

pub const SCREEN_LOOP_KIND: &str = "ntscrt-video-feedback";

/// A Screen Loop preset (NTSCRT's video-feedback files), with the spellings and the drift form of
/// earlier versions migrated.
pub fn parse_screen_loop(json: &str) -> Result<HowlSettings, Error> {
    let root: Value = serde_json::from_str(json)?;
    let src = root.get("values").unwrap_or(&root);
    let mut v: Vec<(String, f64)> = values(src);
    for (old, new) in [("centre_x", "center_x"), ("centre_y", "center_y"), ("colour_drift", "color_drift")] {
        if let Some(i) = v.iter().position(|(k, _)| k == old) {
            if !v.iter().any(|(k, _)| k == new) {
                v[i].0 = new.to_string();
            }
        }
    }
    let take = |v: &mut Vec<(String, f64)>, k: &str| v.iter().position(|(n, _)| n == k).map(|i| v.remove(i).1);
    let distance = take(&mut v, "drift");
    let degrees = take(&mut v, "drift_dir");
    let mut s = HowlSettings::default();
    for (k, f) in &v {
        s.set(k, *f);
    }
    if let Some(d) = distance {
        if !v.iter().any(|(k, _)| k == "drift_x" || k == "drift_y") {
            let a = degrees.unwrap_or(0.0).to_radians();
            let clean = |x: f64| if x.abs() < 1e-12 { 0.0 } else { x };
            s.set("drift_x", clean(d * a.cos()));
            s.set("drift_y", clean(-d * a.sin()));
        }
    }
    Ok(s)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bundled::{LOOKS, SCREEN_LOOPS};

    #[test]
    fn house_defaults_apply() {
        let s = house_ntsc();
        let m = ntsc_to_map(&s);
        assert!((m["luma_smear"].as_f64().unwrap() - 0.6692).abs() < 1e-4);
        assert_eq!(m["vhs_tape_speed"].as_i64(), Some(3));
        assert_eq!(m["scale_with_video_size"].as_bool(), Some(true));
    }

    #[test]
    fn parses_every_bundled_look() {
        assert_eq!(LOOKS.len(), 27);
        let mut keyed = 0;
        for (name, json) in LOOKS {
            let look = Look::parse(json).unwrap_or_else(|e| panic!("{name}: {e}"));
            assert!(look.ntsc.is_some(), "{name}");
            if !look.keys.is_empty() {
                keyed += 1;
                assert!(look.keys.windows(2).all(|w| w[0].time <= w[1].time), "{name}");
                assert!(look.keys.last().unwrap().time <= look.duration + 1e-9, "{name}");
            }
        }
        assert!(keyed >= 15, "{keyed} animated looks");
    }

    #[test]
    fn parses_every_bundled_screen_loop() {
        assert_eq!(SCREEN_LOOPS.len(), 5);
        for (name, json) in SCREEN_LOOPS {
            let s = parse_screen_loop(json).unwrap_or_else(|e| panic!("{name}: {e}"));
            assert!(s.zoom() > 0.0, "{name}");
        }
        let s = parse_screen_loop(SCREEN_LOOPS[1].1).unwrap();
        assert_eq!(s.delay(), 5);
        assert!(s.get("key_invert") >= 0.5);
    }

    #[test]
    fn easings_match_ntscrt() {
        assert_eq!(Easing::EaseIn.apply(0.5), 0.25);
        assert_eq!(Easing::EaseOut.apply(0.5), 0.75);
        assert_eq!(Easing::EaseInOut.apply(0.25), 0.125);
        assert_eq!(Easing::Hold.apply(0.9), 0.0);
    }
}

/// The settings a look file stores, read from wherever they live (the effect's controls).
pub struct LookOut<'a> {
    pub digital: (bool, &'a DigitalSettings),
    pub ntsc: (bool, &'a NtscEffectFullSettings),
    pub downscale: (bool, usize, DownscaleMethod),
    pub glitch: (bool, &'a GlitchSettings),
    pub shader: (bool, &'a str, &'a [(&'a str, f32)]),
}

fn number(v: f64) -> Value {
    if v.fract() == 0.0 && v.abs() < 1e15 { Value::from(v as i64) } else { Value::from(v) }
}

impl LookOut<'_> {
    /// A look file in NTSCRT's format (it opens in the app, and back here).
    pub fn to_json(&self) -> String {
        use crate::digital::params::DIGITAL_PARAMS;
        use crate::glitch::params::GLITCH_PARAMS;

        let (d_on, d) = self.digital;
        let digital: Map<String, Value> = DIGITAL_PARAMS.iter().map(|p| (p.id.to_string(), number(d.get(p.id)))).collect();
        let (g_on, g) = self.glitch;
        let glitch: Map<String, Value> = GLITCH_PARAMS.iter().map(|p| (p.id.to_string(), number(g.get(p.id)))).collect();
        let (s_on, preset, params) = self.shader;
        let shader: Map<String, Value> = params.iter().map(|(n, v)| (n.to_string(), Value::from(*v as f64))).collect();
        let (ds_on, width, method) = self.downscale;
        let preset_label = match width {
            256 => "SNES (256px)",
            320 => "VGA (320px)",
            _ => "Custom",
        };
        let look = serde_json::json!({
            "version": 1,
            "digital": { "enabled": d_on, "values": digital },
            "downscale": {
                "enabled": ds_on,
                "width": width,
                "preset": preset_label,
                "method": if method == DownscaleMethod::Area { "area" } else { "nearest" },
            },
            "glitch": { "enabled": g_on, "values": glitch },
            "ntsc": { "enabled": self.ntsc.0, "settings": ntsc_to_map(self.ntsc.1) },
            "shader": { "enabled": s_on, "preset": preset, "params": shader },
            "timeline": { "enabled": false, "duration": 5, "fps": 30, "keys": [] },
            "view": { "animate": true, "compare": false, "integerScale": true },
        });
        serde_json::to_string_pretty(&look).expect("look serializes")
    }
}

/// A Screen Loop preset in NTSCRT's format.
pub fn screen_loop_to_json(s: &HowlSettings) -> String {
    let values: Map<String, Value> = crate::howlaround::HOWL_PARAMS.iter().map(|p| (p.id.to_string(), number(s.get(p.id)))).collect();
    let preset = serde_json::json!({ "kind": SCREEN_LOOP_KIND, "version": 2, "values": values });
    serde_json::to_string_pretty(&preset).expect("preset serializes")
}

#[cfg(test)]
mod save_tests {
    use super::*;
    use crate::bundled::{LOOKS, SCREEN_LOOPS};

    #[test]
    fn looks_round_trip() {
        for (name, json) in LOOKS {
            let a = Look::parse(json).unwrap();
            let d = a.digital.clone().unwrap_or((false, DigitalSettings::default()));
            let n = a.ntsc.clone().unwrap();
            let g = a.glitch.clone().unwrap_or((false, GlitchSettings::default()));
            let (s_on, s_id, s_vals) = a.shader.clone().unwrap();
            let refs: Vec<(&str, f32)> = s_vals.iter().map(|(k, v)| (k.as_str(), *v)).collect();
            let (ds_on, w, m) = a.downscale.unwrap();
            let out = LookOut { digital: (d.0, &d.1), ntsc: (n.0, &n.1), downscale: (ds_on, w, m), glitch: (g.0, &g.1), shader: (s_on, &s_id, &refs) };
            let b = Look::parse(&out.to_json()).unwrap_or_else(|e| panic!("{name}: {e}"));
            assert_eq!(ntsc_to_map(&b.ntsc.as_ref().unwrap().1), ntsc_to_map(&n.1), "{name} ntsc");
            assert_eq!(b.glitch.unwrap().1, g.1, "{name} glitch");
            assert_eq!(b.downscale, Some((ds_on, w, m)), "{name} downscale");
            let bs = b.shader.unwrap();
            assert_eq!((bs.0, bs.1.as_str()), (s_on, s_id.as_str()), "{name} shader");
            for (k, v) in &s_vals {
                let got = bs.2.iter().find(|(n, _)| n == k).map(|(_, v)| *v);
                assert_eq!(got, Some(*v), "{name} shader {k}");
            }
        }
    }

    #[test]
    fn screen_loops_round_trip() {
        for (name, json) in SCREEN_LOOPS {
            let a = parse_screen_loop(json).unwrap();
            let b = parse_screen_loop(&screen_loop_to_json(&a)).unwrap();
            for p in crate::howlaround::HOWL_PARAMS {
                assert_eq!(a.get(p.id), b.get(p.id), "{name} {}", p.id);
            }
        }
    }
}
