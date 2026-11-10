#![cfg(windows)]

mod keyframes;
mod ntsc_params;
mod pixels;
mod screen_loop;
mod window_handle;

use std::hash::{Hash, Hasher};
use std::num::NonZero;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use after_effects::{self as ae};
use ntsc_rs::settings::standard::NtscEffectFullSettings;
use ntsc_rs::settings::SettingsList;
use ntsc_rs::NtscEffect;
use phosphor_core::bundled::{LOOKS, SCREEN_LOOPS};
use phosphor_core::crt::{CrtPreset, CRT_PRESETS, DEFAULT_PRESET};
use phosphor_core::digital::params::{DigitalKind, DIGITAL_FORMATS, DIGITAL_PARAMS, STRUCTURE_IDS};
use phosphor_core::digital::DigitalSettings;
use phosphor_core::downscale::{downscale as downscale_image, DownscaleMethod, DownscaleSpec};
use phosphor_core::glitch::params::{GlitchKind, GLITCH_PARAMS};
use phosphor_core::glitch::GlitchSettings;
use phosphor_core::howlaround::{HowlKind, HowlLoop, HowlSettings, HOWL_PARAMS};
use phosphor_core::look::{self, parse_screen_loop, screen_loop_to_json, Look, LookOut};
use phosphor_core::pipeline::{ChainStages, CrtStage, DigitalStage, FrameRequest, LoopPass, NtscStage, Pipeline};
use phosphor_core::Image;
use raw_window_handle::Win32WindowHandle;
use window_handle::WindowAndDisplayHandle;

use ntsc_params::NtscParams;
use screen_loop::LoopCache;

const GROUP_DOWNSCALE: u32 = 1;
const GROUP_DIGITAL: u32 = 2;
const GROUP_NTSC: u32 = 3;
const GROUP_GLITCH: u32 = 4;
const GROUP_CRT: u32 = 5;
const GROUP_LOOP: u32 = 6;
const GROUP_CRT_PRESET: u32 = 100;

/// Checkout ids for frames other than the current one.
const EARLIER_BASE: i32 = 1;
const SCENE_BASE: i32 = 10_000;
const MAX_EARLIER_FRAMES: usize = 240;
/// The Screen Loop's run-up works on a picture at most this wide.
const RUN_UP_WIDTH: usize = 640;
/// After Effects asked us to stop (the user moved on): not an error to show.
const CANCELLED: &str = "cancelled";

/// Ids are hashed from these names by the after-effects crate: renaming a variant breaks saved projects.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub enum ParamID {
    LookMenu,
    LoadLook,
    SaveLook,
    GroupStart(u32),
    GroupEnd(u32),
    DownscaleOn,
    DownscalePreset,
    DownscaleWidth,
    DownscaleMethod,
    DigitalOn,
    DigitalFormat,
    Digital(u8),
    NtscOn,
    NtscLoad,
    NtscSave,
    Ntsc(i32),
    NtscGroupStart(i32),
    NtscGroupEnd(i32),
    GlitchOn,
    Glitch(u8),
    CrtOn,
    CrtShader,
    Crt(u8, u8),
    LoopOn,
    LoopPreset,
    LoopRenderOn,
    LoopSave,
    LoopCenter,
    LoopDriftTo,
    Loop(u8),
}

/// A control's value as its stream stores it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ControlValue {
    Slider(f64),
    Checkbox(bool),
    Popup(i32),
}

const DOWNSCALE_PRESETS: &[(&str, usize)] = &[("SNES (256px)", 256), ("VGA (320px)", 320), ("Custom", 0)];
const NO_LOOK: &str = "Choose a look…";

/// Settings sampled over time, valid while the params' all-time state receipt is unchanged.
#[derive(Default)]
struct History<T> {
    receipt: Vec<[i32; 4]>,
    by_frame: HashMap<i64, T>,
}

/// Per effect instance: the pipeline's caches, sampled parameter histories and the Screen Loop.
struct InstanceState {
    pipeline: Pipeline,
    glitch: Mutex<History<GlitchSettings>>,
    digital: Mutex<History<DigitalSettings>>,
    screen_loop: Mutex<LoopCache>,
}

struct Plugin {
    ntsc_list: SettingsList<NtscEffectFullSettings>,
    house_ntsc: NtscEffectFullSettings,
    shaders_root: PathBuf,
    aegp_id: Option<ae::aegp::PluginId>,
    instances: Mutex<HashMap<u64, Arc<InstanceState>>>,
}

/// Effect instance data: an id keying the in-memory caches. A fresh id on every load or duplicate
/// keeps two instances from ever sharing a codec stream or a loop.
struct Instance {
    id: u64,
}

fn fresh_id() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(1);
    let t = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos() as u64);
    t ^ COUNTER.fetch_add(1, Ordering::Relaxed).rotate_left(40)
}

impl Default for Instance {
    fn default() -> Self {
        Self { id: fresh_id() }
    }
}

mod log {
    use std::io::Write;
    use std::sync::Mutex;

    static FILE: Mutex<Option<std::fs::File>> = Mutex::new(None);

    pub fn line(msg: &str) {
        let Ok(mut f) = FILE.lock() else { return };
        if f.is_none() {
            let path = std::env::temp_dir().join("phosphor-ae.log");
            *f = std::fs::OpenOptions::new().create(true).append(true).open(path).ok();
        }
        if let Some(file) = f.as_mut() {
            let t = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_millis());
            let _ = writeln!(file, "{t} [{:?}] {msg}", std::thread::current().id());
            let _ = file.flush();
        }
    }
}

fn module_dir() -> PathBuf {
    use windows::Win32::Foundation::HMODULE;
    use windows::Win32::System::LibraryLoader::{
        GetModuleFileNameW, GetModuleHandleExW, GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS,
        GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT,
    };
    let mut module = HMODULE::default();
    let mut buf = [0u16; 1024];
    unsafe {
        let _ = GetModuleHandleExW(
            GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS | GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT,
            windows::core::PCWSTR(module_dir as *const u16),
            &mut module,
        );
        let n = GetModuleFileNameW(Some(module), &mut buf) as usize;
        PathBuf::from(String::from_utf16_lossy(&buf[..n]))
            .parent()
            .map(|p| p.to_path_buf())
            .unwrap_or_default()
    }
}

impl Default for Plugin {
    fn default() -> Self {
        Self {
            ntsc_list: SettingsList::<NtscEffectFullSettings>::new(),
            house_ntsc: look::house_ntsc(),
            shaders_root: module_dir().join("shaders"),
            aegp_id: None,
            instances: Mutex::new(HashMap::new()),
        }
    }
}

ae::define_effect!(Plugin, Instance, ParamID);

impl AdobePluginInstance for Instance {
    fn flatten(&self) -> Result<(u16, Vec<u8>), Error> {
        Ok((1, self.id.to_le_bytes().to_vec()))
    }

    fn unflatten(_version: u16, _bytes: &[u8]) -> Result<Self, Error> {
        Ok(Self::default())
    }

    fn render(&self, plugin: &mut PluginState, in_layer: &Layer, out_layer: &mut Layer) -> Result<(), ae::Error> {
        let in_data = plugin.in_data;
        if let Err(e) = plugin.global.legacy_render(self.id, &in_data, in_layer, out_layer, plugin.params) {
            log::line(&format!("legacy render error: {e}"));
            plugin.out_data.set_error_msg(&format!("Phosphor: {e}"));
        }
        Ok(())
    }

    fn handle_command(&mut self, plugin: &mut PluginState, command: Command) -> Result<(), Error> {
        let in_data = plugin.in_data;
        match command {
            Command::SmartPreRender { extra } => {
                let r = plugin.global.pre_render(self.id, &in_data, extra, plugin.params);
                if let Err(e) = &r {
                    log::line(&format!("pre_render frame {}: {e:?}", in_data.current_frame()));
                }
                r?;
            }
            Command::SmartRender { extra } => {
                let t = std::time::Instant::now();
                let r = plugin.global.smart_render(self.id, &in_data, extra, plugin.params);
                log::line(&format!(
                    "frame {} in {} ms{}",
                    in_data.current_frame(),
                    t.elapsed().as_millis(),
                    r.as_ref().err().map(|e| format!(" - error: {e}")).unwrap_or_default()
                ));
                match r {
                    Err(e) if e.to_string().contains(CANCELLED) => return Err(Error::InterruptCancel),
                    Err(e) => plugin.out_data.set_error_msg(&format!("Phosphor: {e}")),
                    Ok(()) => {}
                }
            }
            _ => {}
        }
        Ok(())
    }
}

