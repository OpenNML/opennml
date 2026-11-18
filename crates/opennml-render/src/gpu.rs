//! Graphics device, surface and backend selection.

use std::str::FromStr;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow};

use crate::gamma::{GammaPass, gamma_ramp, ramp_is_identity};

/// Graphics backend chosen by the user.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BackendChoice {
    #[default]
    Auto,
    Vulkan,
    Dx12,
}

impl FromStr for BackendChoice {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_ascii_lowercase().as_str() {
            "auto" => Ok(Self::Auto),
            "vulkan" => Ok(Self::Vulkan),
            "dx12" => Ok(Self::Dx12),
            other => Err(format!(
                "unknown backend \"{other}\" (allowed: auto, vulkan, dx12)"
            )),
        }
    }
}

/// Order of the attempts; the respective other backend is always the fallback.
/// `Auto` starts with DX12 (the native API on Windows).
pub fn backend_attempts(choice: BackendChoice) -> Vec<wgpu::Backends> {
    match choice {
        BackendChoice::Auto | BackendChoice::Dx12 => {
            vec![wgpu::Backends::DX12, wgpu::Backends::VULKAN]
        }
        BackendChoice::Vulkan => vec![wgpu::Backends::VULKAN, wgpu::Backends::DX12],
    }
}

/// Valid surface size or `None` (e.g. minimized window - size 0 is forbidden in wgpu).
pub fn surface_size(width: u32, height: u32) -> Option<(u32, u32)> {
    (width > 0 && height > 0).then_some((width, height))
}

/// Limits the surface size to the largest texture dimension the device allows. A window spanning
/// several monitors can be wider; otherwise wgpu rejects such a surface (`TooLarge`).
fn clamp_surface_size((width, height): (u32, u32), max_dimension: u32) -> (u32, u32) {
    (width.min(max_dimension), height.min(max_dimension))
}

/// Surface format without sRGB conversion: the original renders linearly (re/renderer).
/// Prefers `Bgra8Unorm`, then `Rgba8Unorm`, then the first non-sRGB format, otherwise the first.
fn choose_surface_format(formats: &[wgpu::TextureFormat]) -> Option<wgpu::TextureFormat> {
    use wgpu::TextureFormat::{Bgra8Unorm, Rgba8Unorm};
    [Bgra8Unorm, Rgba8Unorm]
        .into_iter()
        .find(|f| formats.contains(f))
        .or_else(|| formats.iter().copied().find(|f| !f.is_srgb()))
        .or_else(|| formats.first().copied())
}

/// What to do with the surface on a size change.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ResizeAction {
    /// Size 0 (minimized): suspend rendering, do not touch the surface.
    Suspend,
    /// Nothing to do.
    Keep,
    /// Reconfigure the surface with this size.
    Reconfigure(u32, u32),
}

/// Decides on a size change; after suspending, it reconfigures even at the same size.
/// The new size is limited to `max_dimension`.
fn resize_action(
    current: (u32, u32),
    suspended: bool,
    new: (u32, u32),
    max_dimension: u32,
) -> ResizeAction {
    let Some(size) = surface_size(new.0, new.1) else {
        return ResizeAction::Suspend;
    };
    let (width, height) = clamp_surface_size(size, max_dimension);
    if !suspended && (width, height) == current {
        ResizeAction::Keep
    } else {
        ResizeAction::Reconfigure(width, height)
    }
}

/// Result of `get_current_texture` without the texture itself (testable without a GPU).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SurfaceStatus {
    Success,
    Suboptimal,
    Timeout,
    Occluded,
    Outdated,
    Lost,
    Validation,
}

impl SurfaceStatus {
    fn of(texture: &wgpu::CurrentSurfaceTexture) -> Self {
        use wgpu::CurrentSurfaceTexture as T;
        match texture {
            T::Success(_) => Self::Success,
            T::Suboptimal(_) => Self::Suboptimal,
            T::Timeout => Self::Timeout,
            T::Occluded => Self::Occluded,
            T::Outdated => Self::Outdated,
            T::Lost => Self::Lost,
            T::Validation => Self::Validation,
        }
    }
}

