//! Submission through Direct3D 11 from feature level 10_0, or WARP where no device has it:
//! the Direct3D Windows 7 has. Frames draw into an sRGB texture, copied to a DXGI 1.1 swap
//! chain that blits into the window's redirection surface, as Windows 7 has no flip model,
//! or where the window has none, to a flip-model one that DirectComposition shows.
use super::*;
use std::{ffi::c_void, ptr};
use windows::{
    Win32::{
        Foundation::{HMODULE, HWND, RECT},
        Graphics::{
            Direct3D::*,
            Direct3D11::*,
            DirectComposition::{IDCompositionDevice, IDCompositionTarget, IDCompositionVisual},
            Dxgi::{Common::*, *},
        },
        System::LibraryLoader::{GetProcAddress, LoadLibraryW},
        UI::WindowsAndMessaging::{
            GWL_EXSTYLE, GetClientRect, GetWindowLongPtrW, WS_EX_NOREDIRECTIONBITMAP,
        },
    },
    core::{Error, GUID, HRESULT, Interface, PCSTR, Result, s, w},
};

// Shader model 4.0 bytecode from `dxbc/compile.ps1`: the build host has no HLSL compiler, and
// Windows 7 has a runtime one only with an optional update.
const DRAW_VERTEX: &[u8] = include_bytes!("dxbc/draw.vertex.dxbc");
const DRAW_FRAGMENT: &[u8] = include_bytes!("dxbc/draw.fragment.dxbc");
const TRANSLUCENT_VERTEX: &[u8] = include_bytes!("dxbc/translucent.vertex.dxbc");
const TRANSLUCENT_FRAGMENT: &[u8] = include_bytes!("dxbc/translucent.fragment.dxbc");

const FORMAT: DXGI_FORMAT = DXGI_FORMAT_R8G8B8A8_UNORM_SRGB;

/// What a successful creating call leaves in its out-parameter.
fn made<T>(create: impl FnOnce(&mut Option<T>) -> Result<()>) -> Result<T> {
    let mut made = None;
    create(&mut made)?;
    Ok(made.expect("A successful call returns what it made"))
}

/// An sRGB RGBA texture, `pixels` its rows if given.
fn texture(
    device: &ID3D11Device,
    size: [u32; 2],
    bind: D3D11_BIND_FLAG,
    pixels: Option<&[u8]>,
) -> Result<ID3D11Texture2D> {
    let description = D3D11_TEXTURE2D_DESC {
        Width: size[0],
        Height: size[1],
        MipLevels: 1,
        ArraySize: 1,
        Format: FORMAT,
        SampleDesc: DXGI_SAMPLE_DESC {
            Count: 1,
            Quality: 0,
        },
        Usage: D3D11_USAGE_DEFAULT,
        BindFlags: bind.0 as u32,
        ..Default::default()
    };
    let data = pixels.map(|pixels| D3D11_SUBRESOURCE_DATA {
        pSysMem: pixels.as_ptr().cast(),
        SysMemPitch: size[0] * 4,
        SysMemSlicePitch: 0,
    });
    made(|out| unsafe {
        device.CreateTexture2D(&description, data.as_ref().map(ptr::from_ref), Some(out))
    })
}

fn description(texture: &ID3D11Texture2D) -> D3D11_TEXTURE2D_DESC {
    let mut description = D3D11_TEXTURE2D_DESC::default();
    unsafe { texture.GetDesc(&mut description) };
    description
}

fn resource(device: &ID3D11Device, texture: &ID3D11Texture2D) -> Result<ID3D11ShaderResourceView> {
    made(|out| unsafe { device.CreateShaderResourceView(texture, None, Some(out)) })
}

/// A texture as the fragment shader samples it, or none where Direct3D made none, as when
/// the device is lost.
pub(super) struct Image(Option<ID3D11ShaderResourceView>);

/// An offscreen frame, which `Renderer::present` shows in the window.
pub struct Target {
    texture: ID3D11Texture2D,
    view: ID3D11RenderTargetView,
    resource: ID3D11ShaderResourceView,
}

