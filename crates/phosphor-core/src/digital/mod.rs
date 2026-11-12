//! The Digital video stage: a camera plus a real codec (port of NTSCRT's CrtCodec).
//!
//! Any frame can be asked for in any order. The stream is re-run from the last point the decoder's
//! picture is known (a keyframe it received, a checkpoint at one it lost, or the last frame this
//! renderer produced under the same history), so a given frame always looks the same.

pub mod camera;
pub mod codec;
pub mod params;

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::sync::{Arc, Mutex};

use camera::Rgb8;
use codec::{CodecState, FrameKind, FrameSettings, Geometry, Picture, Steps};
pub use params::{DigitalFormat, DigitalSettings, DIGITAL_FORMATS, DIGITAL_PARAMS};

use crate::Image;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DigitalTiming {
    pub source_fps: f64,
    pub capture_fps: f64,
    pub keyframe_every: i64,
}

impl DigitalTiming {
    pub fn new(source_fps: f64, settings: &DigitalSettings) -> Self {
        let source_fps = source_fps.max(1.0);
        let capture_fps = source_fps.min(settings.get("frame_rate").max(1.0));
        let seconds = settings.get("keyframe_interval");
        let keyframe_every = if seconds <= 0.0 {
            1
        } else {
            ((seconds * capture_fps).round() as i64).max(2)
        };
        Self {
            source_fps,
            capture_fps,
            keyframe_every,
        }
    }

    fn ratio(&self) -> f64 {
        self.capture_fps / self.source_fps
    }

    pub fn capture_of_frame(&self, n: i64) -> i64 {
        if self.ratio() >= 1.0 {
            n
        } else {
            (n as f64 * self.ratio() + 1e-9).floor() as i64
        }
    }

    pub fn frame_of_capture(&self, c: i64) -> i64 {
        if self.ratio() >= 1.0 {
            c
        } else {
            (c as f64 / self.ratio() - 1e-9).ceil() as i64
        }
    }

    pub fn intra_only(&self) -> bool {
        self.keyframe_every == 1
    }

    pub fn kind(&self, c: i64, datamosh: f64) -> FrameKind {
        if self.intra_only() {
            return FrameKind::IntraOnly;
        }
        if c % self.keyframe_every != 0 {
            return FrameKind::Predicted;
        }
        if codec::drops_key(c / self.keyframe_every, datamosh) {
            FrameKind::DroppedKey
        } else {
            FrameKind::Key
        }
    }
}

/// The capture size for a picture at a capture width: height from the aspect, even.
pub fn capture_size(width: usize, height: usize, capture_width: usize) -> (usize, usize) {
    let cw = capture_width.min(width).max(16);
    let aspect = (width as f64 / height.max(1) as f64).max(0.05);
    (cw, (2 * ((cw as f64 / aspect / 2.0).round() as usize)).max(16))
}

/// Widest capture when the Downscale stage is off (the codec's cost grows with area).
pub const MAX_UNDOWNSCALED_WIDTH: usize = 1280;

#[derive(Clone, Copy, Debug, PartialEq)]
struct StreamKey {
    geometry: Geometry,
    timing: DigitalTiming,
    structure: u64,
}

struct Saved {
    key: StreamKey,
    capture: i64,
    history: u64,
    state: CodecState,
    shown: Arc<Rgb8>,
}

const CHECKPOINT_BUDGET: usize = 96 << 20;

#[derive(Default)]
struct Store {
    /// The last capture rendered (continuing from it is the common case: playback, export).
    last: Option<Saved>,
    /// States at keyframes the decoder lost (the only places its history reaches past).
    checkpoints: Vec<Saved>,
    checkpoint_bytes: usize,
}

/// One frame request. `settings_at(k)` is the settings at source frame k.
pub struct DigitalRequest<'a> {
    pub frame: i64,
    pub capture_size: (usize, usize),
    pub source_fps: f64,
    pub settings_at: &'a dyn Fn(i64) -> DigitalSettings,
}

struct Plan {
    key: StreamKey,
    timing: DigitalTiming,
    structure: DigitalSettings,
    capture: i64,
    hashes: Vec<u64>,
    resume: Option<(i64, CodecState, Arc<Rgb8>)>,
    first: i64,
}

fn structure_hash(s: &DigitalSettings) -> u64 {
    let mut h = DefaultHasher::new();
    for id in params::STRUCTURE_IDS {
        s.get(id).to_bits().hash(&mut h);
    }
    h.finish()
}

