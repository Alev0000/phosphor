use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use librashader::runtime::d3d11::{FilterChain, FilterChainOptions};
use librashader::runtime::{FilterChainParameters, Size, Viewport};
use librashader_common::shader_features::ShaderFeatures;
use windows::Win32::Foundation::HMODULE;
use windows::Win32::Graphics::Direct3D::{
    D3D_DRIVER_TYPE, D3D_DRIVER_TYPE_HARDWARE, D3D_DRIVER_TYPE_WARP, D3D_FEATURE_LEVEL_11_0,
};
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT_R8G8B8A8_UNORM, DXGI_SAMPLE_DESC};

use rayon::prelude::*;

use super::CrtPreset;
use crate::howlaround::{CameraSetup, Rgba8};
use crate::Image;

struct Gpu {
    adapter: String,
    device: ID3D11Device,
    context: ID3D11DeviceContext,
    chains: HashMap<&'static str, FilterChain>,
    /// Reused while the sizes stay the same (every frame of a render).
    input: Option<(u32, u32, ID3D11Texture2D, ID3D11ShaderResourceView)>,
    /// One target per output size (the Screen Loop alternates between two).
    outputs: Vec<(u32, u32, ID3D11Texture2D, ID3D11RenderTargetView, ID3D11Texture2D)>,
    /// The Screen Loop camera's compute shaders (made on first use).
    compute: Option<super::compute::Compute>,
}

// D3D11 devices are free-threaded; the immediate context and the chains are only touched under the
// renderer's mutex.
unsafe impl Send for Gpu {}

/// Runs a CRT preset on the chain input, producing the output-size picture.
pub struct CrtRenderer {
    shaders_root: PathBuf,
    gpu: Mutex<Option<Gpu>>,
}

pub type Error = Box<dyn std::error::Error + Send + Sync>;

fn create_device(driver: D3D_DRIVER_TYPE) -> windows::core::Result<(ID3D11Device, ID3D11DeviceContext)> {
    let mut device = None;
    let mut context = None;
    unsafe {
        D3D11CreateDevice(
            None,
            driver,
            HMODULE::default(),
            D3D11_CREATE_DEVICE_BGRA_SUPPORT,
            Some(&[D3D_FEATURE_LEVEL_11_0]),
            D3D11_SDK_VERSION,
            Some(&mut device),
            None,
            Some(&mut context),
        )?;
    }
    Ok((device.unwrap(), context.unwrap()))
}

/// Direct3D 11 textures stop at 16384 pixels per side; say so instead of "invalid parameter".
fn check_size(what: &str, width: usize, height: usize) -> Result<(), Error> {
    const MAX: usize = D3D11_REQ_TEXTURE2D_U_OR_V_DIMENSION as usize;
    if width == 0 || height == 0 || width > MAX || height > MAX {
        return Err(format!("{what} is {width}x{height}; the GPU stages work from 1 to {MAX} pixels per side").into());
    }
    Ok(())
}

/// The graphics card's name, as Windows reports it.
fn adapter_name(device: &ID3D11Device) -> String {
    use windows::core::Interface;
    use windows::Win32::Graphics::Dxgi::IDXGIDevice;
    unsafe {
        device
            .cast::<IDXGIDevice>()
            .and_then(|d| d.GetAdapter())
            .and_then(|a| a.GetDesc())
            .map(|d| {
                let n = d.Description.iter().position(|&c| c == 0).unwrap_or(d.Description.len());
                String::from_utf16_lossy(&d.Description[..n])
            })
            .unwrap_or_else(|_| "unknown".into())
    }
}

fn texture_desc(width: u32, height: u32, bind: D3D11_BIND_FLAG) -> D3D11_TEXTURE2D_DESC {
    D3D11_TEXTURE2D_DESC {
        Width: width,
        Height: height,
        MipLevels: 1,
        ArraySize: 1,
        Format: DXGI_FORMAT_R8G8B8A8_UNORM,
        SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
        Usage: D3D11_USAGE_DEFAULT,
        BindFlags: bind.0 as u32,
        CPUAccessFlags: 0,
        MiscFlags: 0,
    }
}

