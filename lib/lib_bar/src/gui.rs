//! The bar window: sicompass-ui's window, fonts and colours, with a loop of
//! its own.
//!
//! Not the renderer's `main_loop`, which draws a list and redraws sixty times
//! a second. The bar is one line that changes once a second at most, so this
//! loop sleeps on SDL's event queue (a quarter of a second at a time, to look
//! at the clock, the channel, the status threads and the settings files) and
//! draws only when something shown changed.
//!
//! The colours are the focused row's: its highlight as the background and the
//! text colour on it, so the bar reads as the list's current line.

use std::path::PathBuf;
use std::sync::mpsc::{Receiver, channel};
use std::time::Duration;

use desicompass_bar_protocol::clock::{LocalTime, bar_text};
use desicompass_bar_protocol::settings::BarSettingsFile;
use desicompass_bar_protocol::{Edge, FromBar, ToBar, status};
use sdl3::event::{Event, WindowEvent};
use sicompass_ui::accessibility::{
    self, AccessibilitySettings, KEY_FONT_SCALE, KEY_LANGUAGE, SharedAccessibility,
};
use sicompass_ui::app_state::{AppConfig, AppState, PaletteTheme};
use sicompass_ui::render;
use sicompass_ui::text::{FONT_SIZE_PT, FontRenderer, TEXT_PADDING};

use crate::icons::IconCache;
use crate::ipc::Ipc;
use crate::layout::{ItemSize, Metrics, layout};
use crate::model::{Glyph, Model, Update, items};

/// How long the loop sleeps on the event queue between looks at everything
/// else.
const TICK: Duration = Duration::from_millis(250);

/// How many lines of text tall the bar is. Its one line sits in the middle.
pub const LINES: f32 = 1.7;

/// What the bar needs from the command line.
pub struct Options {
    pub ipc: Option<Ipc>,
    /// No compositor: a window of its own, for development.
    pub standalone: bool,
}

/// `0xRRGGBBAA` as the clear colour's floats.
fn rgba_f32(c: u32) -> [f32; 4] {
    let [r, g, b, a] = c.to_be_bytes();
    [r, g, b, a].map(|v| f32::from(v) / 255.0)
}

/// The bar's height in logical pixels, for `place`: [`LINES`] lines, in the
/// compositor's units rather than the display's pixels.
pub fn logical_height(line_height_px: f32, pixel_density: f32) -> u32 {
    let density = if pixel_density.is_finite() && pixel_density > 0.0 {
        pixel_density
    } else {
        1.0
    };
    (LINES * line_height_px / density).ceil().max(1.0) as u32
}

struct Bar {
    app: AppState,
    ipc: Option<Ipc>,
    access: SharedAccessibility,
    settings: BarSettingsFile,
    status_path: Option<PathBuf>,
    updates: Receiver<Update>,
    model: Model,
    icons: IconCache,
    font_scale: f32,
    language: String,
    /// What was last sent in `place`.
    placed: Option<(Edge, u32)>,
    /// The clock as last drawn.
    clock: String,
    dirty: bool,
    standalone: bool,
}

/// Build the window and run the bar until the compositor closes the channel
/// (or, standalone, until the window is closed).
pub fn run(opts: Options) -> Result<(), String> {
    let access = SharedAccessibility::open_session();
    let effective = access.effective();
    let font_scale = accessibility::font_scale_value(effective.font_scale.as_deref());

    let cfg = AppConfig {
        title: "desicompass bar".to_owned(),
        app_name: "Desicompass bar".to_owned(),
        app_id: "desicompass-bar".to_owned(),
        vulkan_app_name: "desicompass-bar".to_owned(),
        // The compositor gives it the output's width and the height it asks
        // for. This is what it asks for when run alone.
        width: 1280,
        height: 64,
        custom_titlebar: false,
        maximized: false,
        fullscreen: false,
        window_icon: false,
        font_scale,
    };
    let mut app =
        AppState::init_stack(&cfg).map_err(|e| format!("could not start the bar window: {e}"))?;
    // Nothing in the bar takes the keyboard, so there is nothing for a screen
    // reader to read in it: the superkey's Status section says what it shows.
    app.accesskit_adapter = None;
    app.window.set_bordered(false);

    let (tx, updates) = channel();
    crate::status::start(tx);

    let mut bar = Bar {
        app,
        ipc: opts.ipc,
        access,
        settings: BarSettingsFile::open_default(),
        status_path: status::default_path(),
        updates,
        model: Model::default(),
        icons: IconCache::new(),
        font_scale,
        language: effective.language.clone().unwrap_or_else(|| "en-US".into()),
        placed: None,
        clock: String::new(),
        dirty: true,
        standalone: opts.standalone,
    };
    bar.apply_display(&effective);
    bar.place();
    bar.app.window.show();
    bar.run();
    if let Some(p) = &bar.status_path {
        let _ = std::fs::remove_file(p);
    }
    Ok(())
}

