//! The camera half of NTSCRT's `DigitalVideoRenderer.swift`: what the sensor reads through the lens
//! (`scale`), the camera's electronics (`capture`), and the decoder's picture back out (`render`).

use rayon::prelude::*;

use super::codec::{deblock, Geometry, Picture, Plane, SplitMix};
use super::params::DigitalSettings;

/// Tightly packed RGB8 picture.
#[derive(Clone, Debug, PartialEq)]
pub struct Rgb8 {
    pub width: usize,
    pub height: usize,
    pub px: Vec<[u8; 3]>,
}

impl Rgb8 {
    pub fn from_image(img: &crate::Image) -> Self {
        Self {
            width: img.width,
            height: img.height,
            px: img
                .pixels
                .par_iter()
                .map(|p| [0, 1, 2].map(|c| (p[c].clamp(0.0, 1.0) * 255.0 + 0.5) as u8))
                .collect(),
        }
    }
}

#[inline]
fn clamp255(v: f32) -> u8 {
    v.round().clamp(0.0, 255.0) as u8
}

fn lanczos3(t: f32) -> f32 {
    let t = t.abs();
    if t >= 3.0 {
        return 0.0;
    }
    if t < 1e-6 {
        return 1.0;
    }
    let xp = t * std::f32::consts::PI;
    (xp.sin() / xp) * ((xp / 3.0).sin() / (xp / 3.0))
}

/// High-quality resample (stands in for vImage's kvImageHighQualityResampling): separable Lanczos3
/// whose support widens with the minification ratio.
fn resample(src: &Rgb8, dw: usize, dh: usize) -> Rgb8 {
    let (sw, sh) = (src.width, src.height);
    let weights = |s: usize, d: usize| -> Vec<(isize, Vec<f32>)> {
        let scale = (s as f32 / d as f32).max(1.0);
        let support = 3.0 * scale;
        (0..d)
            .map(|o| {
                let center = (o as f32 + 0.5) * s as f32 / d as f32 - 0.5;
                let lo = (center - support).ceil() as isize;
                let hi = (center + support).floor() as isize;
                let mut w: Vec<f32> = (lo..=hi).map(|i| lanczos3((i as f32 - center) / scale)).collect();
                let sum: f32 = w.iter().sum();
                w.iter_mut().for_each(|v| *v /= sum.max(1e-6));
                (lo, w)
            })
            .collect()
    };
    let wx = weights(sw, dw);
    let wy = weights(sh, dh);

    let mid: Vec<[f32; 3]> = (0..sh)
        .into_par_iter()
        .flat_map_iter(|y| {
            let row = &src.px[y * sw..(y + 1) * sw];
            wx.iter().map(move |(lo, w)| {
                let mut acc = [0f32; 3];
                for (k, wk) in w.iter().enumerate() {
                    let p = row[(*lo + k as isize).clamp(0, sw as isize - 1) as usize];
                    for c in 0..3 {
                        acc[c] += p[c] as f32 * wk;
                    }
                }
                acc
            })
        })
        .collect();
    let px: Vec<[u8; 3]> = (0..dh)
        .into_par_iter()
        .flat_map_iter(|oy| {
            let (lo, w) = &wy[oy];
            let mid = &mid;
            (0..dw).map(move |x| {
                let mut acc = [0f32; 3];
                for (k, wk) in w.iter().enumerate() {
                    let sy = (*lo + k as isize).clamp(0, sh as isize - 1) as usize;
                    let p = mid[sy * dw + x];
                    for c in 0..3 {
                        acc[c] += p[c] * wk;
                    }
                }
                acc.map(clamp255)
            })
        })
        .collect();
    Rgb8 {
        width: dw,
        height: dh,
        px,
    }
}

