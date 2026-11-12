//! Runs every bundled look and Screen Loop preset on odd frame sizes, to catch crashes that only
//! show up on hosts that ask for unusual sizes (tiny previews, thumbnails, odd regions).
//! Each case is printed before it runs, so a hard crash leaves its culprit as the last line.
//!
//! cargo run -p phosphor-core --release --example stress_sizes -- <shaders root>

use std::io::Write;
use std::panic::{catch_unwind, AssertUnwindSafe};

use ntsc_rs::NtscEffect;
use phosphor_core::bundled::LOOKS;
use phosphor_core::crt::{CrtPreset, DEFAULT_PRESET};
use phosphor_core::howlaround::HowlLoop;
use phosphor_core::look::{parse_screen_loop, Look};
use phosphor_core::pipeline::{ChainStages, CrtStage, DigitalStage, FrameRequest, LoopPass, NtscStage, Pipeline};
use phosphor_core::Image;

fn frame(w: usize, h: usize) -> Image {
    let mut img = Image::new(w, h);
    for (i, p) in img.pixels.iter_mut().enumerate() {
        let v = ((i * 37) % 251) as f32 / 250.0;
        *p = [v, 1.0 - v, (i % 7) as f32 / 6.0, 1.0];
    }
    img
}

const SIZES: &[(usize, usize)] = &[
    (1, 1), (1, 2), (2, 1), (2, 2), (3, 5), (7, 1), (1, 9), (4, 3), (16, 9), (33, 17), (320, 1),
    (1, 240), (100, 3), (3, 100), (641, 481), (1921, 1081),
];

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let pipeline = Pipeline::new(&a[1]);
    let mut failures = 0;
    let mut case = |name: String, f: &mut dyn FnMut() -> Result<(), String>| {
        print!("{name} ... ");
        let _ = std::io::stdout().flush();
        match catch_unwind(AssertUnwindSafe(|| f())) {
            Ok(Ok(())) => println!("ok"),
            Ok(Err(e)) => println!("error: {e}"),
            Err(p) => {
                failures += 1;
                let s = p.downcast_ref::<&str>().map(|s| s.to_string()).or_else(|| p.downcast_ref::<String>().cloned());
                println!("PANIC: {}", s.unwrap_or_default());
            }
        }
    };

    for (look_name, json) in LOOKS {
        let look = Look::parse(json).unwrap();
        let ntsc_effect: Option<NtscEffect> = look.ntsc.as_ref().filter(|(on, _)| *on).map(|(_, s)| s.into());
        let digital = look.digital.clone().filter(|(on, _)| *on).map(|(_, s)| s);
        let glitch = look.glitch.clone().filter(|(on, _)| *on).map(|(_, s)| s);
        let shader = look.shader.clone().filter(|(on, _, _)| *on);
        let crt_values: Vec<(String, f32)> = shader.as_ref().map(|(_, _, v)| v.clone()).unwrap_or_default();
        let crt_refs: Vec<(&str, f32)> = crt_values.iter().map(|(n, v)| (n.as_str(), *v)).collect();
        let preset = shader
            .as_ref()
            .and_then(|(_, id, _)| CrtPreset::by_id(id))
            .or_else(|| CrtPreset::by_id(DEFAULT_PRESET))
            .unwrap();
        for &(w, h) in SIZES {
            let input = frame(w, h);
            for f in [0i64, 1, 45] {
                let digital_at = |_: i64| digital.clone().unwrap();
                let earlier = |_: i64| Some(input.clone());
                let glitch_at = |_: i64| glitch.clone().unwrap();
                let req = FrameRequest {
                    frame: f,
                    fps: look.timeline_fps,
                    input: &input,
                    out_width: w,
                    out_height: h,
                    digital: digital.as_ref().map(|_| DigitalStage { settings_at: &digital_at, earlier: &earlier }),
                    ntsc: ntsc_effect.as_ref().map(|effect| NtscStage { effect, scale: [1.0, 1.0] }),
                    downscale: look.downscale.filter(|(on, _, _)| *on).map(|(_, w, m)| (w, m)),
                    glitch: glitch.as_ref().map(|_| &glitch_at as &dyn Fn(i64) -> _),
                    crt: shader.as_ref().map(|_| CrtStage { preset, values: &crt_refs }),
                };
                case(format!("look '{look_name}' {w}x{h} frame {f}"), &mut || {
                    let out = pipeline.render(&req).map_err(|e| e.to_string())?;
                    if (out.width, out.height) != (w, h) {
                        return Err(format!("output is {}x{}", out.width, out.height));
                    }
                    Ok(())
                });
            }
        }
    }

    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("looks/screen-loop");
    let effect: NtscEffect = (&phosphor_core::look::house_ntsc()).into();
    let ntsc = NtscStage { effect: &effect, scale: [1.0, 1.0] };
    let crt_preset = CrtPreset::by_id(DEFAULT_PRESET).unwrap();
    let crt = CrtStage { preset: crt_preset, values: &[] };
    for entry in std::fs::read_dir(&dir).unwrap() {
        let path = entry.unwrap().path();
        let name = path.file_name().unwrap().to_string_lossy().to_string();
        let settings = parse_screen_loop(&std::fs::read_to_string(&path).unwrap()).unwrap();
        for &(w, h) in SIZES {
            let scene = frame(w, h);
            for gpu in [true, false] {
                let stages = ChainStages { ntsc: Some(&ntsc), downscale: Some((320, phosphor_core::downscale::DownscaleMethod::Nearest)), glitch: None, fps: 30.0 };
                case(format!("screen loop '{name}' {w}x{h} gpu {gpu}"), &mut || {
                    let cap = phosphor_core::pipeline::capture_size_for(w, h, Some((320, phosphor_core::downscale::DownscaleMethod::Nearest)));
                    let mut l = HowlLoop::new(&settings, cap);
                    for i in 0..4 {
                        let pass = LoopPass {
                            settings: &settings,
                            scene: &scene,
                            time: i as f64 / 30.0,
                            length: 5.0,
                            stages: &stages,
                            crt: Some(&crt),
                            signal_frame: i,
                            glitch_frame: i,
                            gpu,
                        };
                        pipeline.loop_pass(&mut l, &pass, Some((w, h))).map_err(|e| e.to_string())?;
                    }
                    Ok(())
                });
            }
        }
    }
    println!("\ndone, {failures} panics");
}
