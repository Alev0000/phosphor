//! Replica of the plugin's per-pixel layer conversion (8-bit ARGB in, out), for timing.
use std::time::Instant;
use phosphor_core::Image;

fn main() {
    let (w, h) = (1920usize, 1080usize);
    let buf: Vec<u8> = (0..w * h * 4).map(|i| (i % 251) as u8).collect();
    let order = [1usize, 2, 3, 0];
    let read = |b: &[u8]| b[0] as f32 / 255.0;
    let mut best_in = f64::MAX;
    let mut best_out = f64::MAX;
    for _ in 0..7 {
        let t = Instant::now();
        let mut img = Image::new(w, h);
        img.pixels.fill([0.0, 0.0, 0.0, 1.0]);
        for y in 0..h {
            let row = &buf[y * w * 4..];
            for x in 0..w {
                let px = &row[x * 4..(x + 1) * 4];
                let c = |i: usize| read(&px[order[i]..]);
                let a = c(3);
                img.pixels[y * w + x] = [c(0) * a, c(1) * a, c(2) * a, 1.0];
            }
        }
        best_in = best_in.min(t.elapsed().as_secs_f64() * 1000.0);
        let mut out = vec![0u8; w * h * 4];
        let t = Instant::now();
        for y in 0..h {
            let row = &mut out[y * w * 4..];
            for x in 0..w {
                let p = img.pixels[y * w + x];
                let px = &mut row[x * 4..(x + 1) * 4];
                for (i, v) in [p[0], p[1], p[2], 1.0].into_iter().enumerate() {
                    px[order[i]] = (v.clamp(0.0, 1.0) * 255.0).round() as u8;
                }
            }
        }
        best_out = best_out.min(t.elapsed().as_secs_f64() * 1000.0);
        std::hint::black_box(&out);
    }
    println!("read layer 1080p (single thread): {best_in:.1} ms");
    println!("write layer 1080p (single thread): {best_out:.1} ms");
}