// MARK: - parameter helpers

fn checkbox(params: &mut Parameters<ParamID>, id: ParamID, name: &str, label: &str, value: bool, supervise: bool) -> Result<(), Error> {
    params.add_customized(
        id,
        name,
        ae::CheckBoxDef::setup(|c| {
            c.set_default(value);
            c.set_value(value);
            c.set_label(label);
        }),
        |p| {
            if supervise {
                p.set_flag(ParamFlag::SUPERVISE, true);
            }
            -1
        },
    )
}

/// `action`: picking an item does something (applies a look or a format) rather than holding a value.
fn popup(params: &mut Parameters<ParamID>, id: ParamID, name: &str, options: &[&str], value: i32, supervise: bool, action: bool) -> Result<(), Error> {
    params.add_customized(
        id,
        name,
        ae::PopupDef::setup(|p| {
            p.set_options(options);
            p.set_default(value);
            p.set_value(value);
        }),
        |p| {
            if supervise || action {
                p.set_flag(ParamFlag::SUPERVISE, true);
            }
            if action {
                p.set_flag(ParamFlag::CANNOT_TIME_VARY, true);
            }
            -1
        },
    )
}

#[allow(clippy::too_many_arguments)]
fn slider(params: &mut Parameters<ParamID>, id: ParamID, name: &str, min: f64, max: f64, value: f64, precision: i16, percent: bool, supervise: bool) -> Result<(), Error> {
    params.add_customized(
        id,
        name,
        ae::FloatSliderDef::setup(|f| {
            f.set_valid_min(min as f32);
            f.set_slider_min(min as f32);
            f.set_valid_max(max as f32);
            f.set_slider_max(max as f32);
            f.set_default(value);
            f.set_value(value);
            f.set_precision(precision);
            if percent {
                f.set_display_flags(ValueDisplayFlag::PERCENT);
            }
        }),
        |p| {
            if supervise {
                p.set_flag(ParamFlag::SUPERVISE, true);
            }
            -1
        },
    )
}

fn point(params: &mut Parameters<ParamID>, id: ParamID, name: &str, x_percent: f32, y_percent: f32) -> Result<(), Error> {
    params.add_customized(
        id,
        name,
        ae::PointDef::setup(|p| {
            p.set_default((x_percent, y_percent));
            p.set_restrict_bounds(false);
        }),
        |_| -1,
    )
}

fn button(params: &mut Parameters<ParamID>, id: ParamID, text: &str) -> Result<(), Error> {
    params.add_customized(
        id,
        "",
        ae::ButtonDef::setup(|def| {
            def.set_label(text);
        }),
        |p| {
            p.set_flag(ParamFlag::SUPERVISE, true);
            p.set_flag(ParamFlag::CANNOT_TIME_VARY, true);
            -1
        },
    )
}

fn precision_for(step: f32) -> i16 {
    if step <= 0.0 {
        return 3;
    }
    (-(step.log10().floor()) as i16).clamp(0, 3)
}

fn label(text: &str, unit: &str) -> String {
    if unit.is_empty() { text.to_string() } else { format!("{text} ({unit})") }
}

fn checked(params: &Parameters<ParamID>, id: ParamID) -> Result<bool, Error> {
    Ok(params.get(id)?.as_checkbox()?.value())
}

fn checked_at(params: &Parameters<ParamID>, id: ParamID, time: Option<i32>) -> Result<bool, Error> {
    Ok(params.checkout_at(id, time, None, None)?.as_checkbox()?.value())
}

/// Snap a shader value to its step grid, as NTSCRT does for keyframed values.
fn snap_crt(v: f32, min: f32, max: f32, step: f32) -> f32 {
    let v = if step > 0.0 { min + ((v - min) / step).round() * step } else { v };
    v.clamp(min, max)
}

// MARK: - setup

impl AdobePluginGlobal for Plugin {
    fn params_setup(&self, params: &mut Parameters<ParamID>, _in_data: InData, _out_data: OutData) -> Result<(), Error> {
        let mut looks = vec![NO_LOOK];
        looks.extend(LOOKS.iter().map(|(name, _)| *name));
        popup(params, ParamID::LookMenu, "Look", &looks, 1, false, true)?;
        button(params, ParamID::LoadLook, "Load Look File…")?;
        button(params, ParamID::SaveLook, "Save Look File…")?;

        params.add_group(ParamID::GroupStart(GROUP_DOWNSCALE), ParamID::GroupEnd(GROUP_DOWNSCALE), "Downscale", true, |g| {
            checkbox(g, ParamID::DownscaleOn, "Downscale On", "Enabled", true, true)?;
            let names: Vec<&str> = DOWNSCALE_PRESETS.iter().map(|p| p.0).collect();
            popup(g, ParamID::DownscalePreset, "Resolution", &names, 2, true, true)?;
            slider(g, ParamID::DownscaleWidth, "Width (px)", 16.0, 1920.0, look::DEFAULT_DOWNSCALE_WIDTH as f64, 0, false, true)?;
            popup(g, ParamID::DownscaleMethod, "Sampling", &["Chunky", "Smooth"], 1, false, false)
        })?;

        params.add_group(ParamID::GroupStart(GROUP_DIGITAL), ParamID::GroupEnd(GROUP_DIGITAL), "Digital Video", true, |g| {
            checkbox(g, ParamID::DigitalOn, "Digital Video On", "Enabled", false, true)?;
            let mut formats = vec!["Custom"];
            formats.extend(DIGITAL_FORMATS.iter().map(|f| f.name));
            popup(g, ParamID::DigitalFormat, "Format", &formats, 2, true, true)?;
            let webcam = DIGITAL_FORMATS[0].settings();
            for (i, p) in DIGITAL_PARAMS.iter().enumerate() {
                let v = webcam.get(p.id);
                match p.kind {
                    DigitalKind::Choice(options) => popup(g, ParamID::Digital(i as u8), p.label, options, v as i32 + 1, false, false)?,
                    DigitalKind::Slider { min, max, percent, unit } => {
                        let k = if percent { 100.0 } else { 1.0 };
                        let precision = if percent || max - min <= 4.0 { 2 } else { 1 };
                        slider(g, ParamID::Digital(i as u8), &label(p.label, unit), min * k, max * k, v * k, precision, percent, false)?
                    }
                }
            }
            Ok(())
        })?;

        params.add_group(ParamID::GroupStart(GROUP_NTSC), ParamID::GroupEnd(GROUP_NTSC), "NTSC (TV)", true, |g| {
            checkbox(g, ParamID::NtscOn, "NTSC On", "Enabled", true, true)?;
            button(g, ParamID::NtscLoad, "Load ntsc-rs Preset…")?;
            button(g, ParamID::NtscSave, "Save ntsc-rs Preset…")?;
            NtscParams::map_params(g, &self.ntsc_list.setting_descriptors, &self.house_ntsc, &self.house_ntsc)
        })?;

        params.add_group(ParamID::GroupStart(GROUP_GLITCH), ParamID::GroupEnd(GROUP_GLITCH), "Glitch", true, |g| {
            checkbox(g, ParamID::GlitchOn, "Glitch On", "Enabled", true, true)?;
            for (i, p) in GLITCH_PARAMS.iter().enumerate() {
                match p.kind {
                    GlitchKind::Toggle => checkbox(g, ParamID::Glitch(i as u8), p.label, "On", p.default >= 0.5, false)?,
                    GlitchKind::Slider { min, max, percent, unit } => {
                        let k = if percent { 100.0 } else { 1.0 };
                        slider(g, ParamID::Glitch(i as u8), &label(p.label, unit), min * k, max * k, p.default * k, 2, percent, false)?
                    }
                }
            }
            Ok(())
        })?;

        params.add_group(ParamID::GroupStart(GROUP_CRT), ParamID::GroupEnd(GROUP_CRT), "CRT", true, |g| {
            checkbox(g, ParamID::CrtOn, "CRT On", "Enabled", true, true)?;
            let names: Vec<&str> = CRT_PRESETS.iter().map(|p| p.name).collect();
            let default = CRT_PRESETS.iter().position(|p| p.id == DEFAULT_PRESET).unwrap_or(0) as i32 + 1;
            popup(g, ParamID::CrtShader, "Shader", &names, default, true, false)?;
            for (pi, preset) in CRT_PRESETS.iter().enumerate() {
                let gid = GROUP_CRT_PRESET + pi as u32;
                g.add_group(ParamID::GroupStart(gid), ParamID::GroupEnd(gid), &format!("{} settings", preset.name), false, |s| {
                    for (i, p) in preset.params.iter().enumerate() {
                        if p.min >= p.max {
                            continue;
                        }
                        slider(s, ParamID::Crt(pi as u8, i as u8), p.desc.trim().trim_start_matches('*').trim(), p.min as f64, p.max as f64, p.initial as f64, precision_for(p.step), false, p.gate.is_none())?;
                    }
                    Ok(())
                })?;
            }
            Ok(())
        })?;

        params.add_group(ParamID::GroupStart(GROUP_LOOP), ParamID::GroupEnd(GROUP_LOOP), "Screen Loop", true, |g| {
            checkbox(g, ParamID::LoopOn, "Screen Loop On", "Enabled", false, true)?;
            let mut presets = vec!["Custom"];
            presets.extend(SCREEN_LOOPS.iter().map(|(name, _)| *name));
            popup(g, ParamID::LoopPreset, "Screen Loop Preset", &presets, 1, false, true)?;
            // The camera that films the TV: the GPU matches the CPU to within 8-bit rounding.
            popup(g, ParamID::LoopRenderOn, "Camera renders on", &["GPU (recommended)", "CPU (exact)"], 1, false, false)?;
            button(g, ParamID::LoopSave, "Save Screen Loop Preset…")?;
            let defaults = HowlSettings::default();
            let (cx, cy) = ((defaults.get("center_x") + 0.5) * 100.0, (defaults.get("center_y") + 0.5) * 100.0);
            point(g, ParamID::LoopCenter, "Vanishing point", cx as f32, cy as f32)?;
            point(g, ParamID::LoopDriftTo, "Drifts to", cx as f32, cy as f32)?;
            for (i, p) in HOWL_PARAMS.iter().enumerate() {
                match p.kind {
                    HowlKind::Position { .. } => {}
                    HowlKind::Toggle => checkbox(g, ParamID::Loop(i as u8), p.label, "On", p.default >= 0.5, false)?,
                    HowlKind::Slider { min, max, percent, unit } => {
                        let k = if percent { 100.0 } else { 1.0 };
                        let precision = if p.id == "seed" || p.id == "delay" { 0 } else { 1 };
                        slider(g, ParamID::Loop(i as u8), &label(p.label, unit), min * k, max * k, p.default * k, precision, percent, false)?
                    }
                }
            }
            Ok(())
        })?;

        Ok(())
    }

