//! The few AEGP calls Phosphor makes (keyframing a look, hiding controls), with fallbacks to older
//! suite versions so they work back to After Effects 2020.
//!
//! The after-effects crate asks for AEGP_StreamSuite6 and AEGP_KeyframeSuite5, which only exist since
//! After Effects 22.5. Their older versions are the same tables minus a function or two at the end
//! that Phosphor doesn't call, so the newest version available is used through the older layout.

use std::ffi::c_void;
use std::marker::PhantomData;

use after_effects::{self as ae, Error, InData};
use ae::sys;

/// A host suite, released on drop.
struct Suite<T> {
    pica: *mut sys::SPBasicSuite,
    name: &'static [u8],
    version: i32,
    table: *const T,
    _t: PhantomData<T>,
}

impl<T> Suite<T> {
    /// The first of `versions` (newest first) the host has.
    fn acquire(in_data: &InData, name: &'static [u8], versions: &[u32]) -> Result<Self, Error> {
        let pica = in_data.pica_basic_suite_ptr();
        if pica.is_null() {
            return Err(Error::MissingSuite);
        }
        let acquire = unsafe { (*pica).AcquireSuite }.ok_or(Error::MissingSuite)?;
        for &version in versions {
            let mut table: *const c_void = std::ptr::null();
            let err = unsafe { acquire(name.as_ptr().cast(), version as i32, &mut table) };
            if err == 0 && !table.is_null() {
                return Ok(Self { pica, name, version: version as i32, table: table.cast(), _t: PhantomData });
            }
        }
        Err(Error::MissingSuite)
    }

    fn get(&self) -> &T {
        unsafe { &*self.table }
    }
}

impl<T> Drop for Suite<T> {
    fn drop(&mut self) {
        if let Some(release) = unsafe { (*self.pica).ReleaseSuite } {
            unsafe { release(self.name.as_ptr().cast(), self.version) };
        }
    }
}

fn check(err: sys::A_Err) -> Result<(), Error> {
    if err == 0 { Ok(()) } else { Err(Error::Generic) }
}

macro_rules! call {
    ($suite:expr, $f:ident $(, $arg:expr)*) => {
        check(unsafe { ($suite.get().$f.ok_or(Error::MissingSuite)?)($($arg),*) })
    };
}

/// This effect instance as AEGP sees it, with its parameter streams.
pub struct Effect {
    plugin_id: sys::AEGP_PluginID,
    effects: Suite<sys::AEGP_EffectSuite2>,
    streams: Suite<sys::AEGP_StreamSuite5>,
    effect: sys::AEGP_EffectRefH,
}

impl Effect {
    pub fn new(in_data: &InData, plugin_id: sys::AEGP_PluginID) -> Result<Self, Error> {
        let pf = Suite::<sys::AEGP_PFInterfaceSuite1>::acquire(in_data, sys::kAEGPPFInterfaceSuite, &[sys::kAEGPPFInterfaceSuiteVersion1])?;
        let streams = Suite::<sys::AEGP_StreamSuite5>::acquire(
            in_data,
            sys::kAEGPStreamSuite,
            &[sys::kAEGPStreamSuiteVersion6, sys::kAEGPStreamSuiteVersion5, sys::kAEGPStreamSuiteVersion4],
        )?;
        // Only to dispose of the effect ref; AEGP_DisposeEffect sits at the same place in every version.
        let effects = Suite::<sys::AEGP_EffectSuite2>::acquire(
            in_data,
            sys::kAEGPEffectSuite,
            &[sys::kAEGPEffectSuiteVersion5, sys::kAEGPEffectSuiteVersion4, sys::kAEGPEffectSuiteVersion3, sys::kAEGPEffectSuiteVersion2],
        )?;
        let mut effect: sys::AEGP_EffectRefH = std::ptr::null_mut();
        let effect_ref = unsafe { (*in_data.as_ptr()).effect_ref };
        call!(pf, AEGP_GetNewEffectForEffect, plugin_id, effect_ref, &mut effect)?;
        Ok(Self { plugin_id, effects, streams, effect })
    }

    /// The stream of the parameter at `index` (After Effects' parameter index).
    pub fn stream(&self, index: i32) -> Result<Stream<'_>, Error> {
        let mut handle: sys::AEGP_StreamRefH = std::ptr::null_mut();
        call!(self.streams, AEGP_GetNewEffectStreamByIndex, self.plugin_id, self.effect, index, &mut handle)?;
        Ok(Stream { effect: self, handle })
    }
}