fn state_bytes(s: &CodecState) -> usize {
    s.encoder.as_ref().map_or(0, |p| p.byte_count()) * 2
}

pub struct DigitalRenderer {
    store: Mutex<Store>,
}

impl Default for DigitalRenderer {
    fn default() -> Self {
        Self::new()
    }
}

impl DigitalRenderer {
    pub fn new() -> Self {
        Self {
            store: Mutex::new(Store::default()),
        }
    }

    /// The store, emptied if a render panicked while holding it (it may be half-written).
    fn lock_store(&self) -> std::sync::MutexGuard<'_, Store> {
        self.store.lock().unwrap_or_else(|p| {
            let mut g = p.into_inner();
            *g = Store::default();
            g
        })
    }

    fn settings_for(req: &DigitalRequest, timing: &DigitalTiming, structure: &DigitalSettings, c: i64) -> DigitalSettings {
        (req.settings_at)(timing.frame_of_capture(c)).holding_structure(structure)
    }

    fn plan(&self, req: &DigitalRequest) -> Plan {
        let structure = (req.settings_at)(req.frame);
        let timing = DigitalTiming::new(req.source_fps, &structure);
        let geometry = Geometry {
            width: req.capture_size.0,
            height: req.capture_size.1,
            chroma: structure.chroma(),
        };
        let key = StreamKey {
            geometry,
            timing,
            structure: structure_hash(&structure),
        };
        let c = timing.capture_of_frame(req.frame.max(0));

        // Rolling hash of the settings every capture ran under, from capture 0.
        let mut hashes = Vec::with_capacity(c as usize + 1);
        let mut h = 0u64;
        for k in 0..=c {
            let mut hasher = DefaultHasher::new();
            h.hash(&mut hasher);
            Self::settings_for(req, &timing, &structure, k).hash_into(&mut hasher);
            h = hasher.finish();
            hashes.push(h);
        }

        let store = self.lock_store();
        let g = timing.keyframe_every;

        // The earliest capture that must be coded, and any checkpoint to start from.
        let mut resume: Option<(i64, CodecState, Arc<Rgb8>)> = None;
        let start = if timing.intra_only() {
            (c - 1).max(0)
        } else {
            let mut b = (c / g) * g;
            while b > 0 {
                let datamosh = Self::settings_for(req, &timing, &structure, b).get("datamosh");
                if timing.kind(b, datamosh) == FrameKind::Key {
                    break;
                }
                if let Some(cp) = store
                    .checkpoints
                    .iter()
                    .find(|s| s.key == key && s.capture == b && s.history == hashes[b as usize])
                {
                    resume = Some((b, cp.state.clone(), cp.shown.clone()));
                    break;
                }
                b -= g;
            }
            b
        };

        // Continue from the last render when it lies on this history, at or past the start.
        if let Some(last) = &store.last {
            if last.key == key
                && last.capture <= c
                && last.capture >= start
                && resume.as_ref().map_or(true, |(b, _, _)| last.capture > *b)
                && last.history == hashes[last.capture as usize]
            {
                resume = Some((last.capture, last.state.clone(), last.shown.clone()));
            }
        }

        let first = match &resume {
            Some((k, _, _)) => k + 1,
            None => start,
        };
        Plan {
            key,
            timing,
            structure,
            capture: c,
            hashes,
            resume,
            first,
        }
    }

    /// Source frames (other than `req.frame`) whose pixels `render` will ask for.
    pub fn frames_needed(&self, req: &DigitalRequest) -> Vec<i64> {
        let plan = self.plan(req);
        (plan.first..=plan.capture)
            .map(|k| plan.timing.frame_of_capture(k))
            .filter(|&f| f != req.frame)
            .collect()
    }

    /// Replace frame `req.frame` (`current`, full size) with what the camera and codec make of it.
    /// `earlier(f)` supplies source frame f; when it returns None the current frame stands in.
    pub fn render(&self, req: &DigitalRequest, current: &Image, earlier: &dyn Fn(i64) -> Option<Image>) -> Image {
        let plan = self.plan(req);
        let timing = plan.timing;
        let geometry = plan.key.geometry;
        let size = req.capture_size;
        let structure = &plan.structure;
        let c = plan.capture;

        let current_rgb = camera::scale(&Rgb8::from_image(current), size, structure);
        let source_for = |k: i64| -> Rgb8 {
            let f = timing.frame_of_capture(k);
            if f == req.frame {
                return current_rgb.clone();
            }
            match earlier(f) {
                Some(img) => camera::scale(&Rgb8::from_image(&img), size, structure),
                None => current_rgb.clone(),
            }
        };

        let (mut state, mut shown) = match plan.resume {
            Some((_, state, shown)) => (state, shown),
            None => (CodecState::new(geometry), Arc::new(Rgb8 { width: 0, height: 0, px: Vec::new() })),
        };

        if plan.first > c {
            if shown.px.is_empty() {
                let s = Self::settings_for(req, &timing, structure, c);
                let step = Steps::new(s.get("quality"), 1.0).inter;
                if let Some(dec) = &state.decoder {
                    shown = Arc::new(camera::render(dec, &geometry, &s, step));
                }
            }
        } else {
            let mut new_checkpoints = Vec::new();
            for k in plan.first..=c {
                let s = Self::settings_for(req, &timing, structure, k);
                let picture: Picture = camera::capture(&source_for(k), &geometry, &s, k);
                let mut kind = timing.kind(k, s.get("datamosh"));
                if kind != FrameKind::IntraOnly && state.encoder.is_none() {
                    kind = FrameKind::Key;
                }
                let (decoded, step) = codec::step(
                    &mut state,
                    &picture,
                    kind,
                    k,
                    FrameSettings {
                        quality: s.get("quality"),
                        fixed_bitrate: s.get("fixed_bitrate"),
                        data_loss: s.get("data_loss"),
                    },
                );
                shown = Arc::new(camera::render(&decoded, &geometry, &s, step));
                if kind == FrameKind::DroppedKey {
                    new_checkpoints.push((k, state.clone(), shown.clone()));
                }
            }

            let mut store = self.lock_store();
            for (k, st, sh) in new_checkpoints {
                let history = plan.hashes[k as usize];
                if store
                    .checkpoints
                    .iter()
                    .any(|s| s.key == plan.key && s.capture == k && s.history == history)
                {
                    continue;
                }
                store.checkpoint_bytes += state_bytes(&st);
                store.checkpoints.push(Saved {
                    key: plan.key,
                    capture: k,
                    history,
                    state: st,
                    shown: sh,
                });
                while store.checkpoint_bytes > CHECKPOINT_BUDGET && !store.checkpoints.is_empty() {
                    let old = store.checkpoints.remove(0);
                    store.checkpoint_bytes -= state_bytes(&old.state);
                }
            }
            store.last = Some(Saved {
                key: plan.key,
                capture: c,
                history: plan.hashes[c as usize],
                state,
                shown: shown.clone(),
            });
        }

        camera::upscale(&shown, current.width, current.height)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(w: usize, h: usize, t: i64) -> Image {
        let mut img = Image::new(w, h);
        for y in 0..h {
            for x in 0..w {
                let v = ((x as i64 + 3 * t + y as i64 / 4) % 64) as f32 / 63.0;
                img.pixels[y * w + x] = [v, 0.5, 1.0 - v, 1.0];
            }
        }
        img
    }

    fn run(r: &DigitalRenderer, settings: &DigitalSettings, n: i64) -> Image {
        let req = DigitalRequest {
            frame: n,
            capture_size: capture_size(128, 96, 64),
            source_fps: 30.0,
            settings_at: &|_| settings.clone(),
        };
        r.render(&req, &frame(128, 96, n), &|f| Some(frame(128, 96, f)))
    }

    #[test]
    fn any_order_gives_the_same_frame() {
        let mut s = DIGITAL_FORMATS.iter().find(|f| f.id == "video_call").unwrap().settings();
        s.set("keyframe_interval", 1.0);
        let a = DigitalRenderer::new();
        let seq: Vec<Image> = (0..20).map(|n| run(&a, &s, n)).collect();
        let b = DigitalRenderer::new();
        for n in [17, 3, 11, 19, 0] {
            assert_eq!(run(&b, &s, n).pixels, seq[n as usize].pixels, "frame {n}");
        }
    }

    #[test]
    fn datamosh_checkpoints_match_a_straight_run() {
        let s = DIGITAL_FORMATS.iter().find(|f| f.id == "datamosh").unwrap().settings();
        let a = DigitalRenderer::new();
        let straight: Vec<Image> = (0..70).map(|n| run(&a, &s, n)).collect();
        let b = DigitalRenderer::new();
        for n in [65, 40, 69, 5] {
            assert_eq!(run(&b, &s, n).pixels, straight[n as usize].pixels, "frame {n}");
        }
    }
}
