//! Port of NTSCRT's `Receiver.swift`: the TV's sync circuits, simulated scan line by scan line.
//! Design after Trevor Blackwell's analogtv (xscreensaver).

use std::collections::VecDeque;
use std::f64::consts::PI;

use super::params::GlitchSettings;

pub mod timing {
    pub const LINE: f64 = 63.556;
    pub const SYNC: f64 = 4.7;
    pub const BURST_START: f64 = 5.3;
    pub const BURST_END: f64 = 7.8;
    pub const GATE_START: f64 = 4.55;
    pub const GATE_END: f64 = 8.55;
    pub const ACTIVE_START: f64 = 9.4;
    pub const ACTIVE_LENGTH: f64 = 52.656;
    pub const ACTIVE_END: f64 = ACTIVE_START + ACTIVE_LENGTH;
    pub const EQUALIZING_PULSE: f64 = 2.3;
    pub const BROAD_PULSE: f64 = 27.1;
    pub const FIELD_RATE: f64 = 60000.0 / 1001.0;
    pub const LINES_PER_FIELD: f64 = 262.5;
    pub const SUBCARRIER_MHZ: f64 = 315.0 / 88.0;
    pub const BURST_I: f64 = 0.5446390350150271;
    pub const BURST_Q: f64 = -0.8386705679454240;
    pub const BURST_AMPLITUDE: f64 = 20.0 / 92.5;
}
use timing as T;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ReceiverRaster {
    pub width: usize,
    pub active_lines: usize,
    pub vbi_lines: usize,
    pub pre_eq_end: usize,
    pub vsync_end: usize,
    pub post_eq_end: usize,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum LineKind {
    Equalizing,
    Vsync,
    Normal,
}

impl ReceiverRaster {
    pub fn new(width: usize, active_lines: usize) -> Self {
        let width = width.max(1);
        let active_lines = active_lines.max(1);
        let v = 12.max((active_lines as f64 * 20.0 / 242.5).round() as usize);
        let s = (active_lines + v) as f64 / T::LINES_PER_FIELD;
        let eq = 1.max((3.0 * s).round() as usize);
        Self {
            width,
            active_lines,
            vbi_lines: v,
            pre_eq_end: eq,
            vsync_end: 2 * eq,
            post_eq_end: 3 * eq,
        }
    }

    pub fn total_lines(&self) -> usize {
        self.active_lines + self.vbi_lines
    }

    pub fn scale(&self) -> f64 {
        self.total_lines() as f64 / T::LINES_PER_FIELD
    }

    pub fn cc_line(&self) -> usize {
        self.vbi_lines - 1
    }

    fn kind(&self, jf: usize) -> LineKind {
        if jf < self.pre_eq_end {
            LineKind::Equalizing
        } else if jf < self.vsync_end {
            LineKind::Vsync
        } else if jf < self.post_eq_end {
            LineKind::Equalizing
        } else {
            LineKind::Normal
        }
    }

    fn sync_duty(&self, jf: usize) -> f64 {
        match self.kind(jf) {
            LineKind::Equalizing => 2.0 * T::EQUALIZING_PULSE / T::LINE,
            LineKind::Vsync => 2.0 * T::BROAD_PULSE / T::LINE,
            LineKind::Normal => T::SYNC / T::LINE,
        }
    }

    pub fn has_burst(&self, jf: usize) -> bool {
        jf >= self.post_eq_end
    }
}

/// One displayed row: where the TV's horizontal scan started relative to the signal.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct GlitchRow {
    pub u0: f32,
    pub field_line: i32,
    pub len_prev: f32,
    pub len_cur: f32,
    pub len_next: f32,
    pub noise_ire: f32,
    pub hum_ire: f32,
    pub burst_scale: f32,
    pub tape_loss: f32,
    pub gate_shift: f32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GlitchDropout {
    pub field_line: i32,
    pub u_start: f32,
    pub u_length: f32,
}

#[derive(Clone, Debug)]
pub struct GlitchFieldPlan {
    pub raster: ReceiverRaster,
    pub rows: Vec<GlitchRow>,
    pub dropouts: Vec<GlitchDropout>,
    pub chroma_phase: f32,
    pub chroma_gain: f32,
    pub killer: f32,
    pub color_on: bool,
    pub picture_gain: f32,
    pub cc_bits: u32,
    pub field_index: u32,
    pub settings: GlitchSettings,
}

// MARK: deterministic noise (keyed by position, never call order)

pub mod random {
    #[inline(always)]
    pub fn mix(z0: u64) -> u64 {
        let mut z = z0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    #[inline(always)]
    pub fn uniform(seed: u64, purpose: u64, a: i64, b: i64) -> f64 {
        let h = mix(seed ^ mix(purpose ^ mix((a as u64) ^ mix(b as u64))));
        (h >> 11) as f64 * (1.0 / 9_007_199_254_740_992.0)
    }

    #[inline(always)]
    pub fn gauss(seed: u64, purpose: u64, a: i64, b: i64) -> f64 {
        let u1 = uniform(seed, purpose, a, b).max(1e-12);
        let u2 = uniform(seed, purpose.wrapping_add(0x51), a, b);
        (-2.0 * u1.ln()).sqrt() * (2.0 * std::f64::consts::PI * u2).cos()
    }

    pub fn value(seed: u64, purpose: u64, x: f64) -> f64 {
        let i = x.floor();
        let f = x - i;
        let a = uniform(seed, purpose, i as i64, 0) * 2.0 - 1.0;
        let b = uniform(seed, purpose, i as i64 + 1, 0) * 2.0 - 1.0;
        let s = f * f * (3.0 - 2.0 * f);
        a + (b - a) * s
    }

    pub fn phi(x: f64) -> f64 {
        0.5 * libm::erfc(-x / std::f64::consts::SQRT_2)
    }
}
use random as R;

#[inline(always)]
pub fn posmod(a: i64, m: i64) -> i64 {
    let r = a % m;
    if r < 0 { r + m } else { r }
}

#[inline(always)]
pub fn floordiv(a: i64, m: i64) -> i64 {
    if a >= 0 { a / m } else { -((-a + m - 1) / m) }
}

// MARK: knobs → circuit constants

#[derive(Clone, Debug)]
pub struct Knobs {
    pub settings: GlitchSettings,
    pub sigma: f64,
    pub agc: f64,
    pub v_total: f64,
    pub v_lock: f64,
    pub v_center: f64,
    pub v_beta: f64,
    pub h_r: f64,
    pub kp: f64,
    pub ki: f64,
    pub u_max: f64,
    pub hum_ire: f64,
    pub head_switch: f64,
    pub jitter: f64,
    pub crinkle: f64,
    pub clog: f64,
    pub search: f64,
    pub tracking: f64,
    pub dropouts: f64,
    pub captions: bool,
    pub gate_alpha: f64,
    pub chroma_alpha: f64,
    pub acc_gamma: f64,
    pub killer_kappa: f64,
}

impl Knobs {
    pub fn new(settings: &GlitchSettings, raster: &ReceiverRaster) -> Self {
        let s = raster.scale();
        let strength = settings.get("signal_strength").clamp(0.0, 1.0);
        let sigma = (100.0 * 10f64.powf(-1.9 * strength) - 1.26 * strength.powi(4)).max(0.0);
        let agc = (strength / 0.45).min(1.0);

        let v_lock = 0.015;
        let v_knob = settings.get("vertical_hold");
        let built_in = 0.002;
        let v_total =
            built_in + if v_knob < 0.0 { -1.0 } else { 1.0 } * 0.10 * v_knob.abs().powf(1.37);
        let v_center = (built_in / v_lock).asin() / (2.0 * PI);
        let v_beta = 1.0 - (-1.0 / (1.5 * s)).exp();

        let h_knob = settings.get("horizontal_hold").abs();
        let kp_n = 0.04 * 10f64.powf(settings.get("afc_speed").clamp(0.0, 1.0));
        let edge = 0.04 * 10f64.powf(0.5) / (2.0 * PI);
        let h_free = if h_knob <= 0.35 {
            edge * (h_knob / 0.35).powi(2)
        } else {
            edge + 0.0265 * (h_knob - 0.35) / 0.65
        };
        let h_r = if settings.get("horizontal_hold") < 0.0 { -1.0 } else { 1.0 } * h_free / s;

        Self {
            settings: settings.clone(),
            sigma,
            agc,
            v_total,
            v_lock,
            v_center,
            v_beta,
            h_r,
            kp: kp_n / s,
            ki: (kp_n / 1.4).powi(2) / (s * s),
            u_max: 0.0,
            hum_ire: settings.get("hum") * 60.0,
            head_switch: settings.get("head_switch"),
            jitter: settings.get("timebase_jitter"),
            crinkle: settings.get("crinkle"),
            clog: settings.get("head_clog"),
            search: settings.get("search_speed").max(1.0),
            tracking: settings.get("tracking").clamp(0.0, 1.0),
            dropouts: settings.get("dropouts"),
            captions: settings.flag("closed_captions"),
            gate_alpha: 1.0 - (-1.0 / (6.0 * s)).exp(),
            chroma_alpha: 1.0 - (-1.0 / (4.0 * s)).exp(),
            acc_gamma: 1.0 - (-1.0 / (8.0 * s)).exp(),
            killer_kappa: 1.0 - (-1.0 / (15.0 * s)).exp(),
        }
    }
}

/// The TV's color reference: 3.58 MHz oscillator locked to the burst, ACC and color killer.
#[derive(Clone, Copy, Debug)]
pub struct ChromaReference {
    pub phase: f64,
    pub gain: f64,
    pub killer: f64,
    pub color_on: bool,
}

impl Default for ChromaReference {
    fn default() -> Self {
        Self {
            phase: 0.0,
            gain: 1.0,
            killer: 1.0,
            color_on: true,
        }
    }
}

impl ChromaReference {
    pub fn step(&mut self, gi: f64, gq: f64, k: &Knobs) {
        let b0 = T::BURST_AMPLITUDE;
        let mag = (gi * gi + gq * gq).sqrt();
        let (c, s) = ((-self.phase).cos(), (-self.phase).sin());
        let ri = gi * c - gq * s;
        let rq = gi * s + gq * c;
        let dot = ri * T::BURST_I + rq * T::BURST_Q;
        let cross = T::BURST_I * rq - T::BURST_Q * ri;
        if mag > 0.0 {
            self.phase += k.chroma_alpha * (mag / b0).min(1.0) * cross.atan2(dot);
        }
        let (c2, s2) = ((-self.phase).cos(), (-self.phase).sin());
        let in_phase = (gi * c2 - gq * s2) * T::BURST_I + (gi * s2 + gq * c2) * T::BURST_Q;
        self.gain += k.acc_gamma * (b0 / in_phase.max(0.25 * b0) - self.gain);
        self.gain = self.gain.clamp(0.3, 3.0);
        self.killer += k.killer_kappa * ((in_phase / b0).clamp(-1.0, 1.0) - self.killer);
        if self.color_on && self.killer < 0.3 {
            self.color_on = false;
        } else if !self.color_on && self.killer > 0.5 {
            self.color_on = true;
        }
    }
}

/// Full simulation state at a moment (for resuming instead of re-running from zero).
#[derive(Clone, Debug)]
pub struct Snapshot {
    pub time: f64,
    r: i64,
    theta: f64,
    integ: f64,
    v_phase: f64,
    v_integ: f64,
    v_above: bool,
    gate_err: f64,
    gate_log: Vec<f64>,
    triggers: VecDeque<i64>,
    theta_log: Vec<f64>,
}

pub struct ReceiverSimulator {
    pub raster: ReceiverRaster,
    pub seed: u64,
    l: i64,
    us_per_second: f64,
    r: i64,
    theta: f64,
    integ: f64,
    v_phase: f64,
    v_integ: f64,
    v_above: bool,
    triggers: VecDeque<i64>,
    gate_err: f64,
    gate_log: Vec<f64>,
    theta_log: Vec<f64>,
    log_cap: usize,
    knobs: Knobs,
    pub nominal_trigger: usize,
}

const H: f64 = T::LINE;
pub const DEFAULT_SEED: u64 = 0x4E54_5343_5254;

impl ReceiverSimulator {
    pub const WARMUP: f64 = 2.0;

    pub fn new(raster: ReceiverRaster, seed: u64) -> Self {
        Self::with_trigger(raster, seed, Self::calibrate(raster, seed))
    }

    /// A set whose calibration (see `nominal_trigger`) is already known for this raster and seed.
    pub fn with_trigger(raster: ReceiverRaster, seed: u64, nominal_trigger: usize) -> Self {
        let mut sim = Self::uncalibrated(raster, seed);
        sim.nominal_trigger = nominal_trigger;
        sim.reset();
        sim
    }

    fn uncalibrated(raster: ReceiverRaster, seed: u64) -> Self {
        let total = raster.total_lines();
        let log_cap = 4 * total + 64;
        Self {
            raster,
            seed,
            l: total as i64,
            us_per_second: T::LINE * total as f64 * T::FIELD_RATE,
            r: 0,
            theta: 0.0,
            integ: 0.0,
            v_phase: 0.0,
            v_integ: 0.0,
            v_above: false,
            triggers: VecDeque::new(),
            gate_err: 0.0,
            gate_log: vec![0.0; log_cap],
            theta_log: vec![0.0; log_cap],
            log_cap,
            knobs: Knobs::new(&GlitchSettings::default(), &raster),
            nominal_trigger: 0,
        }
    }

    pub fn calibrate(raster: ReceiverRaster, seed: u64) -> usize {
        let mut sim = Self::uncalibrated(raster, seed);
        sim.advance(0.0, &GlitchSettings::default());
        match sim.triggers.back() {
            Some(&t) => posmod(t, raster.total_lines() as i64) as usize,
            None => raster.pre_eq_end + 2,
        }
    }

    pub fn time(&self) -> f64 {
        (self.r as f64 * H + self.theta) / self.us_per_second - Self::WARMUP
    }

    pub fn reset(&mut self) {
        self.r = 0;
        self.theta = 0.0;
        self.integ = 0.0;
        self.v_phase = 0.0;
        self.v_integ = 0.0;
        self.v_above = false;
        self.gate_err = 0.0;
        self.triggers.clear();
        self.theta_log.fill(0.0);
        self.gate_log.fill(0.0);
    }

    pub fn snapshot(&self) -> Snapshot {
        Snapshot {
            time: self.time(),
            r: self.r,
            theta: self.theta,
            integ: self.integ,
            v_phase: self.v_phase,
            v_integ: self.v_integ,
            v_above: self.v_above,
            gate_err: self.gate_err,
            gate_log: self.gate_log.clone(),
            triggers: self.triggers.clone(),
            theta_log: self.theta_log.clone(),
        }
    }

    pub fn restore(&mut self, s: &Snapshot) {
        self.r = s.r;
        self.theta = s.theta;
        self.integ = s.integ;
        self.v_phase = s.v_phase;
        self.v_integ = s.v_integ;
        self.v_above = s.v_above;
        self.gate_err = s.gate_err;
        self.gate_log.clone_from(&s.gate_log);
        self.triggers.clone_from(&s.triggers);
        self.theta_log.clone_from(&s.theta_log);
    }

    /// Run forward to `time` with `settings` in force since the previous call.
    pub fn advance(&mut self, time: f64, settings: &GlitchSettings) {
        self.knobs = Knobs::new(settings, &self.raster);
        let target = (time + Self::WARMUP) * self.us_per_second;
        while self.r as f64 * H + self.theta <= target {
            self.step();
        }
    }

    /// Simulate from a fresh start (or a snapshot) to `time`, sampling `settings_at` once per frame
    /// exactly as frame-by-frame playback would.
    pub fn resimulate(
        &mut self,
        time: f64,
        frame_duration: f64,
        settings_at: &dyn Fn(f64) -> GlitchSettings,
        resume_from: Option<&Snapshot>,
        mut checkpoint: Option<&mut dyn FnMut(Snapshot)>,
    ) {
        let mut k: i64 = 1;
        match resume_from {
            Some(snap) if frame_duration > 0.0 => {
                self.restore(snap);
                k = (snap.time / frame_duration).round() as i64 + 1;
            }
            _ => {
                self.reset();
                self.advance(0.0, &settings_at(0.0));
            }
        }
        if time <= 0.0 || frame_duration <= 0.0 {
            return;
        }
        let mut next_checkpoint = (self.time() + 1.0).floor();
        loop {
            let t = time.min(k as f64 * frame_duration);
            self.advance(t, &settings_at(t));
            if t >= time {
                break;
            }
            if let Some(cb) = checkpoint.as_mut() {
                if t >= next_checkpoint && t == k as f64 * frame_duration {
                    cb(self.snapshot());
                    next_checkpoint = t.floor() + 1.0;
                }
            }
            k += 1;
        }
    }

    fn step(&mut self) {
        let k = self.knobs.clone();
        let seed = self.seed;
        let tau = self.r as f64 * H + self.theta;

        let mut j = (tau / H).round() as i64;
        let mut e = self.timebase_error(j);
        let mut edge = j as f64 * H + e;
        if tau - edge > H / 2.0 {
            j += 1;
            e = self.timebase_error(j);
            edge = j as f64 * H + e;
        } else if edge - tau > H / 2.0 {
            j -= 1;
            e = self.timebase_error(j);
            edge = j as f64 * H + e;
        }
        let _ = e;
        let jf = posmod(j, self.l) as usize;
        let loss = self.signal_loss(j);
        let sigma = self.noise_sigma(loss);
        let sigma_sep = sigma * 0.35;
        let hum_now = self.hum(tau);
        let hum_shift = 0.4 * hum_now;
        let sync_margin = 20.0 - hum_shift;
        let blank_margin = 20.0 + hum_shift;
        let p_detect = if sigma_sep > 0.0 {
            R::phi(sync_margin / sigma_sep)
        } else if sync_margin > 0.0 {
            1.0
        } else {
            0.0
        };
        let p_noise_peak = if sigma_sep > 0.0 {
            R::phi(-blank_margin / sigma_sep)
        } else if blank_margin > 0.0 {
            0.0
        } else {
            1.0
        };
        let sync_present = 1.0 - (loss * loss * 1.2).min(1.0);

        let r = self.r;
        let mut detected: Option<f64> = None;
        let roll = R::uniform(seed, 11, r, 0);
        let p_false = (10.0 * p_noise_peak).min(1.0);
        if roll < p_false {
            detected = Some((R::uniform(seed, 12, r, 0) - 0.5) * H * 0.6);
        } else if R::uniform(seed, 13, r, 0) < p_detect * sync_present {
            let mut d = edge - tau;
            d += R::gauss(seed, 14, r, 0) * sigma_sep * 0.0125;
            d += hum_now * 0.05;
            detected = Some(d);
        }

        let mut next = self.theta + H * k.h_r + self.integ;
        if let Some(d0) = detected {
            let linear = d0.abs() <= H / 4.0;
            let d = (H / (2.0 * PI)) * (2.0 * PI * d0 / H).sin();
            next += k.kp * d;
            if linear {
                self.integ = (self.integ + k.ki * d).clamp(-k.u_max, k.u_max);
            }
        }

        let duty = self.raster.sync_duty(jf) * sync_present;
        let mut measured = duty * p_detect + (1.0 - duty) * p_noise_peak;
        let variance = (duty * p_detect * (1.0 - p_detect)
            + (1.0 - duty) * p_noise_peak * (1.0 - p_noise_peak))
            / 60.0;
        if variance > 0.0 {
            measured += R::gauss(seed, 15, r, 0) * variance.sqrt();
        }
        self.v_integ += (measured - self.v_integ) * k.v_beta;
        let mut vsync = false;
        if self.v_above {
            if self.v_integ < 0.25 {
                self.v_above = false;
            }
        } else if self.v_integ >= 0.45 {
            self.v_above = true;
            vsync = true;
        }

        self.v_phase += 1.0 + (next - self.theta) / H;
        let period = self.l as f64 * (1.0 + k.v_total);
        if vsync {
            let mut e = self.v_phase / period - k.v_center;
            e -= e.round();
            self.v_phase -= k.v_lock * period * (2.0 * PI * e).sin();
        }
        if self.v_phase >= period {
            self.v_phase -= period;
            self.triggers.push_back(r);
            if self.triggers.len() > 16 {
                self.triggers.pop_front();
            }
        }

        if detected.is_some() {
            let mut d = (edge - tau) - self.gate_err;
            d -= H * (d / H).round();
            self.gate_err += k.gate_alpha * d;
            self.gate_err -= H * (self.gate_err / H).round();
        }
        let idx = posmod(r, self.log_cap as i64) as usize;
        self.theta_log[idx] = self.theta;
        self.gate_log[idx] = self.gate_err;
        self.theta = next;
        self.r += 1;
    }

    /// The most recently completed field, as the TV drew it.
    pub fn plan(&self) -> GlitchFieldPlan {
        let k = &self.knobs;
        let a = self.raster.active_lines as i64;
        let offset = self.raster.vbi_lines as i64 - self.nominal_trigger as i64;
        let last_drawn = self.r - 1;
        let scan_start = self
            .triggers
            .iter()
            .rev()
            .find(|&&t| t + offset + a - 1 <= last_drawn)
            .copied()
            .unwrap_or(last_drawn - offset - a + 1);
        let first = scan_start + offset;

        let mut rows = Vec::with_capacity(a as usize);
        for y in 0..a {
            let tv_line = first + y;
            let tau = tv_line as f64 * H + self.logged_theta(tv_line);
            let (j, u0) = self.signal_line(tau);
            let loss = self.signal_loss(j);
            rows.push(GlitchRow {
                u0: u0 as f32,
                field_line: posmod(j, self.l) as i32,
                len_prev: (H + self.timebase_error(j) - self.timebase_error(j - 1)) as f32,
                len_cur: (H + self.timebase_error(j + 1) - self.timebase_error(j)) as f32,
                len_next: (H + self.timebase_error(j + 2) - self.timebase_error(j + 1)) as f32,
                noise_ire: k.sigma as f32,
                hum_ire: self.hum(tau) as f32,
                burst_scale: (1.0 - loss).max(0.0) as f32,
                tape_loss: loss.clamp(0.0, 1.0) as f32,
                gate_shift: self.logged_gate(tv_line) as f32,
            });
        }

        let mut chroma = ChromaReference::default();
        let warm = 4.max((12.0 * self.raster.scale()).round() as i64);
        let mut w = warm;
        while w > 0 {
            let tv_line = first - w;
            w -= 1;
            if self.in_retrace(tv_line) {
                continue;
            }
            let tau = tv_line as f64 * H + self.logged_theta(tv_line);
            let (j, u0) = self.signal_line(tau);
            let jf = posmod(j, self.l) as usize;
            let (mut gi, mut gq) = (0.0, 0.0);
            let g0 = u0 + self.logged_gate(tv_line);
            if self.raster.has_burst(jf) {
                let lo = (g0 + T::GATE_START).max(T::BURST_START);
                let hi = (g0 + T::GATE_END).min(T::BURST_END);
                let overlap = (hi - lo).max(0.0) / (T::BURST_END - T::BURST_START);
                let amp = T::BURST_AMPLITUDE * overlap * k.agc * (1.0 - self.signal_loss(j)).max(0.0);
                gi = amp * T::BURST_I;
                gq = amp * T::BURST_Q;
            }
            let n = self.noise_sigma(self.signal_loss(j)) / 92.5 * 0.45;
            if n > 0.0 {
                gi += R::gauss(self.seed, 21, tv_line, 0) * n;
                gq += R::gauss(self.seed, 22, tv_line, 0) * n;
            }
            chroma.step(gi, gq, k);
        }

        let top_signal_line = self.signal_line(first as f64 * H + self.logged_theta(first)).0;
        let signal_field = floordiv(top_signal_line, self.l);
        GlitchFieldPlan {
            raster: self.raster,
            rows,
            dropouts: self.dropouts(signal_field),
            chroma_phase: chroma.phase as f32,
            chroma_gain: chroma.gain as f32,
            killer: chroma.killer as f32,
            color_on: chroma.color_on,
            picture_gain: k.agc as f32,
            cc_bits: self.caption_bits(signal_field),
            field_index: scan_start as u32,
            settings: k.settings.clone(),
        }
    }

    fn in_retrace(&self, tv_line: i64) -> bool {
        let retrace = 2.max((9.0 * self.raster.scale()).round() as i64);
        self.triggers
            .iter()
            .any(|&t| tv_line >= t && tv_line < t + retrace)
    }

    fn logged_gate(&self, tv_line: i64) -> f64 {
        if tv_line < 0 || tv_line >= self.r || self.r - tv_line > self.log_cap as i64 {
            return self.gate_err;
        }
        self.gate_log[posmod(tv_line, self.log_cap as i64) as usize]
    }

    fn logged_theta(&self, tv_line: i64) -> f64 {
        if tv_line < 0 || tv_line >= self.r || self.r - tv_line > self.log_cap as i64 {
            return self.theta;
        }
        self.theta_log[posmod(tv_line, self.log_cap as i64) as usize]
    }

    fn signal_line(&self, tau: f64) -> (i64, f64) {
        let mut j = (tau / H).floor() as i64;
        if tau < j as f64 * H + self.timebase_error(j) {
            j -= 1;
        } else if tau >= (j + 1) as f64 * H + self.timebase_error(j + 1) {
            j += 1;
        }
        (j, tau - (j as f64 * H + self.timebase_error(j)))
    }

    fn field_time(&self, j: i64) -> f64 {
        j as f64 / (self.l as f64 * T::FIELD_RATE)
    }

    fn head(&self, j: i64) -> i64 {
        let switch_line = self.l - (3.5 * self.raster.scale()).round() as i64;
        let f = floordiv(j, self.l);
        let jf = posmod(j, self.l);
        posmod(f + if jf >= switch_line { 1 } else { 0 }, 2)
    }

    pub fn timebase_error(&self, j: i64) -> f64 {
        let k = &self.knobs;
        let seed = self.seed;
        let mut e = 0.0;
        if k.head_switch > 0.0 {
            let f = floordiv(j + (3.5 * self.raster.scale()).round() as i64, self.l);
            let wobble = 1.0 + 0.15 * R::gauss(seed, 31, f, 0);
            e += if self.head(j) == 0 { 0.5 } else { -0.5 } * k.head_switch * wobble;
        }
        if k.jitter > 0.0 {
            let line_scale = 1.0 / self.raster.scale();
            let slow = R::value(seed, 32, j as f64 * line_scale / 40.0);
            let fast = R::gauss(seed, 33, j, 0) * 0.35;
            e += k.jitter * (slow + fast);
        }
        if k.search > 1.05 {
            let crossing = self.search_at(j).1 as i64;
            let f = floordiv(j, self.l);
            e += 1.2 * R::gauss(seed, 36, f, crossing);
        }
        if k.crinkle > 0.0 {
            let (intensity, envelope) = self.crinkle_at(j);
            if intensity > 0.0 {
                let line_scale = 1.0 / self.raster.scale();
                e += intensity * 4.0 * R::value(seed, 34, j as f64 * line_scale / 3.0);
                e += envelope * 1.5 * R::value(seed, 35, j as f64 * line_scale / 60.0);
            }
        }
        e
    }

    pub fn signal_loss(&self, j: i64) -> f64 {
        let k = &self.knobs;
        let mut loss: f64 = 0.0;
        if k.clog > 0.0 {
            let f = floordiv(j, self.l);
            let dropped = R::uniform(self.seed, 41, f, 0) < k.clog * 0.8;
            let level = if dropped {
                0.85 + 0.15 * R::uniform(self.seed, 42, f, 0)
            } else {
                0.35 * k.clog * k.clog
            };
            loss = loss.max(level.min(1.0));
        }
        if k.crinkle > 0.0 {
            loss = loss.max(self.crinkle_at(j).0);
        }
        if k.search > 1.05 {
            loss = loss.max(self.search_at(j).0);
        }
        if k.tracking > 0.0 {
            loss = loss.max(self.tracking_at(j));
        }
        loss
    }

    fn tracking_at(&self, j: i64) -> f64 {
        let e = self.knobs.tracking;
        let pos = posmod(j, self.l) as f64 / self.l as f64;
        let wander = 0.03 * R::value(self.seed, 37, self.field_time(j) * 1.7);
        let center = 1.0 - 0.55 * e + wander;
        let width = 0.02 + 0.10 * e;
        let mut d = (pos - center).abs();
        d = d.min(1.0 - d);
        ((1.0 - d / width).max(0.0) * (0.4 + e).min(1.0)).min(1.0)
    }

    fn search_at(&self, j: i64) -> (f64, i64) {
        let bars = self.knobs.search - 1.0;
        let pos = posmod(j, self.l) as f64 / self.l as f64;
        let drift = (self.field_time(j) * 0.23) % 1.0;
        let x = pos * bars + drift;
        let phase = x - x.floor();
        let d = phase.min(1.0 - phase);
        let loss = ((1.0 - d / 0.15).max(0.0) * 1.2).min(1.0);
        (loss, x.floor() as i64)
    }

    fn noise_sigma(&self, loss: f64) -> f64 {
        let tape = loss * 90.0;
        (self.knobs.sigma * self.knobs.sigma + tape * tape).sqrt()
    }

    fn hum(&self, tau: f64) -> f64 {
        if self.knobs.hum_ire <= 0.0 {
            return 0.0;
        }
        let t = tau / self.us_per_second;
        self.knobs.hum_ire * (0.8 * (2.0 * PI * 60.0 * t).sin() + 0.3 * (2.0 * PI * 120.0 * t + 0.7).sin())
    }

    fn crinkle_at(&self, j: i64) -> (f64, f64) {
        let seed = self.seed;
        let t = self.field_time(j);
        let slot = 0.5;
        let p = self.knobs.crinkle * 0.6;
        let s = (t / slot).floor() as i64;
        let mut best = (0.0f64, 0.0f64);
        for k in (s - 1)..=s {
            if R::uniform(seed, 51, k, 0) >= p {
                continue;
            }
            let start = k as f64 * slot + R::uniform(seed, 52, k, 0) * slot;
            let duration = 0.12 + 0.35 * R::uniform(seed, 53, k, 0);
            if !(t >= start && t < start + duration) {
                continue;
            }
            let severity = 0.55 + 0.45 * R::uniform(seed, 54, k, 0);
            let progress = (t - start) / duration;
            let envelope = (PI * progress).sin();
            let pos = posmod(j, self.l) as f64 / self.l as f64;
            let speed = 0.8 + 1.6 * R::uniform(seed, 55, k, 0);
            let center = (R::uniform(seed, 56, k, 0) + progress * speed) % 1.0;
            let mut d = (pos - center).abs();
            d = d.min(1.0 - d);
            let width = 0.05 + 0.08 * severity;
            let band = (1.0 - d / width).max(0.0);
            let intensity = (severity * envelope * band * 1.4).min(1.0);
            if intensity > best.0 {
                best = (intensity, envelope);
            } else if envelope > best.1 {
                best.1 = envelope;
            }
        }
        best
    }

    fn dropouts(&self, f: i64) -> Vec<GlitchDropout> {
        let seed = self.seed;
        let rate = self.knobs.dropouts;
        if rate <= 0.0 {
            return Vec::new();
        }
        let mean = rate * rate * 14.0;
        let n = ((mean + mean.sqrt() * R::gauss(seed, 61, f, 0)).round() as i64).clamp(0, 64);
        let mut out = Vec::with_capacity(n as usize);
        for i in 0..n {
            let line = self.raster.vbi_lines
                + (R::uniform(seed, 62, f, i) * self.raster.active_lines as f64) as usize;
            let start = T::ACTIVE_START + R::uniform(seed, 63, f, i) * T::ACTIVE_LENGTH * 0.95;
            let length = (-(R::uniform(seed, 64, f, i).max(1e-6)).ln() * (1.5 + 5.0 * rate))
                .clamp(0.4, 30.0);
            let span = 1 + (R::uniform(seed, 65, f, i).powi(2) * 4.0) as usize;
            for k in 0..span {
                if out.len() >= 64 {
                    break;
                }
                let l = line + k;
                if l >= self.raster.total_lines() {
                    break;
                }
                let drift = R::gauss(seed, 66, f, i * 8 + k as i64) * 0.8;
                let scale = 0.7 + 0.6 * R::uniform(seed, 67, f, i * 8 + k as i64);
                out.push(GlitchDropout {
                    field_line: l as i32,
                    u_start: (start + drift) as f32,
                    u_length: (length * scale) as f32,
                });
            }
        }
        out
    }

    fn caption_bits(&self, f: i64) -> u32 {
        if !self.knobs.captions {
            return 0;
        }
        let ch = |i: i64| -> u32 {
            let c = 0x20 + (R::uniform(self.seed, 71, f, i) * 90.0) as u32;
            let parity = if c.count_ones() % 2 == 0 { 0x80 } else { 0 };
            c | parity
        };
        0b100 | (ch(0) << 3) | (ch(1) << 11)
    }
}
