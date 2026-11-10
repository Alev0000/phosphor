// Bundled NTSCRT looks and Screen Loop presets (from the NTSCRT app's presets folder).

pub const LOOKS: &[(&str, &str)] = &[
    ("5 second blips", include_str!("../looks/5 second blips.json")),
    ("5 second glitch", include_str!("../looks/5 second glitch.json")),
    ("5 seconds mild glitch", include_str!("../looks/5 seconds mild glitch.json")),
    ("15 second glitches", include_str!("../looks/15 second glitches.json")),
    ("Clean CRT", include_str!("../looks/Clean CRT.json")),
    ("Clean VHS", include_str!("../looks/Clean VHS.json")),
    ("Gentle waves loop", include_str!("../looks/Gentle waves loop.json")),
    ("Glitch 1", include_str!("../looks/Glitch 1.json")),
    ("Glitch 2", include_str!("../looks/Glitch 2.json")),
    ("Glitch 3", include_str!("../looks/Gltch 3.json")),
    ("Glitch loop 1", include_str!("../looks/Glitch loop 1.json")),
    ("Glitch pop", include_str!("../looks/Glitch pop.json")),
    ("High Chroma static", include_str!("../looks/High Chroma static.json")),
    ("Low Res", include_str!("../looks/Low Res.json")),
    ("Medium VHS", include_str!("../looks/Medium VHS.json")),
    ("Mild VHS", include_str!("../looks/Mild VHS.json")),
    ("Obliterated", include_str!("../looks/Obliterated.json")),
    ("Readable Text", include_str!("../looks/Readable Text.json")),
    ("Static loop 1", include_str!("../looks/Static loop 1.json")),
    ("Tracking Noise up", include_str!("../looks/Tracking Noise up.json")),
    ("Two blips loop", include_str!("../looks/Two blips loop.json")),
    ("V Hold glitch", include_str!("../looks/V Hold glitch.json")),
    ("Vertical Freak Out", include_str!("../looks/Vertical Freak Out.json")),
    ("Vertical spike loop", include_str!("../looks/Vertical spike loop.json")),
    ("Very wavy", include_str!("../looks/Very wavy.json")),
    ("Wavy loop", include_str!("../looks/Wavy loop.json")),
    ("Webcam (low quality)", include_str!("../looks/Webcam (low quality).json")),
];

pub const SCREEN_LOOPS: &[(&str, &str)] = &[
    ("7 copies slow luma", include_str!("../looks/screen-loop/7 copies slow luma.json")),
    ("12 copies luma key", include_str!("../looks/screen-loop/12 copies luma key.json")),
    ("15 copies color drift", include_str!("../looks/screen-loop/15 copies color drift.json")),
    ("22 copies bottom left", include_str!("../looks/screen-loop/22 copies bottom left.json")),
    ("Bottom to top", include_str!("../looks/screen-loop/Bottom to top.json")),
];
