//! The sidebar search box's dropdown (a YTDugem addition): recent searches while it is empty,
//! then matching recent searches and YouTube Music's typeahead suggestions while typing.
use super::*;

/// Rows shown at most.
const LIMIT: usize = 10;
/// Recent searches shown above the suggestions while typing.
const RECENT_WHILE_TYPING: usize = 3;
const ROW: f32 = 32.0;

#[derive(Clone, Debug, PartialEq)]
pub(super) struct Entry {
    pub text: String,
    /// A remembered search (removable), not a suggestion.
    pub recent: bool,
}

/// What the dropdown lists for `query`: every recent search when it is empty; otherwise the
/// recent searches containing it, then the suggestions not already listed.
pub(super) fn entries(recent: &[String], remote: &[String], query: &str) -> Vec<Entry> {
    let query = query.trim().to_lowercase();
    if query.is_empty() {
        return recent
            .iter()
            .take(LIMIT)
            .map(|text| Entry {
                text: text.clone(),
                recent: true,
            })
            .collect();
    }
    let mut out: Vec<Entry> = recent
        .iter()
        .filter(|r| r.to_lowercase().contains(&query))
        .take(RECENT_WHILE_TYPING)
        .map(|text| Entry {
            text: text.clone(),
            recent: true,
        })
        .collect();
    for text in remote {
        if out.len() >= LIMIT {
            break;
        }
        if !out.iter().any(|e| e.text.eq_ignore_ascii_case(text)) {
            out.push(Entry {
                text: text.clone(),
                recent: false,
            });
        }
    }
    out
}

/// One step of Up/Down through `len` rows; Up from the first row leaves the list.
pub(super) fn step(selected: Option<usize>, len: usize, down: bool) -> Option<usize> {
    if len == 0 {
        return None;
    }
    match (selected, down) {
        (None, true) => Some(0),
        (Some(i), true) => Some((i + 1).min(len - 1)),
        (Some(i), false) => i.checked_sub(1),
        (None, false) => None,
    }
}

#[derive(Default)]
pub(super) struct SearchBox {
    /// The dropdown is showing (the box has focus, or a click on the list is under way).
    shown: bool,
    /// The box had focus last frame; gaining it shows the recent searches.
    focused: bool,
    /// Where the dropdown was drawn last frame, so a click on it keeps it open.
    popup: Option<Rect>,
    /// Suggestions for `asked`.
    remote: Vec<String>,
    selected: Option<usize>,
    /// When the text last changed; suggestions wait for a short pause after typing.
    edited: Option<Instant>,
    /// The query the last suggestion request was sent for.
    asked: String,
    request: u64,
}

pub(super) fn input_id() -> egui::Id {
    egui::Id::unique("search_input")
}