/// Reaction to the state of the surface.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FrameAction {
    Draw,
    /// Reconfigure the surface, skip the frame.
    Reconfigure,
    /// Recreate the surface via `Instance::create_surface` and configure it.
    Recreate,
    /// Skip the frame.
    Skip,
}

fn frame_action(status: SurfaceStatus) -> FrameAction {
    match status {
        SurfaceStatus::Success | SurfaceStatus::Suboptimal => FrameAction::Draw,
        SurfaceStatus::Outdated => FrameAction::Reconfigure,
        SurfaceStatus::Lost => FrameAction::Recreate,
        SurfaceStatus::Timeout | SurfaceStatus::Occluded | SurfaceStatus::Validation => {
            FrameAction::Skip
        }
    }
}

/// Log level for skipped frames; `None` = do not log.
fn skip_log_level(status: SurfaceStatus) -> Option<log::Level> {
    match status {
        SurfaceStatus::Timeout | SurfaceStatus::Validation => Some(log::Level::Warn),
        SurfaceStatus::Occluded => Some(log::Level::Debug),
        _ => None,
    }
}

/// Interval between two attempts to recreate a lost surface.
const SURFACE_RETRY: Duration = Duration::from_millis(500);

/// Result of a frame for the event loop.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrameOutcome {
    /// Frame presented.
    Presented,
    /// Window occluded (e.g. exclusive fullscreen after Alt+Tab): do not redraw constantly.
    Occluded,
    /// Frame skipped for another reason.
    Skipped,
    /// No surface (lost, recreation pending): like occluded, try only occasionally instead of
    /// hogging the CPU.
    NoSurface,
}

/// Graphics device with a surface for a window.
pub struct Gpu {
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
    instance: wgpu::Instance,
    /// Creates the surface target (window) again when the surface was lost.
    make_target: Box<dyn Fn() -> wgpu::SurfaceTarget<'static>>,
    /// `None` as long as the surface could not be recreated after a loss.
    surface: Option<wgpu::Surface<'static>>,
    config: wgpu::SurfaceConfiguration,
    adapter_info: wgpu::AdapterInfo,
    /// Window minimized (size 0): no rendering, no reconfiguration.
    suspended: bool,
    /// Problem state reported last, so the log does not overflow.
    last_skip: Option<SurfaceStatus>,
    /// Earliest next attempt to recreate the surface (only without a surface).
    next_surface_retry: Instant,
    /// Failure to recreate already reported (until the next success).
    surface_error_logged: bool,
    /// Gamma setting, see [`Gpu::set_gamma`].
    gamma: GammaState,
}

/// Gamma ramp of the display: value, switch and the pass (created only when it is needed).
struct GammaState {
    value: f32,
    enabled: bool,
    /// The ramp changes something at 8 bits (otherwise the pass is omitted).
    effective: bool,
    pass: Option<GammaPass>,
}

impl GammaState {
    fn active(&self) -> bool {
        self.enabled && self.effective
    }
}

/// Installs a handler that logs wgpu errors instead of crashing (default: panic).
/// Identical messages in direct succession are output only once.
fn log_uncaptured_errors(device: &wgpu::Device) {
    let last = Mutex::new(String::new());
    device.on_uncaptured_error(Arc::new(move |error: wgpu::Error| {
        let message = error.to_string();
        let mut last = last.lock().unwrap_or_else(|e| e.into_inner());
        if *last != message {
            log::error!("wgpu error: {message}");
            *last = message;
        }
    }));
}