impl CrtRenderer {
    pub fn new(shaders_root: impl AsRef<Path>) -> Self {
        Self {
            shaders_root: shaders_root.as_ref().to_path_buf(),
            gpu: Mutex::new(None),
        }
    }

    pub fn shaders_root(&self) -> &Path {
        &self.shaders_root
    }

    /// The graphics card in use, once the GPU has been started (`None` before the first GPU frame).
    pub fn adapter(&self) -> Option<String> {
        self.gpu.lock().ok()?.as_ref().map(|g| g.adapter.clone())
    }

    fn gpu(slot: &mut Option<Gpu>) -> Result<&mut Gpu, Error> {
        if slot.is_none() {
            let (device, context, warp) = match create_device(D3D_DRIVER_TYPE_HARDWARE) {
                Ok((d, c)) => (d, c, false),
                Err(_) => {
                    let (d, c) = create_device(D3D_DRIVER_TYPE_WARP)?;
                    (d, c, true)
                }
            };
            let mut adapter = adapter_name(&device);
            if warp {
                adapter.push_str(" (software fallback)");
            }
            *slot = Some(Gpu {
                adapter,
                device,
                context,
                chains: HashMap::new(),
                input: None,
                outputs: Vec::new(),
                compute: None,
            });
        }
        Ok(slot.as_mut().unwrap())
    }

    /// Runs `f` on the GPU, starting it if needed. A device the driver removed (driver update or
    /// crash, sleep, a TDR) never works again, so it is dropped and the next frame starts a new one;
    /// so is everything left behind by a panic.
    fn with_gpu<R>(&self, f: impl FnOnce(&mut Gpu) -> Result<R, Error>) -> Result<R, Error> {
        let mut guard = self.gpu.lock().unwrap_or_else(|poisoned| {
            let mut g = poisoned.into_inner();
            *g = None;
            g
        });
        let result = f(Self::gpu(&mut guard)?);
        if result.is_err() && guard.as_ref().is_some_and(|g| unsafe { g.device.GetDeviceRemovedReason() }.is_err()) {
            *guard = None;
        }
        result
    }

    /// The Screen Loop's camera on the GPU (same result as `howlaround::camera_cpu`, to 8-bit rounding).
    pub fn camera(&self, scene: &Image, tv: &Rgba8, setup: &CameraSetup) -> Result<Image, Error> {
        check_size("Screen Loop camera", scene.width, scene.height)?;
        self.with_gpu(|gpu| unsafe {
            if gpu.compute.is_none() {
                gpu.compute = Some(super::compute::Compute::new(&gpu.device)?);
            }
            gpu.compute.as_mut().unwrap().camera(&gpu.device, &gpu.context, scene, tv, setup)
        })
    }

    /// `values` are (name, value) for the preset's parameters; missing ones keep the shader default.
    pub fn render(
        &self,
        input: &Image,
        out_width: usize,
        out_height: usize,
        preset: &'static CrtPreset,
        values: &[(&str, f32)],
        frame_count: usize,
    ) -> Result<Image, Error> {
        check_size("CRT input", input.width, input.height)?;
        check_size("CRT output", out_width, out_height)?;
        self.with_gpu(|gpu| self.render_on(gpu, input, out_width, out_height, preset, values, frame_count))
    }

