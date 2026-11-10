//! The Screen Loop's camera as Direct3D 11 compute shaders (`camera.hlsl`), on the CRT's device.

use rayon::prelude::*;
use windows::core::{s, PCSTR};
use windows::Win32::Graphics::Direct3D::Fxc::D3DCompile;
use windows::Win32::Graphics::Direct3D::{ID3DBlob, ID3DInclude};
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::Dxgi::Common::*;

use super::d3d11::Error;
use crate::howlaround::{CameraSetup, CameraUniforms, Rgba8};
use crate::Image;

const SOURCE: &str = include_str!("camera.hlsl");

struct Tex {
    w: u32,
    h: u32,
    tex: ID3D11Texture2D,
    srv: Option<ID3D11ShaderResourceView>,
    uav: Option<ID3D11UnorderedAccessView>,
}

pub(super) struct Compute {
    camera_cs: ID3D11ComputeShader,
    blur_cs: ID3D11ComputeShader,
    camera_cb: ID3D11Buffer,
    blur_cb: ID3D11Buffer,
    sampler: ID3D11SamplerState,
    scene: Option<Tex>,
    tv: Option<Tex>,
    blur_a: Option<Tex>,
    blur_b: Option<Tex>,
    out: Option<Tex>,
    staging: Option<Tex>,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct BlurUniforms {
    dir: [i32; 2],
    sigma: f32,
    radius: i32,
}

unsafe fn compile(device: &ID3D11Device, entry: PCSTR) -> Result<ID3D11ComputeShader, Error> {
    let mut code: Option<ID3DBlob> = None;
    let mut errors: Option<ID3DBlob> = None;
    let r = D3DCompile(
        SOURCE.as_ptr().cast(),
        SOURCE.len(),
        s!("camera.hlsl"),
        None,
        None::<&ID3DInclude>,
        entry,
        s!("cs_5_0"),
        0,
        0,
        &mut code,
        Some(&mut errors),
    );
    if let Err(e) = r {
        let msg = errors
            .map(|b| String::from_utf8_lossy(std::slice::from_raw_parts(b.GetBufferPointer() as *const u8, b.GetBufferSize())).into_owned())
            .unwrap_or_default();
        return Err(format!("compiling camera.hlsl: {e} {msg}").into());
    }
    let code = code.unwrap();
    let bytes = std::slice::from_raw_parts(code.GetBufferPointer() as *const u8, code.GetBufferSize());
    let mut cs = None;
    device.CreateComputeShader(bytes, None, Some(&mut cs))?;
    Ok(cs.unwrap())
}

unsafe fn constant_buffer(device: &ID3D11Device, size: usize) -> Result<ID3D11Buffer, Error> {
    let desc = D3D11_BUFFER_DESC {
        ByteWidth: size.div_ceil(16) as u32 * 16,
        Usage: D3D11_USAGE_DEFAULT,
        BindFlags: D3D11_BIND_CONSTANT_BUFFER.0 as u32,
        ..Default::default()
    };
    let mut b = None;
    device.CreateBuffer(&desc, None, Some(&mut b))?;
    Ok(b.unwrap())
}

/// A texture of `format`, recreated only when the size changes.
unsafe fn texture(
    device: &ID3D11Device,
    slot: &mut Option<Tex>,
    w: u32,
    h: u32,
    format: DXGI_FORMAT,
    srv: bool,
    uav: bool,
    staging: bool,
) -> Result<(), Error> {
    if matches!(slot, Some(t) if (t.w, t.h) == (w, h)) {
        return Ok(());
    }
    let mut bind = 0u32;
    if srv {
        bind |= D3D11_BIND_SHADER_RESOURCE.0 as u32;
    }
    if uav {
        bind |= D3D11_BIND_UNORDERED_ACCESS.0 as u32;
    }
    let desc = D3D11_TEXTURE2D_DESC {
        Width: w,
        Height: h,
        MipLevels: 1,
        ArraySize: 1,
        Format: format,
        SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
        Usage: if staging { D3D11_USAGE_STAGING } else { D3D11_USAGE_DEFAULT },
        BindFlags: bind,
        CPUAccessFlags: if staging { D3D11_CPU_ACCESS_READ.0 as u32 } else { 0 },
        MiscFlags: 0,
    };
    let mut tex = None;
    device.CreateTexture2D(&desc, None, Some(&mut tex))?;
    let tex = tex.unwrap();
    let mut s = None;
    if srv {
        device.CreateShaderResourceView(&tex, None, Some(&mut s))?;
    }
    let mut u = None;
    if uav {
        device.CreateUnorderedAccessView(&tex, None, Some(&mut u))?;
    }
    *slot = Some(Tex { w, h, tex, srv: s, uav: u });
    Ok(())
}

impl Compute {
    pub(super) unsafe fn new(device: &ID3D11Device) -> Result<Self, Error> {
        let sampler_desc = D3D11_SAMPLER_DESC {
            Filter: D3D11_FILTER_MIN_MAG_MIP_LINEAR,
            AddressU: D3D11_TEXTURE_ADDRESS_CLAMP,
            AddressV: D3D11_TEXTURE_ADDRESS_CLAMP,
            AddressW: D3D11_TEXTURE_ADDRESS_CLAMP,
            MaxLOD: f32::MAX,
            ..Default::default()
        };
        let mut sampler = None;
        device.CreateSamplerState(&sampler_desc, Some(&mut sampler))?;
        Ok(Self {
            camera_cs: compile(device, s!("camera"))?,
            blur_cs: compile(device, s!("blur"))?,
            camera_cb: constant_buffer(device, std::mem::size_of::<CameraUniforms>())?,
            blur_cb: constant_buffer(device, std::mem::size_of::<BlurUniforms>())?,
            sampler: sampler.unwrap(),
            scene: None,
            tv: None,
            blur_a: None,
            blur_b: None,
            out: None,
            staging: None,
        })
    }

