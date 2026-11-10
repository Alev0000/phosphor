use std::sync::Mutex;

use rayon::prelude::*;

/// RGBA frame in 0..1, gamma-encoded (the same space NTSCRT's bgra8Unorm textures hold).
#[derive(Debug, PartialEq)]
pub struct Image {
    pub width: usize,
    pub height: usize,
    pub pixels: Vec<[f32; 4]>,
}

/// Frame-sized buffers handed back by dropped images. A fresh 1080p buffer costs ~3 ms in page
/// faults; a reused one is just cleared.
static POOL: Mutex<Vec<Vec<[f32; 4]>>> = Mutex::new(Vec::new());
/// Smaller buffers aren't worth pooling.
const POOL_MIN_PIXELS: usize = 256 * 1024;
const POOL_MAX_BYTES: usize = 256 << 20;

fn pooled(n: usize) -> Option<Vec<[f32; 4]>> {
    if n < POOL_MIN_PIXELS {
        return None;
    }
    let mut pool = POOL.lock().ok()?;
    // The smallest buffer that fits.
    let i = pool
        .iter()
        .enumerate()
        .filter(|(_, v)| v.capacity() >= n && v.capacity() <= n * 2)
        .min_by_key(|(_, v)| v.capacity())
        .map(|(i, _)| i)?;
    Some(pool.swap_remove(i))
}

impl Drop for Image {
    fn drop(&mut self) {
        let cap = self.pixels.capacity();
        if cap < POOL_MIN_PIXELS {
            return;
        }
        if let Ok(mut pool) = POOL.lock() {
            let bytes: usize = pool.iter().map(|v| v.capacity() * 16).sum();
            if bytes + cap * 16 <= POOL_MAX_BYTES {
                let mut v = std::mem::take(&mut self.pixels);
                v.clear();
                pool.push(v);
            }
        }
    }
}

impl Clone for Image {
    fn clone(&self) -> Self {
        let mut out = Image::uninit(self.width, self.height);
        out.pixels.par_chunks_mut(4096).zip(self.pixels.par_chunks(4096)).for_each(|(d, s)| d.copy_from_slice(s));
        out
    }
}

impl Image {
    pub fn new(width: usize, height: usize) -> Self {
        let n = width * height;
        let pixels = match pooled(n) {
            Some(mut v) => {
                // SAFETY: capacity >= n (see `pooled`), any bits are a valid f32, and every pixel is
                // cleared right here.
                unsafe { v.set_len(n) };
                v.par_chunks_mut(4096).for_each(|c| c.fill([0.0; 4]));
                v
            }
            None => vec![[0.0; 4]; n],
        };
        Self { width, height, pixels }
    }

    /// Same as `new`; for pictures whose every pixel is about to be written.
    pub fn uninit(width: usize, height: usize) -> Self {
        let n = width * height;
        let pixels = match pooled(n) {
            Some(mut v) => {
                // SAFETY: capacity >= n (see `pooled`) and any bits are a valid f32; the pixels hold
                // an earlier picture until the caller overwrites them.
                unsafe { v.set_len(n) };
                v
            }
            None => vec![[0.0; 4]; n],
        };
        Self { width, height, pixels }
    }

    #[inline]
    pub fn get(&self, x: usize, y: usize) -> [f32; 4] {
        self.pixels[y * self.width + x]
    }

    #[inline]
    pub fn get_clamped(&self, x: isize, y: isize) -> [f32; 4] {
        let cx = x.clamp(0, self.width as isize - 1) as usize;
        let cy = y.clamp(0, self.height as isize - 1) as usize;
        self.get(cx, cy)
    }

    pub fn rows_mut(&mut self) -> std::slice::ChunksExactMut<'_, [f32; 4]> {
        self.pixels.chunks_exact_mut(self.width)
    }

    /// Round to 8 bits per channel, as the original pipeline does between stages.
    pub fn quantize_8bit(&mut self) {
        for p in &mut self.pixels {
            for c in p.iter_mut() {
                *c = (c.clamp(0.0, 1.0) * 255.0).round() / 255.0;
            }
        }
    }
}
