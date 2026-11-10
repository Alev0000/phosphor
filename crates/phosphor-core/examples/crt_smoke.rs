//! Renders a test pattern through every CRT preset and writes PNGs.
//!
//! cargo run -p phosphor-core --release --example crt_smoke -- <shaders root> <out dir>

use std::path::PathBuf;

use phosphor_core::crt::{CrtRenderer, CRT_PRESETS};
use phosphor_core::Image;

fn pattern(w: usize, h: usize) -> Image {
    const BARS: [[f32; 3]; 7] = [
        [0.75, 0.75, 0.75],
        [0.75, 0.75, 0.0],
        [0.0, 0.75, 0.75],
        [0.0, 0.75, 0.0],
        [0.75, 0.0, 0.75],
        [0.75, 0.0, 0.0],
        [0.0, 0.0, 0.75],
    ];
    let mut img = Image::new(w, h);
    for y in 0..h {
        for x in 0..w {
            let c = if y < h * 2 / 3 {
                BARS[x * 7 / w]
            } else {
                let v = x as f32 / (w - 1) as f32;
                [v, v, v]
            };
            img.pixels[y * w + x] = [c[0], c[1], c[2], 1.0];
        }
    }
    img
}

pub fn write_png(path: &std::path::Path, img: &Image) {
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
    let args: Vec<String> = std::env::args().collect();
    let root = PathBuf::from(&args[1]);
    let out = PathBuf::from(&args[2]);
    std::fs::create_dir_all(&out).unwrap();
    let input = pattern(256, 224);
    let crt = CrtRenderer::new(&root);
    for preset in CRT_PRESETS {
        let t = std::time::Instant::now();
        match crt.render(&input, 1024, 896, preset, &[], 0) {
            Ok(img) => {
                let path = out.join(format!("crt_{}.png", preset.id));
                write_png(&path, &img);
                let mean: f32 = img.pixels.iter().map(|p| p[0] + p[1] + p[2]).sum::<f32>() / (img.pixels.len() as f32 * 3.0);
                println!("{:<14} ok  {:>6.0} ms  mean {:.3}", preset.id, t.elapsed().as_secs_f64() * 1000.0, mean);
            }
            Err(e) => println!("{:<14} FAILED: {e}", preset.id),
        }
    }
}