impl Bar {
    fn run(&mut self) {
        while self.app.running {
            if self.ipc.as_ref().is_some_and(Ipc::is_closed) {
                tracing::info!("the compositor is gone; the bar ends");
                break;
            }
            let first = self.app.event_pump.wait_event_timeout(TICK);
            let events: Vec<Event> = first
                .into_iter()
                .chain(self.app.event_pump.poll_iter())
                .collect();
            for event in events {
                self.on_event(event);
            }
            self.frame();
        }
        // SAFETY: waiting for the GPU before the window goes.
        unsafe {
            let _ = self.app.device.device_wait_idle();
        }
    }

    fn on_event(&mut self, event: Event) {
        match event {
            Event::Quit { .. } => self.app.running = false,
            Event::Window {
                win_event,
                window_id,
                ..
            } if window_id == self.app.window.id() => match win_event {
                WindowEvent::Resized(..)
                | WindowEvent::PixelSizeChanged(..)
                | WindowEvent::Exposed => {
                    self.app.framebuffer_resized = true;
                    self.dirty = true;
                }
                WindowEvent::DisplayChanged(..) => {
                    self.rebuild_font();
                    self.dirty = true;
                }
                _ => {}
            },
            _ => {}
        }
    }

    /// Everything but the events, once per wake-up.
    fn frame(&mut self) {
        if let Some(ipc) = &self.ipc {
            for msg in ipc.drain() {
                match msg {
                    ToBar::SayTime => crate::speech::say_time(&self.language),
                }
            }
        }

        let mut status_changed = false;
        while let Ok(u) = self.updates.try_recv() {
            status_changed |= self.model.apply(u);
        }
        if status_changed {
            self.dirty = true;
            if let Some(p) = &self.status_path
                && let Err(e) = status::write(p, &self.model.snapshot)
            {
                tracing::warn!("could not write {}: {e}", p.display());
            }
        }

        let changed = self.access.poll();
        if !changed.is_empty() {
            let effective = self.access.effective();
            for (key, value) in &changed {
                match *key {
                    KEY_FONT_SCALE => {
                        self.font_scale = accessibility::font_scale_value(Some(value));
                        self.rebuild_font();
                    }
                    KEY_LANGUAGE => self.language = value.clone(),
                    _ => {}
                }
            }
            self.apply_display(&effective);
            self.dirty = true;
        }
        if self.settings.poll() {
            self.dirty = true;
        }
        self.place();

        let clock = bar_text(
            &LocalTime::now(),
            &self.language,
            self.settings.get().seconds,
        );
        if clock != self.clock {
            self.clock = clock;
            self.dirty = true;
        }

        if self.dirty {
            self.draw();
        }
    }

    /// The colour scheme and shoulder-surfing protection, as the session has
    /// them.
    fn apply_display(&mut self, s: &AccessibilitySettings) {
        let r = &mut self.app.renderer;
        r.palette_theme = if s.color_scheme.as_deref() == Some("light") {
            PaletteTheme::Light
        } else {
            PaletteTheme::Dark
        };
        r.privacy_blank = s.shoulder_surfing_protection == Some(true);
    }

    fn content_scale(&self) -> f32 {
        self.app
            .window
            .get_display()
            .ok()
            .and_then(|d| d.get_content_scale().ok())
            .unwrap_or(1.0)
    }

    /// A new font renderer for the current font scale and display, as the
    /// renderer's own loop does on a font-scale change.
    fn rebuild_font(&mut self) {
        let dpi = (96.0_f32 * self.content_scale() * self.font_scale)
            .round()
            .max(48.0) as u32;
        let app = &mut self.app;
        // SAFETY: the device is idle before the old renderer is destroyed,
        // and the new one is built from the same live device.
        unsafe {
            let _ = app.device.device_wait_idle();
            if let Some(old) = app.font_renderer.take() {
                old.destroy(&app.device);
            }
            match FontRenderer::new(
                &app.device,
                &app.instance,
                app.physical_device,
                app.command_pool,
                app.graphics_queue,
                app.render_pass,
                dpi,
            ) {
                Ok(fr) => app.font_renderer = Some(fr),
                Err(e) => tracing::error!("could not rebuild the font: {e}"),
            }
        }
    }

    /// The height of a line of text, in the display's pixels.
    fn line_height(&self) -> Option<f32> {
        let fr = self.app.font_renderer.as_ref()?;
        let scale = fr.get_text_scale(FONT_SIZE_PT);
        Some(fr.get_line_height(scale, TEXT_PADDING))
    }

