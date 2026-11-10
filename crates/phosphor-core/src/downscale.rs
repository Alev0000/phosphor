use rayon::prelude::*;

use crate::Image;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DownscaleMethod {
    /// "Chunky": one source pixel per block.
    Nearest,
    /// "Smooth": box average of every source pixel the block covers.
    Area,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DownscaleSpec {
    pub width: usize,
    pub height: usize,
    pub method: DownscaleMethod,
}

impl DownscaleSpec {
    /// Height follows the source aspect, rounded to an even line count (min 16).
    pub fn for_source(width: usize, src_w: usize, src_h: usize, method: DownscaleMethod) -> Self {
        let aspect = (src_w as f64 / src_h.max(1) as f64).max(0.05);
        let height = (2 * ((width as f64 / aspect / 2.0).round() as usize)).max(16);
        Self {
            width: width.max(1),
            height,
            method,
        }
    }
}

pub fn downscale(src: &Image, spec: DownscaleSpec) -> Image {
    let mut dst = Image::new(spec.width, spec.height);
    let (sw, sh, dw, dh) = (src.width, src.height, spec.width, spec.height);

    match spec.method {
        DownscaleMethod::Nearest => {
            dst.pixels
                .par_chunks_exact_mut(dw)
                .enumerate()
                .for_each(|(y, row)| {
                    let v = (y as f32 + 0.5) / dh as f32;
                    let sy = ((v * sh as f32).floor() as usize).min(sh - 1);
                    for (x, out) in row.iter_mut().enumerate() {
                        let u = (x as f32 + 0.5) / dw as f32;
                        let sx = ((u * sw as f32).floor() as usize).min(sw - 1);
                        *out = src.get(sx, sy);
                    }
                });
        }
        DownscaleMethod::Area => {
            let rx = sw as f32 / dw as f32;
            let ry = sh as f32 / dh as f32;
            dst.pixels
                .par_chunks_exact_mut(dw)
                .enumerate()
                .for_each(|(gy, row)| {
                    let sy0 = gy as f32 * ry;
                    let sy1 = (gy + 1) as f32 * ry;
                    let (y0, y1) = (sy0.floor() as isize, sy1.ceil() as isize);
                    for (gx, out) in row.iter_mut().enumerate() {
                        let sx0 = gx as f32 * rx;
                        let sx1 = (gx + 1) as f32 * rx;
                        let (x0, x1) = (sx0.floor() as isize, sx1.ceil() as isize);
                        let mut acc = [0.0f32; 4];
                        let mut wsum = 0.0f32;
                        for y in y0..y1 {
                            let fy = ((y as f32 + 1.0).min(sy1) - (y as f32).max(sy0)).clamp(0.0, 1.0);
                            for x in x0..x1 {
                                let fx = ((x as f32 + 1.0).min(sx1) - (x as f32).max(sx0))
                                    .clamp(0.0, 1.0);
                                let w = fx * fy;
                                let p = src.get_clamped(x, y);
                                for c in 0..4 {
                                    acc[c] += p[c] * w;
                                }
                                wsum += w;
                            }
                        }
                        let inv = 1.0 / wsum.max(1e-6);
                        *out = acc.map(|a| a * inv);
                    }
                });
        }
    }

    dst.quantize_8bit();
    dst
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn height_follows_aspect_and_is_even() {
        let s = DownscaleSpec::for_source(256, 1920, 1080, DownscaleMethod::Nearest);
        assert_eq!((s.width, s.height), (256, 144));
        let s = DownscaleSpec::for_source(320, 720, 480, DownscaleMethod::Area);
        assert_eq!(s.height % 2, 0);
    }

    #[test]
    fn area_of_flat_image_is_flat() {
        let mut src = Image::new(64, 48);
        src.pixels.fill([0.5, 0.25, 0.75, 1.0]);
        let out = downscale(&src, DownscaleSpec::for_source(20, 64, 48, DownscaleMethod::Area));
        for p in &out.pixels {
            assert!((p[0] - 128.0 / 255.0).abs() < 1e-6);
        }
    }
}
