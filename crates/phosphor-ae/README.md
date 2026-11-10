# Phosphor

**Analog video for After Effects, from the camera to the tube.** Phosphor is an After Effects port of
[NTSCRT](https://github.com/finnmckenty/NTSCRT) by Finn McKenty, with the same stages, controls,
looks and look:

```
layer → Digital Video (camera + codec) → NTSC/VHS signal (ntsc-rs) → Downscale → Glitch (TV + VCR) → CRT shader
                                   + Screen Loop (a camera filming its own TV)
```

> **Vibecoded.** Phosphor was written by AI for an author who is
> not a programmer, then tested in After Effects. It is free and open source, shared as-is.
> Everything hard in it is other people's work: see **[CREDITS.md](CREDITS.md)**.

## Features

- **Look** menu with NTSCRT's 27 looks (animated looks become After Effects keyframes with NTSCRT's
  easing), Load/Save Look in NTSCRT's format.
- **Digital Video**: 5 camera/codec formats (Webcam, Video call, MiniDV, Early web video, Datamosh).
- **NTSC (TV)**: every ntsc-rs control, with NTSCRT's defaults; load/save ntsc-rs presets.
- **Downscale**: SNES / VGA / custom width, Chunky or Smooth.
- **Glitch**: reception (signal, holds, ghost, hum…) and tape (head switching, tracking, dropouts…).
- **CRT**: 7 RetroArch shaders (crt-royale, crt-easymode, crt-aperture, crt-hyllian, crtsim,
  crtglow gauss/lanczos), with only the selected shader's controls shown.
- **Screen Loop**: video feedback tunnel with draggable vanishing point and 5 presets; the camera
  runs on the GPU (or the CPU, from the effect's menu).

## Install

Windows 10/11, 64-bit, After Effects (tested on 2026; recent versions should work), a Direct3D 11 graphics card.

1. Download `Phosphor-...-windows.zip` from the [releases page](https://github.com/Alev0000/phosphor/releases/latest) and unzip it.
2. Close After Effects.
3. Copy the `Phosphor` folder (with `Phosphor.aex`, `shaders\` and `LICENSES\` inside) to
   `C:\Program Files\Adobe\Common\Plug-ins\7.0\MediaCore\` (Windows asks for administrator permission).
4. Open After Effects: **Effect › Stylize › Phosphor**.

Step-by-step guide with troubleshooting: [INSTALL.md](INSTALL.md).

## Build

From the repository root (Rust stable, Visual Studio 2022 Build Tools with C++, LLVM):

```powershell
cargo test -p phosphor-core --release
cargo build -p phosphor-ae --release
```

The plugin is `target\release\phosphor_ae.dll`, renamed `Phosphor.aex`. The CRT shaders are copied
from slang-shaders by `cargo run -p phosphor-core --release --example gen_crt_params -- <slang-shaders> <out\shaders>`.

## License

Phosphor's own code: MIT OR ISC OR Apache-2.0. The parts ported from NTSCRT are under NTSCRT's MIT
license, Copyright (c) 2026 Finn McKenty ([LICENSES/NTSCRT-MIT.txt](LICENSES/NTSCRT-MIT.txt)).
ntsc-rs, librashader and the shaders keep their own licenses: see [CREDITS.md](CREDITS.md).