    /// Tell the compositor where the bar goes and how tall it is, when that
    /// changed. Alone, size the window itself.
    fn place(&mut self) {
        let Some(lh) = self.line_height() else {
            return;
        };
        let height = logical_height(lh, self.app.window.pixel_density());
        let edge = self.settings.get().position;
        if self.placed == Some((edge, height)) {
            return;
        }
        self.placed = Some((edge, height));
        self.dirty = true;
        match &self.ipc {
            Some(ipc) => ipc.send(&FromBar::Place { edge, height }),
            None if self.standalone => {
                let width = self.app.window.size().0.max(1);
                let _ = self.app.window.set_size(width, height);
            }
            None => {}
        }
    }

    fn draw(&mut self) {
        if self.app.framebuffer_resized {
            self.app.framebuffer_resized = false;
            if !render::rebuild_swapchain(&mut self.app) {
                // No area yet: try again on the next wake-up.
                return;
            }
        }
        self.dirty = false;

        let palette = *self.app.renderer.palette();
        let (bg, fg) = (palette.selected, palette.text);
        let extent = self.app.swapchain_extent;
        let (width, height) = (extent.width as f32, extent.height as f32);
        self.app.clear_color = rgba_f32(if self.app.renderer.privacy_blank {
            palette.background
        } else {
            bg
        });

        let shown = items(&self.model);
        let Some(fr) = self.app.font_renderer.as_mut() else {
            return;
        };
        let scale = fr.get_text_scale(FONT_SIZE_PT);
        let m = Metrics {
            width,
            height,
            line_height: fr.get_line_height(scale, TEXT_PADDING),
            padding: TEXT_PADDING,
            em: fr.get_width_em(scale),
        };
        let baseline = m.line_top() + fr.ascender * scale + TEXT_PADDING;
        let icon = m.icon_size();
        let sizes: Vec<ItemSize> = shown
            .iter()
            .map(|i| ItemSize {
                text_width: i
                    .text
                    .as_deref()
                    .map_or(0.0, |t| fr.measure_text_width(t, scale)),
            })
            .collect();
        let clock_width = fr.measure_text_width(&self.clock, scale);
        let placed = layout(&m, clock_width, &sizes);

        fr.begin_text_rendering();
        fr.prepare_text_for_rendering(&self.clock, placed.clock_x, baseline, scale, fg);
        let mut pictures = Vec::new();
        for (item, at) in shown.iter().zip(&placed.items) {
            if let (Some(t), Some(x)) = (&item.text, at.text_x) {
                fr.prepare_text_for_rendering(t, x, baseline, scale, fg);
            }
            match &item.glyph {
                Glyph::Letter(c) => {
                    let s = c.to_string();
                    let w = fr.measure_text_width(&s, scale);
                    let x = at.icon_x + ((icon - w) / 2.0).round();
                    fr.prepare_text_for_rendering(&s, x, baseline, scale, fg);
                }
                Glyph::Icon(i) => {
                    if let Some(uri) = self.icons.icon(*i, icon as u32, fg) {
                        pictures.push((uri, at.icon_x));
                    }
                }
                Glyph::Picture(n) => {
                    if let Some(p) = self.model.tray.get(*n).and_then(|e| e.picture.as_ref())
                        && let Some(uri) = self.icons.picture(p.width, p.height, &p.rgba)
                    {
                        pictures.push((uri, at.icon_x));
                    }
                }
            }
        }

        if let Some(rr) = self.app.rect_renderer.as_mut() {
            rr.begin_rect_rendering();
            rr.prepare_rectangle(0.0, 0.0, width, height, bg, 0.0);
        }
        if let Some(ir) = self.app.image_renderer.as_mut() {
            ir.begin_image_rendering();
            let top = m.icon_top();
            for (uri, x) in &pictures {
                // SAFETY: the renderer's own device and queue, between frames.
                unsafe { ir.prepare_image(uri, *x, top, icon, icon) };
            }
        }
        render::draw_frame(&mut self.app);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_height_is_1_7_lines_in_the_compositors_pixels() {
        // Rounded up, so the line always fits.
        assert_eq!(logical_height(40.0, 1.0), 68);
        assert_eq!(logical_height(36.0, 1.0), 62);
        // A 2x display: 80 pixels of line are 40 logical.
        assert_eq!(logical_height(80.0, 2.0), 68);
        // A nonsense density counts as 1.
        assert_eq!(logical_height(20.0, 0.0), 34);
        assert_eq!(logical_height(20.0, f32::NAN), 34);
        assert_eq!(logical_height(0.0, 1.0), 1);
    }

    #[test]
    fn colours_become_floats() {
        assert_eq!(rgba_f32(0xFF000080), [1.0, 0.0, 0.0, 128.0 / 255.0]);
    }
}
