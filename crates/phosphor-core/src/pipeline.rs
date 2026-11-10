//! NTSCRT's frame pipeline: Digital video → NTSC (ntsc-rs) → Downscale → Glitch → CRT.

use rayon::prelude::*;
use ntsc_rs::yiq_fielding::{BlitInfo, DeinterlaceMode, Rgbx, YiqView};
use ntsc_rs::NtscEffect;

use crate::digital::{self, DigitalRenderer, DigitalRequest, DigitalSettings};
use crate::downscale::{downscale, DownscaleMethod, DownscaleSpec};
use crate::glitch::{GlitchRenderer, GlitchSettings};
use crate::howlaround::{camera_cpu, HowlLoop, HowlSettings};
use crate::Image;

#[cfg(windows)]
use crate::crt::{CrtPreset, CrtRenderer};

/// Long-lived state shared by every frame of one effect instance.
pub struct Pipeline {
    pub digital: DigitalRenderer,
    pub glitch: GlitchRenderer,
    #[cfg(windows)]
    pub crt: CrtRenderer,
}

impl Pipeline {
    #[cfg(windows)]
    pub fn new(shaders_root: impl AsRef<std::path::Path>) -> Self {
        Self {
            digital: DigitalRenderer::new(),
            glitch: GlitchRenderer::new(),
            crt: CrtRenderer::new(shaders_root),
        }
    }
}

pub struct DigitalStage<'a> {
    pub settings_at: &'a dyn Fn(i64) -> DigitalSettings,
    /// Earlier source frames (full size), for re-running the codec from a keyframe.
    pub earlier: &'a dyn Fn(i64) -> Option<Image>,
}

pub struct NtscStage<'a> {
    pub effect: &'a NtscEffect,
    pub scale: [f32; 2],
}

#[cfg(windows)]
pub struct CrtStage<'a> {
    pub preset: &'static CrtPreset,
    pub values: &'a [(&'a str, f32)],
}

pub struct FrameRequest<'a> {
    pub frame: i64,
    pub fps: f64,
    pub input: &'a Image,
    pub out_width: usize,
    pub out_height: usize,
    pub digital: Option<DigitalStage<'a>>,
    pub ntsc: Option<NtscStage<'a>>,
    pub downscale: Option<(usize, DownscaleMethod)>,
    pub glitch: Option<&'a dyn Fn(i64) -> GlitchSettings>,
    #[cfg(windows)]
    pub crt: Option<CrtStage<'a>>,
}

/// The Digital stage's camera resolution: the Downscale size when it's on, else the picture's own (capped).
pub fn capture_size_for(src_w: usize, src_h: usize, downscale: Option<(usize, DownscaleMethod)>) -> (usize, usize) {
    match downscale {
        Some((w, m)) => {
            let d = DownscaleSpec::for_source(w, src_w, src_h, m);
            (d.width, d.height)
        }
        None => digital::capture_size(src_w, src_h, src_w.min(digital::MAX_UNDOWNSCALED_WIDTH)),
    }
}

impl FrameRequest<'_> {
    pub fn downscale_spec(&self) -> Option<DownscaleSpec> {
        self.downscale
            .map(|(w, m)| DownscaleSpec::for_source(w, self.input.width, self.input.height, m))
    }

    pub fn capture_size(&self) -> (usize, usize) {
        capture_size_for(self.input.width, self.input.height, self.downscale)
    }

    pub fn digital_request<'b>(&'b self, stage: &'b DigitalStage<'b>) -> DigitalRequest<'b> {
        DigitalRequest {
            frame: self.frame,
            capture_size: self.capture_size(),
            source_fps: self.fps,
            settings_at: stage.settings_at,
        }
    }
}