/// Requests a device with the texture limits of the adapter (instead of the WebGPU default 8192: a
/// window spanning several monitors, here 3 x 3072 = 9216 px, needs a wider surface) and installs
/// the error handler.
fn request_device(
    adapter: &wgpu::Adapter,
) -> Result<(wgpu::Device, wgpu::Queue), wgpu::RequestDeviceError> {
    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        required_limits: wgpu::Limits::default().using_resolution(adapter.limits()),
        ..Default::default()
    }))?;
    log_uncaptured_errors(&device);
    Ok((device, queue))
}

impl Gpu {
    /// Tries the backends in the order of [`backend_attempts`].
    pub fn new<T>(target: T, size: (u32, u32), choice: BackendChoice) -> Result<Self>
    where
        T: Into<wgpu::SurfaceTarget<'static>> + Clone + 'static,
    {
        let mut errors = Vec::new();
        for backends in backend_attempts(choice) {
            match Self::try_backend(target.clone(), size, backends) {
                Ok(gpu) => return Ok(gpu),
                Err(e) => {
                    log::warn!("Backend {backends:?} not usable: {e:#}");
                    errors.push(format!("{backends:?}: {e:#}"));
                }
            }
        }
        Err(anyhow!(
            "no graphics backend usable:\n{}",
            errors.join("\n")
        ))
    }