/// What the sensor reads through the lens, at the capture size.
pub fn scale(src: &Rgb8, size: (usize, usize), optics: &DigitalSettings) -> Rgb8 {
    let (cw, ch) = size;
    let (width, height) = (src.width, src.height);
    let mut out = if width == cw && height == ch {
        src.clone()
    } else {
        resample(src, cw, ch)
    };

    let aliasing = optics.get("aliasing") as f32;
    if aliasing > 0.0 {
        let kx = width as f64 / cw as f64;
        let ky = height as f64 / ch as f64;
        out.px.par_chunks_exact_mut(cw).enumerate().for_each(|(y, row)| {
            let sy = (((y as f64 + 0.5) * ky) as usize).min(height - 1);
            let srow = &src.px[sy * width..(sy + 1) * width];
            for (x, o) in row.iter_mut().enumerate() {
                let center = (x as f64 + 0.5) * kx;
                let xg = (center as usize).min(width - 1);
                let xr = ((center - 0.5 * kx).max(0.0) as usize).min(width - 1);
                let xb = ((center + 0.5 * kx) as usize).min(width - 1);
                let (pr, pg, pb) = (srow[xr][0] as f32, srow[xg][1] as f32, srow[xb][2] as f32);
                o[0] = (o[0] as f32 + (pr - o[0] as f32) * aliasing).round() as u8;
                o[1] = (o[1] as f32 + (pg - o[1] as f32) * aliasing).round() as u8;
                o[2] = (o[2] as f32 + (pb - o[2] as f32) * aliasing).round() as u8;
            }
        });
    }

    let blur = optics.get("lens_blur");
    if blur > 0.0 {
        gaussian_blur(&mut out, (0.25 + 1.5 * blur) as f32);
    }
    if optics.get("fields") >= 0.5 {
        for y in (1..ch).step_by(2) {
            let (a, b) = out.px.split_at_mut(y * cw);
            b[..cw].copy_from_slice(&a[(y - 1) * cw..y * cw]);
        }
    }
    out
}

fn gaussian_blur(img: &mut Rgb8, sigma: f32) {
    let (w, h) = (img.width, img.height);
    let r = ((sigma * 2.5).ceil() as isize).max(1);
    let mut k: Vec<f32> = (-r..=r).map(|d| (-((d * d) as f32) / (2.0 * sigma * sigma)).exp()).collect();
    let sum: f32 = k.iter().sum();
    k.iter_mut().for_each(|v| *v /= sum);
    let src = &img.px;
    let tmp: Vec<[f32; 3]> = (0..h)
        .into_par_iter()
        .flat_map_iter(|y| {
            let k = &k;
            (0..w).map(move |x| {
                let mut a = [0f32; 3];
                for d in -r..=r {
                    let p = src[y * w + (x as isize + d).clamp(0, w as isize - 1) as usize];
                    let wt = k[(d + r) as usize];
                    for c in 0..3 {
                        a[c] += p[c] as f32 * wt;
                    }
                }
                a
            })
        })
        .collect();
    img.px.par_chunks_exact_mut(w).enumerate().for_each(|(y, row)| {
        for (x, o) in row.iter_mut().enumerate() {
            let mut a = [0f32; 3];
            for d in -r..=r {
                let p = tmp[(y as isize + d).clamp(0, h as isize - 1) as usize * w + x];
                let wt = k[(d + r) as usize];
                for c in 0..3 {
                    a[c] += p[c] * wt;
                }
            }
            *o = a.map(clamp255);
        }
    });
}