/// ntsc-rs on an 8-bit RGBA copy, exactly as NTSCRT's ntscrs-capi does (BGRA8 there, Bob deinterlace).
pub fn apply_ntsc(img: &Image, effect: &NtscEffect, frame: i64, scale: [f32; 2]) -> Image {
    // The 8-bit copy and the YIQ planes are big (8 + 25 MB at 1080p); fresh ones every frame cost
    // more in page faults than the conversions, so each thread keeps its own.
    thread_local! {
        static BUFFERS: std::cell::RefCell<(Vec<u8>, Vec<f32>)> = const { std::cell::RefCell::new((Vec::new(), Vec::new())) };
    }
    let (w, h) = (img.width, img.height);
    let frame = frame.max(0) as usize;
    let field = effect.use_field.to_yiq_field(frame);
    BUFFERS.with_borrow_mut(|(buf, yiq)| {
        buf.resize(w * h * 4, 0);
        buf.par_chunks_exact_mut(w * 4).zip(img.pixels.par_chunks_exact(w)).for_each(|(dst, src)| {
            for (d, p) in dst.chunks_exact_mut(4).zip(src) {
                let c = |v: f32| (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
                d.copy_from_slice(&[c(p[0]), c(p[1]), c(p[2]), 255]);
            }
        });
        // Every sample is written by the blit before the effect reads it.
        yiq.resize(YiqView::buf_length_for((w, h), field), 0.0);
        let mut view = YiqView::from_parts(yiq, (w, h), field);
        let blit = BlitInfo::from_full_frame(w, h, w * 4);
        view.set_from_strided_buffer::<Rgbx, u8, _>(buf, blit, ());
        effect.apply_effect_to_yiq(&mut view, frame, scale);
        view.write_to_strided_buffer::<Rgbx, u8, _>(buf, blit, DeinterlaceMode::Bob, ());
        let mut out = Image::uninit(w, h);
        out.pixels.par_chunks_exact_mut(w).zip(buf.par_chunks_exact(w * 4)).for_each(|(dst, src)| {
            for (o, p) in dst.iter_mut().zip(src.chunks_exact(4)) {
                *o = [p[0] as f32 / 255.0, p[1] as f32 / 255.0, p[2] as f32 / 255.0, 1.0];
            }
        });
        out
    })
}

/// Nearest enlargement by an integer factor.
fn enlarge(img: &Image, k: usize) -> Image {
    let mut out = Image::new(img.width * k, img.height * k);
    for y in 0..out.height {
        for x in 0..out.width {
            out.pixels[y * out.width + x] = img.get(x / k, y / k);
        }
    }
    out
}

fn area_to(img: &Image, w: usize, h: usize) -> Image {
    downscale(
        img,
        DownscaleSpec {
            width: w,
            height: h,
            method: DownscaleMethod::Area,
        },
    )
}

/// What the picture looks like with the CRT stage off: hard pixels at the output size (NTSCRT's
/// ShaderBypass).
pub fn bypass(img: &Image, ow: usize, oh: usize) -> Image {
    let (iw, ih) = (img.width, img.height);
    if (iw, ih) == (ow, oh) {
        return img.clone();
    }
    let (kx, ky) = (ow / iw, oh / ih);
    if kx >= 1 && kx == ky && iw * kx == ow && ih * ky == oh {
        return enlarge(img, kx);
    }
    if ow > iw && oh > ih {
        let k = ow.div_ceil(iw).max(oh.div_ceil(ih));
        return area_to(&enlarge(img, k), ow, oh);
    }
    area_to(img, ow, oh)
}

/// Multiple of the chain input to render the CRT at before integrating down (NTSCRT's ScanlineGrid).
pub fn supersample_factor(input_height: usize, target_height: usize) -> usize {
    if input_height == 0 || target_height == 0 || target_height % input_height == 0 {
        return 1;
    }
    let rows_per_line = target_height as f64 / input_height as f64;
    if rows_per_line >= 3.0 {
        return 1;
    }
    let mut k = 4;
    while k > 2 && (k * input_height > 4 * target_height || k * input_height > 4096) {
        k -= 2;
    }
    if k * input_height > 4 * target_height || k * input_height > 4096 {
        return 1;
    }
    k
}

pub type Error = Box<dyn std::error::Error + Send + Sync>;

/// One pass of the Screen Loop.
#[cfg(windows)]
#[derive(Clone, Copy)]
pub struct LoopPass<'a> {
    pub settings: &'a HowlSettings,
    pub scene: &'a Image,
    pub time: f64,
    /// The render's length in seconds: what a move spans and a seamless loop repeats over.
    pub length: f64,
    pub stages: &'a ChainStages<'a>,
    pub crt: Option<&'a CrtStage<'a>>,
    pub signal_frame: i64,
    pub glitch_frame: i64,
    /// Film with the GPU camera (falls back to the CPU one if the GPU fails).
    pub gpu: bool,
}

