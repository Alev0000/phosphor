//! ntsc-rs settings ↔ After Effects params, adapted from ntsc-rs's own AE plugin (v0.9.4).

use after_effects::{self as ae, Error, ParamFlag, ParamUIFlags, Parameters, ValueDisplayFlag};
use ntsc_rs::settings::{
    standard::NtscEffectFullSettings, EnumValue, SettingDescriptor, SettingField, SettingID, SettingKind, Settings,
};

use crate::{ControlValue, ParamID};

pub trait IDExt {
    fn ae_id(&self) -> i32;
}

impl<T: Settings> IDExt for SettingID<T> {
    fn ae_id(&self) -> i32 {
        self.id as i32 + 1
    }
}

const LOG_SLIDER_BASE: f64 = 100.0;

/// Maps a slider value to a roughly logarithmic curve, keeping the min and max values where they are.
fn map_logarithmic(value: f64, min: f64, max: f64, base: f64) -> f64 {
    (max - min) * ((f64::powf(base, (value - min) / (max - min)) - 1.0) / (base - 1.0)) + min
}

/// Map it back. This is used for converting default slider values.
fn map_logarithmic_inverse(value: f64, min: f64, max: f64, base: f64) -> f64 {
    f64::log(((value - min) / (max - min)) * (base - 1.0) + 1.0, base) * (max - min) + min
}


pub struct NtscParams;

impl NtscParams {
    pub fn update_controls_disabled(
        params: &mut Parameters<ParamID>,
        descriptors: &[SettingDescriptor<NtscEffectFullSettings>],
        enabled: bool,
    ) -> Result<(), Error> {
        for descriptor in descriptors {
            if let SettingKind::Group { children, .. } = &descriptor.kind {
                let group_enabled = params
                    .get(ParamID::Ntsc(descriptor.id.ae_id()))?
                    .as_checkbox()?
                    .value();
                Self::update_controls_disabled(params, children, enabled && group_enabled)?;
            }
            if let Ok(p) = params.get(ParamID::Ntsc(descriptor.id.ae_id())) {
                let was_enabled = !p.ui_flags().contains(ParamUIFlags::DISABLED);

                if was_enabled != enabled {
                    // The parameter definition must be cloned--modifying it in-place will result in a crash
                    let mut p = p.clone();
                    p.set_ui_flag(ParamUIFlags::DISABLED, !enabled);
                    p.update_param_ui()?;
                }
            }
        }

        Ok(())
    }

    pub fn map_params(
        params: &mut Parameters<ParamID>,
        descriptors: &[SettingDescriptor<NtscEffectFullSettings>],
        default_settings: &NtscEffectFullSettings,
        legacy_default_settings: &NtscEffectFullSettings,
    ) -> Result<(), Error> {
        fn get_defaults<T: SettingField + 'static>(
            defaults: &NtscEffectFullSettings,
            legacy_defaults: &NtscEffectFullSettings,
            descriptor: &SettingDescriptor<NtscEffectFullSettings>,
        ) -> Result<[T; 2], Error> {
            Ok([
                defaults
                    .get_field(&descriptor.id)
                    .map_err(|_| Error::BadCallbackParameter)?,
                legacy_defaults
                    .get_field(&descriptor.id)
                    .map_err(|_| Error::BadCallbackParameter)?,
            ])
        }