    fn handle_command(&mut self, command: Command, in_data: InData, mut out_data: OutData, params: &mut Parameters<ParamID>) -> Result<(), Error> {
        match command {
            Command::GlobalSetup => {
                if in_data.is_premiere() {
                    let pf = suites::PixelFormat::new()?;
                    pf.clear_supported_pixel_formats(in_data.effect_ref())?;
                    pf.add_supported_pixel_format(in_data.effect_ref(), pr::PixelFormat::Bgra4444_8u)?;
                    pf.add_supported_pixel_format(in_data.effect_ref(), pr::PixelFormat::Bgra4444_16u)?;
                    pf.add_supported_pixel_format(in_data.effect_ref(), pr::PixelFormat::Bgra4444_32f)?;
                } else if let Ok(utility) = ae::aegp::suites::Utility::new() {
                    self.aegp_id = utility.register_with_aegp("Phosphor").ok();
                }
            }
            Command::About => {
                out_data.set_return_msg(&format!(
                    "Phosphor {}.{}.{}\r\rAnalog video, from the camera to the tube: digital video, NTSC/VHS signal, downscale, TV and VCR glitches, CRT shaders and screen feedback.\r\rA port of NTSCRT by Finn McKenty. NTSC signal by ntsc-rs (valadaptive). CRT shaders from libretro/slang-shaders (TroggleMonkey, EasyMode, Hyllian, J. Kyle Pittman, Themaister) via librashader (Ronny Chan). Glitch receiver after analogtv by Trevor Blackwell.\r\rVibecoded: written with AI (Claude Code) by a non-programmer. Free and open source; see CREDITS.md next to the plugin.",
                    env!("EFFECT_VERSION_MAJOR"),
                    env!("EFFECT_VERSION_MINOR"),
                    env!("EFFECT_VERSION_PATCH")
                ));
            }
            Command::UpdateParamsUi => self.update_ui(&in_data, params)?,
            Command::UserChangedParam { param_index } => {
                if let Err(e) = self.param_changed(&in_data, &mut out_data, params, param_index) {
                    log::line(&format!("param_changed {param_index}: {e:?}"));
                    return Err(e);
                }
            }
            _ => {}
        }
        Ok(())
    }
}

// MARK: - reading settings

type AnyError = Box<dyn std::error::Error>;

impl Plugin {
    fn state(&self, id: u64) -> Arc<InstanceState> {
        self.instances
            .lock()
            .unwrap()
            .entry(id)
            .or_insert_with(|| {
                Arc::new(InstanceState {
                    pipeline: Pipeline::new(&self.shaders_root),
                    glitch: Mutex::new(History::default()),
                    digital: Mutex::new(History::default()),
                    screen_loop: Mutex::new(LoopCache::default()),
                })
            })
            .clone()
    }

    fn selected_preset(params: &Parameters<ParamID>) -> Result<(usize, &'static CrtPreset), Error> {
        let i = (params.get(ParamID::CrtShader)?.as_popup()?.value() - 1).clamp(0, CRT_PRESETS.len() as i32 - 1) as usize;
        Ok((i, &CRT_PRESETS[i]))
    }

    fn crt_values(params: &Parameters<ParamID>, pi: usize, time: Option<i32>) -> Result<Vec<(&'static str, f32)>, Error> {
        let preset = &CRT_PRESETS[pi];
        let mut out = Vec::with_capacity(preset.params.len());
        for (i, p) in preset.params.iter().enumerate() {
            let v = if p.min >= p.max {
                p.initial
            } else {
                let raw = params.checkout_at(ParamID::Crt(pi as u8, i as u8), time, None, None)?.as_float_slider()?.value() as f32;
                snap_crt(raw, p.min, p.max, p.step)
            };
            out.push((p.name, v));
        }
        Ok(out)
    }

    fn digital_at(params: &Parameters<ParamID>, time: Option<i32>) -> Result<DigitalSettings, Error> {
        let mut s = DigitalSettings::default();
        for (i, p) in DIGITAL_PARAMS.iter().enumerate() {
            let def = params.checkout_at(ParamID::Digital(i as u8), time, None, None)?;
            let v = match p.kind {
                DigitalKind::Choice(_) => (def.as_popup()?.value() - 1) as f64,
                DigitalKind::Slider { percent, .. } => def.as_float_slider()?.value() / if percent { 100.0 } else { 1.0 },
            };
            s.set(p.id, v);
        }
        Ok(s)
    }

    fn glitch_at(params: &Parameters<ParamID>, time: Option<i32>) -> Result<GlitchSettings, Error> {
        let mut s = GlitchSettings::default();
        for (i, p) in GLITCH_PARAMS.iter().enumerate() {
            let def = params.checkout_at(ParamID::Glitch(i as u8), time, None, None)?;
            let v = match p.kind {
                GlitchKind::Toggle => def.as_checkbox()?.value() as u8 as f64,
                GlitchKind::Slider { percent, .. } => def.as_float_slider()?.value() / if percent { 100.0 } else { 1.0 },
            };
            s.set(p.id, v);
        }
        Ok(s)
    }