/// The camera's electronics on what the sensor read, producing the picture the codec codes.
pub fn capture(rgb: &Rgb8, g: &Geometry, settings: &DigitalSettings, index: i64) -> Picture {
    let (w, h) = (g.width, g.height);
    let noise = settings.get("sensor_noise") as f32;
    let nr = settings.get("noise_reduction") as f32;
    let exposure = 2f64.powf(settings.get("exposure")) as f32;
    let warm = settings.get("white_balance") as f32;
    let magenta = settings.get("tint") as f32;
    let gain_r = exposure * (1.0 + 0.25 * warm);
    let gain_b = exposure * (1.0 - 0.25 * warm);
    let gain_g = exposure * (1.0 - 0.15 * magenta);
    let gains = gain_r != 1.0 || gain_g != 1.0 || gain_b != 1.0;

    let rows: Vec<Vec<(f32, f32, f32)>> = (0..h)
        .into_par_iter()
        .map(|y| {
            let mut rng = SplitMix::new(
                (index as u64).wrapping_mul(0x2545_F491_4F6C_DD1D) ^ (y as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15),
            );
            let mut gauss = || ((rng.unit() + rng.unit() - 1.0) as f32) * 2.449;
            (0..w)
                .map(|x| {
                    let p = rgb.px[y * w + x];
                    let (mut r, mut gr, mut b) = (p[0] as f32, p[1] as f32, p[2] as f32);
                    if gains {
                        r *= gain_r;
                        gr *= gain_g;
                        b *= gain_b;
                    }
                    if noise > 0.0 {
                        let shade = 1.3 - 0.6 * (0.299 * r + 0.587 * gr + 0.114 * b).min(255.0) / 255.0;
                        let n = gauss() * 7.0 * noise * shade;
                        let nb = gauss() * 11.0 * noise * shade;
                        let nc = gauss() * 11.0 * noise * shade;
                        r += n + 1.402 * nc;
                        gr += n - 0.344136 * nb - 0.714136 * nc;
                        b += n + 1.772 * nb;
                    }
                    if gains || noise > 0.0 {
                        r = r.clamp(0.0, 255.0);
                        gr = gr.clamp(0.0, 255.0);
                        b = b.clamp(0.0, 255.0);
                    }
                    (
                        0.299 * r + 0.587 * gr + 0.114 * b,
                        128.0 - 0.168736 * r - 0.331264 * gr + 0.5 * b,
                        128.0 + 0.5 * r - 0.418688 * gr - 0.081312 * b,
                    )
                })
                .collect()
        })
        .collect();
    let mut yf = Vec::with_capacity(w * h);
    let mut cbf = Vec::with_capacity(w * h);
    let mut crf = Vec::with_capacity(w * h);
    for row in rows {
        for (a, b, c) in row {
            yf.push(a);
            cbf.push(b);
            crf.push(c);
        }
    }

    if nr > 0.0 {
        yf = sigma_filter(&yf, w, h, 4.0 + 26.0 * nr, (nr * 1.2).min(1.0));
        cbf = box_blur(&cbf, w, h, 2, nr);
        crf = box_blur(&crf, w, h, 2, nr);
    }
    let sharpening = settings.get("sharpening") as f32;
    if sharpening > 0.0 {
        let soft = box_blur(&yf, w, h, 1, 1.0);
        for i in 0..yf.len() {
            yf[i] = (yf[i] + 2.2 * sharpening * (yf[i] - soft[i])).clamp(0.0, 255.0);
        }
    }
    let black = settings.get("black_level") as f32 * 255.0;
    let saturation = settings.get("saturation") as f32;
    if black > 0.0 || saturation != 1.0 {
        for i in 0..yf.len() {
            yf[i] = black + yf[i] * (1.0 - black / 255.0);
            cbf[i] = 128.0 + (cbf[i] - 128.0) * saturation;
            crf[i] = 128.0 + (crf[i] - 128.0) * saturation;
        }
    }

    let mut pic = Picture::new(g);
    let lw = pic.y.width;
    for y in 0..pic.y.height {
        let sy = y.min(h - 1);
        for x in 0..lw {
            pic.y.data[y * lw + x] = yf[sy * w + x.min(w - 1)];
        }
    }
    let (fx, fy) = (g.sx(), g.sy());
    let cw = pic.cb.width;
    let n = (fx * fy) as f32;
    for cy in 0..pic.cb.height {
        for cx in 0..cw {
            let (mut sb, mut sr) = (0f32, 0f32);
            for dy in 0..fy {
                let yy = (cy * fy + dy).min(h - 1);
                for dx in 0..fx {
                    let i = yy * w + (cx * fx + dx).min(w - 1);
                    sb += cbf[i];
                    sr += crf[i];
                }
            }
            pic.cb.data[cy * cw + cx] = sb / n;
            pic.cr.data[cy * cw + cx] = sr / n;
        }
    }
    pic
}

fn sigma_filter(p: &[f32], w: usize, h: usize, threshold: f32, mix: f32) -> Vec<f32> {
    (0..h)
        .into_par_iter()
        .flat_map_iter(|y| {
            (0..w).map(move |x| {
                let c = p[y * w + x];
                let (mut sum, mut n) = (0f32, 0f32);
                for dy in -1isize..=1 {
                    let yy = (y as isize + dy).clamp(0, h as isize - 1) as usize;
                    for dx in -1isize..=1 {
                        let v = p[yy * w + (x as isize + dx).clamp(0, w as isize - 1) as usize];
                        if (v - c).abs() < threshold {
                            sum += v;
                            n += 1.0;
                        }
                    }
                }
                c + (sum / n - c) * mix
            })
        })
        .collect()
}

