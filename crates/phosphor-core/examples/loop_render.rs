//! Renders the first frames of a Screen Loop on a still and writes PNGs.
//!
//! cargo run -p phosphor-core --release --example loop_render -- <shaders root> <input.png> <out dir> [frames]

use std::path::Path;

use ntsc_rs::NtscEffect;
use phosphor_core::crt::{CrtPreset, DEFAULT_PRESET};
use phosphor_core::downscale::DownscaleMethod;
use phosphor_core::howlaround::{HowlLoop, HowlSettings};
use phosphor_core::look::house_ntsc;
use phosphor_core::pipeline::{ChainStages, CrtStage, LoopPass, NtscStage, Pipeline};
use phosphor_core::Image;

fn read_png(path: &Path) -> Image {
    let decoder = png::Decoder::new(std::fs::File::open(path).unwrap());
    let mut reader = decoder.read_info().unwrap();
    let mut buf = vec![0; reader.output_buffer_size()];
    let info = reader.next_frame(&mut buf).unwrap();
    let ch = info.color_type.samples();
    let mut img = Image::new(info.width as usize, info.height as usize);
    for (i, p) in img.pixels.iter_mut().enumerate() {
        let s = &buf[i * ch..i * ch + ch];
        let g = |c: usize| s[c.min(ch - 1)] as f32 / 255.0;
        *p = [g(0), g(1), g(2), 1.0];
    }
    img
}

fn write_png(path: &Path, img: &Image) {
    let file = std::fs::File::create(path).unwrap();
    let mut enc = png::Encoder::new(std::io::BufWriter::new(file), img.width as u32, img.height as u32);
    enc.set_color(png::ColorType::Rgb);
    enc.set_depth(png::BitDepth::Eight);
    let data: Vec<u8> = img
        .pixels
        .iter()
        .flat_map(|p| [0, 1, 2].map(|c| (p[c].clamp(0.0, 1.0) * 255.0).round() as u8))
        .collect();
    enc.write_header().unwrap().write_image_data(&data).unwrap();
}

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let pipeline = Pipeline::new(&a[1]);
    let scene = read_png(Path::new(&a[2]));
    let out_dir = Path::new(&a[3]);
    std::fs::create_dir_all(out_dir).unwrap();
    let frames: i64 = a.get(4).map_or(10, |s| s.parse().unwrap());

    let mut settings = HowlSettings::default();
    settings.set("counter", 1.0);
    let effect: NtscEffect = (&house_ntsc()).into();
    let ntsc = NtscStage { effect: &effect, scale: [1.0, 1.0] };
    let stages = ChainStages { ntsc: Some(&ntsc), downscale: Some((320, DownscaleMethod::Nearest)), glitch: None, fps: 30.0 };
    let preset = CrtPreset::by_id(DEFAULT_PRESET).unwrap();
    let crt = CrtStage { preset, values: &[] };
    let chain_size = {
        let d = phosphor_core::downscale::DownscaleSpec::for_source(320, scene.width, scene.height, DownscaleMethod::Nearest);
        (d.width, d.height)
    };
    let mut l = HowlLoop::new(&settings, chain_size);
    let first = LoopPass {
        settings: &settings,
        scene: &scene,
        time: 0.0,
        length: 5.0,
        stages: &stages,
        crt: Some(&crt),
        signal_frame: 1,
        glitch_frame: 0,
        gpu: std::env::var("CPU_CAMERA").is_err(),
    };
    let t = std::time::Instant::now();
    pipeline.loop_run_up(&mut l, &first, 30.0, &mut |_, _| Ok(())).unwrap();
    println!("run-up: {:.1} s, feedback {}x{}", t.elapsed().as_secs_f64(), l.feedback_size.0, l.feedback_size.1);
    for n in 0..frames {
        let t = std::time::Instant::now();
        let pass = LoopPass { time: n as f64 / 30.0, signal_frame: n + 1, glitch_frame: n, ..first };
        let out = pipeline.loop_pass(&mut l, &pass, Some((scene.width * 2, scene.height * 2))).unwrap().unwrap();
        if n == 0 || n == frames - 1 {
            write_png(&out_dir.join(format!("loop_{n:03}.png")), &out);
        }
        println!("frame {n}: {:.0} ms", t.elapsed().as_secs_f64() * 1000.0);
    }
}
