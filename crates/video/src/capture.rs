//! Windows screen capture via the Desktop Duplication API (DXGI Output
//! Duplication). Produces BGRA [`Frame`]s of a monitor for the screen-mirror
//! pipeline.
//!
//! Flow: create a D3D11 device → get the output's `IDXGIOutputDuplication` →
//! `AcquireNextFrame` (GPU texture) → `CopyResource` into a CPU-readable staging
//! texture → `Map` and copy the BGRA bytes out (honoring the row pitch).
//!
//! `next_frame()` returns `Ok(None)` when no new frame was ready within the
//! timeout (the screen didn't change), and transparently rebuilds the
//! duplication on `DXGI_ERROR_ACCESS_LOST` (e.g. resolution change, secure
//! desktop). EXPERIMENTAL — compiled only with `--features extend` and verified
//! on-device.

use crate::pipeline::{Frame, FrameSource};
use anyhow::Context as _;
use windows::core::Interface;
use windows::Win32::Foundation::HMODULE;
use windows::Win32::Graphics::Direct3D::D3D_DRIVER_TYPE_HARDWARE;
use windows::Win32::Graphics::Direct3D11::{
    D3D11CreateDevice, ID3D11Device, ID3D11DeviceContext, ID3D11Texture2D, D3D11_BIND_FLAG,
    D3D11_CPU_ACCESS_READ, D3D11_CREATE_DEVICE_FLAG, D3D11_MAPPED_SUBRESOURCE, D3D11_MAP_READ,
    D3D11_RESOURCE_MISC_FLAG, D3D11_SDK_VERSION, D3D11_TEXTURE2D_DESC, D3D11_USAGE_STAGING,
};
use windows::Win32::Graphics::Dxgi::Common::DXGI_SAMPLE_DESC;
use windows::Win32::Graphics::Dxgi::{
    IDXGIAdapter, IDXGIDevice, IDXGIOutput, IDXGIOutput1, IDXGIOutputDuplication, IDXGIResource,
    DXGI_ERROR_ACCESS_LOST, DXGI_ERROR_WAIT_TIMEOUT, DXGI_OUTDUPL_FRAME_INFO,
};

/// Captures a single monitor via Desktop Duplication.
pub struct DesktopDuplicationSource {
    device: ID3D11Device,
    context: ID3D11DeviceContext,
    output: IDXGIOutput1,
    dupl: Option<IDXGIOutputDuplication>,
    timeout_ms: u32,
}

impl DesktopDuplicationSource {
    /// Capture the primary output (output index 0 of adapter 0).
    pub fn new() -> anyhow::Result<Self> {
        let (device, context) = create_device()?;

        let dxgi_device: IDXGIDevice = device.cast()?;
        let adapter: IDXGIAdapter = unsafe { dxgi_device.GetAdapter()? };
        let output: IDXGIOutput = unsafe { adapter.EnumOutputs(0)? };
        let output1: IDXGIOutput1 = output.cast()?;

        let mut src = Self {
            device,
            context,
            output: output1,
            dupl: None,
            timeout_ms: 100,
        };
        src.ensure_duplication()?;
        Ok(src)
    }

    fn ensure_duplication(&mut self) -> anyhow::Result<()> {
        if self.dupl.is_none() {
            let dupl = unsafe { self.output.DuplicateOutput(&self.device)? };
            self.dupl = Some(dupl);
        }
        Ok(())
    }
}

impl FrameSource for DesktopDuplicationSource {
    fn next_frame(&mut self) -> anyhow::Result<Option<Frame>> {
        self.ensure_duplication()?;
        // Clone the (refcounted) COM pointer so we don't hold a borrow of `self`
        // across `self.dupl = None` on access-loss.
        let dupl = self.dupl.clone().expect("duplication present");

        let mut frame_info = DXGI_OUTDUPL_FRAME_INFO::default();
        let mut resource: Option<IDXGIResource> = None;
        match unsafe { dupl.AcquireNextFrame(self.timeout_ms, &mut frame_info, &mut resource) } {
            Ok(()) => {}
            Err(e) if e.code() == DXGI_ERROR_WAIT_TIMEOUT => return Ok(None),
            Err(e) if e.code() == DXGI_ERROR_ACCESS_LOST => {
                self.dupl = None; // rebuild on the next call
                return Ok(None);
            }
            Err(e) => return Err(e.into()),
        }

        let result = self.copy_frame(resource);
        // Always release, even if copying failed.
        unsafe {
            let _ = dupl.ReleaseFrame();
        }
        result.map(Some)
    }
}

impl DesktopDuplicationSource {
    fn copy_frame(&self, resource: Option<IDXGIResource>) -> anyhow::Result<Frame> {
        let resource = resource.context("AcquireNextFrame returned no surface")?;
        let frame_tex: ID3D11Texture2D = resource.cast()?;

        let mut desc = D3D11_TEXTURE2D_DESC::default();
        unsafe { frame_tex.GetDesc(&mut desc) };

        // CPU-readable staging copy.
        let staging_desc = D3D11_TEXTURE2D_DESC {
            Width: desc.Width,
            Height: desc.Height,
            MipLevels: 1,
            ArraySize: 1,
            Format: desc.Format,
            SampleDesc: DXGI_SAMPLE_DESC {
                Count: 1,
                Quality: 0,
            },
            Usage: D3D11_USAGE_STAGING,
            BindFlags: D3D11_BIND_FLAG(0).0 as u32,
            CPUAccessFlags: D3D11_CPU_ACCESS_READ.0 as u32,
            MiscFlags: D3D11_RESOURCE_MISC_FLAG(0).0 as u32,
        };
        let mut staging: Option<ID3D11Texture2D> = None;
        unsafe {
            self.device
                .CreateTexture2D(&staging_desc, None, Some(&mut staging))?
        };
        let staging = staging.context("staging texture not created")?;

        unsafe { self.context.CopyResource(&staging, &frame_tex) };

        let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
        unsafe {
            self.context
                .Map(&staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped))?
        };

        let w = desc.Width as usize;
        let h = desc.Height as usize;
        let row_bytes = w * 4;
        let pitch = mapped.RowPitch as usize;
        let mut bgra = vec![0u8; row_bytes * h];
        unsafe {
            let src = mapped.pData as *const u8;
            for y in 0..h {
                std::ptr::copy_nonoverlapping(
                    src.add(y * pitch),
                    bgra.as_mut_ptr().add(y * row_bytes),
                    row_bytes,
                );
            }
            self.context.Unmap(&staging, 0);
        }

        Ok(Frame {
            width: desc.Width,
            height: desc.Height,
            stride: row_bytes as u32,
            bgra,
        })
    }
}

fn create_device() -> anyhow::Result<(ID3D11Device, ID3D11DeviceContext)> {
    let mut device: Option<ID3D11Device> = None;
    let mut context: Option<ID3D11DeviceContext> = None;
    unsafe {
        D3D11CreateDevice(
            None,
            D3D_DRIVER_TYPE_HARDWARE,
            HMODULE::default(),
            D3D11_CREATE_DEVICE_FLAG(0),
            None, // let the runtime pick the feature level
            D3D11_SDK_VERSION,
            Some(&mut device),
            None, // don't need the chosen feature level back
            Some(&mut context),
        )?;
    }
    Ok((
        device.context("D3D11 device not created")?,
        context.context("D3D11 context not created")?,
    ))
}