    fn try_backend<T>(target: T, size: (u32, u32), backends: wgpu::Backends) -> Result<Self>
    where
        T: Into<wgpu::SurfaceTarget<'static>> + Clone + 'static,
    {
        let make_target: Box<dyn Fn() -> wgpu::SurfaceTarget<'static>> =
            Box::new(move || target.clone().into());
        let mut desc = wgpu::InstanceDescriptor::new_without_display_handle();
        desc.backends = backends;
        let instance = wgpu::Instance::new(desc);
        let surface = instance
            .create_surface(make_target())
            .context("creating the surface")?;
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            force_fallback_adapter: false,
            compatible_surface: Some(&surface),
            apply_limit_buckets: false,
        }))
        .context("requesting the graphics card")?;
        let (device, queue) = request_device(&adapter).context("requesting the device")?;
        let max_dimension = device.limits().max_texture_dimension_2d;
        let (width, height) = clamp_surface_size((size.0.max(1), size.1.max(1)), max_dimension);
        let mut config = surface
            .get_default_config(&adapter, width, height)
            .ok_or_else(|| anyhow!("surface is not supported by the adapter"))?;
        let formats = surface.get_capabilities(&adapter).formats;
        config.format =
            choose_surface_format(&formats).ok_or_else(|| anyhow!("surface offers no format"))?;
        surface.configure(&device, &config);
        let adapter_info = adapter.get_info();
        log::info!(
            "Graphics: {} via {:?}, format {:?}, max texture dimension {max_dimension}",
            adapter_info.name,
            adapter_info.backend,
            config.format
        );
        Ok(Self {
            device,
            queue,
            instance,
            make_target,
            surface: Some(surface),
            config,
            adapter_info,
            suspended: false,
            last_skip: None,
            next_surface_retry: Instant::now(),
            surface_error_logged: false,
            gamma: GammaState {
                value: 1.0,
                enabled: true,
                effective: false,
                pass: None,
            },
        })
    }

    /// Sets the gamma ramp like `Renderer_SetGammaRamp(G, G, G)` of the original ([`gamma_ramp`]). It affects only
    /// the window image, not screenshots (those bypass the window via `render_to_image`).
    /// If the ramp maps unchanged at 8 bits (`G = 1`), the additional pass is omitted.
    pub fn set_gamma(&mut self, value: f32) {
        let ramp = gamma_ramp(value);
        let gamma = &mut self.gamma;
        gamma.value = value;
        gamma.effective = !ramp_is_identity(&ramp);
        if gamma.effective {
            let pass = gamma
                .pass
                .get_or_insert_with(|| GammaPass::new(&self.device, self.config.format));
            pass.set_ramp(&self.queue, &ramp);
        }
        log::debug!(
            "Gamma {value}: {}",
            if gamma.active() {
                "ramp active"
            } else if gamma.effective {
                "ramp switched off"
            } else {
                "neutral"
            }
        );
    }

    /// Gamma value set last.
    pub fn gamma(&self) -> f32 {
        self.gamma.value
    }

    /// Switches the gamma ramp on or off (default on); the value is kept.
    pub fn set_gamma_enabled(&mut self, enabled: bool) {
        self.gamma.enabled = enabled;
    }

    /// Whether the gamma ramp is switched on.
    pub fn gamma_enabled(&self) -> bool {
        self.gamma.enabled
    }

    pub fn backend(&self) -> wgpu::Backend {
        self.adapter_info.backend
    }

    pub fn adapter_name(&self) -> &str {
        &self.adapter_info.name
    }

    /// Adapts the surface to the new size. Size 0 (minimized) suspends rendering; the next valid
    /// size reconfigures the surface and resumes it.
    pub fn resize(&mut self, width: u32, height: u32) {
        match resize_action(
            (self.config.width, self.config.height),
            self.suspended,
            (width, height),
            self.device.limits().max_texture_dimension_2d,
        ) {
            ResizeAction::Suspend => {
                if !self.suspended {
                    log::debug!("Window minimized, rendering suspended");
                }
                self.suspended = true;
            }
            ResizeAction::Keep => {}
            ResizeAction::Reconfigure(width, height) => {
                self.suspended = false;
                self.config.width = width;
                self.config.height = height;
                if let Some(surface) = &self.surface {
                    surface.configure(&self.device, &self.config);
                }
            }
        }
    }

    /// Recreates the surface (after `Lost`) and configures it with the current size.
    /// Without success the state stays "no surface"; the next attempt follows at the earliest
    /// after [`SURFACE_RETRY`].
    fn recreate_surface(&mut self) {
        // Release the old surface (with its swapchain) first: DX12 allows only one swapchain per
        // window, otherwise Vulkan reports `NATIVE_WINDOW_IN_USE`.
        self.surface = None;
        self.next_surface_retry = Instant::now() + SURFACE_RETRY;
        match self.instance.create_surface((self.make_target)()) {
            Ok(surface) => {
                // The first configuration of a new surface can fail (e.g. window just in the middle
                // of a mode switch); without an error scope the error would go to the global handler.
                let scope = self.device.push_error_scope(wgpu::ErrorFilter::Validation);
                surface.configure(&self.device, &self.config);
                if let Some(error) = pollster::block_on(scope.pop()) {
                    if !self.surface_error_logged {
                        log::warn!("New surface not configurable, retrying: {error}");
                        self.surface_error_logged = true;
                    }
                    return;
                }
                self.surface = Some(surface);
                if self.surface_error_logged {
                    log::info!("Surface recreated");
                }
                self.surface_error_logged = false;
            }
            Err(e) => {
                if !self.surface_error_logged {
                    log::warn!("Surface could not be recreated, retrying: {e}");
                    self.surface_error_logged = true;
                }
            }
        }
    }

    /// Format of the surface (without sRGB, see `choose_surface_format`).
    pub fn surface_format(&self) -> wgpu::TextureFormat {
        self.config.format
    }

    /// Current size of the surface.
    pub fn size(&self) -> (u32, u32) {
        (self.config.width, self.config.height)
    }

    /// Draws a frame: `draw` receives device, queue, encoder, target and size; then the frame is
    /// presented. With an effective gamma ramp the target is an intermediate frame that the "Gamma" pass
    /// writes into the window.
    pub fn render(
        &mut self,
        draw: impl FnOnce(
            &wgpu::Device,
            &wgpu::Queue,
            &mut wgpu::CommandEncoder,
            &wgpu::TextureView,
            (u32, u32),
        ),
    ) -> FrameOutcome {
        if self.suspended {
            return FrameOutcome::Skipped;
        }
        let Some(surface) = &self.surface else {
            if Instant::now() >= self.next_surface_retry {
                self.recreate_surface();
            }
            return FrameOutcome::NoSurface;
        };
        let texture = surface.get_current_texture();
        let status = SurfaceStatus::of(&texture);
        if matches!(status, SurfaceStatus::Success | SurfaceStatus::Suboptimal) {
            self.last_skip = None;
        } else if self.last_skip != Some(status) {
            self.last_skip = Some(status);
            if let Some(level) = skip_log_level(status) {
                log::log!(level, "Frame skipped: {status:?}");
            }
        }
        let frame = match (frame_action(status), texture) {
            (
                FrameAction::Draw,
                wgpu::CurrentSurfaceTexture::Success(frame)
                | wgpu::CurrentSurfaceTexture::Suboptimal(frame),
            ) => frame,
            (FrameAction::Reconfigure, _) => {
                surface.configure(&self.device, &self.config);
                return FrameOutcome::Skipped;
            }
            (FrameAction::Recreate, _) => {
                // As without a surface: recreation is throttled, the event loop retries every
                // 250 ms instead of recreating a permanently lost surface in every
                // frame.
                self.recreate_surface();
                return FrameOutcome::NoSurface;
            }
            _ if status == SurfaceStatus::Occluded => return FrameOutcome::Occluded,
            _ => return FrameOutcome::Skipped,
        };
        let view = frame
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("Frame"),
            });
        let size = (self.config.width, self.config.height);
        let gamma_active = self.gamma.active();
        match self.gamma.pass.as_mut().filter(|_| gamma_active) {
            Some(pass) => {
                let source = pass.source(&self.device, size);
                draw(&self.device, &self.queue, &mut encoder, &source, size);
                pass.apply(&mut encoder, &view);
            }
            None => draw(&self.device, &self.queue, &mut encoder, &view, size),
        }
        self.queue.submit([encoder.finish()]);
        self.queue.present(frame);
        FrameOutcome::Presented
    }
}

