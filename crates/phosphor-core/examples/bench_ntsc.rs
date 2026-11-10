//! Where ntsc-rs spends its time on a 1080p frame (house look), pass by pass.
//!
//! cargo run -p phosphor-core --release --example bench_ntsc

use std::time::Instant;

use ntsc_rs::yiq_fielding::{BlitInfo, DeinterlaceMode, Rgbx, YiqView};
use ntsc_rs::NtscEffect;
use phosphor_core::look::house_ntsc;

fn time(n: usize, mut f: impl FnMut(usize)) -> f64 {
    f(0);
    let mut v: Vec<f64> = (1..=n)
        .map(|i| {
            let t = Instant::now();
            f(i);
            t.elapsed().as_secs_f64() * 1000.0
        })
        .collect();
    v.sort_by(|a, b| a.total_cmp(b));
    v[v.len() / 2]
}

fn main() {
    let (w, h) = (1920usize, 1080usize);
    let n = 15;
    let buf: Vec<u8> = (0..w * h * 4).map(|i| ((i * 7) % 251) as u8).collect();
    let base = house_ntsc();
    let effect: NtscEffect = (&base).into();
    let field = effect.use_field.to_yiq_field(3);
    println!("field {:?}", field);
    let blit = BlitInfo::from_full_frame(w, h, w * 4);
    let mut yiq = vec![0f32; YiqView::buf_length_for((w, h), field)];
    let row = |name: &str, ms: f64| println!("{name:<40} {ms:>7.2} ms");

    row("rgb -> yiq", time(n, |_| {
        let mut view = YiqView::from_parts(&mut yiq, (w, h), field);
        view.set_from_strided_buffer::<Rgbx, u8, _>(&buf, blit, ());
    }));
    let mut out = buf.clone();
    row("yiq -> rgb (bob)", time(n, |_| {
        let mut view = YiqView::from_parts(&mut yiq, (w, h), field);
        view.write_to_strided_buffer::<Rgbx, u8, _>(&mut out, blit, DeinterlaceMode::Bob, ());
    }));
    let mut run = |name: &str, e: &NtscEffect| {
        let ms = time(n, |i| {
            let mut view = YiqView::from_parts(&mut yiq, (w, h), field);
            view.set_from_strided_buffer::<Rgbx, u8, _>(&buf, blit, ());
            e.apply_effect_to_yiq(&mut view, i, [1.0, 1.0]);
        });
        row(name, ms);
        ms
    };
    let full = run("rgb->yiq + effect (house)", &effect);

    // Turn each optional pass off in turn to see what it costs.
    let off: Vec<(&str, Box<dyn Fn(&mut NtscEffect)>)> = vec![
        ("- head switching", Box::new(|e| e.head_switching = None)),
        ("- tracking noise", Box::new(|e| e.tracking_noise = None)),
        ("- composite noise", Box::new(|e| e.composite_noise = None)),
        ("- ringing", Box::new(|e| e.ringing = None)),
        ("- luma noise", Box::new(|e| e.luma_noise = None)),
        ("- chroma noise", Box::new(|e| e.chroma_noise = None)),
        ("- vhs", Box::new(|e| e.vhs_settings = None)),
        ("- snow", Box::new(|e| e.snow_intensity = 0.0)),
        ("- luma smear", Box::new(|e| e.luma_smear = 0.0)),
        ("- composite sharpening", Box::new(|e| e.composite_sharpening = 0.0)),
        ("- chroma phase noise", Box::new(|e| e.chroma_phase_noise_intensity = 0.0)),
        ("- chroma delay", Box::new(|e| { e.chroma_delay_horizontal = 0.0; e.chroma_delay_vertical = 0 })),
    ];
    let mut all = effect.clone();
    for (name, f) in &off {
        let mut e = effect.clone();
        f(&mut e);
        let ms = run(name, &e);
        println!("{:<40} {:>7.2} ms saved", "", full - ms);
        f(&mut all);
    }
    run("everything optional off", &all);
    let img = phosphor_core::Image { width: w, height: h, pixels: (0..w * h).map(|i| { let v = (i % 251) as f32 / 251.0; [v, 1.0 - v, 0.5, 1.0] }).collect() };
    row("pipeline::apply_ntsc (whole)", time(n, |i| {
        std::hint::black_box(phosphor_core::pipeline::apply_ntsc(&img, &effect, i as i64, [1.0, 1.0]));
    }));
    row("alloc yiq buffer (zeroed, touched)", time(n, |_| {
        let mut v = vec![0f32; YiqView::buf_length_for((w, h), field)];
        v.iter_mut().step_by(1024).for_each(|x| *x = 1.0);
        std::hint::black_box(v);
    }));
    row("alloc Image 1080p (touched)", time(n, |_| {
        let mut v = phosphor_core::Image::new(w, h);
        v.pixels.iter_mut().step_by(256).for_each(|x| x[0] = 1.0);
        std::hint::black_box(v);
    }));
}

#[allow(dead_code)]
fn unused() {}