impl Target {
    fn new(device: &ID3D11Device, size: [u32; 2]) -> Result<Self> {
        let bind = D3D11_BIND_RENDER_TARGET | D3D11_BIND_SHADER_RESOURCE;
        let texture = texture(device, size, bind, None)?;
        let view = made(|out| unsafe { device.CreateRenderTargetView(&texture, None, Some(out)) })?;
        let resource = resource(device, &texture)?;
        Ok(Self {
            texture,
            view,
            resource,
        })
    }

    pub fn size(&self) -> [u32; 2] {
        let description = description(&self.texture);
        [description.Width, description.Height]
    }
}

pub(super) struct Gpu {
    device: ID3D11Device,
    context: ID3D11DeviceContext,
    swap_chain: IDXGISwapChain,
    /// What shows the swap chain in a window without a redirection surface.
    _composition: Option<Composition>,
    layout: ID3D11InputLayout,
    vertex: ID3D11VertexShader,
    fragment: ID3D11PixelShader,
    translucent: (ID3D11VertexShader, ID3D11PixelShader),
    /// One for each of `Blend::STATES`.
    blends: [ID3D11BlendState; 4],
    rasterizer: ID3D11RasterizerState,
    nearest: ID3D11SamplerState,
    linear: ID3D11SamplerState,
    buffer: ID3D11Buffer,
    atlas: (ID3D11Texture2D, ID3D11ShaderResourceView),
    /// Offscreen pictures for groups, the target's size.
    groups: Vec<Target>,
}