fn box_blur(p: &[f32], w: usize, h: usize, r: isize, mix: f32) -> Vec<f32> {
    let n = (2 * r + 1) as f32;
    let mut tmp = p.to_vec();
    let mut out = p.to_vec();
    for y in 0..h {
        for x in 0..w {
            let mut s = 0f32;
            for d in -r..=r {
                s += p[y * w + (x as isize + d).clamp(0, w as isize - 1) as usize];
            }
            tmp[y * w + x] = s / n;
        }
    }
    for y in 0..h {
        for x in 0..w {
            let mut s = 0f32;
            for d in -r..=r {
                s += tmp[(y as isize + d).clamp(0, h as isize - 1) as usize * w + x];
            }
            out[y * w + x] = p[y * w + x] + (s / n - p[y * w + x]) * mix;
        }
    }
    out
}

/// The decoder's picture as RGB8 at the capture size: deblocked on a copy, color interpolated up.
pub fn render(decoded: &Picture, g: &Geometry, settings: &DigitalSettings, step: f32) -> Rgb8 {
    let deblocking = settings.get("deblocking");
    let owned;
    let picture = if deblocking > 0.0 {
        let mut p = decoded.clone();
        deblock(&mut p, deblocking, step);
        owned = p;
        &owned
    } else {
        decoded
    };
    let (w, h) = (g.width, g.height);
    let (fx, fy) = (g.sx() as f32, g.sy() as f32);
    let (cb, cr, luma) = (&picture.cb, &picture.cr, &picture.y);
    let bil = |p: &Plane, cx0: isize, cy0: isize, tx: f32, ty: f32| -> f32 {
        let a = p.clamped(cx0, cy0);
        let b = p.clamped(cx0 + 1, cy0);
        let c = p.clamped(cx0, cy0 + 1);
        let d = p.clamped(cx0 + 1, cy0 + 1);
        (a + (b - a) * tx) + ((c + (d - c) * tx) - (a + (b - a) * tx)) * ty
    };
    let px: Vec<[u8; 3]> = (0..h)
        .into_par_iter()
        .flat_map_iter(|y| {
            let cyf = (y as f32 + 0.5) / fy - 0.5;
            let cy0 = cyf.floor() as isize;
            let ty = cyf - cy0 as f32;
            (0..w).map(move |x| {
                let cxf = (x as f32 + 0.5) / fx - 0.5;
                let cx0 = cxf.floor() as isize;
                let tx = cxf - cx0 as f32;
                let yv = luma.data[y * luma.width + x];
                let u = bil(cb, cx0, cy0, tx, ty) - 128.0;
                let v = bil(cr, cx0, cy0, tx, ty) - 128.0;
                [
                    clamp255(yv + 1.402 * v),
                    clamp255(yv - 0.344136 * u - 0.714136 * v),
                    clamp255(yv + 1.772 * u),
                ]
            })
        })
        .collect();
    Rgb8 {
        width: w,
        height: h,
        px,
    }
}

/// Scale the capture-size picture back up with hard pixels, so a nearest Downscale to the capture
/// size returns it exactly.
pub fn upscale(picture: &Rgb8, width: usize, height: usize) -> crate::Image {
    let (cw, ch) = (picture.width, picture.height);
    let xs: Vec<usize> = (0..width)
        .map(|x| (((x as f64 + 0.5) * cw as f64 / width as f64) as usize).min(cw - 1))
        .collect();
    let mut out = crate::Image::new(width, height);
    out.pixels.par_chunks_exact_mut(width).enumerate().for_each(|(y, row)| {
        let sy = (((y as f64 + 0.5) * ch as f64 / height as f64) as usize).min(ch - 1);
        for (x, o) in row.iter_mut().enumerate() {
            let p = picture.px[sy * cw + xs[x]];
            *o = [p[0] as f32 / 255.0, p[1] as f32 / 255.0, p[2] as f32 / 255.0, 1.0];
        }
    });
    out
}