impl YTDugem {
    /// Before the search field is drawn: Up/Down move through the list and Escape closes it.
    /// Returns the entry Enter should search for, if one is chosen.
    pub(super) fn search_box_keys(&mut self, ctx: &egui::Context) -> Option<String> {
        if !ctx.memory(|m| m.has_focus(input_id())) {
            return None;
        }
        let list = self.search_entries();
        let (down, up, escape) = ctx.input_mut(|i| {
            let shown = self.search_box.shown && !list.is_empty();
            (
                shown && i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowDown),
                shown && i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowUp),
                i.consume_key(egui::Modifiers::NONE, egui::Key::Escape),
            )
        });
        let b = &mut self.search_box;
        if down || up {
            b.selected = step(b.selected, list.len(), down);
        }
        if escape {
            if b.shown {
                b.shown = false;
                b.selected = None;
            } else {
                ctx.memory_mut(|m| m.surrender_focus(input_id()));
            }
        }
        b.selected
            .filter(|_| b.shown)
            .and_then(|i| list.get(i))
            .map(|e| e.text.clone())
    }

    /// The text changed: pick nothing and ask for suggestions after a pause.
    pub(super) fn search_box_edited(&mut self) {
        self.search_box.edited = Some(Instant::now());
        self.search_box.selected = None;
        self.search_box.shown = true;
    }

    /// A query was searched (Enter or a click): close the list.
    pub(super) fn search_box_done(&mut self, ctx: &egui::Context) {
        self.search_box.shown = false;
        self.search_box.selected = None;
        ctx.memory_mut(|m| m.surrender_focus(input_id()));
    }

    fn search_entries(&self) -> Vec<Entry> {
        // Suggestions answer the text they were asked for; older ones are not shown.
        let remote: &[String] = if self.search_box.asked == self.query.trim() {
            &self.search_box.remote
        } else {
            &[]
        };
        entries(&self.library.recent_searches, remote, &self.query)
    }

    pub(super) fn search_suggestions_arrived(
        &mut self,
        id: u64,
        query: String,
        result: Result<Vec<String>, String>,
    ) {
        if id == self.search_box.request && query == self.search_box.asked {
            // A failure just shows no suggestions.
            self.search_box.remote = result.unwrap_or_default();
            self.search_box.selected = None;
        }
    }

    fn search_box_debounce(&mut self, ctx: &egui::Context) {
        const PAUSE: Duration = Duration::from_millis(250);
        let Some(edited) = self.search_box.edited else {
            return;
        };
        let waited = edited.elapsed();
        if waited < PAUSE {
            ctx.request_repaint_after(PAUSE - waited);
            return;
        }
        self.search_box.edited = None;
        let query = self.query.trim().to_owned();
        if query == self.search_box.asked {
            return;
        }
        self.search_box.asked = query.clone();
        self.search_box.request += 1;
        self.search_box.remote.clear();
        if query.is_empty() {
            return;
        }
        let (id, tx) = (self.search_box.request, self.tx.clone());
        std::thread::spawn(move || {
            let result = services::search_suggestions(&query).map_err(err);
            let _ = tx.send(Event::SearchSuggestions(id, query, result));
        });
    }

    /// The dropdown under the search box (`anchor`), drawn over the page.
    pub(super) fn search_dropdown(&mut self, ctx: &egui::Context, anchor: Rect, focused: bool) {
        self.search_box_debounce(ctx);
        let pointer_on_list = self.search_box.popup.is_some_and(|r| {
            ctx.input(|i| i.pointer.interact_pos().is_some_and(|p| r.contains(p)))
        });
        if !focused && !pointer_on_list {
            self.search_box.shown = false;
            self.search_box.selected = None;
        } else if focused && ctx.input(|i| i.pointer.any_pressed()) {
            // Clicking back into the box reopens a list closed with Escape.
            self.search_box.shown = true;
        }
        if focused && !self.search_box.focused {
            // Just focused (Ctrl+F, a click): show the recent searches.
            self.search_box.shown = true;
        }
        self.search_box.focused = focused;
        let list = self.search_entries();
        if !self.search_box.shown || list.is_empty() {
            self.search_box.popup = None;
            return;
        }
        let typing = !self.query.trim().is_empty();
        let mut chosen = None;
        let mut forget = None;
        let mut clear = false;
        let area = egui::Area::new(egui::Id::unique("search-dropdown"))
            .order(egui::Order::Foreground)
            .fixed_pos(anchor.left_bottom() + Vec2::new(0.0, 4.0))
            .show(ctx, |ui| {
                egui::Frame::new()
                    .fill(Color32::from_rgb(36, 37, 34))
                    .stroke(Stroke::new(1.0_f32, Color32::from_gray(63)))
                    .corner_radius(7.0)
                    .inner_margin(egui::Margin::same(4))
                    .show(ui, |ui| {
                        let width = anchor.width() - 8.0;
                        ui.set_width(width);
                        ui.spacing_mut().item_spacing.y = 0.0;
                        if !typing {
                            ui.label(RichText::new("  Recent searches").size(11.5).color(MUTED));
                            ui.add_space(4.0);
                        }
                        for (i, entry) in list.iter().enumerate() {
                            let (rect, response) =
                                ui.allocate_exact_size(Vec2::new(width, ROW), egui::Sense::click());
                            let remove = Rect::from_center_size(
                                Pos2::new(rect.right() - 14.0, rect.center().y),
                                Vec2::splat(22.0),
                            );
                            let over_remove = entry.recent
                                && response.hover_pos().is_some_and(|p| remove.contains(p));
                            let lit = self.search_box.selected == Some(i) || response.hovered();
                            if lit {
                                ui.painter()
                                    .rect_filled(rect, 5.0, widgets::white_alpha(14));
                                ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                            }
                            let icon = if entry.recent {
                                Icon::History
                            } else {
                                Icon::Search
                            };
                            widgets::paint(
                                ui.painter(),
                                icon,
                                Rect::from_min_size(
                                    rect.min + Vec2::new(8.0, 8.0),
                                    Vec2::splat(15.0),
                                ),
                                MUTED,
                            );
                            fit_text(
                                ui.painter(),
                                Pos2::new(rect.left() + 32.0, rect.center().y),
                                egui::Align2::LEFT_CENTER,
                                &entry.text,
                                FontId::proportional(13.5),
                                if lit { WHITE } else { Color32::from_gray(200) },
                                width - if entry.recent { 66.0 } else { 40.0 },
                            );
                            if entry.recent && lit {
                                ui.painter().text(
                                    remove.center(),
                                    egui::Align2::CENTER_CENTER,
                                    "×",
                                    FontId::proportional(16.0),
                                    if over_remove { WHITE } else { MUTED },
                                );
                            }
                            let response = if over_remove {
                                response.on_hover_text("Remove from recent searches")
                            } else {
                                response
                            };
                            if response.clicked() {
                                if over_remove {
                                    forget = Some(entry.text.clone());
                                } else {
                                    chosen = Some(entry.text.clone());
                                }
                            }
                        }
                        if !typing {
                            ui.add_space(4.0);
                            ui.horizontal(|ui| {
                                ui.add_space(4.0);
                                let clear_button = ui.add(
                                    egui::Label::new(
                                        RichText::new("Clear recent searches")
                                            .size(11.5)
                                            .color(MUTED),
                                    )
                                    .sense(egui::Sense::click()),
                                );
                                if clear_button.hovered() {
                                    ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                                }
                                clear = clear_button.clicked();
                            });
                            ui.add_space(4.0);
                        }
                    });
            });
        self.search_box.popup = Some(area.response.rect);
        if let Some(text) = forget {
            store::forget_search(&mut self.library.recent_searches, &text);
            self.search_box.selected = None;
            self.save();
            ctx.memory_mut(|m| m.request_focus(input_id()));
        }
        if clear {
            self.clear_recent_searches();
            ctx.memory_mut(|m| m.request_focus(input_id()));
        }
        if let Some(text) = chosen {
            self.query = text;
            self.search_box_done(ctx);
            self.search();
        }
    }

    pub(super) fn clear_recent_searches(&mut self) {
        self.library.recent_searches.clear();
        self.search_box.selected = None;
        self.save();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn texts(list: &[Entry]) -> Vec<(&str, bool)> {
        list.iter().map(|e| (e.text.as_str(), e.recent)).collect()
    }

    #[test]
    fn empty_box_lists_recent_searches() {
        let recent: Vec<String> = (0..15).map(|i| format!("q{i}")).collect();
        let list = entries(&recent, &["ignored".into()], "  ");
        assert_eq!(list.len(), LIMIT);
        assert!(list.iter().all(|e| e.recent));
        assert_eq!(list[0].text, "q0");
        assert!(entries(&[], &[], "").is_empty());
    }

    #[test]
    fn typing_puts_matching_recent_searches_first_without_repeats() {
        let recent = vec![
            "Dewa 19".to_owned(),
            "noah".to_owned(),
            "dewa budjana".to_owned(),
        ];
        let remote = vec![
            "dewa 19".to_owned(),
            "dewa 19 kangen".to_owned(),
            "dewata".to_owned(),
        ];
        let list = entries(&recent, &remote, "DEWA");
        assert_eq!(
            texts(&list),
            [
                ("Dewa 19", true),
                ("dewa budjana", true),
                ("dewa 19 kangen", false),
                ("dewata", false),
            ]
        );
        let many: Vec<String> = (0..20).map(|i| format!("dewa {i}")).collect();
        assert_eq!(entries(&many, &many, "dewa").len(), LIMIT);
        assert_eq!(
            entries(&many, &[], "dewa").len(),
            RECENT_WHILE_TYPING,
            "a few recent searches leave room for suggestions"
        );
    }

    #[test]
    fn arrow_keys_step_within_the_list() {
        assert_eq!(step(None, 0, true), None);
        assert_eq!(step(None, 3, true), Some(0));
        assert_eq!(step(Some(0), 3, true), Some(1));
        assert_eq!(step(Some(2), 3, true), Some(2));
        assert_eq!(step(Some(1), 3, false), Some(0));
        assert_eq!(step(Some(0), 3, false), None);
        assert_eq!(step(None, 3, false), None);
    }
}