impl Gpu {
    /// A device and a swap chain on `window`, an `HWND`, and the adapter's description.
    pub(super) fn new(window: *mut c_void) -> Result<(Self, String)> {
        let levels = [
            D3D_FEATURE_LEVEL_11_0,
            D3D_FEATURE_LEVEL_10_1,
            D3D_FEATURE_LEVEL_10_0,
        ];
        let create = |driver| {
            let (mut device, mut context) = (None, None);
            unsafe {
                D3D11CreateDevice(
                    None,
                    driver,
                    HMODULE::default(),
                    D3D11_CREATE_DEVICE_FLAG(0),
                    Some(&levels),
                    D3D11_SDK_VERSION,
                    Some(&mut device),
                    None,
                    Some(&mut context),
                )
            }
            .map(|()| (device.expect("A device"), context.expect("A context")))
        };
        let (device, context) =
            create(D3D_DRIVER_TYPE_HARDWARE).or_else(|_| create(D3D_DRIVER_TYPE_WARP))?;
        let adapter = unsafe { device.cast::<IDXGIDevice>()?.GetAdapter()? };
        let name = unsafe { adapter.GetDesc()? }.Description;
        let name = String::from_utf16_lossy(name.split(|&c| c == 0).next().unwrap_or(&[]));
        let level = unsafe { device.GetFeatureLevel() }.0;
        let [major, minor] = [level >> 12, (level >> 8) & 0xf];
        let description = format!("{name}, feature level {major}_{minor}");
        let hwnd = HWND(window);
        let factory: IDXGIFactory = unsafe { adapter.GetParent()? };
        // Windows 10's window has no redirection surface, made for wgpu's DirectComposition.
        let style = unsafe { GetWindowLongPtrW(hwnd, GWL_EXSTYLE) } as u32;
        let (swap_chain, composition) = if style & WS_EX_NOREDIRECTIONBITMAP.0 != 0 {
            let (swap_chain, composition) = compose(&device, &factory, hwnd)?;
            (swap_chain, Some(composition))
        } else {
            let description = DXGI_SWAP_CHAIN_DESC {
                // Windows 7's desktop composes a blit swap chain's alpha only from BGRA.
                BufferDesc: DXGI_MODE_DESC {
                    Format: DXGI_FORMAT_B8G8R8A8_UNORM,
                    ..Default::default()
                },
                SampleDesc: DXGI_SAMPLE_DESC {
                    Count: 1,
                    Quality: 0,
                },
                BufferUsage: DXGI_USAGE_RENDER_TARGET_OUTPUT,
                BufferCount: 1,
                OutputWindow: hwnd,
                Windowed: true.into(),
                SwapEffect: DXGI_SWAP_EFFECT_DISCARD,
                Flags: 0,
            };
            let swap_chain =
                made(|out| unsafe { factory.CreateSwapChain(&device, &description, out).ok() })?;
            (swap_chain, None)
        };
        // DXGI would otherwise watch the window's messages and take Alt+Enter to full screen.
        unsafe {
            factory
                .MakeWindowAssociation(hwnd, DXGI_MWA_NO_WINDOW_CHANGES | DXGI_MWA_NO_ALT_ENTER)?;
        }

        let elements: [_; 9] = std::array::from_fn(|index| {
            let (_, floats, offset) = ATTRIBUTES[index];
            D3D11_INPUT_ELEMENT_DESC {
                SemanticName: PCSTR(c"TEXCOORD".as_ptr().cast()),
                SemanticIndex: index as u32,
                Format: [
                    DXGI_FORMAT_R32_FLOAT,
                    DXGI_FORMAT_R32G32_FLOAT,
                    DXGI_FORMAT_R32G32B32_FLOAT,
                    DXGI_FORMAT_R32G32B32A32_FLOAT,
                ][floats - 1],
                InputSlot: 0,
                AlignedByteOffset: offset as u32,
                InputSlotClass: D3D11_INPUT_PER_VERTEX_DATA,
                InstanceDataStepRate: 0,
            }
        });
        let layout =
            made(|out| unsafe { device.CreateInputLayout(&elements, DRAW_VERTEX, Some(out)) })?;
        let vertex_shader =
            |code| made(|out| unsafe { device.CreateVertexShader(code, None, Some(out)) });
        let pixel_shader =
            |code| made(|out| unsafe { device.CreatePixelShader(code, None, Some(out)) });

        let blend = |factors: [Factor; 4]| {
            let [src, dst, src_alpha, dst_alpha] = factors.map(|factor| match factor {
                Factor::Zero => D3D11_BLEND_ZERO,
                Factor::One => D3D11_BLEND_ONE,
                Factor::SourceAlpha => D3D11_BLEND_SRC_ALPHA,
                Factor::OneMinusSourceAlpha => D3D11_BLEND_INV_SRC_ALPHA,
                Factor::Destination => D3D11_BLEND_DEST_COLOR,
            });
            let mut description = D3D11_BLEND_DESC::default();
            description.RenderTarget[0] = D3D11_RENDER_TARGET_BLEND_DESC {
                BlendEnable: true.into(),
                SrcBlend: src,
                DestBlend: dst,
                BlendOp: D3D11_BLEND_OP_ADD,
                SrcBlendAlpha: src_alpha,
                DestBlendAlpha: dst_alpha,
                BlendOpAlpha: D3D11_BLEND_OP_ADD,
                RenderTargetWriteMask: D3D11_COLOR_WRITE_ENABLE_ALL.0 as u8,
            };
            made(|out| unsafe { device.CreateBlendState(&description, Some(out)) })
        };
        let sampler = |filter| {
            let description = D3D11_SAMPLER_DESC {
                Filter: filter,
                AddressU: D3D11_TEXTURE_ADDRESS_CLAMP,
                AddressV: D3D11_TEXTURE_ADDRESS_CLAMP,
                AddressW: D3D11_TEXTURE_ADDRESS_CLAMP,
                ComparisonFunc: D3D11_COMPARISON_NEVER,
                MaxLOD: f32::MAX,
                ..Default::default()
            };
            made(|out| unsafe { device.CreateSamplerState(&description, Some(out)) })
        };
        let rasterizer = D3D11_RASTERIZER_DESC {
            FillMode: D3D11_FILL_SOLID,
            CullMode: D3D11_CULL_NONE,
            DepthClipEnable: true.into(),
            ScissorEnable: true.into(),
            ..Default::default()
        };
        let buffer = D3D11_BUFFER_DESC {
            ByteWidth: VERTEX_BUFFER_BYTES as u32,
            Usage: D3D11_USAGE_DYNAMIC,
            BindFlags: D3D11_BIND_VERTEX_BUFFER.0 as u32,
            CPUAccessFlags: D3D11_CPU_ACCESS_WRITE.0 as u32,
            ..Default::default()
        };
        let [over, picture, erase, multiply] = Blend::STATES;
        let gpu = Self {
            layout,
            vertex: vertex_shader(DRAW_VERTEX)?,
            fragment: pixel_shader(DRAW_FRAGMENT)?,
            translucent: (
                vertex_shader(TRANSLUCENT_VERTEX)?,
                pixel_shader(TRANSLUCENT_FRAGMENT)?,
            ),
            blends: [
                blend(over)?,
                blend(picture)?,
                blend(erase)?,
                blend(multiply)?,
            ],
            rasterizer: made(|out| unsafe {
                device.CreateRasterizerState(&rasterizer, Some(out))
            })?,
            nearest: sampler(D3D11_FILTER_MIN_MAG_MIP_POINT)?,
            linear: sampler(D3D11_FILTER_MIN_MAG_MIP_LINEAR)?,
            buffer: made(|out| unsafe { device.CreateBuffer(&buffer, None, Some(out)) })?,
            atlas: atlas(&device, ATLAS_SIZE)?,
            groups: Vec::new(),
            device,
            context,
            swap_chain,
            _composition: composition,
        };
        Ok((gpu, description))
    }