/// The stages between a camera picture and the CRT: NTSC, Downscale, Glitch.
pub struct ChainStages<'a> {
    pub ntsc: Option<&'a NtscStage<'a>>,
    pub downscale: Option<(usize, DownscaleMethod)>,
    pub glitch: Option<&'a dyn Fn(i64) -> GlitchSettings>,
    pub fps: f64,
}

impl Pipeline {
    /// A picture through NTSC, Downscale and Glitch. `signal_frame` numbers the frame for ntsc-rs
    /// (NTSCRT's exports count from 1); `glitch_frame` is the frame on the timeline.
    pub fn chain_input(&self, img: &Image, stages: &ChainStages, signal_frame: i64, glitch_frame: i64) -> Image {
        // Each stage makes a new picture; the first one reads the caller's (no copy of a full frame).
        let mut cur: Option<Image> = None;
        if let Some(ntsc) = stages.ntsc {
            cur = Some(apply_ntsc(cur.as_ref().unwrap_or(img), ntsc.effect, signal_frame, ntsc.scale));
        }
        if let Some((w, m)) = stages.downscale {
            let src = cur.as_ref().unwrap_or(img);
            cur = Some(downscale(src, DownscaleSpec::for_source(w, src.width, src.height, m)));
        }
        if let Some(settings_at) = stages.glitch {
            let src = cur.as_ref().unwrap_or(img);
            cur = Some(self.glitch.render(src, glitch_frame, 1.0 / stages.fps.max(1.0), settings_at));
        }
        cur.unwrap_or_else(|| img.clone())
    }

    /// The chain input on the screen: through the CRT shader (integrated down from a whole multiple
    /// when the size would band the scanlines), or hard pixels with the shader off.
    #[cfg(windows)]
    pub fn finish(&self, img: &Image, ow: usize, oh: usize, crt: Option<&CrtStage>, frame_count: i64) -> Result<Image, Error> {
        let Some(crt) = crt else { return Ok(bypass(img, ow, oh)) };
        let fc = frame_count.max(0) as usize;
        let k = supersample_factor(img.height, oh);
        let (rw, rh) = (img.width * k, img.height * k);
        if k > 1 && rw > ow && rh > oh {
            let big = self.crt.render(img, rw, rh, crt.preset, crt.values, fc)?;
            Ok(area_to(&big, ow, oh))
        } else {
            self.crt.render(img, ow, oh, crt.preset, crt.values, fc)
        }
    }

    /// One trip round the Screen Loop: the camera films the scene and the TV, the picture goes
    /// through the chain, and the TV's copy is stored for the passes to come. Returns the frame at
    /// `out` size when asked for.
    #[cfg(windows)]
    pub fn loop_pass(&self, l: &mut HowlLoop, f: &LoopPass, out: Option<(usize, usize)>) -> Result<Option<Image>, Error> {
        let setup = l.camera_setup(f.scene.width, f.scene.height, f.settings, f.time, f.length);
        let gpu = if f.gpu { self.crt.camera(f.scene, l.tv(), &setup).ok() } else { None };
        let camera = gpu.unwrap_or_else(|| camera_cpu(f.scene, l.tv(), &setup));
        l.measure(&camera, f.scene, f.settings);
        let chain = self.chain_input(&camera, f.stages, f.signal_frame, f.glitch_frame);
        let (fw, fh) = l.feedback_size;
        let tv = self.finish(&chain, fw, fh, f.crt, f.signal_frame)?;
        let frame = match out {
            Some((ow, oh)) => Some(self.finish(&chain, ow, oh, f.crt, f.signal_frame)?),
            None => None,
        };
        l.advance(&tv, f.settings);
        Ok(frame)
    }

