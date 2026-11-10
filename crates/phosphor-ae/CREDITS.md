# Phosphor — credits

Phosphor is an After Effects port of **NTSCRT**. Almost everything that makes it look good was made
by the people below. Phosphor itself is the glue: it puts their work inside an After Effects effect.

## Made with AI (vibecoded)

**Phosphor was vibecoded.** Its author is not a programmer: the code was written by an AI
(Anthropic's Claude, through Claude Code), directed, tested and checked in After Effects by a human.
It is shared as-is, in the open, so anyone can read it, fix it and improve it. Bug reports and pull
requests are welcome; expect the rough edges of a project made this way.

## The original app

- **[NTSCRT](https://github.com/finnmckenty/NTSCRT)** by **Finn McKenty**: the macOS app Phosphor
  reproduces. Its pipeline (Digital Video → NTSC → Downscale → Glitch → CRT, plus Screen Loop), its
  Digital Video codec, its Glitch receiver, its Screen Loop video feedback, its 27 looks and 5 Screen
  Loop presets and its default settings were all designed by Finn. Phosphor ports them from Swift
  and Metal to Rust and Direct3D 11, and ships the looks and presets inside the plugin.

  NTSCRT was announced by its author as free and open source, but its repository has no license
  file yet (checked November 2026). The parts of Phosphor ported from NTSCRT remain his work.

## The signal

- **[ntsc-rs](https://github.com/ntsc-rs/ntsc-rs)** by **valadaptive**: the NTSC/VHS signal
  emulation, used unchanged (v0.9.4, the version NTSCRT uses). MIT OR ISC OR Apache-2.0. Phosphor's
  source lives in a fork of the ntsc-rs repository, and the NTSC controls are adapted from ntsc-rs's
  own After Effects plugin.
  - ntsc-rs is a port of **[ntscqt](https://github.com/JargeZ/ntscqt)** by **JargeZ**,
  - a GUI for **[ntsc](https://github.com/zhuker/ntsc)** by **zhuker**,
  - itself a port of **[composite-video-simulator](https://github.com/joncampbell123/composite-video-simulator)**
    by **joncampbell123**.

## The screen

- **[librashader](https://github.com/SnowflakePowered/librashader)** by **SnowflakePowered (Ronny
  Chan)**: runs the RetroArch shaders. MPL-2.0 OR GPL-3.0 (used under MPL-2.0, unmodified; its
  source is at the link). License text: `LICENSES/MPL-2.0.txt`.
- **[libretro/slang-shaders](https://github.com/libretro/slang-shaders)** and the RetroArch
  community: the CRT shaders, shipped as-is in `shaders\` with their license notices in each file.
  - **crt-royale** by **TroggleMonkey**: GPL-2.0-or-later (its blur helpers: MIT).
  - **crt-easymode** and **crt-aperture** by **EasyMode**: GPL.
  - **crt-hyllian** by **Hyllian**: MIT.
  - **crtsim** by **J. Kyle Pittman**: CC0 (public domain).
  - **crtglow** (glow): original by **Themaister**, moiré mitigation by **Timothy Lottes**, added by
    **hunterk**.

  GPL text: `LICENSES/GPL-2.0.txt`. The shaders are separate files the plugin loads at run time.

## The glitches

- **analogtv** by **Trevor Blackwell**, from **[XScreenSaver](https://www.jwz.org/xscreensaver/)**
  (Jamie Zawinski): the design NTSCRT's Glitch stage follows, glitches as the response of simulated
  TV circuits.

## Building blocks

- **[after-effects / premiere crates](https://github.com/virtualritz/after-effects)** by
  **Adrian Eddy** and **Moritz Moeller (virtualritz)**: the Rust bindings for the After Effects SDK.
  Apache-2.0 OR BSD-3-Clause OR MIT OR Zlib.
- **[windows-rs](https://github.com/microsoft/windows-rs)** (Microsoft): Direct3D 11 from Rust.
  MIT OR Apache-2.0.
- **[rayon](https://github.com/rayon-rs/rayon)** (multithreading), **[serde_json](https://github.com/serde-rs/json)**
  (looks), **[rfd](https://github.com/PolyMeilex/rfd)** (file dialogs), **[libm](https://github.com/rust-lang/compiler-builtins)**,
  **[raw-window-handle](https://github.com/rust-windowing/raw-window-handle)**, and the other crates
  listed in `Cargo.lock`, each under its own license (MIT, Apache-2.0 or similar).
- **Adobe After Effects SDK** (Adobe).

## Phosphor's own code

The code written for Phosphor (`crates/phosphor-core`, `crates/phosphor-ae`) is offered under
**MIT OR ISC OR Apache-2.0**, the same terms as ntsc-rs, except for the parts ported from NTSCRT,
which follow whatever license Finn McKenty gives NTSCRT.