    pub(super) fn target(&self, size: [u32; 2]) -> Result<Target> {
        Target::new(&self.device, size)
    }

    /// Copies `target` to the window, premultiplying each pixel again in sRGB, as the
    /// desktop compositing the window over its backdrop needs, which leaves opaque ones be.
    pub(super) fn present(&self, target: &Target) -> Result<()> {
        let context = &self.context;
        let size = target.size();
        let [width, height] = size;
        unsafe {
            let buffers = self.swap_chain.GetDesc()?.BufferDesc;
            if [buffers.Width, buffers.Height] != size {
                let flags = DXGI_SWAP_CHAIN_FLAG(0);
                (self.swap_chain).ResizeBuffers(0, width, height, DXGI_FORMAT_UNKNOWN, flags)?;
            }
            let buffer: ID3D11Texture2D = self.swap_chain.GetBuffer(0)?;
            let view = made(|out| self.device.CreateRenderTargetView(&buffer, None, Some(out)))?;
            let (vertex, fragment) = &self.translucent;
            self.begin(&view, size);
            context.OMSetBlendState(None, None, u32::MAX);
            context.VSSetShader(vertex, None);
            context.PSSetShader(fragment, None);
            context.PSSetShaderResources(0, Some(&[Some(target.resource.clone())]));
            context.Draw(3, 0);
            context.PSSetShaderResources(0, Some(&[None]));
            context.OMSetRenderTargets(None, None);
            self.swap_chain.Present(1, DXGI_PRESENT(0)).ok()
        }
    }