        for descriptor in descriptors {
            match &descriptor.kind {
                SettingKind::Enumeration { options } => {
                    let [default_idx, legacy_default_idx] = get_defaults::<EnumValue>(
                        default_settings,
                        legacy_default_settings,
                        descriptor,
                    )?
                    .map(|default| {
                        options
                            .iter()
                            .position(|item| item.index == default.0)
                            .unwrap() as i32
                            + 1
                    });
                    params.add_customized(
                        ParamID::Ntsc(descriptor.id.ae_id()),
                        descriptor.label,
                        ae::PopupDef::setup(|p| {
                            p.set_options(&options.iter().map(|o| o.label).collect::<Vec<_>>());
                            p.set_default(default_idx);
                            p.set_value(legacy_default_idx);
                        }),
                        |p| {
                            p.set_flag(ParamFlag::START_COLLAPSED, true);
                            p.set_flag(ParamFlag::USE_VALUE_FOR_OLD_PROJECTS, true);
                            -1
                        },
                    )?;
                }
                SettingKind::Percentage { logarithmic } => {
                    let [default_value, legacy_default_value] = get_defaults::<f32>(
                        default_settings,
                        legacy_default_settings,
                        descriptor,
                    )?
                    .map(|default| match (*logarithmic, default as f64) {
                        (true, v) => {
                            map_logarithmic_inverse(v, 0.0, 1.0, LOG_SLIDER_BASE)
                        }
                        (false, v) => v,
                    } * 100.0);
                    params.add_customized(
                        ParamID::Ntsc(descriptor.id.ae_id()),
                        descriptor.label,
                        ae::FloatSliderDef::setup(|f| {
                            f.set_slider_min(0.0);
                            f.set_valid_min(0.0);
                            f.set_slider_max(100.0);
                            f.set_valid_max(100.0);
                            f.set_default(default_value);
                            f.set_value(legacy_default_value);
                            f.set_display_flags(ValueDisplayFlag::PERCENT);
                            f.set_precision(1);
                        }),
                        |p| {
                            p.set_flag(ParamFlag::START_COLLAPSED, true);
                            p.set_flag(ParamFlag::USE_VALUE_FOR_OLD_PROJECTS, true);
                            -1
                        },
                    )?
                }
                SettingKind::IntRange { range } => {
                    let [default_value, legacy_default_value] =
                        get_defaults::<i32>(default_settings, legacy_default_settings, descriptor)?;
                    params.add_customized(
                        ParamID::Ntsc(descriptor.id.ae_id()),
                        descriptor.label,
                        ae::FloatSliderDef::setup(|f| {
                            f.set_slider_min(*range.start() as f32);
                            f.set_valid_min(*range.start() as f32);
                            f.set_slider_max(*range.end() as f32);
                            f.set_valid_max(*range.end() as f32);
                            f.set_default(default_value as f64);
                            f.set_value(legacy_default_value as f64);
                            f.set_precision(0);
                        }),
                        |p| {
                            p.set_flag(ParamFlag::START_COLLAPSED, true);
                            p.set_flag(ParamFlag::USE_VALUE_FOR_OLD_PROJECTS, true);
                            -1
                        },
                    )?
                }
                SettingKind::FloatRange { range, logarithmic } => {
                    let [default_value, legacy_default_value] =
                        get_defaults::<f32>(default_settings, legacy_default_settings, descriptor)?
                            .map(|default| match (*logarithmic, default as f64) {
                                (true, v) => map_logarithmic_inverse(
                                    v,
                                    *range.start() as f64,
                                    *range.end() as f64,
                                    LOG_SLIDER_BASE,
                                ),
                                (false, v) => v,
                            });
                    params.add_customized(
                        ParamID::Ntsc(descriptor.id.ae_id()),
                        descriptor.label,
                        ae::FloatSliderDef::setup(|f| {
                            f.set_slider_min(*range.start());
                            f.set_valid_min(*range.start());
                            f.set_slider_max(*range.end());
                            f.set_valid_max(*range.end());
                            f.set_default(default_value);
                            f.set_value(legacy_default_value);
                            f.set_precision(2);
                        }),
                        |p| {
                            p.set_flag(ParamFlag::START_COLLAPSED, true);
                            p.set_flag(ParamFlag::USE_VALUE_FOR_OLD_PROJECTS, true);
                            -1
                        },
                    )?
                }
                SettingKind::Boolean => {
                    let [default_value, legacy_default_value] = get_defaults::<bool>(
                        default_settings,
                        legacy_default_settings,
                        descriptor,
                    )?;
                    params.add_customized(
                        ParamID::Ntsc(descriptor.id.ae_id()),
                        descriptor.label,
                        ae::CheckBoxDef::setup(|c| {
                            c.set_default(default_value);
                            c.set_value(legacy_default_value);
                            // The effect will fail to load if we don't set the label (by default it's the null pointer)
                            c.set_label(descriptor.label);
                        }),
                        |p| {
                            p.set_flag(ParamFlag::START_COLLAPSED, true);
                            p.set_flag(ParamFlag::USE_VALUE_FOR_OLD_PROJECTS, true);
                            -1
                        },
                    )?;
                }
                SettingKind::Group { children } => {
                    let descriptor_id = descriptor.id.ae_id();
                    let [default_value, legacy_default_value] = get_defaults::<bool>(
                        default_settings,
                        legacy_default_settings,
                        descriptor,
                    )?;
                    params.add_group(
                        ParamID::NtscGroupStart(descriptor_id),
                        ParamID::NtscGroupEnd(descriptor_id),
                        descriptor.label,
                        false,
                        |g| {
                            g.add_customized(
                                ParamID::Ntsc(descriptor_id),
                                descriptor.label,
                                ae::CheckBoxDef::setup(|c| {
                                    c.set_default(default_value);
                                    c.set_value(legacy_default_value);
                                    c.set_label("Enabled");
                                }),
                                |p| {
                                    p.set_flag(ParamFlag::START_COLLAPSED, true);
                                    p.set_flag(ParamFlag::USE_VALUE_FOR_OLD_PROJECTS, true);
                                    -1
                                },
                            )?;
                            Self::map_params(
                                g,
                                children,
                                default_settings,
                                legacy_default_settings,
                            )?;
                            Ok(())
                        },
                    )?;
                }
            }
        }

