//! The Audio output dialog over the player sheet (upstream `AudioOutputSheet`): a bottom sheet
//! listing the output devices, with the volume and a shortcut to the audio settings.
use super::*;

const DIALOG_WIDTH: f32 = 640.0;
const SURFACE: Color32 = Color32::from_rgb(28, 28, 28);
const CARD_ROW: f32 = 80.0;
const SLIDE_SECONDS: f64 = 0.22;

fn bold(size: f32) -> FontId {
    FontId::new(size, egui::FontFamily::Name("bold".into()))
}

/// A circle holding a glyph, the lead of every row in the dialog.
fn disc(ui: &egui::Ui, center: Pos2, icon: Icon, bright: bool) {
    ui.painter().circle_filled(
        center,
        22.0,
        widgets::white_alpha(if bright { 46 } else { 22 }),
    );
    widgets::paint(
        ui.painter(),
        icon,
        Rect::from_center_size(center, Vec2::splat(24.0)),
        WHITE,
    );
}

impl YTDugem {
    /// Drawn over the whole sheet while `output_open`.
    pub(super) fn output_dialog(&mut self, ui: &mut egui::Ui, sheet: Rect) {
        let ctx = ui.ctx().clone();
        // The slide starts afresh each time the dialog opens: it was not drawn last pass.
        let (now, pass) = (ctx.input(|i| i.time), ctx.cumulative_pass_nr());
        let key = ui.scope_id().with("output-dialog-open");
        let (opened, last) =
            ctx.data(|d| d.get_temp::<(f64, u64)>(key))
                .map_or((now, pass), |(o, l)| {
                    if pass.saturating_sub(l) > 2 {
                        (now, pass)
                    } else {
                        (o, pass)
                    }
                });
        ctx.data_mut(|d| d.insert_temp(key, (opened, last)));
        let linear = ((now - opened) / SLIDE_SECONDS).clamp(0.0, 1.0) as f32;
        let t = 1.0 - (1.0 - linear).powi(3);
        if linear < 1.0 {
            ctx.request_repaint();
        }

        // Rows: system default plus every device (or a reading line), then the volume and pipeline card.
        let rows = 1 + self.audio_devices.len() + usize::from(self.audio_devices_loading);
        let want = 64.0 + rows as f32 * (CARD_ROW + 8.0) + 16.0 + 144.0 + 24.0;
        let height = want.min(sheet.height() * 0.7);
        let width = DIALOG_WIDTH.min(sheet.width() - 32.0).max(200.0);
        let top = sheet.bottom() - height * t;
        let dialog = Rect::from_min_size(
            Pos2::new(sheet.center().x - width / 2.0, top),
            Vec2::new(width, height),
        );

        // The scrim closes the dialog; it and the dialog also swallow drags and clicks, so
        // nothing reaches the sheet underneath.
        ui.painter()
            .rect_filled(sheet, 0.0, Color32::from_black_alpha((102.0 * t) as u8));
        let scrim = ui.interact(sheet, key.with("scrim"), egui::Sense::click_and_drag());
        ui.interact(dialog, key.with("dialog"), egui::Sense::click_and_drag());
        if scrim.clicked()
            && ui.input(|i| {
                i.pointer
                    .interact_pos()
                    .is_some_and(|p| !dialog.contains(p))
            })
        {
            self.output_open = false;
            return;
        }

        ui.painter().rect_filled(
            dialog,
            egui::CornerRadius {
                nw: 24,
                ne: 24,
                sw: 0,
                se: 0,
            },
            SURFACE,
        );
        ui.painter().rect_filled(
            Rect::from_center_size(
                Pos2::new(dialog.center().x, dialog.top() + 12.0),
                Vec2::new(36.0, 4.0),
            ),
            2.0,
            widgets::white_alpha(90),
        );
        fit_text(
            ui.painter(),
            Pos2::new(dialog.left() + 24.0, dialog.top() + 44.0),
            egui::Align2::LEFT_CENTER,
            "Audio output",
            bold(22.0),
            WHITE,
            dialog.width() - 48.0,
        );

        let content = Rect::from_min_max(
            Pos2::new(dialog.left() + 16.0, dialog.top() + 68.0),
            Pos2::new(dialog.right() - 16.0, dialog.bottom() - 12.0),
        );
        let mut choice = None;
        let mut settings = false;
        region(ui, content, |u| {
            egui::ScrollArea::vertical()
                .id_salt("output-dialog-scroll")
                .auto_shrink([false, false])
                .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden)
                .show(u, |u| {
                    u.spacing_mut().item_spacing.y = 8.0;
                    let width = u.available_width();
                    let selected = self.library.audio.output_device.clone();
                    if self.device_row(u, width, "System default", selected.is_empty()) {
                        choice = Some(String::new());
                    }
                    for (id, name) in self.audio_devices.clone() {
                        if self.device_row(u, width, &name, selected == id) {
                            choice = Some(id);
                        }
                    }
                    if self.audio_devices_loading {
                        let (r, _) =
                            u.allocate_exact_size(Vec2::new(width, 56.0), egui::Sense::hover());
                        egui::Spinner::new().size(20.0).color(WHITE).paint_at(
                            u,
                            Rect::from_center_size(
                                Pos2::new(r.left() + 36.0, r.center().y),
                                Vec2::splat(20.0),
                            ),
                        );
                        u.painter().text(
                            Pos2::new(r.left() + 66.0, r.center().y),
                            egui::Align2::LEFT_CENTER,
                            "Reading devices…",
                            FontId::proportional(15.0),
                            MUTED,
                        );
                    }
                    u.add_space(8.0);
                    settings = self.volume_and_pipeline(u, width);
                    u.add_space(8.0);
                });
        });
        if let Some(id) = choice {
            self.choose_output(id);
        }
        if settings {
            self.output_open = false;
            self.navigate(Page::Settings);
        }
    }

    /// A device card; returns whether it was clicked.
    fn device_row(&self, ui: &mut egui::Ui, width: f32, name: &str, selected: bool) -> bool {
        let (rect, response) =
            ui.allocate_exact_size(Vec2::new(width, CARD_ROW), egui::Sense::click());
        let response = response.on_hover_cursor(egui::CursorIcon::PointingHand);
        let alpha = if selected {
            26
        } else if response.hovered() {
            14
        } else {
            8
        };
        ui.painter()
            .rect_filled(rect, 20.0, widgets::white_alpha(alpha));
        disc(
            ui,
            Pos2::new(rect.left() + 16.0 + 22.0, rect.center().y),
            Icon::Speaker,
            selected,
        );
        let x = rect.left() + 16.0 + 44.0 + 16.0;
        let max = rect.right() - x - 56.0;
        let dy = if selected { 11.0 } else { 0.0 };
        fit_text(
            ui.painter(),
            Pos2::new(x, rect.center().y - dy),
            egui::Align2::LEFT_CENTER,
            name,
            bold(16.0),
            WHITE,
            max,
        );
        if selected {
            fit_text(
                ui.painter(),
                Pos2::new(x, rect.center().y + 12.0),
                egui::Align2::LEFT_CENTER,
                "Playing here",
                FontId::proportional(14.0),
                MUTED,
                max,
            );
            widgets::paint(
                ui.painter(),
                Icon::Check,
                Rect::from_center_size(
                    Pos2::new(rect.right() - 32.0, rect.center().y),
                    Vec2::splat(24.0),
                ),
                WHITE,
            );
        }
        response.clicked()
    }

    /// The volume row and the "Audio Pipeline" row in one card; true when the latter is clicked.
    fn volume_and_pipeline(&mut self, ui: &mut egui::Ui, width: f32) -> bool {
        let (card, _) = ui.allocate_exact_size(Vec2::new(width, 144.0), egui::Sense::hover());
        ui.painter()
            .rect_filled(card, 20.0, widgets::white_alpha(8));
        let volume = Rect::from_min_size(card.min, Vec2::new(width, 64.0));
        region(ui, volume, |u| {
            u.horizontal_centered(|u| {
                u.add_space(20.0);
                u.spacing_mut().item_spacing.x = 14.0;
                let (r, _) = u.allocate_exact_size(Vec2::splat(24.0), egui::Sense::hover());
                widgets::paint(u.painter(), Icon::Volume, r, WHITE);
                self.volume_slider(u, (width - 100.0).max(40.0), "output-volume");
            });
        });
        let pipeline = Rect::from_min_max(Pos2::new(card.left(), volume.bottom()), card.max);
        let response = ui
            .interact(
                pipeline,
                ui.scope_id().with("output-pipeline"),
                egui::Sense::click(),
            )
            .on_hover_cursor(egui::CursorIcon::PointingHand);
        if response.hovered() {
            ui.painter().rect_filled(
                pipeline,
                egui::CornerRadius {
                    nw: 0,
                    ne: 0,
                    sw: 20,
                    se: 20,
                },
                widgets::white_alpha(8),
            );
        }
        disc(
            ui,
            Pos2::new(pipeline.left() + 38.0, pipeline.center().y),
            Icon::Waveform,
            false,
        );
        let x = pipeline.left() + 16.0 + 44.0 + 16.0;
        let max = pipeline.right() - x - 48.0;
        fit_text(
            ui.painter(),
            Pos2::new(x, pipeline.center().y - 11.0),
            egui::Align2::LEFT_CENTER,
            "Audio Pipeline",
            bold(16.0),
            WHITE,
            max,
        );
        let backend = self.player.audio_backend();
        let subtitle = if backend.is_empty() {
            self.output_name()
        } else {
            backend.to_owned()
        };
        fit_text(
            ui.painter(),
            Pos2::new(x, pipeline.center().y + 12.0),
            egui::Align2::LEFT_CENTER,
            &subtitle,
            FontId::proportional(14.0),
            MUTED,
            max,
        );
        widgets::paint(
            ui.painter(),
            Icon::CaretRight,
            Rect::from_center_size(
                Pos2::new(pipeline.right() - 28.0, pipeline.center().y),
                Vec2::splat(20.0),
            ),
            widgets::white_alpha(180),
        );
        response.clicked()
    }

    /// Picks an output and moves the running song onto it. WASAPI exclusive mode only works on
    /// a `wasapi/` device (as `Player::load_with_audio` enforces), so the pick is refused then.
    fn choose_output(&mut self, id: String) {
        if cfg!(windows)
            && self.library.audio.wasapi_exclusive
            && !id.is_empty()
            && !id.to_ascii_lowercase().starts_with("wasapi/")
        {
            self.message = "WASAPI exclusive mode needs a wasapi/ device; pick one or turn exclusive mode off in Settings".into();
            return;
        }
        if self.library.audio.output_device == id {
            return;
        }
        self.library.audio.output_device = id.clone();
        self.save();
        let result = self.player.set_audio_device(&id);
        self.report(result);
    }
}
