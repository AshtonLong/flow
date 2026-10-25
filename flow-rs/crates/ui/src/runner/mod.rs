//! The window's event loop: winit and egui-winit for the window and input,
//! [`crate::render`] to paint, and a GDI DIB section to present. This
//! replaces eframe, whose renderers both need a GPU context (60–90 MB of
//! driver memory for a settings window).
//!
//! Repaints happen only when egui asks (input, `request_repaint`, timers),
//! paced to the monitor's refresh rate, so an idle window uses no CPU.

mod gdi;

use std::sync::Arc;
use std::time::{Duration, Instant};

use egui::{Color32, ViewportBuilder, ViewportCommand, ViewportId, ViewportInfo};
use egui_winit::winit::application::ApplicationHandler;
use egui_winit::winit::event::{StartCause, WindowEvent};
use egui_winit::winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop, EventLoopProxy};
use egui_winit::winit::window::{Window, WindowId};

use crate::render::{self, Renderer, Target};

/// What the event loop drives.
pub(crate) trait App {
    /// Builds the frame.
    fn ui(&mut self, ui: &mut egui::Ui);
    /// Sees (and may rewrite) the input before egui does.
    fn raw_input_hook(&mut self, _ctx: &egui::Context, _raw_input: &mut egui::RawInput) {}
    /// The colour the frame is cleared to (premultiplied).
    fn clear_color(&self) -> Color32;
}

/// Creates the app once the window exists; gets the window's raw HWND (0 off Windows).
pub(crate) type AppCreator<A> = Box<dyn FnOnce(&egui::Context, isize) -> A>;

#[derive(Debug)]
enum UserEvent {
    Repaint { when: Instant, pass: u64 },
    AccessKit(egui_winit::accesskit_winit::Event),
}

impl From<egui_winit::accesskit_winit::Event> for UserEvent {
    fn from(event: egui_winit::accesskit_winit::Event) -> Self {
        Self::AccessKit(event)
    }
}

struct Running<A> {
    window: Arc<Window>,
    egui: egui_winit::State,
    app: A,
    surface: gdi::Surface,
    info: ViewportInfo,
    shown: bool,
}

struct Runner<A> {
    ctx: egui::Context,
    builder: ViewportBuilder,
    create: Option<AppCreator<A>>,
    proxy: EventLoopProxy<UserEvent>,
    running: Option<Running<A>>,
    renderer: Renderer,
    start: Instant,
    /// When the next frame is due, if one was asked for.
    repaint_at: Option<Instant>,
    last_frame: Option<Instant>,
    /// The shortest time between frames (the monitor's refresh period).
    frame_interval: Duration,
    stats: Option<Stats>,
    error: Option<String>,
    /// We asked for the coming `RedrawRequested` (else Windows did, and wants every pixel).
    redraw_requested: bool,
    full_present: bool,
}

/// Frame timings, printed every 120 frames when `FLOW_FRAME_STATS` is set.
#[derive(Default)]
struct Stats {
    frames: u32,
    ui: Duration,
    raster: Duration,
    present: Duration,
    worst: Duration,
}

impl Stats {
    fn add(&mut self, ui: Duration, raster: Duration, present: Duration) {
        self.frames += 1;
        self.ui += ui;
        self.raster += raster;
        self.present += present;
        self.worst = self.worst.max(ui + raster + present);
        if self.frames == 120 {
            let ms = |d: Duration| d.as_secs_f64() * 1000.0 / 120.0;
            eprintln!(
                "frame stats: ui+tessellate {:.2} ms, raster {:.2} ms, present {:.2} ms, worst {:.2} ms",
                ms(self.ui),
                ms(self.raster),
                ms(self.present),
                self.worst.as_secs_f64() * 1000.0
            );
            *self = Self::default();
        }
    }
}

/// Runs `builder`'s window until it closes.
pub(crate) fn run<A: App + 'static>(
    builder: ViewportBuilder,
    paced: bool,
    create: AppCreator<A>,
) -> Result<(), String> {
    let event_loop = EventLoop::<UserEvent>::with_user_event().build().map_err(|e| e.to_string())?;
    let ctx = egui::Context::default();
    let proxy = event_loop.create_proxy();
    {
        let proxy = proxy.clone();
        ctx.set_request_repaint_callback(move |info| {
            let _ = proxy.send_event(UserEvent::Repaint {
                when: Instant::now() + info.delay,
                pass: info.current_cumulative_pass_nr,
            });
        });
    }
    let mut runner = Runner {
        ctx,
        builder: builder.with_visible(false),
        create: Some(create),
        proxy,
        running: None,
        renderer: Renderer::new(),
        start: Instant::now(),
        repaint_at: None,
        last_frame: None,
        frame_interval: if paced { Duration::from_micros(16_667) } else { Duration::ZERO },
        stats: std::env::var_os("FLOW_FRAME_STATS").map(|_| Stats::default()),
        error: None,
        redraw_requested: false,
        full_present: false,
    };
    event_loop.run_app(&mut runner).map_err(|e| e.to_string())?;
    match runner.error {
        Some(err) => Err(err),
        None => Ok(()),
    }
}