        Ok(())
    }

    /// Each control's value (in the units its stream stores) for `settings`.
    pub fn stream_values(
        descriptors: &[SettingDescriptor<NtscEffectFullSettings>],
        settings: &NtscEffectFullSettings,
        out: &mut Vec<(ParamID, ControlValue)>,
    ) {
        for descriptor in descriptors {
            let id = ParamID::Ntsc(descriptor.id.ae_id());
            match &descriptor.kind {
                SettingKind::Enumeration { options, .. } => {
                    let setting = settings.get_field::<EnumValue>(&descriptor.id).unwrap().0;
                    let pos = options.iter().position(|item| item.index == setting).unwrap_or(0);
                    out.push((id, ControlValue::Popup(pos as i32 + 1)));
                }
                SettingKind::Percentage { logarithmic, .. } => {
                    let setting: f64 = settings.get_field::<f32>(&descriptor.id).unwrap().into();
                    let v = if *logarithmic { map_logarithmic_inverse(setting, 0.0, 1.0, LOG_SLIDER_BASE) } else { setting };
                    out.push((id, ControlValue::Slider(v * 100.0)));
                }
                SettingKind::IntRange { .. } => {
                    let setting = settings.get_field::<i32>(&descriptor.id).unwrap();
                    out.push((id, ControlValue::Slider(setting as f64)));
                }
                SettingKind::FloatRange { range, logarithmic, .. } => {
                    let setting: f64 = settings.get_field::<f32>(&descriptor.id).unwrap().into();
                    let v = if *logarithmic {
                        map_logarithmic_inverse(setting, *range.start() as f64, *range.end() as f64, LOG_SLIDER_BASE)
                    } else {
                        setting
                    };
                    out.push((id, ControlValue::Slider(v)));
                }
                SettingKind::Boolean => {
                    out.push((id, ControlValue::Checkbox(settings.get_field::<bool>(&descriptor.id).unwrap())));
                }
                SettingKind::Group { children, .. } => {
                    out.push((id, ControlValue::Checkbox(settings.get_field::<bool>(&descriptor.id).unwrap())));
                    Self::stream_values(children, settings, out);
                }
            }
        }
    }

    /// The ntsc-rs settings the controls hold at `time` (None: the current time).
    pub fn apply_settings(
        descriptors: &[SettingDescriptor<NtscEffectFullSettings>],
        params: &Parameters<ParamID>,
        time: Option<i32>,
    ) -> Result<NtscEffectFullSettings, Error> {
        let mut settings = NtscEffectFullSettings::default();

        fn apply_settings_list(
            descriptors: &[SettingDescriptor<NtscEffectFullSettings>],
            params: &Parameters<ParamID>,
            time: Option<i32>,
            settings: &mut NtscEffectFullSettings,
        ) -> Result<(), Error> {
            for descriptor in descriptors {
                let id = ParamID::Ntsc(descriptor.id.ae_id());
                let get = || params.get_at(id, time, None, None);
                match &descriptor.kind {
                    SettingKind::Enumeration { options, .. } => {
                        let selected_item_position = get()?.as_popup()?.value() - 1;
                        if selected_item_position < 0 {
                            continue;
                        }
                        let menu_enum_value = options[selected_item_position as usize].index;
                        settings
                            .set_field::<EnumValue>(&descriptor.id, EnumValue(menu_enum_value))
                            .map_err(|_| Error::BadCallbackParameter)?;
                    }
                    SettingKind::Percentage { logarithmic, .. } => {
                        let mut slider_value = get()?.as_float_slider()?.value() * 0.01;
                        if *logarithmic {
                            slider_value = map_logarithmic(slider_value, 0.0, 1.0, LOG_SLIDER_BASE);
                        }
                        settings
                            .set_field::<f32>(&descriptor.id, slider_value as f32)
                            .map_err(|_| Error::BadCallbackParameter)?;
                    }
                    SettingKind::IntRange { .. } => {
                        let slider_value = get()?.as_float_slider()?.value().round() as i32;
                        settings
                            .set_field::<i32>(&descriptor.id, slider_value)
                            .map_err(|_| Error::BadCallbackParameter)?;
                    }
                    SettingKind::FloatRange { logarithmic, range, .. } => {
                        let mut slider_value = get()?.as_float_slider()?.value();
                        if *logarithmic {
                            slider_value =
                                map_logarithmic(slider_value, *range.start() as f64, *range.end() as f64, LOG_SLIDER_BASE);
                        }
                        settings
                            .set_field::<f32>(&descriptor.id, slider_value as f32)
                            .map_err(|_| Error::BadCallbackParameter)?;
                    }
                    SettingKind::Boolean => {
                        settings
                            .set_field::<bool>(&descriptor.id, get()?.as_checkbox()?.value())
                            .map_err(|_| Error::BadCallbackParameter)?;
                    }
                    SettingKind::Group { children, .. } => {
                        settings
                            .set_field::<bool>(&descriptor.id, get()?.as_checkbox()?.value())
                            .map_err(|_| Error::BadCallbackParameter)?;
                        apply_settings_list(children, params, time, settings)?;
                    }
                }
            }
            Ok(())
        }

        apply_settings_list(descriptors, params, time, &mut settings)?;
        Ok(settings)
    }
}
