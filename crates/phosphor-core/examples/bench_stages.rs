//! Times each stage of the pipeline on a synthetic frame.
//!
//! cargo run -p phosphor-core --release --example bench_stages -- <shaders root> [width height]

use std::time::Instant;

use ntsc_rs::NtscEffect;
use phosphor_core::crt::{CrtPreset, CrtRenderer};
use phosphor_core::digital::{self, DigitalRenderer, DigitalRequest, DIGITAL_FORMATS};
use phosphor_core::downscale::{downscale, DownscaleMethod, DownscaleSpec};
use phosphor_core::glitch::{GlitchRenderer, GlitchSettings};
use phosphor_core::howlaround::{HowlLoop, HowlSettings};
use phosphor_core::look::house_ntsc;
use phosphor_core::pipeline::{apply_ntsc, ChainStages, CrtStage, LoopPass, NtscStage, Pipeline};
use phosphor_core::Image;

fn frame(w: usize, h: usize, t: usize) -> Image {
    let mut img = Image::new(w, h);
    for y in 0..h {
        for x in 0..w {
            let v = (((x * 7 + y * 3 + t * 11) % 256) as f32) / 255.0;
            img.pixels[y * w + x] = [v, (x as f32 / w as f32), (y as f32 / h as f32), 1.0];
        }
    }
    img
}