    /// sRGB-encoded RGBA rows of `target`, top first.
    pub(super) fn read_pixels(&self, target: &Target) -> Result<Vec<u8>> {
        let mut description = description(&target.texture);
        let [width, height] = [description.Width, description.Height];
        description.Usage = D3D11_USAGE_STAGING;
        description.BindFlags = 0;
        description.CPUAccessFlags = D3D11_CPU_ACCESS_READ.0 as u32;
        let row = width as usize * 4;
        let mut pixels = Vec::with_capacity(row * height as usize);
        unsafe {
            let staging = made(|out| self.device.CreateTexture2D(&description, None, Some(out)))?;
            self.context.CopyResource(&staging, &target.texture);
            let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
            self.context
                .Map(&staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped))?;
            for y in 0..height as usize {
                let start = mapped.pData.cast::<u8>().add(y * mapped.RowPitch as usize);
                pixels.extend_from_slice(std::slice::from_raw_parts(start, row));
            }
            self.context.Unmap(&staging, 0);
        }
        Ok(pixels)
    }

    /// Blends in linear light, its targets being sRGB.
    pub(super) fn blends_linear(&self) -> bool {
        true
    }

    /// Offscreen pictures' rows run top first.
    pub(super) fn flipped(&self) -> bool {
        false
    }

    pub(super) fn max_texture_dimension(&self) -> u32 {
        if unsafe { self.device.GetFeatureLevel() }.0 >= D3D_FEATURE_LEVEL_11_0.0 {
            D3D11_REQ_TEXTURE2D_U_OR_V_DIMENSION
        } else {
            // Feature level 10's.
            8192
        }
    }

    pub(super) fn atlas_side(&self) -> u32 {
        description(&self.atlas.0).Width
    }

    pub(super) fn new_atlas(&mut self, side: u32) {
        match atlas(&self.device, side) {
            Ok(atlas) => self.atlas = atlas,
            Err(error) => eprintln!("No glyph atlas {side} pixels square: {error}"),
        }
    }

    pub(super) fn write_atlas(&self, origin: [u32; 2], size: [u32; 2], rgba: &[u8]) {
        let area = D3D11_BOX {
            left: origin[0],
            top: origin[1],
            front: 0,
            right: origin[0] + size[0],
            bottom: origin[1] + size[1],
            back: 1,
        };
        unsafe {
            self.context.UpdateSubresource(
                &self.atlas.0,
                0,
                Some(&area),
                rgba.as_ptr().cast(),
                size[0] * 4,
                0,
            );
        }
    }

    pub(super) fn upload_image(&self, image: &RasterImage) -> Image {
        let texture = texture(
            &self.device,
            image.size,
            D3D11_BIND_SHADER_RESOURCE,
            Some(image.pixels()),
        );
        let view = texture.and_then(|texture| resource(&self.device, &texture));
        Image(
            view.inspect_err(|error| eprintln!("No image texture: {error}"))
                .ok(),
        )
    }

    /// Binds `view`, `size` pixels, as the render target, with the pipeline's state.
    fn begin(&self, view: &ID3D11RenderTargetView, size: [u32; 2]) {
        let context = &self.context;
        unsafe {
            context.PSSetShaderResources(0, Some(&[None]));
            context.OMSetRenderTargets(Some(&[Some(view.clone())]), None);
            context.IASetPrimitiveTopology(D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST);
            context.RSSetState(&self.rasterizer);
            context.RSSetViewports(Some(&[D3D11_VIEWPORT {
                TopLeftX: 0.0,
                TopLeftY: 0.0,
                Width: size[0] as f32,
                Height: size[1] as f32,
                MinDepth: 0.0,
                MaxDepth: 1.0,
            }]));
            context.RSSetScissorRects(Some(&[RECT {
                left: 0,
                top: 0,
                right: size[0] as i32,
                bottom: size[1] as i32,
            }]));
        }
    }

    /// Clears `target`, `size` device pixels, to linear `clear` and draws the prepared
    /// batches, each group's offscreen first.
    pub(super) fn submit(
        &mut self,
        frame: &Frame<'_>,
        target: &Target,
        size: [u32; 2],
        clear: [f32; 4],
    ) {
        if self
            .groups
            .first()
            .is_some_and(|picture| picture.size() != size)
        {
            self.groups.clear();
        }
        while self.groups.len() < frame.groups.len() {
            match Target::new(&self.device, size) {
                Ok(picture) => self.groups.push(picture),
                Err(error) => {
                    eprintln!("No offscreen picture: {error}");
                    return;
                }
            }
        }
        let gpu = &*self;
        let context = &gpu.context;
        let bytes: &[u8] = bytemuck::cast_slice(frame.vertices);
        let bytes = &bytes[..bytes.len().min(VERTEX_BUFFER_BYTES as usize)];
        unsafe {
            let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
            if let Err(error) = context.Map(
                &gpu.buffer,
                0,
                D3D11_MAP_WRITE_DISCARD,
                0,
                Some(&mut mapped),
            ) {
                eprintln!("No vertex buffer: {error}");
                return;
            }
            ptr::copy_nonoverlapping(bytes.as_ptr(), mapped.pData.cast(), bytes.len());
            context.Unmap(&gpu.buffer, 0);
            context.IASetInputLayout(&gpu.layout);
            let buffer = Some(gpu.buffer.clone());
            let stride = size_of::<Vertex>() as u32;
            context.IASetVertexBuffers(0, 1, Some(&buffer), Some(&stride), Some(&0));
        }
        let paint = |target: &Target, clear: [f32; 4], batches: &[Batch]| unsafe {
            gpu.begin(&target.view, size);
            context.VSSetShader(&gpu.vertex, None);
            context.PSSetShader(&gpu.fragment, None);
            context.ClearRenderTargetView(&target.view, &clear);
            for batch in batches {
                let [x, y, w, h] = batch.scissor.map(|value| value as i32);
                let rect = RECT {
                    left: x,
                    top: y,
                    right: x + w,
                    bottom: y + h,
                };
                context.RSSetScissorRects(Some(&[rect]));
                let (resource, sampler) = match batch.blend {
                    Blend::Image(id) => (frame.image::<Image>(id).0.as_ref(), &gpu.linear),
                    Blend::Group(index) => (Some(&gpu.groups[index].resource), &gpu.linear),
                    Blend::Over | Blend::Erase | Blend::Multiply => {
                        (Some(&gpu.atlas.1), &gpu.nearest)
                    }
                };
                context.OMSetBlendState(&gpu.blends[batch.blend.state()], None, u32::MAX);
                context.PSSetSamplers(0, Some(&[Some(sampler.clone())]));
                context.PSSetShaderResources(0, Some(&[resource.cloned()]));
                context.Draw(batch.vertices.len() as u32, batch.vertices.start);
            }
        };
        for (group, picture) in frame.groups.iter().zip(&gpu.groups) {
            paint(picture, [0.0; 4], &group.batches);
        }
        paint(target, clear, frame.batches);
        unsafe {
            context.PSSetShaderResources(0, Some(&[None]));
            context.OMSetRenderTargets(None, None);
        }
    }
}

