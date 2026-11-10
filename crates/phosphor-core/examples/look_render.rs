//! Renders one frame of a still through the full pipeline with an NTSCRT look.
//!
//! cargo run -p phosphor-core --release --example look_render -- <shaders root> <input.png> <look.json> <out.png> [frame] [out_w out_h]

use std::path::Path;

use ntsc_rs::NtscEffect;
use phosphor_core::crt::{CrtPreset, DEFAULT_PRESET};
use phosphor_core::look::Look;
use phosphor_core::pipeline::{CrtStage, DigitalStage, FrameRequest, NtscStage, Pipeline};
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
        *p = if ch >= 3 { [g(0), g(1), g(2), 1.0] } else { [g(0), g(0), g(0), 1.0] };
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
    let input = read_png(Path::new(&a[2]));
    let look = Look::parse(&std::fs::read_to_string(&a[3]).unwrap()).unwrap();
    let frame: i64 = a.get(5).map_or(0, |s| s.parse().unwrap());
    let (ow, oh) = match (a.get(6), a.get(7)) {
        (Some(w), Some(h)) => (w.parse().unwrap(), h.parse().unwrap()),
        _ => (input.width, input.height),
    };

    let pipeline = Pipeline::new(&a[1]);
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

    let digital_at = |_: i64| digital.clone().unwrap();
    let earlier = |_: i64| Some(input.clone());
    let glitch_at = |_: i64| glitch.clone().unwrap();

    let req = FrameRequest {
        frame,
        fps: look.timeline_fps,
        input: &input,
        out_width: ow,
        out_height: oh,
        digital: digital.as_ref().map(|_| DigitalStage {
            settings_at: &digital_at,
            earlier: &earlier,
        }),
        ntsc: ntsc_effect.as_ref().map(|effect| NtscStage {
            effect,
            scale: [1.0, 1.0],
        }),
        downscale: look.downscale.filter(|(on, _, _)| *on).map(|(_, w, m)| (w, m)),
        glitch: glitch.as_ref().map(|_| &glitch_at as &dyn Fn(i64) -> _),
        crt: shader.as_ref().map(|_| CrtStage {
            preset,
            values: &crt_refs,
        }),
    };
    let t = std::time::Instant::now();
    let out = pipeline.render(&req).unwrap();
    println!("{} -> {}x{} in {:.0} ms", a[3], out.width, out.height, t.elapsed().as_secs_f64() * 1000.0);
    write_png(Path::new(&a[4]), &out);
}
