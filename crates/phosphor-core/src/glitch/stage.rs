//! CPU port of NTSCRT's `GlitchStage.swift` Metal kernels (gate, chroma loop, compose).

use rayon::prelude::*;

use super::receiver::{GlitchDropout, GlitchFieldPlan, GlitchRow, Knobs, timing as T};
use crate::Image;

const SYNC_END: f32 = 4.7;
const BURST_S: f32 = 5.3;
const BURST_E: f32 = 7.8;
const GATE_S: f32 = 4.55;
const GATE_E: f32 = 8.55;
const ACT_S: f32 = 9.4;
const ACT_L: f32 = 52.656;
const EQ_PULSE: f32 = 2.3;
const BROAD: f32 = 27.1;
const BURST_DIR: [f32; 2] = [0.5446390350, -0.8386705679];
const TAU32: f32 = 6.2831853;

struct Uniforms {
    width: u32,
    vbi_lines: u32,
    total_lines: u32,
    pre_eq_end: u32,
    vsync_end: u32,
    post_eq_end: u32,
    cc_enabled: bool,
    cc_bits: u32,
    field_index: u32,
    seed: u32,
    doc_enabled: bool,
    taps_i: u32,
    taps_q: u32,
    color_on_init: bool,
    us_per_pixel: f32,
    line_us: f32,
    ghost_level: f32,
    ghost_delay: f32,
    ghost_cos: f32,
    ghost_sin: f32,
    brightness: f32,
    picture_gain: f32,
    burst_amp: f32,
    chroma_alpha: f32,
    acc_gamma: f32,
    killer_kappa: f32,
    phase_init: f32,
    gain_init: f32,
    killer_init: f32,
    gate_noise: f32,
}

#[inline]
fn rgb2yiq(c: [f32; 3]) -> [f32; 3] {
    [
        0.299 * c[0] + 0.587 * c[1] + 0.114 * c[2],
        0.596 * c[0] - 0.274 * c[1] - 0.322 * c[2],
        0.211 * c[0] - 0.523 * c[1] + 0.312 * c[2],
    ]
}

#[inline]
fn yiq2rgb(y: [f32; 3]) -> [f32; 3] {
    [
        y[0] + 0.9561706854 * y[1] + 0.6214325663 * y[2],
        y[0] - 0.2726886023 * y[1] - 0.6468132370 * y[2],
        y[0] - 1.1037440822 * y[1] + 1.7006230947 * y[2],
    ]
}

#[inline]
fn ire2y(ire: f32) -> f32 {
    (ire - 7.5) / 92.5
}

#[inline]
fn hash4(a: u32, b: u32, c: u32, d: u32) -> u32 {
    let mut h = a.wrapping_mul(0x8DA6B343)
        ^ b.wrapping_mul(0xD8163841)
        ^ c.wrapping_mul(0xCB1AB31F)
        ^ d.wrapping_mul(0x165667B1);
    h ^= h >> 15;
    h = h.wrapping_mul(0x2C1B3C6D);
    h ^= h >> 12;
    h = h.wrapping_mul(0x297A2D39);
    h ^= h >> 15;
    h
}

#[inline]
fn uni(a: u32, b: u32, c: u32, d: u32) -> f32 {
    ((hash4(a, b, c, d) >> 8) as f32 + 0.5) * (1.0 / 16777216.0)
}

#[inline]
fn gauss(a: u32, b: u32, c: u32, d: u32) -> f32 {
    let u1 = uni(a, b, c, d);
    let u2 = uni(a ^ 0x5bd1e995, b, c, d.wrapping_add(7));
    (-2.0 * u1.ln()).sqrt() * (TAU32 * u2).cos()
}

