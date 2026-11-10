# Installing Phosphor in After Effects

Phosphor is an After Effects effect that makes any footage look like analog TV: digital camera,
NTSC/VHS signal, TV and VCR glitches, a tube screen (CRT) and the "camera filming its own TV"
tunnel. It is an After Effects version of the **NTSCRT** app by Finn McKenty.

> **Vibecoded.** The code was written by an AI (Claude, through Claude Code) for an author who is
> not a programmer, and tested in After Effects. It is free and open source. Everyone who did the
> hard parts is credited in `CREDITS.md`.

---

## What you need

- Windows 10 or 11, 64-bit.
- Adobe After Effects (tested on 2026; recent versions should work).
- A graphics card with Direct3D 11 (practically any from the last 10 years). Without one the effect
  still works, just slower.

## Step by step

### 1. Close After Effects

While it is open, Windows won't let you copy the plugin.

### 2. Download and unzip Phosphor

Download `Phosphor-...-windows.zip` from the releases page:
https://github.com/Alev0000/phosphor/releases/latest

Right-click the zip › **Extract All…**. You get a `Phosphor` folder containing:

```
Phosphor\
├─ Phosphor.aex      the plugin
├─ shaders\          the CRT shaders (don't delete, the effect needs them)
├─ LICENSES\         the licenses
└─ CREDITS.md        the credits
```

### 3. Open Adobe's plug-ins folder

In File Explorer, paste this address into the bar at the top and press Enter:

```
C:\Program Files\Adobe\Common\Plug-ins\7.0\MediaCore
```

### 4. Copy the whole `Phosphor` folder into it

Drag the `Phosphor` folder (the folder, not just `Phosphor.aex`) into `MediaCore`. Windows asks for
administrator permission: click **Continue**.

You should end up with:

```
C:\Program Files\Adobe\Common\Plug-ins\7.0\MediaCore\Phosphor\Phosphor.aex
C:\Program Files\Adobe\Common\Plug-ins\7.0\MediaCore\Phosphor\shaders\...
```

### 5. Open After Effects and apply the effect

1. Select a layer (video, image, anything).
2. Menu **Effect › Stylize › Phosphor**, or search "Phosphor" in the **Effects & Presets** panel.
3. In the **Look** menu, pick one of NTSCRT's 27 looks. Done.

---

## First steps

| Control | What it does |
|---|---|
| **Look** | 27 ready-made looks. Animated ones become After Effects keyframes. |
| **Load / Save Look File…** | Opens and saves looks in NTSCRT's format (they work in both apps). |
| **Digital Video** | Digital camera and codec: Webcam, Video call, MiniDV, Early web video, Datamosh. |
| **NTSC (TV)** | The analog signal (ntsc-rs): noise, color bleed, VHS. |
| **Downscale** | Lowers the resolution (SNES, VGA or a custom width). |
| **Glitch** | Reception and tape defects: weak signal, rolling picture, ghosting, tracking. |
| **CRT** | The tube screen: 7 RetroArch shaders. |
| **Screen Loop** | The endless tunnel: a camera filming its own TV. Drag the **Vanishing point** in the comp window. |

Tips:

- The Screen Loop is computed in order, frame by frame. Jumping to the middle of the timeline takes
  a few seconds to build the tunnel; playing forward is fast.
- **Camera renders on** (in Screen Loop): leave it on **GPU (recommended)**. **CPU (exact)** is the
  slower reference.

---

## Troubleshooting

**The effect doesn't show up in the menu.**
Check that the path is exactly as in step 4 (the `Phosphor` folder inside `MediaCore`) and restart
After Effects. Search "Phosphor" in the Effects & Presets panel.

**The CRT does nothing / a shader error appears.**
The `shaders` folder must sit next to `Phosphor.aex`. Copy the whole `Phosphor` folder again.

**"Access denied" when copying.**
After Effects (or Premiere / Media Encoder) is still open. Close them all and try again.

**Something went wrong and I want someone to look at it.**
Phosphor writes a log to `%TEMP%\phosphor-ae.log` (paste that address into File Explorer). Attach it
to an issue at https://github.com/Alev0000/phosphor/issues.

## Uninstall

Close After Effects and delete the folder
`C:\Program Files\Adobe\Common\Plug-ins\7.0\MediaCore\Phosphor`.

## Update

Same as installing: close After Effects and copy the new `Phosphor` folder over the old one,
replacing the files.