/// Graphics device without a window (screenshots), backend order like [`Gpu::new`].
pub fn headless_device(
    choice: BackendChoice,
) -> Result<(wgpu::Device, wgpu::Queue, wgpu::AdapterInfo)> {
    let mut errors = Vec::new();
    for backends in backend_attempts(choice) {
        let mut desc = wgpu::InstanceDescriptor::new_without_display_handle();
        desc.backends = backends;
        let instance = wgpu::Instance::new(desc);
        let adapter =
            match pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                force_fallback_adapter: false,
                compatible_surface: None,
                apply_limit_buckets: false,
            })) {
                Ok(adapter) => adapter,
                Err(e) => {
                    errors.push(format!("{backends:?}: {e}"));
                    continue;
                }
            };
        match request_device(&adapter) {
            Ok((device, queue)) => return Ok((device, queue, adapter.get_info())),
            Err(e) => errors.push(format!("{backends:?}: {e}")),
        }
    }
    Err(anyhow!(
        "no graphics backend usable:\n{}",
        errors.join("\n")
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backend_attempts_always_fall_back_to_the_other() {
        use wgpu::Backends;
        assert_eq!(
            backend_attempts(BackendChoice::Auto),
            [Backends::DX12, Backends::VULKAN]
        );
        assert_eq!(
            backend_attempts(BackendChoice::Dx12),
            [Backends::DX12, Backends::VULKAN]
        );
        assert_eq!(
            backend_attempts(BackendChoice::Vulkan),
            [Backends::VULKAN, Backends::DX12]
        );
    }

    #[test]
    fn backend_choice_parses_case_insensitive() {
        assert_eq!("VULKAN".parse(), Ok(BackendChoice::Vulkan));
        assert_eq!("dx12".parse(), Ok(BackendChoice::Dx12));
        assert_eq!("auto".parse(), Ok(BackendChoice::Auto));
        assert!("opengl".parse::<BackendChoice>().is_err());
    }

    #[test]
    fn surface_size_rejects_zero() {
        assert_eq!(surface_size(0, 600), None);
        assert_eq!(surface_size(800, 0), None);
        assert_eq!(surface_size(800, 600), Some((800, 600)));
    }

    #[test]
    fn zero_size_suspends_and_valid_size_after_suspend_reconfigures() {
        assert_eq!(
            resize_action((800, 600), false, (0, 0), 8192),
            ResizeAction::Suspend
        );
        assert_eq!(
            resize_action((800, 600), false, (0, 600), 8192),
            ResizeAction::Suspend
        );
        assert_eq!(
            resize_action((800, 600), true, (0, 0), 8192),
            ResizeAction::Suspend
        );
        assert_eq!(
            resize_action((800, 600), false, (800, 600), 8192),
            ResizeAction::Keep
        );
        assert_eq!(
            resize_action((800, 600), false, (1024, 768), 8192),
            ResizeAction::Reconfigure(1024, 768)
        );
        // After minimizing, the same size as before must still be reconfigured.
        assert_eq!(
            resize_action((800, 600), true, (800, 600), 8192),
            ResizeAction::Reconfigure(800, 600)
        );
    }

    #[test]
    fn resize_clamps_to_max_texture_dimension() {
        // Window across three 3072 monitors (9216 px) on a device with 8192 as the limit.
        assert_eq!(
            resize_action((800, 600), false, (9216, 1728), 8192),
            ResizeAction::Reconfigure(8192, 1728)
        );
        assert_eq!(
            resize_action((8192, 1728), false, (9300, 1728), 8192),
            ResizeAction::Keep
        );
        assert_eq!(
            resize_action((800, 600), false, (0, 20_000), 8192),
            ResizeAction::Suspend
        );
        assert_eq!(clamp_surface_size((20_000, 9000), 16_384), (16_384, 9000));
        assert_eq!(clamp_surface_size((3072, 1728), 8192), (3072, 1728));
    }

    #[test]
    fn surface_format_prefers_linear_bgra_then_rgba() {
        use wgpu::TextureFormat::*;
        assert_eq!(
            choose_surface_format(&[Bgra8UnormSrgb, Bgra8Unorm, Rgba8Unorm]),
            Some(Bgra8Unorm)
        );
        assert_eq!(
            choose_surface_format(&[Rgba8UnormSrgb, Rgba8Unorm]),
            Some(Rgba8Unorm)
        );
        assert_eq!(
            choose_surface_format(&[Bgra8UnormSrgb, Rgb10a2Unorm, Rgba16Float]),
            Some(Rgb10a2Unorm)
        );
        // Only sRGB present: then simply the first format.
        assert_eq!(
            choose_surface_format(&[Rgba8UnormSrgb, Bgra8UnormSrgb]),
            Some(Rgba8UnormSrgb)
        );
        assert_eq!(choose_surface_format(&[]), None);
    }

    #[test]
    fn frame_action_per_surface_status() {
        use SurfaceStatus::*;
        assert_eq!(frame_action(Success), FrameAction::Draw);
        assert_eq!(frame_action(Suboptimal), FrameAction::Draw);
        assert_eq!(frame_action(Outdated), FrameAction::Reconfigure);
        assert_eq!(frame_action(Lost), FrameAction::Recreate);
        assert_eq!(frame_action(Timeout), FrameAction::Skip);
        assert_eq!(frame_action(Occluded), FrameAction::Skip);
        assert_eq!(frame_action(Validation), FrameAction::Skip);
    }

    #[test]
    fn skip_log_levels() {
        use SurfaceStatus::*;
        assert_eq!(skip_log_level(Timeout), Some(log::Level::Warn));
        assert_eq!(skip_log_level(Validation), Some(log::Level::Warn));
        assert_eq!(skip_log_level(Occluded), Some(log::Level::Debug));
        assert_eq!(skip_log_level(Success), None);
    }
}