    /// The Screen Loop's settings at `time`; positions relative to a `w` × `h` frame.
    fn howl_at(params: &Parameters<ParamID>, time: Option<i32>, w: f64, h: f64) -> Result<HowlSettings, Error> {
        let mut s = HowlSettings::default();
        for (i, p) in HOWL_PARAMS.iter().enumerate() {
            match p.kind {
                HowlKind::Position { .. } => {}
                HowlKind::Toggle => s.set(p.id, checked_at(params, ParamID::Loop(i as u8), time)? as u8 as f64),
                HowlKind::Slider { percent, .. } => {
                    let v = params.checkout_at(ParamID::Loop(i as u8), time, None, None)?.as_float_slider()?.value();
                    s.set(p.id, v / if percent { 100.0 } else { 1.0 });
                }
            }
        }
        let center = params.checkout_at(ParamID::LoopCenter, time, None, None)?.as_point()?.value();
        let to = params.checkout_at(ParamID::LoopDriftTo, time, None, None)?.as_point()?.value();
        let (cx, cy) = (center.0 as f64 / w.max(1.0) - 0.5, center.1 as f64 / h.max(1.0) - 0.5);
        s.set("center_x", cx.clamp(-0.5, 0.5));
        s.set("center_y", cy.clamp(-0.5, 0.5));
        s.set("drift_x", (to.0 as f64 / w.max(1.0) - 0.5 - cx).clamp(-1.0, 1.0));
        s.set("drift_y", (to.1 as f64 / h.max(1.0) - 0.5 - cy).clamp(-1.0, 1.0));
        Ok(s)
    }

    /// After Effects' all-time state receipts for `ids`: they change whenever a value, keyframe or
    /// expression on those params does. The PF param-utils suite is allowed on render threads.
    fn receipt(in_data: &InData, params: &Parameters<ParamID>, ids: impl Iterator<Item = ParamID>) -> Option<Vec<[i32; 4]>> {
        let effect = in_data.effect();
        let mut out = Vec::new();
        for id in ids {
            let index = params.index(id)?;
            let state = effect.current_param_state(index as i32, None, None).ok()?;
            out.push(state.reservedAL);
        }
        Some(out)
    }

    fn all_ids(params: &Parameters<ParamID>) -> Vec<ParamID> {
        (1..params.num_params()).map(|i| params.type_at(i)).collect()
    }

    fn sampled<T: Clone>(hist: &Mutex<History<T>>, receipt: &Option<Vec<[i32; 4]>>, k: i64, sample: impl FnOnce() -> T) -> T {
        let Some(receipt) = receipt else { return sample() };
        {
            let mut h = hist.lock().unwrap();
            if &h.receipt != receipt {
                h.receipt = receipt.clone();
                h.by_frame.clear();
            }
            if let Some(v) = h.by_frame.get(&k) {
                return v.clone();
            }
        }
        let v = sample();
        let mut h = hist.lock().unwrap();
        if &h.receipt == receipt {
            h.by_frame.insert(k, v.clone());
        }
        v
    }

    fn digital_ids() -> impl Iterator<Item = ParamID> {
        (0..DIGITAL_PARAMS.len()).map(|i| ParamID::Digital(i as u8))
    }

    fn glitch_ids() -> impl Iterator<Item = ParamID> {
        (0..GLITCH_PARAMS.len()).map(|i| ParamID::Glitch(i as u8))
    }

    fn fps(in_data: &InData) -> f64 {
        if in_data.time_step() == 0 { 30.0 } else { in_data.time_scale() as f64 / in_data.time_step() as f64 }
    }

    fn frame_time(in_data: &InData, k: i64) -> i32 {
        (k * in_data.time_step() as i64) as i32
    }

    fn layer_seconds(in_data: &InData) -> f64 {
        let total = unsafe { (*in_data.as_ptr()).total_time };
        (total as f64 / in_data.time_scale().max(1) as f64).max(0.1)
    }

    fn downscale_setting(params: &Parameters<ParamID>) -> Result<Option<(usize, DownscaleMethod)>, Error> {
        if !checked(params, ParamID::DownscaleOn)? {
            return Ok(None);
        }
        let width = params.get(ParamID::DownscaleWidth)?.as_float_slider()?.value().round().max(16.0) as usize;
        let method = if params.get(ParamID::DownscaleMethod)?.as_popup()?.value() == 2 {
            DownscaleMethod::Area
        } else {
            DownscaleMethod::Nearest
        };
        Ok(Some((width, method)))
    }

    fn chain_size(w: usize, h: usize, downscale: Option<(usize, DownscaleMethod)>) -> (usize, usize) {
        match downscale {
            Some((dw, m)) => {
                let d = DownscaleSpec::for_source(dw, w, h, m);
                (d.width, d.height)
            }
            None => (w, h),
        }
    }

    /// Everything a Screen Loop's state depends on (params over all time, frame geometry).
    fn loop_key(in_data: &InData, params: &Parameters<ParamID>, w: usize, h: usize) -> u64 {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        Self::receipt(in_data, params, Self::all_ids(params).into_iter()).hash(&mut hasher);
        (w, h, in_data.time_step(), in_data.time_scale()).hash(&mut hasher);
        unsafe { (*in_data.as_ptr()).total_time }.hash(&mut hasher);
        hasher.finish()
    }
}

// MARK: - rendering

#[derive(Default, Clone)]
struct PreData {
    earlier: Vec<i64>,
    scenes: Vec<i64>,
}

impl Plugin {
    fn pre_render(&self, id: u64, in_data: &InData, mut extra: PreRenderExtra, params: &mut Parameters<ParamID>) -> Result<(), Error> {
        let mut req = extra.output_request();
        req.preserve_rgb_of_zero_alpha = 1;
        req.rect.left = 0;
        req.rect.top = 0;
        req.rect.right = pixels::ceil_mul_rational(in_data.width(), in_data.downsample_x());
        req.rect.bottom = pixels::ceil_mul_rational(in_data.height(), in_data.downsample_y());

        let in_res = extra.callbacks().checkout_layer(0, 0, &req, in_data.current_time(), in_data.time_step(), in_data.time_scale())?;
        let out_w = pixels::ceil_mul_rational(in_res.ref_width, in_data.downsample_x());
        let out_h = pixels::ceil_mul_rational(in_res.ref_height, in_data.downsample_y());
        let rect = Rect { left: 0, top: 0, right: out_w, bottom: out_h };
        extra.set_result_rect(rect);
        extra.set_max_result_rect(rect);
        extra.set_returns_extra_pixels(true);

        let params: &Parameters<ParamID> = params;
        let frame = in_data.current_frame().round() as i64;
        let (w, h) = (out_w.max(1) as usize, out_h.max(1) as usize);
        let state = self.state(id);
        let mut data = PreData::default();

        if checked(params, ParamID::LoopOn)? {
            // The loop runs in order: check out the scene frames since the state it continues from.
            let key = Self::loop_key(in_data, params, w, h);
            let plan = state.screen_loop.lock().unwrap().plan(key, frame);
            data.scenes = LoopCache::scene_frames(&plan, frame);
            for (i, f) in data.scenes.iter().enumerate() {
                extra.callbacks().checkout_layer(0, SCENE_BASE + i as i32, &req, Self::frame_time(in_data, *f), in_data.time_step(), in_data.time_scale())?;
            }
        } else if checked(params, ParamID::DigitalOn)? {
            // The codec re-runs from the last keyframe: check out those frames.
            let receipt = Self::receipt(in_data, params, Self::digital_ids());
            let now = Self::digital_at(params, None)?;
            let settings_at = |k: i64| -> DigitalSettings {
                if k == frame {
                    return now.clone();
                }
                Self::sampled(&state.digital, &receipt, k, || {
                    Self::digital_at(params, Some(Self::frame_time(in_data, k))).unwrap_or_else(|_| now.clone())
                })
            };
            let dreq = phosphor_core::digital::DigitalRequest {
                frame,
                capture_size: phosphor_core::pipeline::capture_size_for(w, h, Self::downscale_setting(params)?),
                source_fps: Self::fps(in_data),
                settings_at: &settings_at,
            };
            let mut earlier = state.pipeline.digital.frames_needed(&dreq);
            earlier.retain(|&f| f >= 0);
            if earlier.len() > MAX_EARLIER_FRAMES {
                earlier.drain(..earlier.len() - MAX_EARLIER_FRAMES);
            }
            for (i, f) in earlier.iter().enumerate() {
                extra.callbacks().checkout_layer(0, EARLIER_BASE + i as i32, &req, Self::frame_time(in_data, *f), in_data.time_step(), in_data.time_scale())?;
            }
            data.earlier = earlier;
        }
        extra.set_pre_render_data(data);
        Ok(())
    }