#[inline]
fn mix(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

#[inline]
fn smoothstep(e0: f32, e1: f32, x: f32) -> f32 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

fn locate(rw: &GlitchRow, mut u: f32, l: u32) -> (i32, f32) {
    let mut line = rw.field_line;
    if u < 0.0 {
        u += rw.len_prev;
        line -= 1;
    } else if u >= rw.len_cur {
        u -= rw.len_cur;
        line += 1;
        if u >= rw.len_next {
            u -= rw.len_next;
            line += 1;
        }
    }
    let il = l as i32;
    (((line % il) + il) % il, u)
}

fn in_dropout(drops: &[GlitchDropout], line: i32, u: f32) -> Option<f32> {
    drops
        .iter()
        .find(|d| d.field_line == line && u >= d.u_start && u < d.u_start + d.u_length)
        .map(|d| d.u_start)
}

fn caption_ire(u: f32, bits: u32) -> f32 {
    let t = u - 10.5;
    let period = 1.986f32;
    if t < 0.0 {
        return 0.0;
    }
    if t < 7.0 * period {
        return 25.0 - 25.0 * (TAU32 * t / period).cos();
    }
    let b = (t - 7.0 * period) / period;
    let k = b.floor() as i32;
    if !(0..19).contains(&k) {
        return 0.0;
    }
    let on = ((bits >> k as u32) & 1) as f32;
    let prev = if k > 0 { ((bits >> (k - 1) as u32) & 1) as f32 } else { 0.0 };
    let edge = smoothstep(0.0, 0.25, b - k as f32);
    50.0 * mix(prev, on, edge)
}

struct Sig {
    yiq: [f32; 3],
    rgb: [f32; 3],
    picture: bool,
    exact: bool,
}

#[inline]
fn texel(src: &Image, x: i32, y: i32) -> [f32; 3] {
    let p = src.get(x as usize, y as usize);
    [p[0], p[1], p[2]]
}

fn signal_at(src: &Image, u_: &Uniforms, line: i32, u: f32) -> Sig {
    let mut s = Sig {
        yiq: [0.0; 3],
        rgb: [0.0; 3],
        picture: false,
        exact: false,
    };
    let vbi = u_.vbi_lines as i32;
    let half_line = u_.line_us * 0.5;
    let mut ire = 0.0f32;
    if line < vbi {
        if line < u_.pre_eq_end as i32
            || (line >= u_.vsync_end as i32 && line < u_.post_eq_end as i32)
        {
            if u < EQ_PULSE || (u >= half_line && u < half_line + EQ_PULSE) {
                ire = -40.0;
            }
        } else if line < u_.vsync_end as i32 {
            if u < BROAD || (u >= half_line && u < half_line + BROAD) {
                ire = -40.0;
            }
        } else if u < SYNC_END {
            ire = -40.0;
        } else if line == vbi - 1 && u_.cc_enabled && u >= ACT_S && u < ACT_S + ACT_L {
            ire = caption_ire(u, u_.cc_bits);
        }
        s.yiq = [ire2y(ire), 0.0, 0.0];
        return s;
    }
    if u < SYNC_END {
        s.yiq = [ire2y(-40.0), 0.0, 0.0];
        return s;
    }
    if u < ACT_S || u >= ACT_S + ACT_L {
        s.yiq = [ire2y(0.0), 0.0, 0.0];
        return s;
    }
    let row = line - vbi;
    let xs = (u - ACT_S) / u_.us_per_pixel - 0.5;
    let x0 = xs.floor();
    let f = xs - x0;
    let w = u_.width as i32;
    let ix = x0 as i32;
    if !(1e-3..=1.0 - 1e-3).contains(&f) {
        let xi = (if f < 0.5 { ix } else { ix + 1 }).clamp(0, w - 1);
        s.rgb = texel(src, xi, row);
        s.exact = true;
    } else {
        let a = texel(src, ix.clamp(0, w - 1), row);
        let b = texel(src, (ix + 1).clamp(0, w - 1), row);
        s.rgb = [mix(a[0], b[0], f), mix(a[1], b[1], f), mix(a[2], b[2], f)];
    }
    s.picture = true;
    s.yiq = rgb2yiq(s.rgb);
    s
}

fn snap(v: f32, target: f32) -> f32 {
    if (v - target).abs() < 1e-6 { target } else { v }
}

/// Draw the field `plan` describes. `input` is the chain input (rows = active lines).
pub fn render_field(input: &Image, plan: &GlitchFieldPlan) -> Image {
    let raster = &plan.raster;
    let rows = &plan.rows;
    if rows.len() != input.height || rows.is_empty() {
        return input.clone();
    }
    let s = &plan.settings;
    let knobs = Knobs::new(s, raster);
    let us_per_pixel = T::ACTIVE_LENGTH / input.width as f64;
    let ghost_delay = s.get("ghost_delay");
    let ghost_angle = -2.0 * std::f64::consts::PI * T::SUBCARRIER_MHZ * ghost_delay;

    let u = Uniforms {
        width: input.width as u32,
        vbi_lines: raster.vbi_lines as u32,
        total_lines: raster.total_lines() as u32,
        pre_eq_end: raster.pre_eq_end as u32,
        vsync_end: raster.vsync_end as u32,
        post_eq_end: raster.post_eq_end as u32,
        cc_enabled: s.flag("closed_captions"),
        cc_bits: plan.cc_bits,
        field_index: plan.field_index,
        seed: 0x9E3779B9,
        doc_enabled: s.flag("dropout_compensation"),
        taps_i: 1.max((0.38 / us_per_pixel).round() as u32),
        taps_q: 1.max((0.83 / us_per_pixel).round() as u32),
        color_on_init: plan.color_on,
        us_per_pixel: us_per_pixel as f32,
        line_us: T::LINE as f32,
        ghost_level: s.get("ghost_level") as f32,
        ghost_delay: ghost_delay as f32,
        ghost_cos: ghost_angle.cos() as f32,
        ghost_sin: ghost_angle.sin() as f32,
        brightness: s.get("brightness").clamp(0.0, 1.0) as f32,
        picture_gain: snap(plan.picture_gain, 1.0),
        burst_amp: (T::BURST_AMPLITUDE * knobs.agc) as f32,
        chroma_alpha: knobs.chroma_alpha as f32,
        acc_gamma: knobs.acc_gamma as f32,
        killer_kappa: knobs.killer_kappa as f32,
        phase_init: snap(plan.chroma_phase, 0.0),
        gain_init: snap(plan.chroma_gain, 1.0),
        killer_init: snap(plan.killer, 1.0),
        gate_noise: 0.45,
    };
    let drops: Vec<GlitchDropout> = plan.dropouts.iter().take(64).copied().collect();

    // Pass 1: what the burst gate sees on each row.
    let gate: Vec<[f32; 2]> = rows
        .par_iter()
        .enumerate()
        .map(|(y, rw)| {
            let mut g = [0.0f32; 2];
            const N: i32 = 16;
            for i in 0..N {
                let uu = rw.u0 + rw.gate_shift + GATE_S + (i as f32 + 0.5) / N as f32 * (GATE_E - GATE_S);
                let (line, lu) = locate(rw, uu, u.total_lines);
                if line >= u.post_eq_end as i32 && (BURST_S..BURST_E).contains(&lu) {
                    g[0] += BURST_DIR[0] * u.burst_amp * rw.burst_scale;
                    g[1] += BURST_DIR[1] * u.burst_amp * rw.burst_scale;
                } else if line >= u.vbi_lines as i32 && lu >= ACT_S && lu < ACT_S + ACT_L {
                    let sg = signal_at(input, &u, line, lu);
                    g[0] += sg.yiq[1] * u.picture_gain;
                    g[1] += sg.yiq[2] * u.picture_gain;
                }
            }
            let norm = (GATE_E - GATE_S) / ((BURST_E - BURST_S) * N as f32);
            g[0] *= norm;
            g[1] *= norm;
            let n = rw.noise_ire / 92.5 * u.gate_noise;
            if n > 0.0 {
                g[0] += gauss(u.seed, u.field_index, y as u32, 101) * n;
                g[1] += gauss(u.seed, u.field_index, y as u32, 102) * n;
            }
            g
        })
        .collect();

    // Pass 2: the color reference loop, top to bottom.
    let mut color = vec![[0.0f32; 3]; rows.len()];
    {
        let (mut phase, mut gain, mut killer) = (u.phase_init, u.gain_init, u.killer_init);
        let mut on = u.color_on_init;
        let b0 = 0.2162162162f32;
        for (y, g) in gate.iter().enumerate() {
            let mag = (g[0] * g[0] + g[1] * g[1]).sqrt();
            let (c, sn) = ((-phase).cos(), (-phase).sin());
            let r = [g[0] * c - g[1] * sn, g[0] * sn + g[1] * c];
            let d = r[0] * BURST_DIR[0] + r[1] * BURST_DIR[1];
            let x = BURST_DIR[0] * r[1] - BURST_DIR[1] * r[0];
            if mag > 0.0 {
                phase += u.chroma_alpha * (mag / b0).min(1.0) * x.atan2(d);
            }
            let (c2, s2) = ((-phase).cos(), (-phase).sin());
            let in_phase = (g[0] * c2 - g[1] * s2) * BURST_DIR[0] + (g[0] * s2 + g[1] * c2) * BURST_DIR[1];
            gain += u.acc_gamma * (b0 / in_phase.max(0.25 * b0) - gain);
            gain = gain.clamp(0.3, 3.0);
            killer += u.killer_kappa * ((in_phase / b0).clamp(-1.0, 1.0) - killer);
            if on && killer < 0.3 {
                on = false;
            } else if !on && killer > 0.5 {
                on = true;
            }
            let ph = if phase.abs() < 1e-4 { 0.0 } else { phase };
            let gn = if (gain - 1.0).abs() < 1e-4 { 1.0 } else { gain };
            color[y] = [(-ph).cos(), (-ph).sin(), if on { gn } else { 0.0 }];
        }
    }

    // Pass 3: compose the screen.
    let mut out = Image::new(input.width, input.height);
    out.pixels
        .par_chunks_exact_mut(input.width)
        .enumerate()
        .for_each(|(gy, out_row)| {
            let rw = &rows[gy];
            let rc = color[gy];
            for (gx, px) in out_row.iter_mut().enumerate() {
                let uu = rw.u0 + ACT_S + (gx as f32 + 0.5) * u.us_per_pixel;
                let (l_line, l_u) = locate(rw, uu, u.total_lines);

                let mut sig_line = l_line;
                let mut streak = false;
                let mut tail = 0.0f32;
                if let Some(d_start) = in_dropout(&drops, l_line, l_u) {
                    if u.doc_enabled && l_u - d_start >= 0.7 {
                        let mut src_line = l_line - 1;
                        let mut k = 0;
                        while k < 6
                            && src_line > u.vbi_lines as i32
                            && in_dropout(&drops, src_line, l_u).is_some()
                        {
                            src_line -= 1;
                            k += 1;
                        }
                        sig_line = (u.vbi_lines as i32).max(src_line);
                    } else {
                        streak = true;
                    }
                } else if !u.doc_enabled {
                    for d in &drops {
                        if d.field_line == l_line
                            && l_u >= d.u_start + d.u_length
                            && l_u < d.u_start + d.u_length + 1.5
                        {
                            tail = tail.max(1.0 - (l_u - d.u_start - d.u_length) / 1.5);
                        }
                    }
                }

                let sg = signal_at(input, &u, sig_line, l_u);
                let plain = sg.picture
                    && sg.exact
                    && sig_line == l_line
                    && !streak
                    && tail == 0.0
                    && u.ghost_level == 0.0
                    && rw.noise_ire == 0.0
                    && rw.hum_ire == 0.0
                    && rw.tape_loss == 0.0
                    && u.picture_gain == 1.0
                    && u.brightness == 0.0
                    && rc[0] == 1.0
                    && rc[1] == 0.0
                    && rc[2] == 1.0;
                if plain {
                    *px = [sg.rgb[0], sg.rgb[1], sg.rgb[2], 1.0];
                    continue;
                }

                let mut yiq = sg.yiq;
                if streak {
                    yiq = [1.0, 0.0, 0.0];
                }
                if tail > 0.0 {
                    yiq[0] = mix(yiq[0], -0.08, tail * 0.6);
                }

                if u.ghost_level != 0.0 {
                    let mut ug = l_u - u.ghost_delay;
                    let mut gl = l_line;
                    if ug < 0.0 {
                        ug += u.line_us;
                        gl = (gl - 1 + u.total_lines as i32) % u.total_lines as i32;
                    }
                    let g = signal_at(input, &u, gl, ug);
                    let iq = [
                        g.yiq[1] * u.ghost_cos - g.yiq[2] * u.ghost_sin,
                        g.yiq[1] * u.ghost_sin + g.yiq[2] * u.ghost_cos,
                    ];
                    yiq[0] += u.ghost_level * (g.yiq[0] - ire2y(0.0));
                    yiq[1] += u.ghost_level * iq[0];
                    yiq[2] += u.ghost_level * iq[1];
                }

                let (gxu, gyu) = (gx as u32, gy as u32);
                if rw.tape_loss > 0.0 {
                    let seg = 0.5 + 1.6 * uni(u.seed, u.field_index, gyu, 201);
                    let pos = l_u + uni(u.seed, u.field_index, gyu, 202) * seg;
                    let run = (pos.max(0.0) / seg) as u32;
                    let g = gauss(u.seed ^ 0xF00D, u.field_index, run, gyu);
                    let sparkle = if uni(u.seed ^ 0xBEEF, u.field_index, run, gyu) > 0.93 { 0.6 } else { 0.0 };
                    let streak_v = (0.35 + 0.32 * g + sparkle).clamp(-0.08, 1.05);
                    yiq[0] = mix(yiq[0], streak_v, rw.tape_loss);
                    yiq[1] *= 1.0 - rw.tape_loss;
                    yiq[2] *= 1.0 - rw.tape_loss;
                }

                if u.picture_gain != 1.0 {
                    let ire = 7.5 + 92.5 * yiq[0];
                    yiq[0] = ire2y(ire * u.picture_gain);
                    yiq[1] *= u.picture_gain;
                    yiq[2] *= u.picture_gain;
                }

                yiq[0] += rw.hum_ire / 92.5;

                if rw.noise_ire > 0.0 {
                    let n = rw.noise_ire / 92.5;
                    yiq[0] += gauss(u.seed, u.field_index, gxu, gyu.wrapping_mul(3)) * n;
                    let (mut ni, mut nq) = (0.0f32, 0.0f32);
                    for k in 0..u.taps_i {
                        ni += gauss(u.seed ^ 0x1234, u.field_index, gxu + k, gyu.wrapping_mul(3) + 1);
                    }
                    for k in 0..u.taps_q {
                        nq += gauss(u.seed ^ 0x4321, u.field_index, gxu + k, gyu.wrapping_mul(3) + 2);
                    }
                    yiq[1] += ni / (u.taps_i as f32).sqrt() * n * 0.35;
                    yiq[2] += nq / (u.taps_q as f32).sqrt() * n * 0.25;
                }

                let iq = [
                    (yiq[1] * rc[0] - yiq[2] * rc[1]) * rc[2],
                    (yiq[1] * rc[1] + yiq[2] * rc[0]) * rc[2],
                ];
                yiq[1] = iq[0];
                yiq[2] = iq[1];

                if u.brightness > 0.0 {
                    let cutoff = 7.5 - 47.5 * u.brightness;
                    let ire = 7.5 + 92.5 * yiq[0];
                    yiq[0] = (ire - cutoff) / (100.0 - cutoff);
                    let k = 92.5 / (100.0 - cutoff);
                    yiq[1] *= k;
                    yiq[2] *= k;
                }

                let rgb = yiq2rgb(yiq);
                *px = [rgb[0].clamp(0.0, 1.0), rgb[1].clamp(0.0, 1.0), rgb[2].clamp(0.0, 1.0), 1.0];
            }
        });
    out.quantize_8bit();
    out
}
