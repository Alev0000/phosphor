//! Converting After Effects / Premiere layers to and from the pipeline's RGBA float image.

use after_effects::{self as ae, Layer, RationalScale};
use phosphor_core::Image;
use rayon::prelude::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fmt {
    /// After Effects: A, R, G, B.
    Argb8,
    /// After Effects 16 bpc: 0..=32768.
    Argb16,
    Argb32f,
    /// Premiere: B, G, R, A.
    Bgra8,
    Bgra16,
    Bgra32f,
}

impl From<ae::PixelFormat> for Fmt {
    fn from(f: ae::PixelFormat) -> Self {
        match f {
            ae::PixelFormat::Argb64 => Fmt::Argb16,
            ae::PixelFormat::Argb128 => Fmt::Argb32f,
            _ => Fmt::Argb8,
        }
    }
}

impl From<ae::pr::PixelFormat> for Fmt {
    fn from(f: ae::pr::PixelFormat) -> Self {
        match f {
            ae::pr::PixelFormat::Bgra4444_16u => Fmt::Bgra16,
            ae::pr::PixelFormat::Bgra4444_32f => Fmt::Bgra32f,
            _ => Fmt::Bgra8,
        }
    }
}

impl Fmt {
    fn bytes_per_channel(self) -> usize {
        match self {
            Fmt::Argb8 | Fmt::Bgra8 => 1,
            Fmt::Argb16 | Fmt::Bgra16 => 2,
            Fmt::Argb32f | Fmt::Bgra32f => 4,
        }
    }

    /// Channel offsets of R, G, B, A within a pixel.
    fn order(self) -> [usize; 4] {
        match self {
            Fmt::Argb8 | Fmt::Argb16 | Fmt::Argb32f => [1, 2, 3, 0],
            Fmt::Bgra8 | Fmt::Bgra16 | Fmt::Bgra32f => [2, 1, 0, 3],
        }
    }
}

pub fn ceil_mul_rational(n: i32, scale: RationalScale) -> i32 {
    let num = n * scale.num;
    let den = scale.den as i32;
    num / den + (num % den != 0) as i32
}

fn row_offset(row_bytes: isize, height: usize, y: usize) -> usize {
    if row_bytes >= 0 {
        y * row_bytes as usize
    } else {
        (height - 1 - y) * (-row_bytes) as usize
    }
}

/// One pixel's R, G, B and alpha in 0..1.
#[inline(always)]
fn decode(fmt: Fmt, px: &[u8], order: [usize; 4], bpc: usize) -> [f32; 4] {
    match fmt {
        Fmt::Argb8 | Fmt::Bgra8 => order.map(|o| px[o] as f32 * (1.0 / 255.0)),
        Fmt::Argb16 => order.map(|o| u16::from_ne_bytes([px[o * bpc], px[o * bpc + 1]]) as f32 * (1.0 / 32768.0)),
        Fmt::Bgra16 => order.map(|o| u16::from_ne_bytes([px[o * bpc], px[o * bpc + 1]]) as f32 * (1.0 / 65535.0)),
        Fmt::Argb32f | Fmt::Bgra32f => order.map(|o| {
            let i = o * bpc;
            f32::from_ne_bytes([px[i], px[i + 1], px[i + 2], px[i + 3]])
        }),
    }
}

#[inline(always)]
fn encode(fmt: Fmt, px: &mut [u8], order: [usize; 4], bpc: usize, rgb: [f32; 4]) {
    for (i, v) in [rgb[0], rgb[1], rgb[2], 1.0].into_iter().enumerate() {
        let v = v.clamp(0.0, 1.0);
        let at = order[i] * bpc;
        match fmt {
            Fmt::Argb8 | Fmt::Bgra8 => px[at] = (v * 255.0 + 0.5) as u8,
            Fmt::Argb16 => px[at..at + 2].copy_from_slice(&((v * 32768.0 + 0.5) as u16).to_ne_bytes()),
            Fmt::Bgra16 => px[at..at + 2].copy_from_slice(&((v * 65535.0 + 0.5) as u16).to_ne_bytes()),
            Fmt::Argb32f | Fmt::Bgra32f => px[at..at + 4].copy_from_slice(&v.to_ne_bytes()),
        }
    }
}

/// The layer placed at its origin on a `width` × `height` canvas (transparent areas read as black).
/// Rows convert in parallel.
pub fn read_layer(layer: &Layer, width: usize, height: usize, fmt: Fmt) -> Result<Image, ae::Error> {
    let mut img = Image::uninit(width, height);
    let (lw, lh) = (layer.width(), layer.height());
    let rb = layer.row_bytes();
    let origin = layer.origin();
    let (ox, oy) = (origin.h.max(0) as usize, origin.v.max(0) as usize);
    let bpc = fmt.bytes_per_channel();
    let order = fmt.order();
    let buf = layer.buffer();
    let px_bytes = 4 * bpc;
    // An origin past the canvas edge leaves nothing to copy (and must not index past the row).
    let n = lw.min(width.saturating_sub(ox));
    img.pixels.par_chunks_exact_mut(width).enumerate().for_each(|(ty, out)| {
        out.fill([0.0, 0.0, 0.0, 1.0]);
        let Some(y) = ty.checked_sub(oy).filter(|&y| y < lh) else { return };
        if n == 0 {
            return;
        }
        let row = &buf[row_offset(rb, lh, y)..];
        for (x, o) in out[ox..ox + n].iter_mut().enumerate() {
            let c = decode(fmt, &row[x * px_bytes..(x + 1) * px_bytes], order, bpc);
            // Composite over black, as the original's opaque frames are.
            *o = [c[0] * c[3], c[1] * c[3], c[2] * c[3], 1.0];
        }
    });
    Ok(img)
}

/// Rows convert in parallel.
pub fn write_layer(layer: &mut Layer, img: &Image, fmt: Fmt) -> Result<(), ae::Error> {
    let (lw, lh) = (layer.width(), layer.height());
    let rb = layer.row_bytes();
    let stride = rb.unsigned_abs();
    let bpc = fmt.bytes_per_channel();
    let order = fmt.order();
    let px_bytes = 4 * bpc;
    let n = lw.min(img.width);
    let rows = lh.min(img.height);
    let buf = layer.buffer_mut();
    buf.par_chunks_mut(stride).enumerate().for_each(|(c, row)| {
        let y = if rb >= 0 { c } else { lh.wrapping_sub(1).wrapping_sub(c) };
        if y >= rows {
            return;
        }
        let src = &img.pixels[y * img.width..y * img.width + n];
        for (x, p) in src.iter().enumerate() {
            encode(fmt, &mut row[x * px_bytes..(x + 1) * px_bytes], order, bpc, *p);
        }
    });
    Ok(())
}