impl Drop for Effect {
    fn drop(&mut self) {
        if let Some(dispose) = self.effects.get().AEGP_DisposeEffect {
            unsafe { dispose(self.effect) };
        }
    }
}

pub struct Stream<'a> {
    effect: &'a Effect,
    handle: sys::AEGP_StreamRefH,
}

impl Drop for Stream<'_> {
    fn drop(&mut self) {
        if let Some(dispose) = self.effect.streams.get().AEGP_DisposeStream {
            unsafe { dispose(self.handle) };
        }
    }
}

/// Show or hide controls in the Effect Controls panel.
pub struct Dynamic(Suite<sys::AEGP_DynamicStreamSuite4>);

impl Dynamic {
    pub fn new(in_data: &InData) -> Result<Self, Error> {
        Ok(Self(Suite::acquire(in_data, sys::kAEGPDynamicStreamSuite, &[sys::kAEGPDynamicStreamSuiteVersion4])?))
    }

    pub fn set_hidden(&self, stream: &Stream, hidden: bool) -> Result<(), Error> {
        call!(self.0, AEGP_SetDynamicStreamFlag, stream.handle, sys::AEGP_DynStreamFlag_HIDDEN as _, 0, hidden as u8)
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Interp {
    Linear,
    Bezier,
    Hold,
}

impl Interp {
    fn raw(self) -> sys::AEGP_KeyframeInterpolationType {
        (match self {
            Interp::Linear => sys::AEGP_KeyInterp_LINEAR,
            Interp::Bezier => sys::AEGP_KeyInterp_BEZIER,
            Interp::Hold => sys::AEGP_KeyInterp_HOLD,
        }) as _
    }
}

pub struct Keys(Suite<sys::AEGP_KeyframeSuite4>);

impl Keys {
    pub fn new(in_data: &InData) -> Result<Self, Error> {
        Ok(Self(Suite::acquire(
            in_data,
            sys::kAEGPKeyframeSuite,
            &[sys::kAEGPKeyframeSuiteVersion5, sys::kAEGPKeyframeSuiteVersion4, sys::kAEGPKeyframeSuiteVersion3],
        )?))
    }

    pub fn count(&self, stream: &Stream) -> Result<i32, Error> {
        let mut n: sys::A_long = 0;
        call!(self.0, AEGP_GetStreamNumKFs, stream.handle, &mut n)?;
        Ok(n)
    }

    pub fn delete(&self, stream: &Stream, index: i32) -> Result<(), Error> {
        call!(self.0, AEGP_DeleteKeyframe, stream.handle, index)
    }

    /// Adds one-dimensional keyframes at `times` (layer time, in `scale` units) as one undo step.
    pub fn add(&self, stream: &Stream, times: &[i32], scale: u32, values: &[f64]) -> Result<(), Error> {
        let mut info: sys::AEGP_AddKeyframesInfoH = std::ptr::null_mut();
        call!(self.0, AEGP_StartAddKeyframes, stream.handle, &mut info)?;
        let mut result = Ok(());
        for (&t, &v) in times.iter().zip(values) {
            let time = sys::A_Time { value: t, scale };
            let mut index: sys::A_long = 0;
            result = call!(self.0, AEGP_AddKeyframes, info, sys::AEGP_LTimeMode_LayerTime as _, &time, &mut index);
            if result.is_err() {
                break;
            }
            let value = sys::AEGP_StreamValue2 { streamH: stream.handle, val: sys::AEGP_StreamVal2 { one_d: v } };
            result = call!(self.0, AEGP_SetAddKeyframe, info, index, &value);
            if result.is_err() {
                break;
            }
        }
        // Always end the batch, so the stream isn't left mid-edit.
        let end = call!(self.0, AEGP_EndAddKeyframes, result.is_ok() as u8, info);
        result.and(end)
    }

    pub fn set_interpolation(&self, stream: &Stream, index: i32, in_interp: Interp, out_interp: Interp) -> Result<(), Error> {
        call!(self.0, AEGP_SetKeyframeInterpolation, stream.handle, index, in_interp.raw(), out_interp.raw())
    }

    pub fn set_temporal_ease(&self, stream: &Stream, index: i32, in_ease: &sys::AEGP_KeyframeEase, out_ease: &sys::AEGP_KeyframeEase) -> Result<(), Error> {
        call!(self.0, AEGP_SetKeyframeTemporalEase, stream.handle, index, 0, in_ease, out_ease)
    }
}