/// Median of `n` runs, in ms.
fn time(n: usize, mut f: impl FnMut(usize)) -> f64 {
    f(0);
    let mut v: Vec<f64> = (1..=n)
        .map(|i| {
            let t = Instant::now();
            f(i);
            t.elapsed().as_secs_f64() * 1000.0
        })
        .collect();
    v.sort_by(|a, b| a.total_cmp(b));
    v[v.len() / 2]
}

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let (w, h) = match (a.get(2), a.get(3)) {
        (Some(w), Some(h)) => (w.parse().unwrap(), h.parse().unwrap()),
        _ => (1920usize, 1080usize),
    };
    let n = 9;
    println!("frame {w}x{h}, {} threads, median of {n}\n", rayon::current_num_threads());
    let src = frame(w, h, 0);
    let ds = DownscaleSpec::for_source(320, w, h, DownscaleMethod::Nearest);
    let small = downscale(&src, ds);
    let effect: NtscEffect = (&house_ntsc()).into();

    let row = |name: &str, ms: f64| println!("{name:<44} {ms:>8.1} ms");

    // Pixel format conversion as the plugin does it (f32 RGBA <-> 8-bit), for scale.
    row("copy frame (f32 RGBA clone)", time(n, |_| {
        std::hint::black_box(src.clone());
    }));

    let webcam = DIGITAL_FORMATS[0].settings();
    let mut video_call = DIGITAL_FORMATS[1].settings();
    video_call.set("frame_rate", 30.0);
    let dr = DigitalRenderer::new();
    row("Digital Video - Webcam (intra, 320 capture)", time(n, |i| {
        let req = DigitalRequest { frame: i as i64, capture_size: (ds.width, ds.height), source_fps: 30.0, settings_at: &|_| webcam.clone() };
        std::hint::black_box(dr.render(&req, &src, &|_| None));
    }));
    let dr2 = DigitalRenderer::new();
    row("Digital Video - Video call (predicted frame)", time(n, |i| {
        let req = DigitalRequest { frame: 1 + i as i64, capture_size: (ds.width, ds.height), source_fps: 30.0, settings_at: &|_| video_call.clone() };
        std::hint::black_box(dr2.render(&req, &src, &|f| Some(frame(w, h, f as usize))));
    }));
    let _ = digital::capture_size;

    row("NTSC (ntsc-rs, house look) full res", time(n, |i| {
        std::hint::black_box(apply_ntsc(&src, &effect, i as i64, [1.0, 1.0]));
    }));
    let half = downscale(&src, DownscaleSpec { width: w / 2, height: h / 2, method: DownscaleMethod::Area });
    row("NTSC at half res (for comparison)", time(n, |i| {
        std::hint::black_box(apply_ntsc(&half, &effect, i as i64, [1.0, 1.0]));
    }));

    row("Downscale Chunky -> 320", time(n, |_| {
        std::hint::black_box(downscale(&src, ds));
    }));
    row("Downscale Smooth -> 320", time(n, |_| {
        std::hint::black_box(downscale(&src, DownscaleSpec { method: DownscaleMethod::Area, ..ds }));
    }));

    let gr = GlitchRenderer::new();
    let neutral = GlitchSettings::default();
    row("Glitch neutral (frame 150, cached history)", time(n, |_| {
        std::hint::black_box(gr.render(&small, 150, 1.0 / 30.0, &|_| neutral.clone()));
    }));
    let mut busy = GlitchSettings::default();
    busy.set("signal_strength", 0.5);
    busy.set("vertical_hold", 0.5);
    busy.set("dropouts", 0.6);
    busy.set("ghost_level", 0.4);
    let gr2 = GlitchRenderer::new();
    row("Glitch busy (frame 150, cached history)", time(n, |_| {
        std::hint::black_box(gr2.render(&small, 150, 1.0 / 30.0, &|_| busy.clone()));
    }));
    row("Glitch busy (frame 150, cold: simulate 5 s)", time(3, |_| {
        std::hint::black_box(GlitchRenderer::new().render(&small, 150, 1.0 / 30.0, &|_| busy.clone()));
    }));

    let crt = CrtRenderer::new(&a[1]);
    for id in ["glow_gauss", "royale"] {
        let p = CrtPreset::by_id(id).unwrap();
        row(&format!("CRT {id} -> {w}x{h} (upload+render+readback)"), time(n, |i| {
            std::hint::black_box(crt.render(&small, w, h, p, &[], i).unwrap());
        }));
    }

    let pipeline = Pipeline::new(&a[1]);
    let ntsc = NtscStage { effect: &effect, scale: [1.0, 1.0] };
    let stages = ChainStages { ntsc: Some(&ntsc), downscale: Some((320, DownscaleMethod::Nearest)), glitch: None, fps: 30.0 };
    let preset = CrtPreset::by_id("glow_gauss").unwrap();
    let crt_stage = CrtStage { preset, values: &[] };
    row("Whole frame: NTSC + Downscale + CRT", time(n, |i| {
        let chain = pipeline.chain_input(&src, &stages, i as i64, i as i64);
        std::hint::black_box(pipeline.finish(&chain, w, h, Some(&crt_stage), i as i64).unwrap());
    }));

    let hs = HowlSettings::default();
    let mut l = HowlLoop::new(&hs, (ds.width, ds.height));
    row("Screen Loop: camera image only", time(n, |_| {
        std::hint::black_box(l.camera_image(&src, &hs, 1.0, 5.0));
    }));
    let mut l = HowlLoop::new(&hs, (ds.width, ds.height));
    let setup = l.camera_setup(w, h, &hs, 1.0, 5.0);
    row("Screen Loop: camera on the GPU", time(n, |_| {
        std::hint::black_box(pipeline.crt.camera(&src, l.tv(), &setup).unwrap());
    }));
    for gpu in [false, true] {
    let mut l = HowlLoop::new(&hs, (ds.width, ds.height));
    row(&format!("Screen Loop: one full pass ({})", if gpu { "GPU camera" } else { "CPU camera" }), time(n, |i| {
        let pass = LoopPass { settings: &hs, scene: &src, time: i as f64 / 30.0, length: 5.0, stages: &stages, crt: Some(&crt_stage), signal_frame: i as i64, glitch_frame: i as i64, gpu };
        std::hint::black_box(pipeline.loop_pass(&mut l, &pass, Some((w, h))).unwrap());
    }));
    }
}
