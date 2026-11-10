//! Writing a look's timeline into After Effects keyframes (AEGP; UI thread only).
//!
//! NTSCRT eases each segment by the keyframe it leaves, with CSS curves. Ease in (u²) and ease out
//! (1 − (1 − u)²) are quadratic Béziers, which After Effects' temporal ease reproduces exactly with
//! 33⅓% influence and a speed of 0 or twice the segment's average; ease in-out uses 50% influence
//! at speed 0 on both ends (the same midpoint slope).

use after_effects::{self as ae, aegp, Error, InData, Parameters};
use phosphor_core::look::Easing;

use crate::{ControlValue, ParamID};

/// One control's values at each look keyframe.
pub struct Track {
    pub id: ParamID,
    pub values: Vec<ControlValue>,
}

fn effect(in_data: &InData, plugin_id: aegp::PluginId) -> Result<aegp::EffectRefHandle, Error> {
    aegp::suites::PFInterface::new()?.new_effect_for_effect(in_data.effect_ref(), plugin_id)
}

/// Remove every keyframe from `ids` (a newly applied look replaces the old look's animation).
pub fn clear(in_data: &InData, params: &Parameters<ParamID>, plugin_id: aegp::PluginId, ids: &[ParamID]) -> Result<(), Error> {
    let effect = effect(in_data, plugin_id)?;
    let streams = aegp::suites::Stream::new()?;
    let keys = aegp::suites::Keyframe::new()?;
    for id in ids {
        let Some(index) = params.index(*id) else { continue };
        let Ok(stream) = streams.new_effect_stream_by_index(&effect, plugin_id, index as i32) else { continue };
        let n = keys.stream_num_kfs(&stream).unwrap_or(0);
        for k in (0..n).rev() {
            let _ = keys.delete_keyframe(&stream, k);
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
    let effect = effect(in_data, plugin_id)?;
    let streams = aegp::suites::Stream::new()?;
    let keys = aegp::suites::Keyframe::new()?;
    let scale = in_data.time_scale().max(1);
    let one_third = 100.0 / 3.0;

    for track in tracks {
        let Some(index) = params.index(track.id) else { continue };
        let stream = streams.new_effect_stream_by_index(&effect, plugin_id, index as i32)?;
        {
            let mut add = keys.start_add_keyframes(&stream)?;
            for (t, v) in times.iter().zip(&track.values) {
                let time = ae::Time { value: (t * scale as f64).round() as i32, scale };
                let i = add.add_keyframes(aegp::TimeMode::LayerTime, time)?;
                add.set_add_keyframe(i, &stream, aegp::StreamValue::OneD(number(v)))?;
            }
        }
        // Checkboxes and menus hold between keyframes by themselves.
        if keys.stream_num_kfs(&stream)? as usize != times.len() || !matches!(track.values.first(), Some(ControlValue::Slider(_))) {
            continue;
        }
        let avg = |a: usize, b: usize| (number(&track.values[b]) - number(&track.values[a])) / (times[b] - times[a]).max(1e-6);
        for k in 0..times.len() {
            let (in_interp, in_ease) = match k.checked_sub(1) {
                Some(p) => match easings[p] {
                    Easing::EaseIn => (aegp::KeyframeInterpolation::Bezier, ease(2.0 * avg(p, k), one_third)),
                    Easing::EaseOut => (aegp::KeyframeInterpolation::Bezier, ease(0.0, one_third)),
                    Easing::EaseInOut => (aegp::KeyframeInterpolation::Bezier, ease(0.0, 50.0)),
                    Easing::Linear | Easing::Hold => (aegp::KeyframeInterpolation::Linear, ease(avg(p, k), one_third)),
                },
                None => (aegp::KeyframeInterpolation::Linear, ease(0.0, one_third)),
            };
            let (out_interp, out_ease) = if k + 1 < times.len() {
                match easings[k] {
                    Easing::EaseIn => (aegp::KeyframeInterpolation::Bezier, ease(0.0, one_third)),
                    Easing::EaseOut => (aegp::KeyframeInterpolation::Bezier, ease(2.0 * avg(k, k + 1), one_third)),
                    Easing::EaseInOut => (aegp::KeyframeInterpolation::Bezier, ease(0.0, 50.0)),
                    Easing::Linear => (aegp::KeyframeInterpolation::Linear, ease(avg(k, k + 1), one_third)),
                    Easing::Hold => (aegp::KeyframeInterpolation::Hold, ease(0.0, one_third)),
                }
            } else {
                (aegp::KeyframeInterpolation::Linear, ease(0.0, one_third))
            };
            let idx = k as i32;
            let _ = keys.set_keyframe_interpolation(&stream, idx, in_interp, out_interp);
            if in_interp == aegp::KeyframeInterpolation::Bezier || out_interp == aegp::KeyframeInterpolation::Bezier {
                let _ = keys.set_keyframe_temporal_ease(&stream, idx, 0, &in_ease, &out_ease);
            }
        }
    }
    Ok(())
}
