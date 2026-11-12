//! Writing a look's timeline into After Effects keyframes (AEGP; UI thread only).
//!
//! NTSCRT eases each segment by the keyframe it leaves, with CSS curves. Ease in (u²) and ease out
//! (1 − (1 − u)²) are quadratic Béziers, which After Effects' temporal ease reproduces exactly with
//! 33⅓% influence and a speed of 0 or twice the segment's average; ease in-out uses 50% influence
//! at speed 0 on both ends (the same midpoint slope).

use after_effects::{self as ae, aegp, Error, InData, Parameters};

use crate::aegp_compat::{Effect, Interp, Keys};
use phosphor_core::look::Easing;

use crate::{ControlValue, ParamID};

/// One control's values at each look keyframe.
pub struct Track {
    pub id: ParamID,
    pub values: Vec<ControlValue>,
}

/// Remove every keyframe from `ids` (a newly applied look replaces the old look's animation).
pub fn clear(in_data: &InData, params: &Parameters<ParamID>, plugin_id: aegp::PluginId, ids: &[ParamID]) -> Result<(), Error> {
    let effect = Effect::new(in_data, plugin_id)?;
    let keys = Keys::new(in_data)?;
    for id in ids {
        let Some(index) = params.index(*id) else { continue };
        let Ok(stream) = effect.stream(index as i32) else { continue };
        let n = keys.count(&stream).unwrap_or(0);
        for k in (0..n).rev() {
            let _ = keys.delete(&stream, k);
        }
    }
    Ok(())
}

fn number(v: &ControlValue) -> f64 {
    match *v {
        ControlValue::Slider(x) => x,
        ControlValue::Checkbox(b) => b as u8 as f64,
        ControlValue::Popup(i) => i as f64,
    }
}

fn ease(speed: f64, influence: f64) -> ae::sys::AEGP_KeyframeEase {
    ae::sys::AEGP_KeyframeEase { speedF: speed, influenceF: influence }
}

/// Keyframe each track at `times` (layer seconds), easing each segment like NTSCRT.
pub fn write(
    in_data: &InData,
    params: &Parameters<ParamID>,
    plugin_id: aegp::PluginId,
    times: &[f64],
    easings: &[Easing],
    tracks: &[Track],
) -> Result<(), Error> {
    let effect = Effect::new(in_data, plugin_id)?;
    let keys = Keys::new(in_data)?;
    let scale = in_data.time_scale().max(1);
    let one_third = 100.0 / 3.0;
    let key_times: Vec<i32> = times.iter().map(|t| (t * scale as f64).round() as i32).collect();

    for track in tracks {
        let Some(index) = params.index(track.id) else { continue };
        let stream = effect.stream(index as i32)?;
        let values: Vec<f64> = track.values.iter().map(number).collect();
        keys.add(&stream, &key_times, scale, &values)?;
        // Checkboxes and menus hold between keyframes by themselves.
        if keys.count(&stream)? as usize != times.len() || !matches!(track.values.first(), Some(ControlValue::Slider(_))) {
            continue;
        }
        let avg = |a: usize, b: usize| (number(&track.values[b]) - number(&track.values[a])) / (times[b] - times[a]).max(1e-6);
        for k in 0..times.len() {
            let (in_interp, in_ease) = match k.checked_sub(1) {
                Some(p) => match easings[p] {
                    Easing::EaseIn => (Interp::Bezier, ease(2.0 * avg(p, k), one_third)),
                    Easing::EaseOut => (Interp::Bezier, ease(0.0, one_third)),
                    Easing::EaseInOut => (Interp::Bezier, ease(0.0, 50.0)),
                    Easing::Linear | Easing::Hold => (Interp::Linear, ease(avg(p, k), one_third)),
                },
                None => (Interp::Linear, ease(0.0, one_third)),
            };
            let (out_interp, out_ease) = if k + 1 < times.len() {
                match easings[k] {
                    Easing::EaseIn => (Interp::Bezier, ease(0.0, one_third)),
                    Easing::EaseOut => (Interp::Bezier, ease(2.0 * avg(k, k + 1), one_third)),
                    Easing::EaseInOut => (Interp::Bezier, ease(0.0, 50.0)),
                    Easing::Linear => (Interp::Linear, ease(avg(k, k + 1), one_third)),
                    Easing::Hold => (Interp::Hold, ease(0.0, one_third)),
                }
            } else {
                (Interp::Linear, ease(0.0, one_third))
            };
            let idx = k as i32;
            let _ = keys.set_interpolation(&stream, idx, in_interp, out_interp);
            if in_interp == Interp::Bezier || out_interp == Interp::Bezier {
                let _ = keys.set_temporal_ease(&stream, idx, &in_ease, &out_ease);
            }
        }
    }
    Ok(())
}