    #[allow(clippy::too_many_arguments)]
    fn render_on(
        &self,
        gpu: &mut Gpu,
        input: &Image,
        out_width: usize,
        out_height: usize,
        preset: &'static CrtPreset,
        values: &[(&str, f32)],
        frame_count: usize,
    ) -> Result<Image, Error> {
        if !gpu.chains.contains_key(preset.id) {
            let path = self.shaders_root.join(preset.path);
            let chain = unsafe {
                FilterChain::load_from_path(
                    &path,
                    ShaderFeatures::NONE,
                    &gpu.device,
                    Some(&FilterChainOptions {
                        force_no_mipmaps: false,
                        disable_cache: false,
                    }),
                )
            }
            .map_err(|e| format!("{}: {e}", path.display()))?;
            gpu.chains.insert(preset.id, chain);
        }

        let (iw, ih) = (input.width as u32, input.height as u32);
        let (ow, oh) = (out_width as u32, out_height as u32);
        let device = gpu.device.clone();
        let context = gpu.context.clone();
        {
            let chain = gpu.chains.get_mut(preset.id).unwrap();
            for p in preset.params {
                let v = values.iter().find(|(n, _)| *n == p.name).map_or(p.initial, |(_, v)| *v);
                chain.parameters().set_parameter_value(p.name, v);
            }
        }

        unsafe {
            let rgba: Vec<u8> = input
                .pixels
                .par_iter()
                .flat_map_iter(|p| {
                    let c = |v: f32| (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
                    [c(p[0]), c(p[1]), c(p[2]), 255]
                })
                .collect();
            if !matches!(&gpu.input, Some((w, h, ..)) if (*w, *h) == (iw, ih)) {
                let mut tex = None;
                device.CreateTexture2D(&texture_desc(iw, ih, D3D11_BIND_SHADER_RESOURCE), None, Some(&mut tex))?;
                let tex = tex.unwrap();
                let mut srv = None;
                device.CreateShaderResourceView(&tex, None, Some(&mut srv))?;
                gpu.input = Some((iw, ih, tex, srv.unwrap()));
            }
            if !gpu.outputs.iter().any(|(w, h, ..)| (*w, *h) == (ow, oh)) {
                let mut tex = None;
                device.CreateTexture2D(&texture_desc(ow, oh, D3D11_BIND_RENDER_TARGET | D3D11_BIND_SHADER_RESOURCE), None, Some(&mut tex))?;
                let tex = tex.unwrap();
                let mut rtv = None;
                device.CreateRenderTargetView(&tex, None, Some(&mut rtv))?;
                let mut staging_desc = texture_desc(ow, oh, D3D11_BIND_FLAG(0));
                staging_desc.Usage = D3D11_USAGE_STAGING;
                staging_desc.CPUAccessFlags = D3D11_CPU_ACCESS_READ.0 as u32;
                let mut staging = None;
                device.CreateTexture2D(&staging_desc, None, Some(&mut staging))?;
                if gpu.outputs.len() >= 3 {
                    gpu.outputs.remove(0);
                }
                gpu.outputs.push((ow, oh, tex, rtv.unwrap(), staging.unwrap()));
            }
            let (_, _, in_tex, srv) = gpu.input.clone().unwrap();
            let (_, _, out_tex, rtv, staging) = gpu.outputs.iter().find(|(w, h, ..)| (*w, *h) == (ow, oh)).cloned().unwrap();
            context.UpdateSubresource(&in_tex, 0, None, rgba.as_ptr().cast(), iw * 4, 0);
            context.ClearRenderTargetView(&rtv, &[0.0, 0.0, 0.0, 1.0]);
            let chain = gpu.chains.get_mut(preset.id).unwrap();

            let viewport = Viewport {
                x: 0.0,
                y: 0.0,
                mvp: None,
                output: &rtv,
                size: Size { width: ow, height: oh },
            };
            chain.frame(Some(&context), &srv, &viewport, frame_count, None)?;
            context.CopyResource(&staging, &out_tex);

            let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
            context.Map(&staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped))?;
            let mut out = Image::uninit(out_width, out_height);
            let pitch = mapped.RowPitch as usize;
            let src = std::slice::from_raw_parts(mapped.pData as *const u8, pitch * (out_height - 1) + out_width * 4);
            out.pixels.par_chunks_exact_mut(out_width).enumerate().for_each(|(y, row)| {
                let line = &src[y * pitch..y * pitch + out_width * 4];
                for (o, p) in row.iter_mut().zip(line.chunks_exact(4)) {
                    *o = [p[0] as f32 / 255.0, p[1] as f32 / 255.0, p[2] as f32 / 255.0, 1.0];
                }
            });
            context.Unmap(&staging, 0);
            Ok(out)
        }
    }
}