    fn smart_render(&self, id: u64, in_data: &InData, extra: SmartRenderExtra, params: &mut Parameters<ParamID>) -> Result<(), AnyError> {
        let Some(input_world) = extra.callbacks().checkout_layer_pixels(0)? else { return Ok(()) };
        let Some(mut output_world) = extra.callbacks().checkout_output()? else { return Ok(()) };
        let (ow, oh) = (output_world.width(), output_world.height());
        let input = pixels::read_layer(&input_world, ow, oh, input_world.pixel_format()?.into())?;
        let data: PreData = extra.pre_render_data::<PreData>().cloned().unwrap_or_default();
        let fetch = |list: &[i64], base: i32, f: i64| -> Option<Image> {
            let i = list.iter().position(|&e| e == f)?;
            let world = extra.callbacks().checkout_layer_pixels((base + i as i32) as u32).ok()??;
            let fmt = world.pixel_format().ok()?.into();
            pixels::read_layer(&world, ow, oh, fmt).ok()
        };
        let earlier = |f: i64| fetch(&data.earlier, EARLIER_BASE, f);
        let scene = |f: i64| fetch(&data.scenes, SCENE_BASE, f);
        let out = self.render_frame(id, in_data, params, &input, ow, oh, &earlier, &scene)?;
        let fmt = output_world.pixel_format()?.into();
        pixels::write_layer(&mut output_world, &out, fmt)?;
        Ok(())
    }