#[cfg(windows)]
fn hwnd_of(window: &Window) -> isize {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    match window.window_handle().map(|h| h.as_raw()) {
        Ok(RawWindowHandle::Win32(handle)) => handle.hwnd.get(),
        _ => 0,
    }
}

#[cfg(not(windows))]
fn hwnd_of(_window: &Window) -> isize {
    0
}

impl<A: App> Runner<A> {
    fn create_window(&mut self, event_loop: &ActiveEventLoop) -> Result<(), String> {
        let attributes = egui_winit::create_winit_window_attributes(&self.ctx, self.builder.clone());
        let window = Arc::new(event_loop.create_window(attributes).map_err(|e| e.to_string())?);
        egui_winit::apply_viewport_builder_to_window(&self.ctx, &window, &self.builder);
        // Centre on the monitor's work area, as eframe's `centered` did.
        if let Some(monitor) = window.current_monitor() {
            let (m_pos, m_size) = (monitor.position(), monitor.size());
            let size = window.outer_size();
            let x = m_pos.x + (m_size.width as i32 - size.width as i32) / 2;
            let y = m_pos.y + (m_size.height as i32 - size.height as i32) / 2;
            window.set_outer_position(egui_winit::winit::dpi::PhysicalPosition::new(x.max(m_pos.x), y.max(m_pos.y)));
        }
        if let Some(refresh) = window.current_monitor().and_then(|m| m.refresh_rate_millihertz())
            && !self.frame_interval.is_zero()
            && refresh > 0
        {
            self.frame_interval = Duration::from_secs_f64(1000.0 / f64::from(refresh));
        }
        let mut egui = egui_winit::State::new(
            self.ctx.clone(),
            ViewportId::ROOT,
            event_loop,
            Some(window.scale_factor() as f32),
            window.theme(),
            Some(8192),
        );
        egui.init_accesskit(event_loop, &window, self.proxy.clone());
        let hwnd = hwnd_of(&window);
        let surface = gdi::Surface::new(hwnd).ok_or("could not create the drawing surface")?;
        let mut info = ViewportInfo::default();
        egui_winit::update_viewport_info(&mut info, &self.ctx, &window, true);
        let create = self.create.take().ok_or("the app was already created")?;
        let app = create(&self.ctx, hwnd);
        self.running = Some(Running { window, egui, app, surface, info, shown: false });
        self.repaint_at = Some(Instant::now());
        Ok(())
    }

    fn paint(&mut self, event_loop: &ActiveEventLoop) {
        let Some(running) = self.running.as_mut() else { return };
        let Running { window, egui, app, surface, info, shown } = running;
        let frame_start = Instant::now();
        self.last_frame = Some(frame_start);

        egui_winit::update_viewport_info(info, &self.ctx, window, false);
        let mut raw_input = egui.take_egui_input(window);
        raw_input.time = Some(self.start.elapsed().as_secs_f64());
        raw_input.viewports = std::iter::once((ViewportId::ROOT, info.clone())).collect();
        app.raw_input_hook(&self.ctx, &mut raw_input);
        let output = self.ctx.run_ui(raw_input, |ui| app.ui(ui));
        info.events.clear();
        egui.handle_platform_output_with_event_loop(window, event_loop, output.platform_output);

        let size = window.inner_size();
        let (width, height) = (size.width as usize, size.height as usize);
        let minimized = window.is_minimized().unwrap_or(false);
        let mut actions = Vec::new();
        let mut close = false;
        for (id, viewport) in output.viewport_output {
            if id != ViewportId::ROOT {
                continue;
            }
            close |= viewport.commands.contains(&ViewportCommand::Close);
            egui_winit::process_viewport_commands(&self.ctx, info, viewport.commands, window, &mut actions);
        }

        let primitives = self.ctx.tessellate(output.shapes, output.pixels_per_point);
        let ui_done = Instant::now();
        self.renderer.set_textures(&output.textures_delta);
        let mut raster = Duration::ZERO;
        let mut present = Duration::ZERO;
        let resized = if minimized { None } else { surface.resize(width, height) };
        if let Some(new_buffer) = resized {
            if new_buffer {
                self.renderer.invalidate();
            }
            let pixels = surface.pixels();
            let mut changed = self.renderer.paint(
                Target { pixels, width, height },
                app.clear_color(),
                &primitives,
                output.pixels_per_point,
            );
            let raster_done = Instant::now();
            raster = raster_done - ui_done;
            // Windows asking for a paint (not us) gets the whole frame.
            if std::mem::take(&mut self.full_present) || new_buffer {
                changed = 0..height;
            }
            surface.present(changed);
            present = raster_done.elapsed();
            for action in actions {
                if let egui_winit::ActionRequested::Screenshot(callback) = action {
                    let pixels = surface.pixels();
                    let image =
                        egui::ColorImage::new([width, height], pixels.iter().map(|&p| render::unpack(p)).collect());
                    callback.complete(Arc::new(image));
                }
            }
        }
        self.renderer.free_textures(output.textures_delta);
        if let Some(stats) = self.stats.as_mut() {
            stats.add(ui_done - frame_start, raster, present);
        }
        if !*shown {
            *shown = true;
            // Shown once there is something to show (no white flash), and brought
            // forward as Electron's `win.show()` does.
            window.set_visible(true);
            window.focus_window();
        }
        if close || info.events.iter().any(|e| matches!(e, egui::ViewportEvent::Close)) {
            event_loop.exit();
        }
    }