type Composition = (
    IDCompositionDevice,
    IDCompositionTarget,
    IDCompositionVisual,
);

/// A flip-model swap chain with premultiplied alpha, which DirectComposition shows over
/// all of `hwnd`.
fn compose(
    device: &ID3D11Device,
    factory: &IDXGIFactory,
    hwnd: HWND,
) -> Result<(IDXGISwapChain, Composition)> {
    let mut client = RECT::default();
    unsafe { GetClientRect(hwnd, &mut client)? };
    let description = DXGI_SWAP_CHAIN_DESC1 {
        Width: client.right.max(1) as u32,
        Height: client.bottom.max(1) as u32,
        Format: DXGI_FORMAT_B8G8R8A8_UNORM,
        SampleDesc: DXGI_SAMPLE_DESC {
            Count: 1,
            Quality: 0,
        },
        BufferUsage: DXGI_USAGE_RENDER_TARGET_OUTPUT,
        BufferCount: 2,
        Scaling: DXGI_SCALING_STRETCH,
        SwapEffect: DXGI_SWAP_EFFECT_FLIP_SEQUENTIAL,
        AlphaMode: DXGI_ALPHA_MODE_PREMULTIPLIED,
        ..Default::default()
    };
    // Windows 7 has no dcomp.dll, so it is looked up rather than linked.
    type Create = unsafe extern "system" fn(*mut c_void, *const GUID, *mut *mut c_void) -> HRESULT;
    unsafe {
        let factory: IDXGIFactory2 = factory.cast()?;
        let swap_chain = factory.CreateSwapChainForComposition(device, &description, None)?;
        let library = LoadLibraryW(w!("dcomp.dll"))?;
        let create = GetProcAddress(library, s!("DCompositionCreateDevice"))
            .ok_or_else(Error::from_thread)?;
        let create = std::mem::transmute::<unsafe extern "system" fn() -> isize, Create>(create);
        let mut composition = ptr::null_mut();
        let dxgi: IDXGIDevice = device.cast()?;
        create(dxgi.as_raw(), &IDCompositionDevice::IID, &mut composition).ok()?;
        let composition = IDCompositionDevice::from_raw(composition);
        let target = composition.CreateTargetForHwnd(hwnd, true)?;
        let visual = composition.CreateVisual()?;
        visual.SetContent(&swap_chain)?;
        target.SetRoot(&visual)?;
        composition.Commit()?;
        Ok((swap_chain.cast()?, (composition, target, visual)))
    }
}

/// An empty atlas `side` texels square.
fn atlas(device: &ID3D11Device, side: u32) -> Result<(ID3D11Texture2D, ID3D11ShaderResourceView)> {
    let texture = texture(device, [side; 2], D3D11_BIND_SHADER_RESOURCE, None)?;
    let view = resource(device, &texture)?;
    Ok((texture, view))
}
