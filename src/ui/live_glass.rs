//! Liquid Glass's player bar is real glass: it blurs and refracts the page behind it, covers and
//! text included. egui_glass draws the page a second time in a twin context, off screen, for the
//! glass to sample. The twin pass gets no input, and of the window's textures it only knows those
//! registered with egui_glass; it loads pictures through the window's loaders.
use super::*;
use eframe::{egui_wgpu, wgpu};

/// How far past the bar the twin pass draws the page: the glass samples this far out for its
/// blur and its edge refraction.
const SAMPLE_MARGIN: f32 = 48.0;

/// The player bar's material: dark glass, so white text stays legible over a pale cover.
pub(super) const BAR: egui_glass::GlassStyle = egui_glass::GlassStyle {
    corner_radius: floating_bar::HEIGHT / 2.0,
    blur: 14.0,
    refraction: 16.0,
    chromatic: 0.1,
    tint: Color32::from_rgba_unmultiplied_const(14, 16, 22, 150),
    ..egui_glass::GlassStyle::dark()
};

/// Liquid Glass's GPU state. `None` without a wgpu renderer, as in the kittest harness.
pub(super) struct GlassGpu {
    rs: egui_wgpu::RenderState,
    live: egui_glass::LiveBackdrop,
    /// Ids of dropped [`NativeTexture`]s. A frame may still draw one after it is dropped, so
    /// they are freed at the start of the next frame, once that one has been rendered.
    retired: Arc<Mutex<Vec<egui::TextureId>>>,
    /// The twin context, once it has drawn: it keeps its own textures of the pictures it shows.
    twin: Option<egui::Context>,
}

/// A texture both the screen and the twin pass can draw. Freed the frame after it is dropped.
pub(super) struct NativeTexture {
    id: egui::TextureId,
    retired: Arc<Mutex<Vec<egui::TextureId>>>,
}

impl Drop for NativeTexture {
    fn drop(&mut self) {
        self.retired
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(self.id);
    }
}

/// The ambient artwork: the texture the screen draws, plus a native copy for the twin pass
/// when there is a GPU to put it on.
pub(super) struct Backdrop {
    pub(super) handle: egui::TextureHandle,
    pub(super) native: Option<NativeTexture>,
}

impl Backdrop {
    /// The texture to draw with in this pass of `ctx`.
    pub(super) fn id(&self, ctx: &egui::Context) -> egui::TextureId {
        match &self.native {
            Some(native) if twin_pass(ctx) => native.id,
            _ => self.handle.id(),
        }
    }
}

impl GlassGpu {
    pub(super) fn new(rs: &egui_wgpu::RenderState) -> Self {
        // One sample: eframe's default `multisampling` of 0.
        egui_glass::init(rs, 1);
        // The twin lays the page out again: it needs the same fonts, "bold" included.
        let live = egui_glass::LiveBackdrop::new(rs, Some(font_definitions()));
        Self {
            rs: rs.clone(),
            live,
            retired: Arc::default(),
            twin: None,
        }
    }

    /// Uploads `image` (premultiplied sRGB, as egui uploads its own) as a native texture.
    pub(super) fn upload(&self, image: &egui::ColorImage) -> NativeTexture {
        let [width, height] = image.size.map(|s| s as u32);
        let size = wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        };
        let texture = self.rs.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("ytdugem-backdrop"),
            size,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8UnormSrgb,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        self.rs.queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            image.as_raw(),
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(4 * width),
                rows_per_image: Some(height),
            },
            size,
        );
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        NativeTexture {
            id: egui_glass::register_native_texture(&self.rs, &view),
            retired: self.retired.clone(),
        }
    }

    /// Frees the native textures dropped during the last frame, which has been rendered by now.
    pub(super) fn free_retired(&self) {
        let ids = std::mem::take(
            &mut *self
                .retired
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
        );
        for id in ids {
            egui_glass::free_native_texture_id(&self.rs, &id);
        }
    }

    /// The twin context, once it has drawn.
    pub(super) fn twin(&self) -> Option<&egui::Context> {
        self.twin.as_ref()
    }

    /// Lets the twin load pictures through the window's loaders, so they share downloads and
    /// decoded images; only the twin's textures are its own.
    fn adopt_twin(&mut self, window: &egui::Context, twin: &egui::Context) {
        if self.twin.is_some() {
            return;
        }
        let loaders = window.loaders();
        for loader in loaders.bytes.lock().iter() {
            twin.add_bytes_loader(loader.clone());
        }
        for loader in loaders.image.lock().iter() {
            twin.add_image_loader(loader.clone());
        }
        self.twin = Some(twin.clone());
    }
}

fn twin_id() -> egui::Id {
    egui::Id::unique("ytdugem-glass-twin")
}

/// Whether `ctx` is drawing the twin pass. Its memory is copied from the window's at the start
/// of every twin pass, so the mark set there never outlives it.
pub(super) fn twin_pass(ctx: &egui::Context) -> bool {
    ctx.data(|d| d.get_temp::<bool>(twin_id())).unwrap_or(false)
}

/// The part of `page` the twin pass draws: around the bar, as far as the glass samples. `None`
/// when no bar is drawn.
fn twin_clip(page: Rect, shown: f32) -> Option<Rect> {
    floating_bar::placed_bar(page, shown).map(|bar| bar.expand(SAMPLE_MARGIN).intersect(page))
}

impl YTDugem {
    /// [`Self::page_layers`], drawn a second time off screen around the player bar in Liquid
    /// Glass, so the bar's glass can show them. Returns the page rect.
    pub(super) fn page_under_glass(&mut self, ui: &mut egui::Ui, shown: f32) -> Rect {
        let live = match &self.glass {
            Some(gpu) if glass(ui) => gpu.live.clone(),
            _ => return self.page_layers(ui),
        };
        let window = ui.ctx().clone();
        let mut page = Rect::NOTHING;
        live.run_with_quality(ui, BG, egui_glass::LiveBackdropQuality::Performance, |ui| {
            // The window's pass comes first and sets `page`; the twin pass follows.
            if ui.ctx() != &window {
                let Some(clip) = twin_clip(page, shown) else {
                    return;
                };
                ui.ctx().data_mut(|d| d.insert_temp(twin_id(), true));
                if let Some(gpu) = &mut self.glass {
                    gpu.adopt_twin(&window, ui.ctx());
                }
                ui.set_clip_rect(clip.intersect(ui.clip_rect()));
            }
            page = self.page_layers(ui);
        });
        page
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_twin_pass_draws_only_the_page_around_the_bar() {
        let page = Rect::from_min_max(Pos2::new(0.0, 64.0), Pos2::new(1220.0, 780.0));
        let bar = floating_bar::placed_bar(page, 0.0).unwrap();
        let clip = twin_clip(page, 0.0).unwrap();
        assert!(clip.contains_rect(bar));
        assert!(page.contains_rect(clip));
        // The blur and refraction margin above the bar; the page's own edge below it.
        assert_eq!(clip.top(), bar.top() - SAMPLE_MARGIN);
        assert_eq!(clip.bottom(), page.bottom());
        assert!(clip.height() < page.height() / 3.0);
        // The bar follows the sheet down, and once it is gone nothing is drawn twice.
        assert_eq!(
            twin_clip(page, 0.5).unwrap().top(),
            floating_bar::placed_bar(page, 0.5).unwrap().top() - SAMPLE_MARGIN
        );
        assert_eq!(twin_clip(page, 1.0), None);
        assert_eq!(
            twin_clip(Rect::from_min_size(Pos2::ZERO, Vec2::splat(100.0)), 0.0),
            None
        );
    }
}
