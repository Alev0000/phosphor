//! Screen Loop: video feedback ("howlaround"), a camcorder pointed at the TV that shows the
//! camcorder's own picture. Port of NTSCRT's `Howlaround.swift` (its Metal kernels on the CPU).
//!
//! Each pass the camera films the scene with the TV in it, showing what the TV displayed `delay`
//! passes ago; that picture goes through the whole chain and onto the TV for the next pass. The
//! nth copy in the tunnel has been through everything n times.

use std::f64::consts::PI;

use rayon::prelude::*;

use crate::glitch::receiver::random;
use crate::Image;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HowlGroup {
    Framing,
    Movement,
    Camera,
    Signal,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum HowlKind {
    Slider { min: f64, max: f64, percent: bool, unit: &'static str },
    Toggle,
    /// A coordinate of a point on the frame (from its middle, in fractions of width/height, y down).
    Position { min: f64, max: f64 },
}

#[derive(Clone, Copy, Debug)]
pub struct HowlParam {
    pub id: &'static str,
    pub label: &'static str,
    pub group: HowlGroup,
    pub kind: HowlKind,
    pub default: f64,
}

use HowlGroup::*;

const fn slider(id: &'static str, label: &'static str, group: HowlGroup, min: f64, max: f64, percent: bool, unit: &'static str, default: f64) -> HowlParam {
    HowlParam { id, label, group, kind: HowlKind::Slider { min, max, percent, unit }, default }
}
const fn toggle(id: &'static str, label: &'static str, group: HowlGroup) -> HowlParam {
    HowlParam { id, label, group, kind: HowlKind::Toggle, default: 0.0 }
}
const fn position(id: &'static str, label: &'static str, group: HowlGroup, min: f64, max: f64, default: f64) -> HowlParam {
    HowlParam { id, label, group, kind: HowlKind::Position { min, max }, default }
}

pub const HOWL_PARAMS: &[HowlParam] = &[
    slider("zoom", "Zoom", Framing, 0.0, 1.4, true, "", 0.87),
    position("center_x", "Vanishing point left – right", Framing, -0.5, 0.5, -0.2),
    position("center_y", "Vanishing point up – down", Framing, -0.5, 0.5, -0.22),
    slider("roll", "Roll", Framing, -45.0, 45.0, false, "°", 0.0),
    slider("turn", "Side angle", Framing, -45.0, 45.0, false, "°", -4.0),
    slider("tilt", "Vertical angle", Framing, -45.0, 45.0, false, "°", 0.0),
    slider("shake", "Handheld", Movement, 0.0, 1.0, true, "", 0.4),
    slider("seed", "Random seed", Movement, 1.0, 99.0, false, "", 1.0),
    position("drift_x", "Drift left – right", Movement, -1.0, 1.0, 0.0),
    position("drift_y", "Drift up – down", Movement, -1.0, 1.0, 0.0),
    slider("push", "Push", Movement, -0.5, 0.5, true, "", 0.0),
    slider("spin", "Spin", Movement, -180.0, 180.0, false, "°", 0.0),
    toggle("loop", "Seamless loop", Movement),
    slider("focus", "Softness", Camera, 0.0, 1.0, true, "", 0.2),
    slider("brightness", "Screen brightness", Camera, 0.5, 1.5, true, "", 1.0),
    slider("contrast", "Contrast", Camera, 0.6, 1.6, true, "", 1.05),
    slider("color_drift", "Color drift", Camera, -1.0, 1.0, true, "", -0.3),
    slider("hue_drift", "Hue drift", Camera, -30.0, 30.0, false, "° per pass", 0.0),
    slider("auto_exposure", "Auto exposure", Camera, 0.0, 1.0, true, "", 0.0),
    slider("key", "Luma key", Signal, 0.0, 1.0, true, "", 0.0),
    toggle("key_invert", "Key the dark parts", Signal),
    slider("delay", "Delay", Signal, 1.0, 20.0, false, "frames", 1.0),
    toggle("counter", "Camcorder counter", Signal),
];

pub fn param(id: &str) -> Option<&'static HowlParam> {
    HOWL_PARAMS.iter().find(|p| p.id == id)
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Pose {
    pub zoom: f64,
    pub center_x: f64,
    pub center_y: f64,
    pub roll: f64,
    pub turn: f64,
    pub tilt: f64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct HowlSettings {
    pub values: Vec<(&'static str, f64)>,
}

impl Default for HowlSettings {
    fn default() -> Self {
        Self { values: HOWL_PARAMS.iter().map(|p| (p.id, p.default)).collect() }
    }
}

impl HowlSettings {
    pub fn get(&self, id: &str) -> f64 {
        self.values
            .iter()
            .find(|(k, _)| *k == id)
            .map(|(_, v)| *v)
            .or_else(|| param(id).map(|p| p.default))
            .unwrap_or(0.0)
    }

    pub fn set(&mut self, id: &str, v: f64) {
        if let Some(p) = param(id) {
            match self.values.iter_mut().find(|(k, _)| *k == p.id) {
                Some(e) => e.1 = v,
                None => self.values.push((p.id, v)),
            }
        }
    }

    pub fn zoom(&self) -> f64 {
        self.get("zoom").max(0.0)
    }
    pub fn tv_in_view(&self) -> bool {
        self.zoom() > 0.001
    }
    pub fn delay(&self) -> usize {
        (self.get("delay").round() as i64).clamp(1, 20) as usize
    }
    pub fn counter(&self) -> bool {
        self.get("counter") >= 0.5
    }
    pub fn seamless(&self) -> bool {
        self.get("loop") >= 0.5
    }
    pub fn seed(&self) -> u64 {
        (self.get("seed").round() as i64).max(1) as u64
    }

    pub fn base_pose(&self) -> Pose {
        Pose {
            zoom: self.zoom(),
            center_x: self.get("center_x"),
            center_y: self.get("center_y"),
            roll: self.get("roll"),
            turn: self.get("turn"),
            tilt: self.get("tilt"),
        }
    }

    const HANDHELD_CENTER: (f64, f64) = (0.050, 0.045);

    /// Where the camera is `t` seconds into a render `length` seconds long.
    pub fn pose(&self, t: f64, length: f64) -> Pose {
        let mut p = self.base_pose();
        if !self.tv_in_view() {
            return p;
        }
        let l = length.max(0.1);
        let m = if self.seamless() {
            let s = (PI * t / l).sin();
            s * s
        } else {
            let x = (t / l).clamp(0.0, 1.0);
            x * x * (3.0 - 2.0 * x)
        };
        p.center_x += self.get("drift_x") * m;
        p.center_y += self.get("drift_y") * m;
        p.zoom *= (1.0 + self.get("push") * m).max(0.05);
        p.roll += self.get("spin") * m;
        let shake = self.get("shake");
        if shake > 0.0 {
            let hands = Hands::new(self.seed(), self.seamless().then_some(l));
            p.center_x += shake * Self::HANDHELD_CENTER.0 * hands.value(0, t);
            p.center_y += shake * Self::HANDHELD_CENTER.1 * hands.value(1, t);
            p.turn += shake * 6.0 * hands.value(2, t);
            p.tilt += shake * 4.5 * hands.value(3, t);
            p.roll += shake * 3.5 * hands.value(4, t);
            p.zoom *= 1.0 + shake * 0.06 * hands.value(5, t);
        }
        p
    }

    /// How deep the tunnel goes (passes until its center is a couple of scan lines), or None when
    /// the copies grow instead.
    pub fn visible_copies(&self, aspect: f64, chain_height: usize) -> Option<usize> {
        if !self.tv_in_view() {
            return Some(0);
        }
        let p = self.base_pose();
        let rate = shrink(&p, (p.center_x + 0.5, p.center_y + 0.5), aspect)?;
        if rate >= 0.995 {
            return None;
        }
        let n = (2.0 / chain_height.max(16) as f64).ln() / rate.ln();
        Some((n.floor() as usize).clamp(1, 200))
    }

    /// The run-up before the first frame: `build` passes at one frame each, then `settle` at the
    /// real delay.
    pub fn run_up_plan(&self, aspect: f64, chain_height: usize) -> (usize, usize) {
        if !self.tv_in_view() {
            return (0, 0);
        }
        let copies = self.visible_copies(aspect, chain_height).unwrap_or(60);
        let exposure = if self.get("auto_exposure") > 0.0 { 30 } else { 0 };
        let build = copies + 4 + exposure;
        let settle = if self.seamless() { (self.delay() * (copies + 4).min(12)).min(400) } else { self.delay() };
        (build, settle)
    }
}

type V3 = [f64; 3];

fn add(a: V3, b: V3) -> V3 {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}
fn sub(a: V3, b: V3) -> V3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
fn mul(a: V3, k: f64) -> V3 {
    [a[0] * k, a[1] * k, a[2] * k]
}

/// The screen's axes, normal and center in camera space (image plane at z = 1, frame height 1 and
/// width `aspect`, y down), placed so the tunnel's center maps to itself pass after pass.
pub fn screen(pose: &Pose, aspect: f64) -> (V3, V3, V3, V3) {
    let deg = PI / 180.0;
    let yaw = pose.turn.clamp(-60.0, 60.0) * deg;
    let pitch = pose.tilt.clamp(-60.0, 60.0) * deg;
    let roll = pose.roll * deg;
    let ry = |v: V3| [yaw.cos() * v[0] + yaw.sin() * v[2], v[1], -yaw.sin() * v[0] + yaw.cos() * v[2]];
    let rx = |v: V3| [v[0], pitch.cos() * v[1] - pitch.sin() * v[2], pitch.sin() * v[1] + pitch.cos() * v[2]];
    let rz = |v: V3| [roll.cos() * v[0] - roll.sin() * v[1], roll.sin() * v[0] + roll.cos() * v[1], v[2]];
    let r = |v: V3| ry(rx(rz(v)));
    let ax = r([1.0, 0.0, 0.0]);
    let ay = r([0.0, 1.0, 0.0]);
    let q = [pose.center_x * aspect, pose.center_y, 1.0];
    let offset = mul(add(mul(ax, pose.center_x * aspect), mul(ay, pose.center_y)), pose.zoom);
    let along = (1.0 + offset[2]).max(0.25);
    (ax, ay, r([0.0, 0.0, 1.0]), sub(mul(q, along), offset))
}

fn camera_point(s: (f64, f64), pose: &Pose, aspect: f64) -> Option<(f64, f64)> {
    let (ax, ay, _, c) = screen(pose, aspect);
    let p = add(add(c, mul(ax, (s.0 - 0.5) * pose.zoom * aspect)), mul(ay, (s.1 - 0.5) * pose.zoom));
    if p[2] <= 1e-4 {
        return None;
    }
    Some((p[0] / p[2] / aspect + 0.5, p[1] / p[2] + 0.5))
}

fn shrink(pose: &Pose, s: (f64, f64), aspect: f64) -> Option<f64> {
    let e = 1e-4;
    let px = camera_point((s.0 + e, s.1), pose, aspect)?;
    let mx = camera_point((s.0 - e, s.1), pose, aspect)?;
    let py = camera_point((s.0, s.1 + e), pose, aspect)?;
    let my = camera_point((s.0, s.1 - e), pose, aspect)?;
    let jx = ((px.0 - mx.0) / (2.0 * e), (px.1 - mx.1) / (2.0 * e));
    let jy = ((py.0 - my.0) / (2.0 * e), (py.1 - my.1) / (2.0 * e));
    let tr = jx.0 + jy.1;
    let det = jx.0 * jy.1 - jy.0 * jx.1;
    let disc = tr * tr / 4.0 - det;
    Some(if disc >= 0.0 {
        let r = disc.sqrt();
        (tr / 2.0 + r).abs().max((tr / 2.0 - r).abs())
    } else {
        det.abs().sqrt()
    })
}

/// The camera operator's hands: a slow sway plus a little tremor, the same for the same seed.
struct Hands {
    parts: Vec<Vec<(f64, f64, f64)>>,
}

impl Hands {
    fn new(seed: u64, period: Option<f64>) -> Self {
        let snap = |f: f64| match period {
            Some(p) => (f * p).round().max(1.0) / p,
            None => f,
        };
        let mut parts = Vec::with_capacity(6);
        for axis in 0..6u64 {
            let mut p = Vec::new();
            let mut power = 0.0;
            for k in 0..3i64 {
                let u = random::uniform(seed, 900 + axis, k, 0);
                let v = random::uniform(seed, 920 + axis, k, 0);
                let w = random::uniform(seed, 940 + axis, k, 0);
                let amp = 0.5 + 0.5 * w;
                p.push((snap(0.15 + 0.55 * u), 2.0 * PI * v, amp));
                power += amp * amp / 2.0;
            }
            let scale = 0.45 / power.sqrt();
            for e in &mut p {
                e.2 *= scale;
            }
            for k in 0..2i64 {
                let u = random::uniform(seed, 960 + axis, k, 0);
                let v = random::uniform(seed, 980 + axis, k, 0);
                p.push((snap(4.0 + 3.0 * u), 2.0 * PI * v, 0.02));
            }
            parts.push(p);
        }
        Self { parts }
    }

    fn value(&self, axis: usize, t: f64) -> f64 {
        self.parts[axis].iter().map(|(f, ph, a)| a * (2.0 * PI * f * t + ph).sin()).sum()
    }
}

/// RGBA8 picture: the TV's pictures kept in the loop (they come out of an 8-bit shader target).
#[derive(Clone, Debug)]
pub struct Rgba8 {
    pub width: usize,
    pub height: usize,
    pub px: Vec<[u8; 4]>,
}

impl Rgba8 {
    pub fn black(width: usize, height: usize) -> Self {
        Self { width, height, px: vec![[0, 0, 0, 255]; width * height] }
    }

    pub fn from_image(img: &Image) -> Self {
        Self {
            width: img.width,
            height: img.height,
            px: img
                .pixels
                .iter()
                .map(|p| {
                    let c = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
                    [c(p[0]), c(p[1]), c(p[2]), 255]
                })
                .collect(),
        }
    }

    pub fn bytes(&self) -> usize {
        self.px.len() * 4
    }

    #[inline]
    fn texel(&self, x: isize, y: isize) -> [f32; 3] {
        let p = self.px[y.clamp(0, self.height as isize - 1) as usize * self.width + x.clamp(0, self.width as isize - 1) as usize];
        [p[0] as f32 / 255.0, p[1] as f32 / 255.0, p[2] as f32 / 255.0]
    }

    /// Bilinear, normalized coordinates, clamp to edge.
    fn sample(&self, u: f32, v: f32) -> [f32; 3] {
        let x = u * self.width as f32 - 0.5;
        let y = v * self.height as f32 - 0.5;
        let (x0, y0) = (x.floor(), y.floor());
        let (fx, fy) = (x - x0, y - y0);
        let (x0, y0) = (x0 as isize, y0 as isize);
        let a = self.texel(x0, y0);
        let b = self.texel(x0 + 1, y0);
        let c = self.texel(x0, y0 + 1);
        let d = self.texel(x0 + 1, y0 + 1);
        let mut o = [0f32; 3];
        for i in 0..3 {
            let top = a[i] + (b[i] - a[i]) * fx;
            let bot = c[i] + (d[i] - c[i]) * fx;
            o[i] = top + (bot - top) * fy;
        }
        o
    }

    pub fn blurred(&self, sigma: f64) -> Rgba8 {
        let radius = ((2.5 * sigma).ceil() as isize).min(64);
        let k: Vec<f32> = (-radius..=radius).map(|d| (-0.5 * (d * d) as f64 / (sigma * sigma)).exp() as f32).collect();
        let sum: f32 = k.iter().sum();
        let (w, h) = (self.width, self.height);
        let pass = |src: &Vec<[f32; 3]>, horizontal: bool| -> Vec<[f32; 3]> {
            (0..h)
                .into_par_iter()
                .flat_map_iter(|y| {
                    let k = &k;
                    (0..w).map(move |x| {
                        let mut acc = [0f32; 3];
                        for d in -radius..=radius {
                            let (sx, sy) = if horizontal {
                                ((x as isize + d).clamp(0, w as isize - 1) as usize, y)
                            } else {
                                (x, (y as isize + d).clamp(0, h as isize - 1) as usize)
                            };
                            let p = src[sy * w + sx];
                            let wt = k[(d + radius) as usize];
                            for c in 0..3 {
                                acc[c] += p[c] * wt;
                            }
                        }
                        acc.map(|v| v / sum)
                    })
                })
                .collect()
        };
        let src: Vec<[f32; 3]> = self.px.iter().map(|p| [p[0] as f32, p[1] as f32, p[2] as f32]).collect();
        let out = pass(&pass(&src, true), false);
        Rgba8 {
            width: w,
            height: h,
            px: out.iter().map(|p| [p[0].round() as u8, p[1].round() as u8, p[2].round() as u8, 255]).collect(),
        }
    }
}

/// The camcorder counter ("0:04"): white figures with a dark edge, as a coverage mask.
pub fn counter_mask(secs: i64) -> (usize, usize, Vec<f32>, Vec<f32>) {
    const FONT: [[u8; 7]; 11] = [
        [0b01110, 0b10001, 0b10011, 0b10101, 0b11001, 0b10001, 0b01110],
        [0b00100, 0b01100, 0b00100, 0b00100, 0b00100, 0b00100, 0b01110],
        [0b01110, 0b10001, 0b00001, 0b00010, 0b00100, 0b01000, 0b11111],
        [0b11111, 0b00010, 0b00100, 0b00010, 0b00001, 0b10001, 0b01110],
        [0b00010, 0b00110, 0b01010, 0b10010, 0b11111, 0b00010, 0b00010],
        [0b11111, 0b10000, 0b11110, 0b00001, 0b00001, 0b10001, 0b01110],
        [0b00110, 0b01000, 0b10000, 0b11110, 0b10001, 0b10001, 0b01110],
        [0b11111, 0b00001, 0b00010, 0b00100, 0b01000, 0b01000, 0b01000],
        [0b01110, 0b10001, 0b10001, 0b01110, 0b10001, 0b10001, 0b01110],
        [0b01110, 0b10001, 0b10001, 0b01111, 0b00001, 0b00010, 0b01100],
        [0b00000, 0b01100, 0b01100, 0b00000, 0b01100, 0b01100, 0b00000],
    ];
    let secs = secs.max(0);
    let text = format!("{}:{:02}", secs / 60, secs % 60);
    let glyphs: Vec<usize> = text.chars().map(|c| if c == ':' { 10 } else { c.to_digit(10).unwrap_or(0) as usize }).collect();
    // Glyph cells 6 wide (5 + gap), 7 tall, with a 1-cell pad for the edge.
    let (w, h) = (glyphs.len() * 6 + 2, 9);
    let mut fill = vec![0f32; w * h];
    for (i, g) in glyphs.iter().enumerate() {
        for (row, bits) in FONT[*g].iter().enumerate() {
            for col in 0..5 {
                if bits & (1 << (4 - col)) != 0 {
                    fill[(row + 1) * w + 1 + i * 6 + col] = 1.0;
                }
            }
        }
    }
    let mut edge = vec![0f32; w * h];
    for y in 0..h as isize {
        for x in 0..w as isize {
            let mut e = 0f32;
            for (dx, dy) in [(-1, 0), (1, 0), (0, -1), (0, 1), (1, 1)] {
                let (sx, sy) = (x + dx, y + dy);
                if sx >= 0 && sy >= 0 && sx < w as isize && sy < h as isize {
                    e = e.max(fill[sy as usize * w + sx as usize]);
                }
            }
            edge[y as usize * w + x as usize] = e * 0.75;
        }
    }
    (w, h, fill, edge)
}

/// The camera kernel's inputs, laid out as NTSCRT's Metal `CameraUniforms` (and the HLSL cbuffer).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct CameraUniforms {
    pub axis_x: [f32; 4],
    pub axis_y: [f32; 4],
    pub normal: [f32; 4],
    pub center: [f32; 4],
    pub balance: [f32; 4],
    pub counter_rect: [f32; 4],
    /// aspect, zoom, brightness, contrast
    pub params: [f32; 4],
    /// hue cos, hue sin, exposure, TV in view
    pub params2: [f32; 4],
    /// key level, key invert, center dot normal, unused
    pub key: [f32; 4],
}

pub struct CameraSetup {
    pub uniforms: CameraUniforms,
    /// Focus blur of the TV picture, in its own pixels (0: none).
    pub sigma: f32,
    /// The counter mask: width, height, fill, edge.
    pub counter: Option<(usize, usize, Vec<f32>, Vec<f32>)>,
}

/// The camera on the CPU (the reference the GPU version is checked against).
pub fn camera_cpu(scene: &Image, tv: &Rgba8, setup: &CameraSetup) -> Image {
    let blurred;
    let tv = if setup.sigma > 0.0 {
        blurred = tv.blurred(setup.sigma as f64);
        &blurred
    } else {
        tv
    };
    let u = &setup.uniforms;
    let (w, h) = (scene.width, scene.height);
    let (axf, ayf, nf, cf) = (u.axis_x, u.axis_y, u.normal, u.center);
    let [aspect_f, zoom, bright, contrast] = u.params;
    let [hc, hs, exposure, in_view] = u.params2;
    let [key, key_invert, cn, _] = u.key;
    let balance = u.balance;
    let rect = u.counter_rect;
    let mut out = Image::uninit(w, h);
    out.pixels.par_chunks_exact_mut(w).enumerate().for_each(|(gy, row)| {
        for (gx, px) in row.iter_mut().enumerate() {
            let sp = scene.pixels[gy * w + gx];
            let mut col = [sp[0], sp[1], sp[2]];
            let uv = ((gx as f32 + 0.5) / w as f32, (gy as f32 + 0.5) / h as f32);
            if in_view > 0.5 {
                let d = [(uv.0 - 0.5) * aspect_f, uv.1 - 0.5, 1.0];
                let denom = d[0] * nf[0] + d[1] * nf[1] + d[2] * nf[2];
                if denom.abs() > 1e-6 {
                    let t = cn / denom;
                    if t > 0.0 {
                        let p = [d[0] * t - cf[0], d[1] * t - cf[1], d[2] * t - cf[2]];
                        let sx = (p[0] * axf[0] + p[1] * axf[1] + p[2] * axf[2]) / (zoom * aspect_f) + 0.5;
                        let sy = (p[0] * ayf[0] + p[1] * ayf[1] + p[2] * ayf[2]) / zoom + 0.5;
                        let ex = sx.min(1.0 - sx) * zoom * w as f32;
                        let ey = sy.min(1.0 - sy) * zoom * h as f32;
                        let cover = (ex.min(ey) + 0.5).clamp(0.0, 1.0);
                        if cover > 0.0 {
                            let mut s = tv.sample(sx, sy);
                            for i in 0..3 {
                                s[i] = ((s[i] * bright - 0.5) * contrast + 0.5) * balance[i];
                            }
                            let y = 0.299 * s[0] + 0.587 * s[1] + 0.114 * s[2];
                            let i = 0.596 * s[0] - 0.274 * s[1] - 0.322 * s[2];
                            let q = 0.211 * s[0] - 0.523 * s[1] + 0.312 * s[2];
                            let i2 = i * hc - q * hs;
                            let q2 = i * hs + q * hc;
                            let s = [
                                (y + 0.956 * i2 + 0.621 * q2).clamp(0.0, 1.0),
                                (y - 0.272 * i2 - 0.647 * q2).clamp(0.0, 1.0),
                                (y - 1.106 * i2 + 1.703 * q2).clamp(0.0, 1.0),
                            ];
                            for i in 0..3 {
                                col[i] += (s[i] - col[i]) * cover;
                            }
                        }
                    }
                }
            }
            if key > 0.0 {
                let mut y = 0.299 * sp[0] + 0.587 * sp[1] + 0.114 * sp[2];
                if key_invert > 0.5 {
                    y = 1.0 - y;
                }
                let level = 1.0 - key;
                let tt = ((y - (level - 0.06)) / 0.12).clamp(0.0, 1.0);
                let m = tt * tt * (3.0 - 2.0 * tt);
                for i in 0..3 {
                    col[i] += (sp[i] - col[i]) * m;
                }
            }
            for c in &mut col {
                *c *= exposure;
            }
            if let Some((cw, ch, fill, edge)) = &setup.counter {
                if uv.0 >= rect[0] && uv.0 <= rect[2] && uv.1 >= rect[1] && uv.1 <= rect[3] {
                    let fx = ((uv.0 - rect[0]) / (rect[2] - rect[0]) * *cw as f32) as usize;
                    let fy = ((uv.1 - rect[1]) / (rect[3] - rect[1]) * *ch as f32) as usize;
                    let k = fy.min(ch - 1) * cw + fx.min(cw - 1);
                    let (f, e) = (fill[k], edge[k]);
                    let a = f.max(e);
                    for c in &mut col {
                        *c = *c * (1.0 - a) + f;
                    }
                }
            }
            *px = [col[0].clamp(0.0, 1.0), col[1].clamp(0.0, 1.0), col[2].clamp(0.0, 1.0), 1.0];
        }
    });
    out
}

/// The loop's state between passes.
#[derive(Clone)]
pub struct HowlLoop {
    pub feedback_size: (usize, usize),
    pub chain_size: (usize, usize),
    ring: Vec<Rgba8>,
    frame: usize,
    lag: usize,
    exposure: f64,
    measured: Option<(f64, f64)>,
}

impl HowlLoop {
    /// `chain_size` is the chain input (the Downscale size, else the source's).
    pub fn new(settings: &HowlSettings, chain_size: (usize, usize)) -> Self {
        let (w, h) = (chain_size.0.max(1), chain_size.1.max(1));
        let mut k = 2usize.max(2 * (640.0 / w as f64).ceil() as usize);
        while k > 1 && w * k > 2048 {
            k -= 1;
        }
        let feedback_size = (w * k, h * k);
        let ring = (0..=settings.delay()).map(|_| Rgba8::black(feedback_size.0, feedback_size.1)).collect();
        Self {
            feedback_size,
            chain_size,
            ring,
            frame: 0,
            lag: settings.delay(),
            exposure: 1.0,
            measured: None,
        }
    }

    pub fn bytes(&self) -> usize {
        self.ring.iter().map(Rgba8::bytes).sum()
    }

    fn write_slot(&self) -> usize {
        self.frame % self.ring.len()
    }

    fn read_slot(&self) -> usize {
        let n = self.ring.len() as isize;
        (((self.frame as isize - self.lag as isize) % n + n) % n) as usize
    }

    pub fn set_lag(&mut self, lag: usize) {
        self.lag = lag.clamp(1, self.ring.len() - 1);
    }

    /// The TV picture the camera films this pass.
    pub fn tv(&self) -> &Rgba8 {
        &self.ring[self.read_slot()]
    }

    /// Everything the camera kernel needs for this pass (see `camera_cpu` and the GPU version).
    pub fn camera_setup(&self, w: usize, h: usize, settings: &HowlSettings, time: f64, length: f64) -> CameraSetup {
        let aspect = w as f64 / h.max(1) as f64;
        let l = length.max(0.1);
        let t = if time >= 0.0 && !settings.seamless() { time % l } else { time };
        let pose = settings.pose(t, l);

        let sigma_lines = 2.5 * settings.get("focus");
        let scale = self.feedback_size.1 as f64 / self.chain_size.1.max(1) as f64;
        let sigma = (sigma_lines * scale / pose.zoom.min(1.0).max(0.25)).min(24.0);
        let in_view = settings.tv_in_view();

        let (ax, ay, n, c) = screen(&pose, aspect);
        let wb = 0.07 * settings.get("color_drift");
        let (r, b) = (1.0 + wb, 1.0 - wb);
        let luma = 0.299 * r + 0.587 + 0.114 * b;
        let hue = settings.get("hue_drift") * PI / 180.0;
        let f4 = |v: V3, w: f32| [v[0] as f32, v[1] as f32, v[2] as f32, w];
        let (nf, cf) = (f4(n, 0.0), f4(c, 0.0));
        let cn = cf[0] * nf[0] + cf[1] * nf[1] + cf[2] * nf[2];

        let counter = settings.counter().then(|| counter_mask(time.floor() as i64));
        let rect = match &counter {
            Some((cw, ch, _, _)) => {
                let hgt = 0.075;
                let wid = hgt * (*cw as f64 / *ch as f64) / aspect;
                [0.07, (0.86 - hgt) as f32, (0.07 + wid) as f32, 0.86]
            }
            None => [0.0; 4],
        };
        CameraSetup {
            uniforms: CameraUniforms {
                axis_x: f4(ax, 0.0),
                axis_y: f4(ay, 0.0),
                normal: nf,
                center: cf,
                balance: [(r / luma) as f32, (1.0 / luma) as f32, (b / luma) as f32, 0.0],
                counter_rect: rect,
                params: [aspect as f32, pose.zoom.max(0.001) as f32, settings.get("brightness") as f32, settings.get("contrast") as f32],
                params2: [hue.cos() as f32, hue.sin() as f32, self.exposure as f32, if in_view { 1.0 } else { 0.0 }],
                key: [
                    if in_view { settings.get("key") as f32 } else { 0.0 },
                    if settings.get("key_invert") >= 0.5 { 1.0 } else { 0.0 },
                    cn,
                    0.0,
                ],
            },
            sigma: if in_view && sigma > 0.3 { sigma as f32 } else { 0.0 },
            counter,
        }
    }

    /// The camcorder's exposure meter: its own picture against the scene, acted on a pass late.
    pub fn measure(&mut self, camera: &Image, scene: &Image, settings: &HowlSettings) {
        if settings.get("auto_exposure") > 0.0 {
            let mean = |img: &Image| -> f64 {
                img.pixels.par_iter().map(|p| (0.299 * p[0] + 0.587 * p[1] + 0.114 * p[2]) as f64).sum::<f64>()
                    / img.pixels.len().max(1) as f64
            };
            self.measured = Some((mean(camera), mean(scene)));
        }
    }

    /// What the camera sees: the scene with the TV in it showing the picture from `lag` passes ago.
    pub fn camera_image(&mut self, scene: &Image, settings: &HowlSettings, time: f64, length: f64) -> Image {
        let setup = self.camera_setup(scene.width, scene.height, settings, time, length);
        let out = camera_cpu(scene, self.tv(), &setup);
        self.measure(&out, scene, settings);
        out
    }

    /// Store the TV's picture for this pass and rotate the loop; the auto exposure reacts to what it
    /// measured, a pass late.
    pub fn advance(&mut self, tv_picture: &Image, settings: &HowlSettings) {
        let slot = self.write_slot();
        self.ring[slot] = Rgba8::from_image(tv_picture);
        if let Some((seen, target)) = self.measured.take() {
            let rate = 0.6 * settings.get("auto_exposure");
            self.exposure = (self.exposure * (target.max(0.02) / seen.max(0.02)).powf(rate)).clamp(0.25, 4.0);
        }
        self.frame += 1;
    }

    /// Copy the latest TV picture into every slot (switching from the run-up's one-frame lag to the
    /// real delay).
    pub fn fill_history(&mut self) {
        let n = self.ring.len();
        let latest = self.ring[(self.frame + n - 1) % n].clone();
        for slot in &mut self.ring {
            *slot = latest.clone();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tunnel_center_is_a_fixed_point() {
        let s = HowlSettings::default();
        let p = s.base_pose();
        let aspect = 4.0 / 3.0;
        let c = (p.center_x + 0.5, p.center_y + 0.5);
        let mapped = camera_point(c, &p, aspect).unwrap();
        assert!((mapped.0 - c.0).abs() < 1e-9 && (mapped.1 - c.1).abs() < 1e-9);
        let copies = s.visible_copies(aspect, 240).unwrap();
        assert!(copies > 5 && copies < 200, "{copies}");
    }

    #[test]
    fn zero_zoom_shows_the_scene() {
        let mut s = HowlSettings::default();
        s.set("zoom", 0.0);
        let mut scene = Image::new(64, 48);
        scene.pixels.iter_mut().enumerate().for_each(|(i, p)| *p = [(i % 64) as f32 / 63.0, 0.5, 0.25, 1.0]);
        let mut l = HowlLoop::new(&s, (64, 48));
        let out = l.camera_image(&scene, &s, 0.0, 5.0);
        assert_eq!(out.pixels, scene.pixels);
    }
}
