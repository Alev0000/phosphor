//! Port of NTSCRT's `DigitalCodec.swift`: a small, real block-transform video codec (MPEG/H.263/DV
//! family). Deterministic: the same captures and settings give the same pictures, bit for bit.

use std::sync::{Arc, OnceLock};

use rayon::prelude::*;

use super::params::ChromaSubsampling;

#[derive(Clone, Debug, PartialEq)]
pub struct Plane {
    pub width: usize,
    pub height: usize,
    pub data: Vec<f32>,
}

impl Plane {
    pub fn new(width: usize, height: usize, fill: f32) -> Self {
        Self {
            width,
            height,
            data: vec![fill; width * height],
        }
    }

    #[inline(always)]
    pub fn clamped(&self, x: isize, y: isize) -> f32 {
        let cy = y.clamp(0, self.height as isize - 1) as usize;
        let cx = x.clamp(0, self.width as isize - 1) as usize;
        self.data[cy * self.width + cx]
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Picture {
    pub y: Plane,
    pub cb: Plane,
    pub cr: Plane,
}

impl Picture {
    pub fn new(g: &Geometry) -> Self {
        Self {
            y: Plane::new(g.luma_w(), g.luma_h(), 0.0),
            cb: Plane::new(g.chroma_w(), g.chroma_h(), 128.0),
            cr: Plane::new(g.chroma_w(), g.chroma_h(), 128.0),
        }
    }

    pub fn planes(&self) -> [&Plane; 3] {
        [&self.y, &self.cb, &self.cr]
    }

    pub fn planes_mut(&mut self) -> [&mut Plane; 3] {
        [&mut self.y, &mut self.cb, &mut self.cr]
    }

    pub fn byte_count(&self) -> usize {
        (self.y.data.len() + self.cb.data.len() + self.cr.data.len()) * 4
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Geometry {
    pub width: usize,
    pub height: usize,
    pub chroma: ChromaSubsampling,
}

impl Geometry {
    pub fn luma_w(&self) -> usize {
        self.width.div_ceil(16) * 16
    }
    pub fn luma_h(&self) -> usize {
        self.height.div_ceil(16) * 16
    }
    pub fn sx(&self) -> usize {
        self.chroma.factors().0
    }
    pub fn sy(&self) -> usize {
        self.chroma.factors().1
    }
    pub fn chroma_w(&self) -> usize {
        (self.luma_w() / self.sx()).div_ceil(8) * 8
    }
    pub fn chroma_h(&self) -> usize {
        (self.luma_h() / self.sy()).div_ceil(8) * 8
    }
    pub fn mb_cols(&self) -> usize {
        self.luma_w() / 16
    }
    pub fn mb_rows(&self) -> usize {
        self.luma_h() / 16
    }
    pub fn mb_count(&self) -> usize {
        self.mb_cols() * self.mb_rows()
    }
    fn factors(&self, plane: usize) -> (usize, usize) {
        if plane == 0 { (1, 1) } else { (self.sx(), self.sy()) }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FrameKind {
    IntraOnly,
    Key,
    DroppedKey,
    Predicted,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct MotionVector {
    pub dx: i32,
    pub dy: i32,
}

/// The codec's state between frames. `encoder` and `decoder` may be the same picture (`Arc::ptr_eq`).
#[derive(Clone, Debug)]
pub struct CodecState {
    pub geometry: Geometry,
    pub encoder: Option<Arc<Picture>>,
    pub decoder: Option<Arc<Picture>>,
    pub vectors: Vec<MotionVector>,
    pub diverged: bool,
}

impl CodecState {
    pub fn new(geometry: Geometry) -> Self {
        Self {
            geometry,
            encoder: None,
            decoder: None,
            vectors: vec![MotionVector::default(); geometry.mb_count()],
            diverged: false,
        }
    }
}

/// SplitMix64.
#[derive(Clone, Copy, Debug)]
pub struct SplitMix {
    state: u64,
}

impl SplitMix {
    pub fn new(seed: u64) -> Self {
        Self { state: seed }
    }
    pub fn next_u64(&mut self) -> u64 {
        self.state = self.state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
    pub fn unit(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64
    }
}

// MARK: quantization

const LUMA_TABLE: [f32; 64] = [
    16., 11., 10., 16., 24., 40., 51., 61., 12., 12., 14., 19., 26., 58., 60., 55., 14., 13., 16.,
    24., 40., 57., 69., 56., 14., 17., 22., 29., 51., 87., 80., 62., 18., 22., 37., 56., 68., 109.,
    103., 77., 24., 35., 55., 64., 81., 104., 113., 92., 49., 64., 78., 87., 103., 121., 120., 101.,
    72., 92., 95., 98., 112., 100., 103., 99.,
];
const CHROMA_TABLE: [f32; 64] = [
    17., 18., 24., 47., 99., 99., 99., 99., 18., 21., 26., 66., 99., 99., 99., 99., 24., 26., 56.,
    99., 99., 99., 99., 99., 47., 66., 99., 99., 99., 99., 99., 99., 99., 99., 99., 99., 99., 99.,
    99., 99., 99., 99., 99., 99., 99., 99., 99., 99., 99., 99., 99., 99., 99., 99., 99., 99., 99.,
    99., 99., 99., 99., 99., 99., 99.,
];

pub fn table_scale(quality: f64) -> f32 {
    let q = 2.0 + 93.0 * quality.clamp(0.0, 1.0);
    ((if q < 50.0 { 5000.0 / q } else { 200.0 - 2.0 * q }) as f32) / 100.0
}

#[derive(Clone, Debug)]
pub struct Steps {
    pub luma: [f32; 64],
    pub chroma: [f32; 64],
    pub inter: f32,
}

impl Steps {
    pub fn new(quality: f64, squeeze: f32) -> Self {
        let s = table_scale(quality) * squeeze;
        Self {
            luma: LUMA_TABLE.map(|v| (v * s).round().max(1.0)),
            chroma: CHROMA_TABLE.map(|v| (v * s).round().max(1.0)),
            inter: (16.0 * s).max(1.0),
        }
    }
}

// MARK: DCT

fn basis() -> &'static [f32; 64] {
    static B: OnceLock<[f32; 64]> = OnceLock::new();
    B.get_or_init(|| {
        let mut c = [0f32; 64];
        for u in 0..8 {
            let a = if u == 0 { (1.0f64 / 8.0).sqrt() } else { (2.0f64 / 8.0).sqrt() };
            for x in 0..8 {
                c[u * 8 + x] =
                    (a * ((2 * x + 1) as f64 * u as f64 * std::f64::consts::PI / 16.0).cos()) as f32;
            }
        }
        c
    })
}

#[inline(always)]
fn forward_dct(input: &[f32; 64], out: &mut [f32; 64], c: &[f32; 64]) {
    let mut tmp = [0f32; 64];
    for y in 0..8 {
        for u in 0..8 {
            let mut s = 0f32;
            for x in 0..8 {
                s += c[u * 8 + x] * input[y * 8 + x];
            }
            tmp[y * 8 + u] = s;
        }
    }
    for v in 0..8 {
        for u in 0..8 {
            let mut s = 0f32;
            for y in 0..8 {
                s += c[v * 8 + y] * tmp[y * 8 + u];
            }
            out[v * 8 + u] = s;
        }
    }
}

#[inline(always)]
fn inverse_dct(coef: &[f32; 64], out: &mut [f32; 64], c: &[f32; 64]) {
    let mut tmp = [0f32; 64];
    for y in 0..8 {
        for u in 0..8 {
            let mut s = 0f32;
            for v in 0..8 {
                s += c[v * 8 + y] * coef[v * 8 + u];
            }
            tmp[y * 8 + u] = s;
        }
    }
    for y in 0..8 {
        for x in 0..8 {
            let mut s = 0f32;
            for u in 0..8 {
                s += c[u * 8 + x] * tmp[y * 8 + u];
            }
            out[y * 8 + x] = s;
        }
    }
}

// MARK: keyframes

pub fn code_intra(source: &Picture, steps: &Steps, g: &Geometry) -> Picture {
    let c = basis();
    let mut recon = Picture::new(g);
    for (i, (src, dst)) in source.planes().into_iter().zip(recon.planes_mut()).enumerate() {
        let table = if i == 0 { &steps.luma } else { &steps.chroma };
        let w = src.width;
        dst.data
            .par_chunks_exact_mut(8 * w)
            .enumerate()
            .for_each(|(ty, dst_rows)| {
                let mut block = [0f32; 64];
                let mut coef = [0f32; 64];
                for tx in 0..(w / 8) {
                    let origin = ty * 8 * w + tx * 8;
                    for y in 0..8 {
                        for x in 0..8 {
                            block[y * 8 + x] = src.data[origin + y * w + x] - 128.0;
                        }
                    }
                    forward_dct(&block, &mut coef, c);
                    for k in 0..64 {
                        coef[k] = (coef[k] / table[k]).round() * table[k];
                    }
                    inverse_dct(&coef, &mut block, c);
                    for y in 0..8 {
                        for x in 0..8 {
                            dst_rows[y * w + tx * 8 + x] = (block[y * 8 + x] + 128.0).round().clamp(0.0, 255.0);
                        }
                    }
                }
            });
    }
    recon
}

// MARK: prediction

#[inline(always)]
fn sad(src: &Plane, rf: &Plane, bx: usize, by: usize, dx: i32, dy: i32, limit: f32) -> f32 {
    let w = src.width;
    let rx = bx as isize + dx as isize;
    let ry = by as isize + dy as isize;
    let mut total = 0f32;
    if rx >= 0 && ry >= 0 && rx as usize + 16 <= rf.width && ry as usize + 16 <= rf.height {
        let (rx, ry) = (rx as usize, ry as usize);
        for y in 0..16 {
            let s = &src.data[(by + y) * w + bx..][..16];
            let r = &rf.data[(ry + y) * w + rx..][..16];
            for x in 0..16 {
                total += (s[x] - r[x]).abs();
            }
            if total >= limit {
                return total;
            }
        }
    } else {
        for y in 0..16 {
            let s = &src.data[(by + y) * w + bx..][..16];
            for x in 0..16 {
                total += (s[x] - rf.clamped(rx + x as isize, ry + y as isize)).abs();
            }
            if total >= limit {
                return total;
            }
        }
    }
    total
}

const SEARCH_RANGE: i32 = 24;

pub fn search_motion(
    source: &Picture,
    reference: &Picture,
    previous: &[MotionVector],
    g: &Geometry,
) -> (Vec<MotionVector>, f32) {
    let src = &source.y;
    let rf = &reference.y;
    let cols = g.mb_cols();
    let rows: Vec<(Vec<MotionVector>, f32)> = (0..g.mb_rows())
        .into_par_iter()
        .map(|my| {
            let mut out = Vec::with_capacity(cols);
            let mut left = MotionVector::default();
            let mut sum = 0f32;
            for mx in 0..cols {
                let (bx, by) = (mx * 16, my * 16);
                let cost = |v: (i32, i32), limit: f32| -> f32 {
                    if v.0.abs() > SEARCH_RANGE || v.1.abs() > SEARCH_RANGE {
                        return f32::INFINITY;
                    }
                    let bias = if v == (0, 0) { -48.0 } else { 0.0 };
                    sad(src, rf, bx, by, v.0, v.1, limit - bias) + bias
                };
                let mut best = (0, 0);
                let mut best_cost = cost(best, f32::INFINITY);
                for cand in [previous[my * cols + mx], left] {
                    let v = (cand.dx, cand.dy);
                    if v == best {
                        continue;
                    }
                    let cst = cost(v, best_cost);
                    if cst < best_cost {
                        best_cost = cst;
                        best = v;
                    }
                }
                let mut step = 8;
                while step >= 1 {
                    let mut moved = true;
                    let mut guard = 0;
                    while moved && guard < 8 {
                        moved = false;
                        guard += 1;
                        for d in [(step, 0), (-step, 0), (0, step), (0, -step)] {
                            let v = (best.0 + d.0, best.1 + d.1);
                            let cst = cost(v, best_cost);
                            if cst < best_cost {
                                best_cost = cst;
                                best = v;
                                moved = true;
                            }
                        }
                    }
                    step /= 2;
                }
                let mv = MotionVector {
                    dx: best.0,
                    dy: best.1,
                };
                out.push(mv);
                left = mv;
                sum += sad(src, rf, bx, by, best.0, best.1, f32::INFINITY);
            }
            (out, sum)
        })
        .collect();
    let mut vectors = Vec::with_capacity(g.mb_count());
    let mut total = 0f32;
    for (v, s) in rows {
        vectors.extend(v);
        total += s;
    }
    (vectors, total / (g.luma_w() * g.luma_h()) as f32)
}

pub fn compensate(reference: &Picture, vectors: &[MotionVector], g: &Geometry) -> Picture {
    let mut prediction = Picture::new(g);
    let cols = g.mb_cols();
    let mb_rows = g.mb_rows();
    for (i, (rf, dst)) in reference.planes().into_iter().zip(prediction.planes_mut()).enumerate() {
        let (fx, fy) = g.factors(i);
        let (bw, bh) = (16 / fx, 16 / fy);
        let w = dst.width;
        let h = dst.height;
        dst.data.par_chunks_exact_mut(w).enumerate().for_each(|(y, row)| {
            let my = y / bh;
            if my >= mb_rows {
                return;
            }
            for mx in 0..cols {
                let v = vectors[my * cols + mx];
                let dx = (v.dx as f32 / fx as f32).round() as isize;
                let dy = (v.dy as f32 / fy as f32).round() as isize;
                let x0 = mx * bw;
                for x in x0..w.min(x0 + bw) {
                    row[x] = rf.clamped(x as isize + dx, y as isize + dy);
                }
            }
        });
        if bw * cols < w || bh * mb_rows < h {
            let (lx, ly) = (bw * cols - 1, bh * mb_rows - 1);
            for y in 0..h {
                for x in 0..w {
                    if x >= bw * cols || y >= bh * mb_rows {
                        dst.data[y * w + x] = dst.data[y.min(ly) * w + x.min(lx)];
                    }
                }
            }
        }
    }
    prediction
}

pub fn code_residual(source: &Picture, prediction: &Picture, step: f32, g: &Geometry) -> Picture {
    let c = basis();
    let mut residual = Picture::new(g);
    for ((src, pred), dst) in source
        .planes()
        .into_iter()
        .zip(prediction.planes())
        .zip(residual.planes_mut())
    {
        let w = src.width;
        dst.data
            .par_chunks_exact_mut(8 * w)
            .enumerate()
            .for_each(|(ty, dst_rows)| {
                let mut block = [0f32; 64];
                let mut coef = [0f32; 64];
                for tx in 0..(w / 8) {
                    let origin = ty * 8 * w + tx * 8;
                    for y in 0..8 {
                        for x in 0..8 {
                            let i = origin + y * w + x;
                            block[y * 8 + x] = src.data[i] - pred.data[i];
                        }
                    }
                    forward_dct(&block, &mut coef, c);
                    let mut any = false;
                    for k in 0..64 {
                        let q = coef[k] / step;
                        let level = if q < 0.0 { -(-q + 0.2).floor() } else { (q + 0.2).floor() };
                        coef[k] = level * step;
                        if level != 0.0 {
                            any = true;
                        }
                    }
                    if any {
                        inverse_dct(&coef, &mut block, c);
                        for y in 0..8 {
                            dst_rows[y * w + tx * 8..][..8].copy_from_slice(&block[y * 8..][..8]);
                        }
                    } else {
                        for y in 0..8 {
                            dst_rows[y * w + tx * 8..][..8].fill(0.0);
                        }
                    }
                }
            });
    }
    residual
}

pub fn reconstruct(prediction: &Picture, residual: &Picture, lost: Option<&[bool]>, g: &Geometry) -> Picture {
    let mut recon = Picture::new(g);
    let cols = g.mb_cols();
    for (i, ((pred, res), dst)) in prediction
        .planes()
        .into_iter()
        .zip(residual.planes())
        .zip(recon.planes_mut())
        .enumerate()
    {
        let (fx, fy) = g.factors(i);
        let w = dst.width;
        dst.data.par_chunks_exact_mut(w).enumerate().for_each(|(y, row)| {
            let my = (y * fy / 16).min(g.mb_rows() - 1);
            for x in 0..w {
                let k = y * w + x;
                let mut v = pred.data[k];
                let mx = (x * fx / 16).min(cols - 1);
                if lost.map_or(true, |l| !l[my * cols + mx]) {
                    v += res.data[k];
                }
                row[x] = v.round().clamp(0.0, 255.0);
            }
        });
    }
    recon
}

// MARK: errors

pub fn lost_blocks(amount: f64, index: i64, g: &Geometry) -> Option<Vec<bool>> {
    if amount <= 0.0 {
        return None;
    }
    let n = g.mb_count();
    let mut lost = vec![false; n];
    let mut rng = SplitMix::new((index as u64).wrapping_mul(0x9E37_79B9) ^ 0x10C5);
    let mean_run = 1.0 + g.mb_cols() as f64 * 2.0 / 3.0;
    let start = (amount * 0.3 / mean_run).min(1.0);
    let mut i = 0;
    let mut any = false;
    while i < n {
        if rng.unit() < start {
            let u = rng.unit();
            let run = 1 + (u * u * (2 * g.mb_cols()) as f64) as usize;
            for j in i..n.min(i + run) {
                lost[j] = true;
            }
            any = true;
            i += run;
        } else {
            i += 1;
        }
    }
    any.then_some(lost)
}

pub fn drops_key(key_index: i64, amount: f64) -> bool {
    if amount <= 0.0 || key_index <= 0 {
        return false;
    }
    let mut rng = SplitMix::new((key_index as u64).wrapping_mul(0xD1B5_4A32_D192_ED03) ^ 0xDA7A);
    rng.unit() < amount
}

// MARK: complexity

pub fn squeeze(complexity: f32, typical: f32, strictness: f64) -> f32 {
    if strictness <= 0.0 {
        return 1.0;
    }
    let ratio = (complexity / typical).max(0.05);
    ratio.powf(strictness as f32).clamp(0.5, 6.0)
}

pub fn detail(p: &Picture, g: &Geometry) -> f32 {
    let w = p.y.width;
    let rows: Vec<f32> = (0..g.height)
        .into_par_iter()
        .map(|y| {
            let mut s = 0f32;
            let row = &p.y.data[y * w..];
            let below = &p.y.data[(y + 1).min(g.height - 1) * w..];
            for x in 0..(g.width - 1) {
                s += (row[x + 1] - row[x]).abs() + (below[x] - row[x]).abs();
            }
            s
        })
        .collect();
    rows.iter().sum::<f32>() / (g.width * g.height) as f32
}

// MARK: one frame

#[derive(Clone, Copy, Debug)]
pub struct FrameSettings {
    pub quality: f64,
    pub fixed_bitrate: f64,
    pub data_loss: f64,
}

/// Code `source` as `kind`, advancing `state`. Returns the decoder's picture and the inter step.
pub fn step(state: &mut CodecState, source: &Picture, kind: FrameKind, index: i64, s: FrameSettings) -> (Arc<Picture>, f32) {
    let g = state.geometry;
    let lost = lost_blocks(s.data_loss, index, &g);

    match kind {
        FrameKind::IntraOnly | FrameKind::Key | FrameKind::DroppedKey => {
            let sq = squeeze(detail(source, &g), 14.0, s.fixed_bitrate);
            let steps = Steps::new(s.quality, sq);
            let previous = state.encoder.clone();
            let recon = Arc::new(code_intra(source, &steps, &g));
            state.encoder = Some(recon.clone());
            state.vectors = vec![MotionVector::default(); g.mb_count()];
            match kind {
                FrameKind::IntraOnly => {
                    if let (Some(lost), Some(previous)) = (&lost, &previous) {
                        let mut shown = (*recon).clone();
                        copy_blocks(previous, &mut shown, lost, &g);
                        state.decoder = Some(Arc::new(shown));
                        state.diverged = true;
                    } else {
                        state.decoder = Some(recon);
                        state.diverged = false;
                    }
                }
                FrameKind::Key => {
                    state.decoder = Some(recon);
                    state.diverged = false;
                }
                _ => {
                    if state.decoder.is_none() {
                        state.decoder = Some(Arc::new((*recon).clone()));
                    }
                    state.diverged = !Arc::ptr_eq(state.decoder.as_ref().unwrap(), &recon);
                }
            }
            (state.decoder.clone().unwrap(), steps.inter)
        }
        FrameKind::Predicted => {
            let Some(enc_ref) = state.encoder.clone() else {
                return step(state, source, FrameKind::Key, index, s);
            };
            let (vectors, error) = search_motion(source, &enc_ref, &state.vectors, &g);
            let sq = squeeze(error, 3.0, s.fixed_bitrate);
            let steps = Steps::new(s.quality, sq);
            let prediction = compensate(&enc_ref, &vectors, &g);
            let residual = code_residual(source, &prediction, steps.inter, &g);
            let enc_recon = Arc::new(reconstruct(&prediction, &residual, None, &g));

            if !state.diverged && lost.is_none() {
                state.decoder = Some(enc_recon.clone());
            } else {
                let mut dec_vectors = vectors.clone();
                if let Some(lost) = &lost {
                    let cols = g.mb_cols();
                    for i in 0..g.mb_count() {
                        if lost[i] {
                            dec_vectors[i] = if i >= cols && !lost[i - cols] {
                                dec_vectors[i - cols]
                            } else {
                                MotionVector::default()
                            };
                        }
                    }
                }
                let dec_ref = state.decoder.clone().unwrap_or_else(|| enc_ref.clone());
                let dec_prediction = compensate(&dec_ref, &dec_vectors, &g);
                let dec_recon = reconstruct(&dec_prediction, &residual, lost.as_deref(), &g);
                state.decoder = Some(Arc::new(dec_recon));
                state.diverged = true;
            }
            state.encoder = Some(enc_recon);
            state.vectors = vectors;
            (state.decoder.clone().unwrap(), steps.inter)
        }
    }
}

pub fn copy_blocks(source: &Picture, target: &mut Picture, mask: &[bool], g: &Geometry) {
    let cols = g.mb_cols();
    for (i, (src, dst)) in source.planes().into_iter().zip(target.planes_mut()).enumerate() {
        let (fx, fy) = g.factors(i);
        let w = dst.width;
        for y in 0..dst.height {
            let my = (y * fy / 16).min(g.mb_rows() - 1);
            for x in 0..w {
                if mask[my * cols + (x * fx / 16).min(cols - 1)] {
                    dst.data[y * w + x] = src.data[y * w + x];
                }
            }
        }
    }
}

// MARK: deblocking

pub fn deblock(p: &mut Picture, strength: f64, step: f32) {
    if strength <= 0.0 {
        return;
    }
    let w8 = strength as f32;
    let threshold = strength as f32 * (4.0 + 0.9 * step);
    for plane in p.planes_mut() {
        let (w, h) = (plane.width, plane.height);
        let d = &mut plane.data;
        let mut filter = |a: usize, b: usize, c: usize, e: usize| {
            let (p1, p0, q0, q1) = (d[a], d[b], d[c], d[e]);
            let delta = q0 - p0;
            if !(delta.abs() < threshold && (p1 - p0).abs() < threshold / 2.0 && (q1 - q0).abs() < threshold / 2.0) {
                return;
            }
            d[b] = p0 + delta * 0.375 * w8;
            d[c] = q0 - delta * 0.375 * w8;
            d[a] = p1 + delta * 0.125 * w8;
            d[e] = q1 - delta * 0.125 * w8;
        };
        for y in 0..h {
            let mut x = 8;
            while x < w {
                let k = y * w + x;
                filter(k - 2, k - 1, k, k + 1);
                x += 8;
            }
        }
        let mut y = 8;
        while y < h {
            for x in 0..w {
                let k = y * w + x;
                filter(k - 2 * w, k - w, k, k + w);
            }
            y += 8;
        }
    }
}