    fn legacy_render(&self, id: u64, in_data: &InData, in_layer: &Layer, out_layer: &mut Layer, params: &mut Parameters<ParamID>) -> Result<(), AnyError> {
        if !in_data.is_premiere() {
            return Ok(());
        }
        let (ow, oh) = (out_layer.width(), out_layer.height());
        let input = pixels::read_layer(in_layer, ow, oh, in_layer.pr_pixel_format()?.into())?;
        let out = self.render_frame(id, in_data, params, &input, ow, oh, &|_| None, &|_| None)?;
        let fmt = out_layer.pr_pixel_format()?.into();
        pixels::write_layer(out_layer, &out, fmt)?;
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    fn render_frame(
        &self,
        id: u64,
        in_data: &InData,
        params: &mut Parameters<ParamID>,
        input: &Image,
        ow: usize,
        oh: usize,
        earlier: &dyn Fn(i64) -> Option<Image>,
        scene_at: &dyn Fn(i64) -> Option<Image>,
    ) -> Result<Image, AnyError> {
        let params: &Parameters<ParamID> = params;
        let frame = in_data.current_frame().round() as i64;
        let state = self.state(id);
        let fps = Self::fps(in_data);

        let ntsc_on = checked(params, ParamID::NtscOn)?;
        let ntsc_at = |time: Option<i32>| -> Result<Option<NtscEffect>, Error> {
            if !ntsc_on {
                return Ok(None);
            }
            Ok(Some((&NtscParams::apply_settings(&self.ntsc_list.setting_descriptors, params, time)?).into()))
        };
        let ntsc_scale = |e: &NtscEffect| -> [f32; 2] {
            if e.scale.as_ref().is_some_and(|s| s.scale_with_video_size) {
                [1.0, 1.0]
            } else {
                [in_data.downsample_x().into(), in_data.downsample_y().into()]
            }
        };

        let glitch_on = checked(params, ParamID::GlitchOn)?;
        let glitch_receipt = if glitch_on { Self::receipt(in_data, params, Self::glitch_ids()) } else { None };
        let glitch_now = Self::glitch_at(params, None)?;
        let glitch_at = |k: i64| -> GlitchSettings {
            if k == frame {
                return glitch_now.clone();
            }
            Self::sampled(&state.glitch, &glitch_receipt, k, || {
                Self::glitch_at(params, Some(Self::frame_time(in_data, k))).unwrap_or_else(|_| glitch_now.clone())
            })
        };

        let crt_on = checked(params, ParamID::CrtOn)?;
        let (pi, preset) = Self::selected_preset(params)?;
        let downscale = Self::downscale_setting(params)?;

        if checked(params, ParamID::LoopOn)? {
            return self.render_loop(&state, in_data, params, input, ow, oh, scene_at, &ntsc_at, &ntsc_scale, glitch_on.then_some(&glitch_at as &dyn Fn(i64) -> GlitchSettings), crt_on.then_some((pi, preset)), downscale);
        }

        let digital_on = checked(params, ParamID::DigitalOn)?;
        let digital_receipt = if digital_on { Self::receipt(in_data, params, Self::digital_ids()) } else { None };
        let digital_now = Self::digital_at(params, None)?;
        let digital_at = |k: i64| -> DigitalSettings {
            if k == frame {
                return digital_now.clone();
            }
            Self::sampled(&state.digital, &digital_receipt, k, || {
                Self::digital_at(params, Some(Self::frame_time(in_data, k))).unwrap_or_else(|_| digital_now.clone())
            })
        };

        let ntsc_effect = ntsc_at(None)?;
        let crt_values = Self::crt_values(params, pi, None)?;
        let req = FrameRequest {
            frame,
            fps,
            input,
            out_width: ow,
            out_height: oh,
            digital: digital_on.then_some(DigitalStage { settings_at: &digital_at, earlier }),
            ntsc: ntsc_effect.as_ref().map(|effect| NtscStage { effect, scale: ntsc_scale(effect) }),
            downscale,
            glitch: glitch_on.then_some(&glitch_at as &dyn Fn(i64) -> GlitchSettings),
            crt: crt_on.then_some(CrtStage { preset, values: &crt_values }),
        };
        state.pipeline.render(&req).map_err(|e| e.to_string().into())
    }

    #[allow(clippy::too_many_arguments)]
    fn render_loop(
        &self,
        state: &InstanceState,
        in_data: &InData,
        params: &Parameters<ParamID>,
        input: &Image,
        ow: usize,
        oh: usize,
        scene_at: &dyn Fn(i64) -> Option<Image>,
        ntsc_at: &dyn Fn(Option<i32>) -> Result<Option<NtscEffect>, Error>,
        ntsc_scale: &dyn Fn(&NtscEffect) -> [f32; 2],
        glitch: Option<&dyn Fn(i64) -> GlitchSettings>,
        crt: Option<(usize, &'static CrtPreset)>,
        downscale: Option<(usize, DownscaleMethod)>,
    ) -> Result<Image, AnyError> {
        let n = in_data.current_frame().round() as i64;
        let fps = Self::fps(in_data);
        let length = Self::layer_seconds(in_data);
        let (w, h) = (input.width, input.height);
        let key = Self::loop_key(in_data, params, w, h);
        let chain_size = Self::chain_size(w, h, downscale);

        let loop_gpu = params.get(ParamID::LoopRenderOn)?.as_popup()?.value() == 1;
        let mut cache = state.screen_loop.lock().unwrap();
        let plan = cache.plan(key, n);

        // One pass at frame k, with the settings in force then.
        let run_pass = |l: &mut HowlLoop, k: i64, run_up: bool, out: Option<(usize, usize)>| -> Result<Option<Image>, AnyError> {
            let time = Some(Self::frame_time(in_data, k));
            let settings = Self::howl_at(params, time, w as f64, h as f64)?;
            let effect = ntsc_at(time)?;
            let ntsc = effect.as_ref().map(|e| NtscStage { effect: e, scale: ntsc_scale(e) });
            let stages = ChainStages { ntsc: ntsc.as_ref(), downscale, glitch, fps };
            let crt_values = match crt {
                Some((pi, _)) => Self::crt_values(params, pi, time)?,
                None => Vec::new(),
            };
            let crt_stage = crt.map(|(_, preset)| CrtStage { preset, values: &crt_values });
            let owned_scene;
            let scene = if k == n {
                input
            } else {
                owned_scene = scene_at(k).unwrap_or_else(|| input.clone());
                &owned_scene
            };
            let pass = LoopPass {
                settings: &settings,
                scene,
                time: k as f64 / fps,
                length,
                stages: &stages,
                crt: crt_stage.as_ref(),
                signal_frame: k + 1,
                glitch_frame: k,
                gpu: loop_gpu,
            };
            if run_up {
                // The run-up only fills the tunnel's depths, where the copies are tiny: run it on a
                // smaller picture when the Downscale sets the chain's size anyway.
                let small;
                let pass = if downscale.is_some() && scene.width > RUN_UP_WIDTH {
                    small = downscale_image(scene, DownscaleSpec::for_source(RUN_UP_WIDTH, scene.width, scene.height, DownscaleMethod::Area));
                    LoopPass { scene: &small, ..pass }
                } else {
                    pass
                };
                let mut progress = |done: usize, total: usize| -> Result<(), phosphor_core::pipeline::Error> {
                    in_data.interact().progress(done as i32, total as i32 + 1).map_err(|_| CANCELLED.into())
                };
                state.pipeline.loop_run_up(l, &pass, fps, &mut progress).map_err(|e| e.to_string())?;
                return Ok(None);
            }
            Ok(state.pipeline.loop_pass(l, &pass, out).map_err(|e| e.to_string())?)
        };

        let mut l = match plan.resume.and_then(|k| cache.state(k)) {
            Some(l) => l,
            None => {
                let settings = Self::howl_at(params, Some(Self::frame_time(in_data, plan.start)), w as f64, h as f64)?;
                let mut l = HowlLoop::new(&settings, chain_size);
                run_pass(&mut l, plan.start, true, None)?;
                cache.store(plan.start - 1, n, &l);
                l
            }
        };
        let resumed_at = plan.resume.map_or(plan.start - 1, |k| k);
        let mut out = None;
        for k in (resumed_at + 1)..=n {
            if in_data.interact().progress((k - resumed_at) as i32, (n - resumed_at) as i32 + 1).is_err() {
                return Err(CANCELLED.into());
            }
            let target = (k == n).then_some((ow, oh));
            out = run_pass(&mut l, k, false, target)?;
            cache.store(k, n, &l);
        }
        out.ok_or_else(|| "Screen Loop produced no frame".into())
    }
}

// MARK: - UI

impl Plugin {
    fn set_hidden(&self, in_data: &InData, params: &Parameters<ParamID>, ids: &[ParamID], hidden: bool) {
        let Some(plugin_id) = self.aegp_id else { return };
        let run = || -> Result<(), Error> {
            let effect = ae::aegp::suites::PFInterface::new()?.new_effect_for_effect(in_data.effect_ref(), plugin_id)?;
            let streams = ae::aegp::suites::Stream::new()?;
            let dynamic = ae::aegp::suites::DynamicStream::new()?;
            for id in ids {
                let Some(index) = params.index(*id) else { continue };
                let stream = streams.new_effect_stream_by_index(&effect, plugin_id, index as i32)?;
                dynamic.set_dynamic_stream_flag(&stream, ae::aegp::DynamicStreamFlags::Hidden, false, hidden)?;
            }
            Ok(())
        };
        let _ = run();
    }

    fn set_disabled(params: &mut Parameters<ParamID>, id: ParamID, disabled: bool) -> Result<(), Error> {
        if let Ok(p) = params.get(id) {
            if p.ui_flags().contains(ParamUIFlags::DISABLED) != disabled {
                let mut p = p.clone();
                p.set_ui_flag(ParamUIFlags::DISABLED, disabled);
                p.update_param_ui()?;
            }
        }
        Ok(())
    }

    fn update_ui(&self, in_data: &InData, params: &mut Parameters<ParamID>) -> Result<(), Error> {
        let ntsc_on = checked(params, ParamID::NtscOn)?;
        NtscParams::update_controls_disabled(params, &self.ntsc_list.setting_descriptors, ntsc_on)?;
        for id in [ParamID::NtscLoad, ParamID::NtscSave] {
            Self::set_disabled(params, id, !ntsc_on)?;
        }

        let loop_on = checked(params, ParamID::LoopOn)?;
        // NTSCRT doesn't run the Digital video stage under a Screen Loop.
        let digital_on = checked(params, ParamID::DigitalOn)? && !loop_on;
        Self::set_disabled(params, ParamID::DigitalFormat, !digital_on)?;
        for i in 0..DIGITAL_PARAMS.len() {
            Self::set_disabled(params, ParamID::Digital(i as u8), !digital_on)?;
        }
        let downscale_on = checked(params, ParamID::DownscaleOn)?;
        for id in [ParamID::DownscalePreset, ParamID::DownscaleWidth, ParamID::DownscaleMethod] {
            Self::set_disabled(params, id, !downscale_on)?;
        }
        let glitch_on = checked(params, ParamID::GlitchOn)?;
        for i in 0..GLITCH_PARAMS.len() {
            Self::set_disabled(params, ParamID::Glitch(i as u8), !glitch_on)?;
        }
        for id in [ParamID::LoopPreset, ParamID::LoopRenderOn, ParamID::LoopCenter, ParamID::LoopDriftTo] {
            Self::set_disabled(params, id, !loop_on)?;
        }
        for (i, p) in HOWL_PARAMS.iter().enumerate() {
            if !matches!(p.kind, HowlKind::Position { .. }) {
                Self::set_disabled(params, ParamID::Loop(i as u8), !loop_on)?;
            }
        }

        // CRT: show only the selected shader's controls, and gray out ones its other values switch off.
        let crt_on = checked(params, ParamID::CrtOn)?;
        Self::set_disabled(params, ParamID::CrtShader, !crt_on)?;
        let (selected, _) = Self::selected_preset(params)?;
        let input_height = match Self::downscale_setting(params)? {
            Some((w, m)) => DownscaleSpec::for_source(w, in_data.width().max(1) as usize, in_data.height().max(1) as usize, m).height,
            None => in_data.height().max(1) as usize,
        };
        for (pi, preset) in CRT_PRESETS.iter().enumerate() {
            if !in_data.is_premiere() {
                self.set_hidden(in_data, params, &[ParamID::GroupStart(GROUP_CRT_PRESET + pi as u32)], pi != selected);
            }
            if pi != selected {
                continue;
            }
            let values = Self::crt_values(params, pi, None)?;
            let value_of = |n: &str| values.iter().find(|(k, _)| *k == n).map(|(_, v)| *v);
            for (i, p) in preset.params.iter().enumerate() {
                if p.min >= p.max {
                    continue;
                }
                let active = crt_on && p.active(value_of, input_height);
                Self::set_disabled(params, ParamID::Crt(pi as u8, i as u8), !active)?;
            }
        }
        Ok(())
    }

    fn window_handle(in_data: &InData) -> Option<WindowAndDisplayHandle> {
        let hwnd = if in_data.is_premiere() {
            premiere::suites::Window::new().ok()?.get_main_window() as usize as isize
        } else {
            ae::aegp::suites::Utility::new().ok()?.main_hwnd().ok()? as usize as isize
        };
        NonZero::<isize>::new(hwnd).map(|h| unsafe { WindowAndDisplayHandle::new(Win32WindowHandle::new(h)) })
    }

    fn pick_json(in_data: &InData, filter: &str) -> Option<PathBuf> {
        let mut dialog = rfd::FileDialog::new().add_filter(filter, &["json"]);
        if let Some(h) = Self::window_handle(in_data) {
            dialog = dialog.set_parent(&h);
        }
        dialog.pick_file()
    }

    fn set(params: &mut Parameters<ParamID>, id: ParamID, v: ControlValue) -> Result<(), Error> {
        let mut p = params.get_mut(id)?;
        match v {
            ControlValue::Slider(x) => {
                let mut s = p.as_float_slider_mut()?;
                s.set_value(x);
                s.set_value_changed();
            }
            ControlValue::Checkbox(b) => {
                let mut s = p.as_checkbox_mut()?;
                s.set_value(b);
                s.set_value_changed();
            }
            ControlValue::Popup(i) => {
                let mut s = p.as_popup_mut()?;
                s.set_value(i);
                s.set_value_changed();
            }
        }
        Ok(())
    }

    fn set_point(params: &mut Parameters<ParamID>, id: ParamID, v: (f32, f32)) -> Result<(), Error> {
        let mut p = params.get_mut(id)?;
        let mut s = p.as_point_mut()?;
        s.set_value(v);
        s.set_value_changed();
        Ok(())
    }

    fn digital_values(s: &DigitalSettings, out: &mut Vec<(ParamID, ControlValue)>) {
        for (i, p) in DIGITAL_PARAMS.iter().enumerate() {
            let v = s.get(p.id);
            out.push((
                ParamID::Digital(i as u8),
                match p.kind {
                    DigitalKind::Choice(_) => ControlValue::Popup(v.round() as i32 + 1),
                    DigitalKind::Slider { percent, .. } => ControlValue::Slider(v * if percent { 100.0 } else { 1.0 }),
                },
            ));
        }
    }

    fn glitch_values(s: &GlitchSettings, out: &mut Vec<(ParamID, ControlValue)>) {
        for (i, p) in GLITCH_PARAMS.iter().enumerate() {
            let v = s.get(p.id);
            out.push((
                ParamID::Glitch(i as u8),
                match p.kind {
                    GlitchKind::Toggle => ControlValue::Checkbox(v >= 0.5),
                    GlitchKind::Slider { percent, .. } => ControlValue::Slider(v * if percent { 100.0 } else { 1.0 }),
                },
            ));
        }
    }

    fn shader_values(pi: usize, values: &[(String, f32)], out: &mut Vec<(ParamID, ControlValue)>) {
        for (i, p) in CRT_PRESETS[pi].params.iter().enumerate() {
            if p.min >= p.max {
                continue;
            }
            let v = values.iter().find(|(n, _)| n == p.name).map_or(p.initial, |(_, v)| *v);
            out.push((ParamID::Crt(pi as u8, i as u8), ControlValue::Slider(v.clamp(p.min, p.max) as f64)));
        }
    }

    fn downscale_values(width: usize, out: &mut Vec<(ParamID, ControlValue)>) {
        out.push((ParamID::DownscaleWidth, ControlValue::Slider(width as f64)));
        let preset = DOWNSCALE_PRESETS.iter().position(|p| p.1 == width).unwrap_or(DOWNSCALE_PRESETS.len() - 1);
        out.push((ParamID::DownscalePreset, ControlValue::Popup(preset as i32 + 1)));
    }

    /// Every animatable control a look sets.
    fn look_controls(&self) -> Vec<ParamID> {
        let mut ids: Vec<ParamID> = Vec::new();
        ids.extend((0..DIGITAL_PARAMS.len()).map(|i| ParamID::Digital(i as u8)));
        ids.extend((0..GLITCH_PARAMS.len()).map(|i| ParamID::Glitch(i as u8)));
        let mut ntsc = Vec::new();
        NtscParams::stream_values(&self.ntsc_list.setting_descriptors, &self.house_ntsc, &mut ntsc);
        ids.extend(ntsc.into_iter().map(|(id, _)| id));
        for (pi, preset) in CRT_PRESETS.iter().enumerate() {
            ids.extend((0..preset.params.len()).filter(|&i| preset.params[i].min < preset.params[i].max).map(|i| ParamID::Crt(pi as u8, i as u8)));
        }
        ids
    }

    fn apply_look(&self, in_data: &InData, params: &mut Parameters<ParamID>, look: &Look) -> Result<(), Error> {
        let mut base: Vec<(ParamID, ControlValue)> = Vec::new();
        let shader_index = look.shader.as_ref().and_then(|(_, id, _)| CRT_PRESETS.iter().position(|p| p.id == id));

        base.push((ParamID::DigitalOn, ControlValue::Checkbox(look.digital.as_ref().is_some_and(|(on, _)| *on))));
        if let Some((_, s)) = &look.digital {
            base.push((ParamID::DigitalFormat, ControlValue::Popup(1)));
            Self::digital_values(s, &mut base);
        }
        if let Some((on, s)) = &look.ntsc {
            base.push((ParamID::NtscOn, ControlValue::Checkbox(*on)));
            NtscParams::stream_values(&self.ntsc_list.setting_descriptors, s, &mut base);
        }
        if let Some((on, width, method)) = look.downscale {
            base.push((ParamID::DownscaleOn, ControlValue::Checkbox(on)));
            Self::downscale_values(width, &mut base);
            base.push((ParamID::DownscaleMethod, ControlValue::Popup(if method == DownscaleMethod::Area { 2 } else { 1 })));
        }
        if let Some((on, s)) = &look.glitch {
            base.push((ParamID::GlitchOn, ControlValue::Checkbox(*on)));
            Self::glitch_values(s, &mut base);
        }
        if let Some((on, _, values)) = &look.shader {
            base.push((ParamID::CrtOn, ControlValue::Checkbox(*on)));
            if let Some(pi) = shader_index {
                base.push((ParamID::CrtShader, ControlValue::Popup(pi as i32 + 1)));
                Self::shader_values(pi, values, &mut base);
            }
        }

        // A new look replaces the previous look's animation.
        let plugin_id = self.aegp_id.filter(|_| !in_data.is_premiere());
        if let Some(plugin_id) = plugin_id {
            keyframes::clear(in_data, params, plugin_id, &self.look_controls())?;
        }
        for (id, v) in &base {
            Self::set(params, *id, *v)?;
        }
        let Some(plugin_id) = plugin_id.filter(|_| look.keys.len() >= 2) else { return Ok(()) };

        // One snapshot per look keyframe, then a track for each control whose value changes.
        let snapshots: Vec<Vec<(ParamID, ControlValue)>> = look
            .keys
            .iter()
            .map(|k| {
                let mut v = Vec::new();
                if look.digital.is_some() {
                    let mut d = Vec::new();
                    Self::digital_values(&k.digital, &mut d);
                    // The stream's structure holds for a whole render.
                    d.retain(|(id, _)| !matches!(id, ParamID::Digital(i) if STRUCTURE_IDS.contains(&DIGITAL_PARAMS[*i as usize].id)));
                    v.extend(d);
                }
                if look.ntsc.is_some() {
                    NtscParams::stream_values(&self.ntsc_list.setting_descriptors, &k.ntsc, &mut v);
                }
                if look.glitch.is_some() {
                    Self::glitch_values(&k.glitch, &mut v);
                }
                if let Some(pi) = shader_index {
                    Self::shader_values(pi, &k.shader, &mut v);
                }
                v
            })
            .collect();
        let tracks: Vec<keyframes::Track> = snapshots[0]
            .iter()
            .enumerate()
            .filter_map(|(i, (id, first))| {
                let values: Vec<ControlValue> = snapshots.iter().map(|s| s[i].1).collect();
                values.iter().any(|v| v != first).then_some(keyframes::Track { id: *id, values })
            })
            .collect();
        let times: Vec<f64> = look.keys.iter().map(|k| k.time).collect();
        let easings: Vec<_> = look.keys.iter().map(|k| k.easing).collect();
        keyframes::write(in_data, params, plugin_id, &times, &easings, &tracks)?;
        log::line(&format!("look: {} keyframes on {} controls", times.len(), tracks.len()));
        Ok(())
    }

    fn apply_screen_loop(in_data: &InData, params: &mut Parameters<ParamID>, s: &HowlSettings) -> Result<(), Error> {
        let (w, h) = (in_data.width().max(1) as f64, in_data.height().max(1) as f64);
        for (i, p) in HOWL_PARAMS.iter().enumerate() {
            let v = s.get(p.id);
            match p.kind {
                HowlKind::Position { .. } => {}
                HowlKind::Toggle => Self::set(params, ParamID::Loop(i as u8), ControlValue::Checkbox(v >= 0.5))?,
                HowlKind::Slider { percent, .. } => Self::set(params, ParamID::Loop(i as u8), ControlValue::Slider(v * if percent { 100.0 } else { 1.0 }))?,
            }
        }
        let (cx, cy) = (s.get("center_x") + 0.5, s.get("center_y") + 0.5);
        Self::set_point(params, ParamID::LoopCenter, ((cx * w) as f32, (cy * h) as f32))?;
        Self::set_point(params, ParamID::LoopDriftTo, (((cx + s.get("drift_x")) * w) as f32, ((cy + s.get("drift_y")) * h) as f32))?;
        Self::set(params, ParamID::LoopOn, ControlValue::Checkbox(true))
    }

    fn load_file(&self, in_data: &InData, out_data: &mut OutData, params: &mut Parameters<ParamID>) -> Result<(), Error> {
        let Some(path) = Self::pick_json(in_data, "NTSCRT look, Screen Loop or ntsc-rs preset") else { return Ok(()) };
        let text = match std::fs::read_to_string(&path) {
            Ok(t) => t,
            Err(e) => {
                out_data.set_error_msg(&format!("Couldn't read {}: {e}", path.display()));
                return Ok(());
            }
        };
        if text.contains(look::SCREEN_LOOP_KIND) {
            match parse_screen_loop(&text) {
                Ok(s) => {
                    Self::set(params, ParamID::LoopPreset, ControlValue::Popup(1))?;
                    Self::apply_screen_loop(in_data, params, &s)?;
                }
                Err(e) => out_data.set_error_msg(&format!("Couldn't load the Screen Loop preset: {e}")),
            }
            return Ok(());
        }
        match Look::parse(&text) {
            Ok(l) => {
                Self::set(params, ParamID::LookMenu, ControlValue::Popup(1))?;
                self.apply_look(in_data, params, &l)?;
            }
            Err(e) => out_data.set_error_msg(&format!("Couldn't load the look: {e}")),
        }
        Ok(())
    }

    fn save_json(in_data: &InData, out_data: &mut OutData, name: &str, text: &str) {
        let mut dialog = rfd::FileDialog::new().add_filter("JSON", &["json"]).set_file_name(name);
        if let Some(h) = Self::window_handle(in_data) {
            dialog = dialog.set_parent(&h);
        }
        let Some(path) = dialog.save_file() else { return };
        if let Err(e) = std::fs::write(&path, text) {
            out_data.set_error_msg(&format!("Couldn't save {}: {e}", path.display()));
        }
    }

    /// The look at the current time, in NTSCRT's file format.
    fn save_look(&self, in_data: &InData, out_data: &mut OutData, params: &mut Parameters<ParamID>) -> Result<(), Error> {
        let params: &Parameters<ParamID> = params;
        let digital = Self::digital_at(params, None)?;
        let ntsc = NtscParams::apply_settings(&self.ntsc_list.setting_descriptors, params, None)?;
        let glitch = Self::glitch_at(params, None)?;
        let (pi, preset) = Self::selected_preset(params)?;
        let shader = Self::crt_values(params, pi, None)?;
        let width = params.get(ParamID::DownscaleWidth)?.as_float_slider()?.value().round().max(16.0) as usize;
        let method = if params.get(ParamID::DownscaleMethod)?.as_popup()?.value() == 2 { DownscaleMethod::Area } else { DownscaleMethod::Nearest };
        let out = LookOut {
            digital: (checked(params, ParamID::DigitalOn)?, &digital),
            ntsc: (checked(params, ParamID::NtscOn)?, &ntsc),
            downscale: (checked(params, ParamID::DownscaleOn)?, width, method),
            glitch: (checked(params, ParamID::GlitchOn)?, &glitch),
            shader: (checked(params, ParamID::CrtOn)?, preset.id, &shader),
        };
        Self::save_json(in_data, out_data, "Phosphor look.json", &out.to_json());
        Ok(())
    }

    fn param_changed(&self, in_data: &InData, out_data: &mut OutData, params: &mut Parameters<ParamID>, index: usize) -> Result<(), Error> {
        match params.type_at(index) {
            ParamID::LookMenu => {
                let i = params.get(ParamID::LookMenu)?.as_popup()?.value() - 2;
                if let Some((name, json)) = usize::try_from(i).ok().and_then(|i| LOOKS.get(i)) {
                    match Look::parse(json) {
                        Ok(l) => self.apply_look(in_data, params, &l)?,
                        Err(e) => out_data.set_error_msg(&format!("Couldn't load {name}: {e}")),
                    }
                }
            }
            ParamID::LoadLook => self.load_file(in_data, out_data, params)?,
            ParamID::SaveLook => self.save_look(in_data, out_data, params)?,
            ParamID::LoopSave => {
                let s = Self::howl_at(params, None, in_data.width() as f64, in_data.height() as f64)?;
                Self::save_json(in_data, out_data, "Screen loop.json", &screen_loop_to_json(&s));
            }
            ParamID::LoopPreset => {
                let i = params.get(ParamID::LoopPreset)?.as_popup()?.value() - 2;
                if let Some((_, json)) = usize::try_from(i).ok().and_then(|i| SCREEN_LOOPS.get(i)) {
                    if let Ok(s) = parse_screen_loop(json) {
                        Self::apply_screen_loop(in_data, params, &s)?;
                    }
                }
            }
            ParamID::NtscLoad => {
                let Some(path) = Self::pick_json(in_data, "ntsc-rs preset") else { return Ok(()) };
                match std::fs::read_to_string(&path).map_err(|e| e.to_string()).and_then(|s| self.ntsc_list.from_json(&s).map_err(|e| e.to_string())) {
                    Ok(s) => {
                        let mut v = Vec::new();
                        NtscParams::stream_values(&self.ntsc_list.setting_descriptors, &s, &mut v);
                        for (id, x) in v {
                            Self::set(params, id, x)?;
                        }
                    }
                    Err(e) => out_data.set_error_msg(&format!("Couldn't load the preset: {e}")),
                }
            }
            ParamID::NtscSave => {
                let mut dialog = rfd::FileDialog::new().add_filter("ntsc-rs preset", &["json"]).set_file_name("settings.json");
                if let Some(h) = Self::window_handle(in_data) {
                    dialog = dialog.set_parent(&h);
                }
                let Some(path) = dialog.save_file() else { return Ok(()) };
                let settings = NtscParams::apply_settings(&self.ntsc_list.setting_descriptors, params, None)?;
                let res = std::fs::File::create(path)
                    .map_err(|e| e.to_string())
                    .and_then(|mut f| self.ntsc_list.write_json_to_io(&settings, &mut f).map_err(|e| e.to_string()));
                if let Err(e) = res {
                    out_data.set_error_msg(&format!("Couldn't save the preset: {e}"));
                }
            }
            ParamID::DigitalFormat => {
                let i = params.get(ParamID::DigitalFormat)?.as_popup()?.value() - 2;
                if let Some(f) = usize::try_from(i).ok().and_then(|i| DIGITAL_FORMATS.get(i)) {
                    let mut v = Vec::new();
                    Self::digital_values(&f.settings(), &mut v);
                    v.push((ParamID::DownscaleOn, ControlValue::Checkbox(true)));
                    Self::downscale_values(f.width, &mut v);
                    for (id, x) in v {
                        Self::set(params, id, x)?;
                    }
                }
            }
            ParamID::DownscalePreset => {
                let i = (params.get(ParamID::DownscalePreset)?.as_popup()?.value() - 1).max(0) as usize;
                if let Some(&(_, w)) = DOWNSCALE_PRESETS.get(i) {
                    if w > 0 {
                        Self::set(params, ParamID::DownscaleWidth, ControlValue::Slider(w as f64))?;
                    }
                }
            }
            ParamID::DownscaleWidth => {
                let w = params.get(ParamID::DownscaleWidth)?.as_float_slider()?.value().round() as usize;
                let preset = DOWNSCALE_PRESETS.iter().position(|p| p.1 == w).unwrap_or(DOWNSCALE_PRESETS.len() - 1);
                Self::set(params, ParamID::DownscalePreset, ControlValue::Popup(preset as i32 + 1))?;
            }
            ParamID::DigitalOn => {
                // Like NTSCRT: switching the stage on for the first time picks its format's size.
                if checked(params, ParamID::DigitalOn)? && params.get(ParamID::DigitalFormat)?.as_popup()?.value() >= 2 {
                    let i = (params.get(ParamID::DigitalFormat)?.as_popup()?.value() - 2) as usize;
                    if let Some(f) = DIGITAL_FORMATS.get(i) {
                        let mut v = vec![(ParamID::DownscaleOn, ControlValue::Checkbox(true))];
                        Self::downscale_values(f.width, &mut v);
                        for (id, x) in v {
                            Self::set(params, id, x)?;
                        }
                    }
                }
            }
            _ => {}
        }
        Ok(())
    }
}