    fn schedule(&mut self, when: Instant) {
        self.repaint_at = Some(self.repaint_at.map_or(when, |at| at.min(when)));
    }
}

impl<A: App> ApplicationHandler<UserEvent> for Runner<A> {
    fn new_events(&mut self, _event_loop: &ActiveEventLoop, _cause: StartCause) {}

    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.running.is_none()
            && let Err(err) = self.create_window(event_loop)
        {
            self.error = Some(err);
            event_loop.exit();
        }
    }

    fn user_event(&mut self, _event_loop: &ActiveEventLoop, event: UserEvent) {
        match event {
            UserEvent::Repaint { when, pass } => {
                let current = self.ctx.cumulative_pass_nr_for(ViewportId::ROOT);
                if current == pass || current == pass + 1 {
                    self.schedule(when);
                }
            }
            UserEvent::AccessKit(event) => {
                let Some(running) = self.running.as_mut() else { return };
                match event.window_event {
                    egui_winit::accesskit_winit::WindowEvent::InitialTreeRequested => {
                        self.ctx.enable_accesskit();
                        self.schedule(Instant::now());
                    }
                    egui_winit::accesskit_winit::WindowEvent::ActionRequested(request) => {
                        running.egui.on_accesskit_action_request(request);
                        self.schedule(Instant::now());
                    }
                    egui_winit::accesskit_winit::WindowEvent::AccessibilityDeactivated => self.ctx.disable_accesskit(),
                }
            }
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::RedrawRequested => {
                self.repaint_at = None;
                self.full_present |= !std::mem::take(&mut self.redraw_requested);
                self.paint(event_loop);
            }
            WindowEvent::CloseRequested | WindowEvent::Destroyed => event_loop.exit(),
            event => {
                let Some(running) = self.running.as_mut() else { return };
                let response = running.egui.on_window_event(&running.window, &event);
                if matches!(event, WindowEvent::Resized(_) | WindowEvent::ScaleFactorChanged { .. }) {
                    // Live resizing runs a modal loop; paint straight away so the frame follows the edge.
                    running.window.request_redraw();
                    self.redraw_requested = true;
                } else if response.repaint {
                    self.schedule(Instant::now());
                }
            }
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        let Some(running) = self.running.as_ref() else {
            event_loop.set_control_flow(ControlFlow::Wait);
            return;
        };
        let Some(at) = self.repaint_at else {
            event_loop.set_control_flow(ControlFlow::Wait);
            return;
        };
        // Not more often than the monitor refreshes.
        let at = match self.last_frame {
            Some(last) => at.max(last + self.frame_interval),
            None => at,
        };
        if at > Instant::now() {
            event_loop.set_control_flow(ControlFlow::WaitUntil(at));
            return;
        }
        self.repaint_at = None;
        event_loop.set_control_flow(ControlFlow::Wait);
        // Hidden and minimised windows get no WM_PAINT: run the frame here.
        if !running.shown || running.window.is_minimized().unwrap_or(false) {
            self.paint(event_loop);
        } else {
            running.window.request_redraw();
            self.redraw_requested = true;
        }
    }
}