    /// The run-up before the first frame, so it already shows the whole tunnel (NTSCRT's
    /// `HowlaroundLoop.runUp`): passes at one frame's lag on the first scene, then at the real delay.
    #[cfg(windows)]
    /// `progress(done, total)` is called before each pass; returning an error stops the run-up.
    pub fn loop_run_up(&self, l: &mut HowlLoop, first: &LoopPass, fps: f64, progress: &mut dyn FnMut(usize, usize) -> Result<(), Error>) -> Result<(), Error> {
        let aspect = first.scene.width as f64 / first.scene.height.max(1) as f64;
        let (build, settle) = first.settings.run_up_plan(aspect, l.chain_size.1);
        let frames = build + settle;
        let delay = first.settings.delay();
        for k in 0..frames {
            progress(k, frames)?;
            if k == build && build > 0 && delay > 1 {
                l.fill_history();
            }
            l.set_lag(if k < build { 1 } else { delay });
            let pass = LoopPass {
                time: first.time + (k as f64 - frames as f64) / fps.max(1.0),
                signal_frame: 100_000 + k as i64,
                ..*first
            };
            self.loop_pass(l, &pass, None)?;
        }
        l.set_lag(delay);
        Ok(())
    }

    #[cfg(windows)]
    pub fn render(&self, req: &FrameRequest) -> Result<Image, Error> {
        let digital_out = req.digital.as_ref().map(|stage| {
            let dreq = req.digital_request(stage);
            self.digital.render(&dreq, req.input, stage.earlier)
        });
        let img = digital_out.as_ref().unwrap_or(req.input);
        let stages = ChainStages {
            ntsc: req.ntsc.as_ref(),
            downscale: req.downscale,
            glitch: req.glitch,
            fps: req.fps,
        };
        let chain = self.chain_input(img, &stages, req.frame + 1, req.frame);
        self.finish(&chain, req.out_width, req.out_height, req.crt.as_ref(), req.frame + 1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Reused buffers (different sizes and fields in a row) give exactly ntsc-rs's own result.
    #[test]
    fn apply_ntsc_matches_ntsc_rs() {
        let effect: NtscEffect = (&crate::look::house_ntsc()).into();
        for (w, h, frame) in [(97usize, 61usize, 3i64), (64, 48, 4), (97, 61, 5), (33, 20, 0)] {
            let img = Image {
                width: w,
                height: h,
                pixels: (0..w * h).map(|i| [(i % 13) as f32 / 12.0, (i % 7) as f32 / 6.0, (i % 5) as f32 / 4.0, 1.0]).collect(),
            };
            let ours = apply_ntsc(&img, &effect, frame, [1.0, 1.0]);
            let mut buf: Vec<u8> = img.pixels.iter().flat_map(|p| p.map(|v| (v * 255.0 + 0.5) as u8)).collect();
            effect.apply_effect_to_buffer::<Rgbx, u8>((w, h), &mut buf, frame as usize, [1.0, 1.0]);
            for (o, p) in ours.pixels.iter().zip(buf.chunks_exact(4)) {
                for c in 0..3 {
                    assert_eq!((o[c] * 255.0).round() as u8, p[c]);
                }
            }
        }
    }

    #[test]
    fn supersample_matches_ntscrt() {
        assert_eq!(supersample_factor(224, 448), 1);
        assert_eq!(supersample_factor(224, 1080), 1);
        assert_eq!(supersample_factor(416, 702), 4);
        assert_eq!(supersample_factor(1080, 720), 2);
    }

    #[test]
    fn bypass_whole_multiple_is_nearest() {
        let mut img = Image::new(2, 1);
        img.pixels = vec![[1.0, 0.0, 0.0, 1.0], [0.0, 0.0, 1.0, 1.0]];
        let out = bypass(&img, 6, 3);
        assert_eq!(out.get(2, 2), [1.0, 0.0, 0.0, 1.0]);
        assert_eq!(out.get(3, 0), [0.0, 0.0, 1.0, 1.0]);
    }
}
