//! Checks the GPU camera of the Screen Loop against the CPU reference.
//!
//! cargo run -p phosphor-core --release --example camera_compare -- <shaders root>

use phosphor_core::crt::CrtRenderer;
use phosphor_core::howlaround::{camera_cpu, HowlLoop, HowlSettings};
use phosphor_core::Image;

fn pattern(w: usize, h: usize, t: usize) -> Image {
    let mut img = Image::new(w, h);
    for y in 0..h {
        for x in 0..w {
            let v = (((x / 8 + y / 8 + t) % 2) as f32) * 0.8 + 0.1;
            img.pixels[y * w + x] = [v, x as f32 / w as f32, y as f32 / h as f32, 1.0];
        }
    }
    img
}

fn main() {
    let root = std::env::args().nth(1).expect("shaders root");
    let crt = CrtRenderer::new(&root);
    let (w, h) = (1280, 720);
    let scene = pattern(w, h, 0);
    let cases: &[(&str, &[(&str, f64)])] = &[
        ("default", &[]),
        ("focus + drift", &[("focus", 1.5), ("color_drift", 0.8), ("hue_drift", 25.0), ("brightness", 1.2), ("contrast", 1.3)]),
        ("tilted + key", &[("tilt", 12.0), ("turn", -8.0), ("roll", 5.0), ("zoom", 0.8), ("key", 0.5)]),
        ("key invert + counter", &[("key", 0.4), ("key_invert", 1.0), ("counter", 1.0), ("spin", 3.0)]),
    ];
    let mut worst = 0f32;
    for (name, values) in cases {
        let mut s = HowlSettings::default();
        for (k, v) in *values {
            s.set(k, *v);
        }
        let mut l = HowlLoop::new(&s, (320, 180));
        let (fw, fh) = l.feedback_size;
        l.advance(&pattern(fw, fh, 1), &s);
        l.advance(&pattern(fw, fh, 2), &s);
        let setup = l.camera_setup(w, h, &s, 2.3, 5.0);
        let cpu = camera_cpu(&scene, l.tv(), &setup);
        let gpu = crt.camera(&scene, l.tv(), &setup).expect("gpu camera");
        let (mut max, mut sum, mut over) = (0f32, 0f64, 0usize);
        for (a, b) in cpu.pixels.iter().zip(&gpu.pixels) {
            for c in 0..3 {
                let d = (a[c] - b[c]).abs();
                max = max.max(d);
                sum += d as f64;
                if d > 3.0 / 255.0 {
                    over += 1;
                }
            }
        }
        let mean = sum / (cpu.pixels.len() * 3) as f64;
        println!(
            "{name:<24} max {:5.1}/255  mean {:6.3}/255  >3/255: {:.4}%  sigma {:.2}",
            max * 255.0,
            mean * 255.0,
            over as f64 * 100.0 / (cpu.pixels.len() * 3) as f64,
            setup.sigma
        );
        worst = worst.max(mean as f32 * 255.0);
    }
    println!("worst mean difference {worst:.3}/255");
}
