PHOSPHOR - analog TV, VHS and CRT looks for After Effects
==========================================================

An After Effects port of NTSCRT by Finn McKenty. Free and open source.
https://github.com/Alev0000/phosphor


INSTALL
-------

You need Windows 10/11 (64-bit) and After Effects 2020 or newer
(tested on 2026).

1. Close After Effects.
2. Copy the whole "Phosphor" folder (not just Phosphor.aex) into:
      C:\Program Files\Adobe\Common\Plug-ins\7.0\MediaCore\
   Click "Continue" when Windows asks for administrator permission.
3. Open After Effects: Effect > Stylize > Phosphor.
4. Pick a look from the "Look" menu.


TIPS
----

- Screen Loop (the endless tunnel) is computed frame by frame: playing
  forward is fast, jumping to the middle of the timeline takes a few seconds.
- Drag the "Vanishing point" in the comp window to move the tunnel.


PROBLEMS
--------

- Effect not in the menu: check the folder is exactly
  ...\MediaCore\Phosphor\Phosphor.aex, then restart After Effects.
- CRT does nothing: the "shaders" folder must sit next to Phosphor.aex.
- "Access denied" when copying: close After Effects, Premiere and
  Media Encoder, then try again.
- "Crash occurred while invoking effect plug-in Phosphor": the log below
  records what crashed and where. Please send it with your After Effects
  version.
- Anything else: open an issue at
  https://github.com/Alev0000/phosphor/issues
  and attach %TEMP%\phosphor-ae.log


UNINSTALL / UPDATE
------------------

Uninstall: delete ...\MediaCore\Phosphor (with After Effects closed).
Update: copy the new Phosphor folder over the old one.


Credits and licenses: Phosphor\CREDITS.txt and Phosphor\LICENSES\
Vibecoded: written with AI by a non-programmer, shared as-is.