    pub(super) unsafe fn camera(
        &mut self,
        device: &ID3D11Device,
        ctx: &ID3D11DeviceContext,
        scene: &Image,
        tv: &Rgba8,
        setup: &CameraSetup,
    ) -> Result<Image, Error> {
        let (w, h) = (scene.width as u32, scene.height as u32);
        let (tw, th) = (tv.width as u32, tv.height as u32);
        texture(device, &mut self.scene, w, h, DXGI_FORMAT_R8G8B8A8_UNORM, true, false, false)?;
        texture(device, &mut self.tv, tw, th, DXGI_FORMAT_R8G8B8A8_UNORM, true, false, false)?;
        texture(device, &mut self.out, w, h, DXGI_FORMAT_R8G8B8A8_UNORM, false, true, false)?;
        texture(device, &mut self.staging, w, h, DXGI_FORMAT_R8G8B8A8_UNORM, false, false, true)?;

        let rgba: Vec<u8> = scene
            .pixels
            .par_iter()
            .flat_map_iter(|p| {
                let c = |v: f32| (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
                [c(p[0]), c(p[1]), c(p[2]), 255]
            })
            .collect();
        ctx.UpdateSubresource(&self.scene.as_ref().unwrap().tex, 0, None, rgba.as_ptr().cast(), w * 4, 0);
        ctx.UpdateSubresource(&self.tv.as_ref().unwrap().tex, 0, None, tv.px.as_ptr().cast(), tw * 4, 0);

        // The camera's focus: two blur passes over the TV picture.
        let mut tv_srv = self.tv.as_ref().unwrap().srv.clone();
        if setup.sigma > 0.0 {
            texture(device, &mut self.blur_a, tw, th, DXGI_FORMAT_R16G16B16A16_FLOAT, true, true, false)?;
            texture(device, &mut self.blur_b, tw, th, DXGI_FORMAT_R16G16B16A16_FLOAT, true, true, false)?;
            let radius = ((2.5 * setup.sigma).ceil() as i32).min(64);
            ctx.CSSetShader(&self.blur_cs, None);
            for (src, dst, dir) in [
                (tv_srv.clone(), self.blur_a.as_ref().unwrap().uav.clone(), [1, 0]),
                (self.blur_a.as_ref().unwrap().srv.clone(), self.blur_b.as_ref().unwrap().uav.clone(), [0, 1]),
            ] {
                let u = BlurUniforms { dir, sigma: setup.sigma, radius };
                ctx.UpdateSubresource(&self.blur_cb, 0, None, (&u as *const BlurUniforms).cast(), 0, 0);
                ctx.CSSetConstantBuffers(1, Some(&[Some(self.blur_cb.clone())]));
                ctx.CSSetShaderResources(3, Some(&[src]));
                ctx.CSSetUnorderedAccessViews(1, 1, Some(&dst), None);
                ctx.Dispatch(tw.div_ceil(16), th.div_ceil(16), 1);
                ctx.CSSetShaderResources(3, Some(&[None]));
                ctx.CSSetUnorderedAccessViews(1, 1, Some(&None), None);
            }
            tv_srv = self.blur_b.as_ref().unwrap().srv.clone();
        }

        // The counter mask (tiny; made fresh when shown).
        let mut counter_srv: Option<ID3D11ShaderResourceView> = None;
        let mut _counter_tex = None;
        if let Some((cw, ch, fill, edge)) = &setup.counter {
            let data: Vec<f32> = fill.iter().zip(edge).flat_map(|(f, e)| [*f, *e]).collect();
            let desc = D3D11_TEXTURE2D_DESC {
                Width: *cw as u32,
                Height: *ch as u32,
                MipLevels: 1,
                ArraySize: 1,
                Format: DXGI_FORMAT_R32G32_FLOAT,
                SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
                Usage: D3D11_USAGE_DEFAULT,
                BindFlags: D3D11_BIND_SHADER_RESOURCE.0 as u32,
                CPUAccessFlags: 0,
                MiscFlags: 0,
            };
            let init = D3D11_SUBRESOURCE_DATA { pSysMem: data.as_ptr().cast(), SysMemPitch: *cw as u32 * 8, SysMemSlicePitch: 0 };
            let mut tex = None;
            device.CreateTexture2D(&desc, Some(&init), Some(&mut tex))?;
            let tex = tex.unwrap();
            device.CreateShaderResourceView(&tex, None, Some(&mut counter_srv))?;
            _counter_tex = Some(tex);
        }

        ctx.UpdateSubresource(&self.camera_cb, 0, None, (&setup.uniforms as *const CameraUniforms).cast(), 0, 0);
        ctx.CSSetShader(&self.camera_cs, None);
        ctx.CSSetConstantBuffers(0, Some(&[Some(self.camera_cb.clone())]));
        ctx.CSSetSamplers(0, Some(&[Some(self.sampler.clone())]));
        ctx.CSSetShaderResources(0, Some(&[self.scene.as_ref().unwrap().srv.clone(), tv_srv, counter_srv]));
        ctx.CSSetUnorderedAccessViews(0, 1, Some(&self.out.as_ref().unwrap().uav.clone()), None);
        ctx.Dispatch(w.div_ceil(16), h.div_ceil(16), 1);
        ctx.CSSetShaderResources(0, Some(&[None, None, None]));
        ctx.CSSetUnorderedAccessViews(0, 1, Some(&None), None);
        ctx.CSSetShader(None, None);

        let staging = &self.staging.as_ref().unwrap().tex;
        ctx.CopyResource(staging, &self.out.as_ref().unwrap().tex);
        let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
        ctx.Map(staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped))?;
        let pitch = mapped.RowPitch as usize;
        let (wu, hu) = (w as usize, h as usize);
        let src = std::slice::from_raw_parts(mapped.pData as *const u8, pitch * (hu - 1) + wu * 4);
        let mut out = Image::uninit(wu, hu);
        out.pixels.par_chunks_exact_mut(wu).enumerate().for_each(|(y, row)| {
            for (o, p) in row.iter_mut().zip(src[y * pitch..y * pitch + wu * 4].chunks_exact(4)) {
                *o = [p[0] as f32 / 255.0, p[1] as f32 / 255.0, p[2] as f32 / 255.0, 1.0];
            }
        });
        ctx.Unmap(staging, 0);
        Ok(out)
    }
}
