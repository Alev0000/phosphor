//! The TV/VCR glitch stage: a receiver simulation plus the pass that draws what it displays.

pub mod params;
pub mod receiver;
pub mod stage;

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::sync::Mutex;

pub use params::{GlitchSettings, GLITCH_PARAMS};
use receiver::{ReceiverRaster, ReceiverSimulator, Snapshot, DEFAULT_SEED};

use crate::Image;

fn timing_hash(prev: u64, s: &GlitchSettings) -> u64 {
    let mut h = DefaultHasher::new();
    prev.hash(&mut h);
    for id in GlitchSettings::TIMING_IDS {
        s.get(id).to_bits().hash(&mut h);
    }
    h.finish()
}

struct Checkpoint {
    frame: i64,
    /// Rolling hash of the timing knobs over frames 0..=frame: a checkpoint is only valid for the
    /// same history.
    history: u64,
    snapshot: Snapshot,
}

struct Cache {
    raster: ReceiverRaster,
    frame_duration: f64,
    /// The set's calibration for this raster (a 2 s simulation; the same every frame).
    nominal_trigger: usize,
    checkpoints: Vec<Checkpoint>,
    /// The most recently rendered frames: playback continues from the frame before.
    recent: Vec<Checkpoint>,
}

const RECENT: usize = 4;

/// Renders the glitch stage for any frame, in any order, deterministically: the frame at time t
/// is what a set switched on 2 s before time zero shows after playing the history up to t.
/// Thread-safe so After Effects can render frames in parallel.
pub struct GlitchRenderer {
    cache: Mutex<Option<Cache>>,
}

impl Default for GlitchRenderer {
    fn default() -> Self {
        Self::new()
    }
}

impl GlitchRenderer {
    pub fn new() -> Self {
        Self {
            cache: Mutex::new(None),
        }
    }

    /// `settings_at(k)` returns the settings in force at frame k (frame 0 = time zero).
    pub fn render(
        &self,
        chain_input: &Image,
        frame: i64,
        frame_duration: f64,
        settings_at: &dyn Fn(i64) -> GlitchSettings,
    ) -> Image {
        let raster = ReceiverRaster::new(chain_input.width, chain_input.height);
        let frame = frame.max(0);

        // Rolling history hash for every frame up to the target.
        let neutral = GlitchSettings::default();
        let mut all_neutral = true;
        let mut hashes = Vec::with_capacity(frame as usize + 1);
        let mut h = 0u64;
        for k in 0..=frame {
            let s = settings_at(k);
            all_neutral &= s == neutral;
            h = timing_hash(h, &s);
            hashes.push(h);
        }
        // A healthy set on a clean signal, all along, shows its input exactly.
        if all_neutral {
            return chain_input.clone();
        }

        let resume = {
            let mut guard = self.cache.lock().unwrap();
            let valid = matches!(&*guard, Some(c) if c.raster == raster && c.frame_duration == frame_duration);
            if !valid {
                *guard = Some(Cache {
                    raster,
                    frame_duration,
                    nominal_trigger: ReceiverSimulator::calibrate(raster, DEFAULT_SEED),
                    checkpoints: Vec::new(),
                    recent: Vec::new(),
                });
            }
            let cache = guard.as_ref().unwrap();
            let usable = |c: &&Checkpoint| c.frame <= frame && hashes[c.frame as usize] == c.history;
            let best = cache
                .checkpoints
                .iter()
                .chain(cache.recent.iter())
                .filter(usable)
                .max_by_key(|c| c.frame);
            (cache.nominal_trigger, best.map(|c| (c.frame, c.snapshot.clone())))
        };
        let (nominal_trigger, resume) = resume;

        let mut sim = ReceiverSimulator::with_trigger(raster, DEFAULT_SEED, nominal_trigger);
        let start_frame = match &resume {
            Some((f, snap)) => {
                sim.restore(snap);
                // A snapshot sits just past its frame: this runs no lines, it only sets the knobs.
                sim.advance(*f as f64 * frame_duration, &settings_at(*f));
                *f
            }
            None => {
                sim.reset();
                sim.advance(0.0, &settings_at(0));
                0
            }
        };

        let frames_per_checkpoint = (1.0 / frame_duration).round().max(1.0) as i64;
        let mut new_checkpoints = Vec::new();
        for k in (start_frame + 1)..=frame {
            sim.advance(k as f64 * frame_duration, &settings_at(k));
            if k % frames_per_checkpoint == 0 && k < frame {
                new_checkpoints.push(Checkpoint {
                    frame: k,
                    history: hashes[k as usize],
                    snapshot: sim.snapshot(),
                });
            }
        }

        {
            let mut guard = self.cache.lock().unwrap();
            if let Some(cache) = guard.as_mut() {
                if cache.raster == raster && cache.frame_duration == frame_duration {
                    if !cache.recent.iter().any(|e| e.frame == frame && e.history == hashes[frame as usize]) {
                        cache.recent.push(Checkpoint { frame, history: hashes[frame as usize], snapshot: sim.snapshot() });
                        if cache.recent.len() > RECENT {
                            cache.recent.remove(0);
                        }
                    }
                    for c in new_checkpoints {
                        if !cache
                            .checkpoints
                            .iter()
                            .any(|e| e.frame == c.frame && e.history == c.history)
                        {
                            cache.checkpoints.push(c);
                        }
                    }
                    cache.checkpoints.sort_by_key(|c| c.frame);
                }
            }
        }

        let plan = sim.plan();
        stage::render_field(chain_input, &plan)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gradient(w: usize, h: usize) -> Image {
        let mut img = Image::new(w, h);
        for y in 0..h {
            for x in 0..w {
                let v = (((x + y) % 256) as f32 / 255.0 * 255.0).round() / 255.0;
                img.pixels[y * w + x] = [v, 1.0 - v, (x % 7) as f32 / 7.0, 1.0];
            }
        }
        img.quantize_8bit();
        img
    }

    #[test]
    fn healthy_set_reproduces_input_exactly() {
        let img = gradient(256, 224);
        let r = GlitchRenderer::new();
        let out = r.render(&img, 45, 1.0 / 30.0, &|_| GlitchSettings::default());
        assert_eq!(out.pixels, img.pixels);
    }

    #[test]
    fn same_frame_is_deterministic_in_any_order() {
        let img = gradient(160, 120);
        let mut s = GlitchSettings::default();
        s.set("vertical_hold", 0.6);
        s.set("signal_strength", 0.5);
        let r = GlitchRenderer::new();
        let a = r.render(&img, 70, 1.0 / 30.0, &|_| s.clone());
        let _ = r.render(&img, 10, 1.0 / 30.0, &|_| s.clone());
        let b = r.render(&img, 70, 1.0 / 30.0, &|_| s.clone());
        let fresh = GlitchRenderer::new().render(&img, 70, 1.0 / 30.0, &|_| s.clone());
        assert_eq!(a.pixels, b.pixels);
        assert_eq!(a.pixels, fresh.pixels);
        assert_ne!(a.pixels, img.pixels);
    }
}
