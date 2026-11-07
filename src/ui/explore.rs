//! Explore (upstream desktop `ExploreScreen` and `MoodGenrePlaylistsScreen`): YouTube Music's
//! mood and genre categories as striped tiles, each opening its playlist shelves. Beyond
//! upstream, the landing page also carries music.youtube.com/explore's New releases, Charts and
//! Moods & genres buttons and the Explore browse page's own shelves.
use super::*;
use services::{MoodGenre, MoodGenreSection};
use std::collections::{HashSet, VecDeque};

/// Gap between mood tiles, in both directions (upstream `MOOD_SPACING`).
const MOOD_SPACING: f32 = 12.0;
/// The narrowest a tile gets before its row drops a column (upstream `MOOD_MIN_CARD_WIDTH`).
const MOOD_MIN_WIDTH: f32 = 220.0;
const MOOD_MAX_COLUMNS: usize = 6;
/// Width over height (upstream `MOOD_CARD_ASPECT`).
const MOOD_ASPECT: f32 = 1.72;
/// How much of the tile the solid stripe covers before the artwork starts.
const MOOD_STRIPE: f32 = 0.28;
const MOOD_RADIUS: u8 = 14;
/// Rows of categories on the landing page; "See all" opens every section.
const MOOD_PREVIEW_ROWS: usize = 3;
/// Category covers resolved at once, as upstream's `Semaphore(4)`.
const ARTWORK_WORKERS: usize = 4;

/// One part of Explore fetched in the background. `Idle` loads on the next frame it is shown.
#[derive(Clone, Default)]
pub(super) enum Loadable<T> {
    #[default]
    Idle,
    Loading,
    Ready(T),
    Failed(String),
}

/// Which Explore page is open. Like upstream's `selectedMoodGenre`, it lives on the Explore
/// page itself, so returning from an opened album lands back on it.
#[derive(Clone, Default, PartialEq)]
pub(super) enum View {
    #[default]
    Main,
    NewReleases,
    Charts,
    Moods,
    Category(MoodGenre),
}

/// The shelf pages Explore fetches.
#[derive(Clone, Copy, PartialEq, Debug)]
pub(super) enum Feed {
    Explore,
    NewReleases,
    Charts,
    Category,
}

#[derive(Default)]
pub(super) struct Explore {
    pub(super) view: View,
    /// Bumped on refresh and account change; answers to older requests are dropped.
    pub(super) request: u64,
    /// Bumped on every category opened, so a slow answer for another category is dropped.
    category_request: u64,
    shelves: Loadable<Vec<HomeShelf>>,
    new_releases: Loadable<Vec<HomeShelf>>,
    charts: Loadable<Vec<HomeShelf>>,
    category: Loadable<Vec<HomeShelf>>,
    moods: Loadable<Vec<MoodGenreSection>>,
    /// Stops the artwork workers of a superseded request.
    artwork_stop: Arc<AtomicBool>,
}

impl Explore {
    /// Forgets everything fetched (keeping the open page), so it reloads when next shown.
    pub(super) fn reset(&mut self) {
        self.artwork_stop.store(true, Ordering::Relaxed);
        services::forget_mood_genre_shelves();
        let view = std::mem::take(&mut self.view);
        *self = Explore {
            view,
            request: self.request + 1,
            category_request: self.category_request + 1,
            ..Default::default()
        };
    }
    pub(super) fn request_for(&self, feed: Feed) -> u64 {
        if feed == Feed::Category {
            self.category_request
        } else {
            self.request
        }
    }
    fn slot(&mut self, feed: Feed) -> &mut Loadable<Vec<HomeShelf>> {
        match feed {
            Feed::Explore => &mut self.shelves,
            Feed::NewReleases => &mut self.new_releases,
            Feed::Charts => &mut self.charts,
            Feed::Category => &mut self.category,
        }
    }
    pub(super) fn shelves_loaded(&mut self, feed: Feed, result: Result<Vec<HomeShelf>, String>) {
        *self.slot(feed) = match result {
            Ok(shelves) if shelves.is_empty() => Loadable::Failed(empty_message(feed).into()),
            Ok(shelves) => Loadable::Ready(shelves),
            Err(e) => Loadable::Failed(e),
        };
    }
    pub(super) fn artwork_loaded(&mut self, key: &str, url: String) {
        if let Loadable::Ready(sections) = &mut self.moods {
            for item in sections.iter_mut().flat_map(|s| s.items.iter_mut()) {
                if item.key() == key {
                    item.thumbnail = Some(url.clone());
                }
            }
        }
    }
}

/// Upstream's empty states: the landing page's, and a category's.
fn empty_message(feed: Feed) -> &'static str {
    if feed == Feed::Category {
        "Nothing to explore here yet"
    } else {
        "Nothing to explore right now"
    }
}

impl YTDugem {
    /// Keeps one scroll position per Explore page, as the other pages have.
    pub(super) fn explore_scroll_key(&self) -> String {
        if self.page != Page::Explore {
            return String::new();
        }
        match &self.explore.view {
            View::Main => "main".into(),
            View::NewReleases => "new".into(),
            View::Charts => "charts".into(),
            View::Moods => "moods".into(),
            View::Category(mood) => mood.key(),
        }
    }
    fn load_explore_shelves(&mut self, feed: Feed) {
        let mood = match &self.explore.view {
            View::Category(mood) => Some(mood.clone()),
            _ => None,
        };
        if feed == Feed::Category && mood.is_none() {
            return;
        }
        let id = self.explore.request_for(feed);
        *self.explore.slot(feed) = Loadable::Loading;
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let result = match (feed, mood) {
                (Feed::Explore, _) => services::explore(),
                (Feed::NewReleases, _) => services::new_releases(),
                (Feed::Charts, _) => services::charts(),
                (Feed::Category, Some(m)) => {
                    services::mood_genre_shelves(&m.browse_id, m.params.as_deref())
                }
                (Feed::Category, None) => return,
            };
            let _ = tx.send(Event::ExploreShelves(id, feed, result.map_err(err)));
        });
    }
    fn load_moods(&mut self) {
        self.explore.moods = Loadable::Loading;
        let id = self.explore.request;
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let _ = tx.send(Event::Moods(id, services::moods_and_genres().map_err(err)));
        });
    }
    pub(super) fn moods_loaded(&mut self, result: Result<Vec<MoodGenreSection>, String>) {
        self.explore.moods = match result {
            Ok(sections) if sections.is_empty() => {
                Loadable::Failed(empty_message(Feed::Explore).into())
            }
            Ok(sections) => Loadable::Ready(sections),
            Err(e) => Loadable::Failed(e),
        };
        self.load_mood_artwork();
    }
    /// Category buttons carry no artwork of their own, so a few at a time borrow a cover from
    /// the shelves they open (upstream's `LaunchedEffect(exploreState)`).
    fn load_mood_artwork(&mut self) {
        let Loadable::Ready(sections) = &self.explore.moods else {
            return;
        };
        let mut seen = HashSet::new();
        let pending: VecDeque<MoodGenre> = flattened(sections)
            .into_iter()
            .filter(|m| m.thumbnail.is_none() && seen.insert(m.key()))
            .collect();
        if pending.is_empty() {
            return;
        }
        self.explore.artwork_stop.store(true, Ordering::Relaxed);
        self.explore.artwork_stop = Arc::new(AtomicBool::new(false));
        let queue = Arc::new(Mutex::new(pending));
        let id = self.explore.request;
        for _ in 0..ARTWORK_WORKERS {
            let (queue, stop, tx) = (
                queue.clone(),
                self.explore.artwork_stop.clone(),
                self.tx.clone(),
            );
            std::thread::spawn(move || {
                while !stop.load(Ordering::Relaxed) {
                    let Some(mood) = queue.lock().ok().and_then(|mut q| q.pop_front()) else {
                        break;
                    };
                    if let Ok(Some(url)) =
                        services::mood_genre_artwork(&mood.browse_id, mood.params.as_deref())
                    {
                        if tx.send(Event::MoodArtwork(id, mood.key(), url)).is_err() {
                            break;
                        }
                    }
                }
            });
        }
    }
    fn open_mood(&mut self, mood: MoodGenre) {
        self.explore.category_request += 1;
        self.explore.category = Loadable::Idle;
        self.explore.view = View::Category(mood);
    }

    pub(super) fn explore_view(&mut self, ui: &mut egui::Ui) {
        let view = self.explore.view.clone();
        // Fetch on first visit only; results stay cached until Refresh or an account change.
        let feed = match &view {
            View::Main => Some(Feed::Explore),
            View::NewReleases => Some(Feed::NewReleases),
            View::Charts => Some(Feed::Charts),
            View::Category(_) => Some(Feed::Category),
            View::Moods => None,
        };
        if let Some(feed) = feed {
            if matches!(self.explore.slot(feed), Loadable::Idle) {
                self.load_explore_shelves(feed);
            }
        }
        if matches!(view, View::Main | View::Moods) && matches!(self.explore.moods, Loadable::Idle)
        {
            self.load_moods();
        }
        match view {
            View::Main => self.explore_landing(ui),
            View::NewReleases => {
                self.explore_header(ui, "New releases");
                self.explore_shelves(ui, Feed::NewReleases, "explore-new");
            }
            View::Charts => {
                self.explore_header(ui, "Charts");
                self.explore_shelves(ui, Feed::Charts, "explore-charts");
            }
            View::Moods => {
                self.explore_header(ui, "Moods & genres");
                let moods = self.explore.moods.clone();
                if let Some(sections) = self.load_state(ui, &moods, |e| &mut e.moods) {
                    for section in &sections {
                        ui.label(heading(&section.title, 22.0));
                        ui.add_space(10.0);
                        if let Some(mood) = mood_grid(ui, &section.items) {
                            self.open_mood(mood);
                        }
                        ui.add_space(22.0);
                    }
                }
            }
            View::Category(mood) => {
                self.explore_header(ui, &mood.title);
                self.explore_shelves(ui, Feed::Category, "explore-category");
            }
        }
    }
    /// The landing page: the three page buttons, Explore's shelves, then a preview of the
    /// mood and genre tiles.
    fn explore_landing(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = MOOD_SPACING;
            let width = ((ui.available_width() - 36.0 - MOOD_SPACING * 3.0) / 3.0).max(150.0);
            for (icon, label, view) in [
                (Icon::Disc, "New releases", View::NewReleases),
                (Icon::Trending, "Charts", View::Charts),
                (Icon::Smiley, "Moods & genres", View::Moods),
            ] {
                if page_button(ui, icon, label, width).clicked() {
                    self.explore.view = view;
                }
            }
            if icon_action(ui, true, Icon::Refresh, "Refresh Explore").clicked() {
                self.explore.reset();
            }
        });
        ui.add_space(30.0);
        self.explore_shelves(ui, Feed::Explore, "explore");
        ui.horizontal(|ui| {
            ui.label(heading("Moods & genres", 24.0));
            ui.with_layout(egui::Layout::right_to_left(Align::Center), |ui| {
                if matches!(self.explore.moods, Loadable::Ready(_))
                    && ui.button("See all").clicked()
                {
                    self.explore.view = View::Moods;
                }
            });
        });
        ui.add_space(10.0);
        let moods = self.explore.moods.clone();
        if let Some(sections) = self.load_state(ui, &moods, |e| &mut e.moods) {
            let all = flattened(&sections);
            let shown = mood_columns(ui.available_width()) * MOOD_PREVIEW_ROWS;
            if let Some(mood) = mood_grid(ui, &all[..all.len().min(shown)]) {
                self.open_mood(mood);
            }
        }
    }
    /// A sub-page's heading: back to the landing page, its title, and a refresh.
    fn explore_header(&mut self, ui: &mut egui::Ui, title: &str) {
        ui.horizontal(|ui| {
            if icon_action(ui, true, Icon::Back, "Back to Explore").clicked() {
                self.explore.view = View::Main;
            }
            ui.label(heading(title, 26.0));
            ui.with_layout(egui::Layout::right_to_left(Align::Center), |ui| {
                if icon_action(ui, true, Icon::Refresh, "Refresh").clicked() {
                    self.explore.reset();
                }
            });
        });
        ui.add_space(16.0);
    }
    fn explore_shelves(&mut self, ui: &mut egui::Ui, feed: Feed, salt: &str) {
        let state = self.explore.slot(feed).clone();
        if let Some(shelves) = self.load_state(ui, &state, move |e| e.slot(feed)) {
            self.shelf_rows(ui, &shelves, salt);
        }
    }
    /// Shows loading or an error with Retry; the data once it is ready.
    fn load_state<T: Clone>(
        &mut self,
        ui: &mut egui::Ui,
        state: &Loadable<T>,
        slot: impl FnOnce(&mut Explore) -> &mut Loadable<T>,
    ) -> Option<T> {
        match state {
            Loadable::Ready(data) => return Some(data.clone()),
            Loadable::Idle | Loadable::Loading => {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label(RichText::new("Loading YouTube Music…").color(MUTED));
                });
            }
            Loadable::Failed(message) => {
                ui.label(RichText::new(message).color(MUTED));
                if ui.button("Retry").clicked() {
                    *slot(&mut self.explore) = Loadable::Idle;
                }
            }
        }
        ui.add_space(30.0);
        None
    }
}

/// Every category once, in the server's order: the landing page reads them as one grid.
fn flattened(sections: &[MoodGenreSection]) -> Vec<MoodGenre> {
    let mut seen = HashSet::new();
    sections
        .iter()
        .flat_map(|s| &s.items)
        .filter(|m| seen.insert(m.key()))
        .cloned()
        .collect()
}

/// Upstream `moodColumns`: as many tiles as fit, between two and six.
fn mood_columns(available: f32) -> usize {
    (((available + MOOD_SPACING) / (MOOD_MIN_WIDTH + MOOD_SPACING)).max(0.0) as usize)
        .clamp(2, MOOD_MAX_COLUMNS)
}

/// Lays `items` out as rows of tiles; returns the one clicked.
fn mood_grid(ui: &mut egui::Ui, items: &[MoodGenre]) -> Option<MoodGenre> {
    let columns = mood_columns(ui.available_width());
    let width = ((ui.available_width() - MOOD_SPACING * (columns - 1) as f32) / columns as f32)
        .floor()
        .max(60.0);
    let size = Vec2::new(width, (width / MOOD_ASPECT).round());
    let mut clicked = None;
    for row in items.chunks(columns) {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = MOOD_SPACING;
            for item in row {
                if mood_tile(ui, item, size).clicked() {
                    clicked = Some(item.clone());
                }
            }
        });
        ui.add_space((MOOD_SPACING - ui.spacing().item_spacing.y).max(0.0));
    }
    clicked
}

/// Upstream `MoodGenreCard`: a solid stripe down the left, the category's cover tinted in the
/// stripe's colour filling the rest, and the title over both.
fn mood_tile(ui: &mut egui::Ui, item: &MoodGenre, size: Vec2) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(size, egui::Sense::click());
    response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, &item.title));
    if !ui.is_rect_visible(rect) {
        return response;
    }
    let tone = mood_tone(item.color, &item.title);
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, MOOD_RADIUS, tone.stripe);
    let art = Rect::from_min_max(
        Pos2::new(rect.left() + rect.width() * MOOD_STRIPE, rect.top()),
        rect.max,
    );
    let corners = egui::CornerRadius {
        nw: 0,
        sw: 0,
        ne: MOOD_RADIUS,
        se: MOOD_RADIUS,
    };
    painter.rect_filled(art, corners, tone.placeholder);
    if let Some(url) = item
        .thumbnail
        .as_deref()
        .filter(|u| u.starts_with("https://"))
    {
        let px = if art.width().max(art.height()) * ui.ctx().pixels_per_point() <= 240.0 {
            240
        } else {
            544
        };
        // egui has no duotone filter; multiplying by the highlight tints the cover the same way.
        egui::Image::new(artwork_at(url, px))
            .uv(cover_uv(art.size()))
            .tint(tone.highlight)
            .corner_radius(corners)
            .paint_at(ui, art);
    }
    if response.hovered() {
        painter.rect_filled(rect, MOOD_RADIUS, Color32::from_black_alpha(40));
    }
    let mut job = egui::text::LayoutJob::single_section(
        item.title.clone(),
        egui::TextFormat::simple(
            FontId::new(16.0, egui::FontFamily::Name("bold".into())),
            WHITE,
        ),
    );
    job.wrap = egui::text::TextWrapping {
        max_width: (rect.width() - 28.0).max(8.0),
        max_rows: 2,
        break_anywhere: false,
        overflow_character: Some('…'),
    };
    let galley = painter.layout_job(job);
    let pos = rect.min + Vec2::new(14.0, 12.0);
    // Pale stripes would swallow white type; a soft shadow keeps it legible.
    painter.galley_with_override_text_color(
        pos + Vec2::new(0.0, 1.0),
        galley.clone(),
        Color32::from_black_alpha(120),
    );
    painter.galley(pos, galley, WHITE);
    response
}

/// One of the landing page's wide page buttons, as music.youtube.com/explore draws them.
fn page_button(ui: &mut egui::Ui, icon: Icon, label: &str, width: f32) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(Vec2::new(width, 56.0), egui::Sense::click());
    response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, label));
    let painter = ui.painter_at(rect);
    painter.rect_filled(
        rect,
        10.0,
        widgets::white_alpha(if response.hovered() { 28 } else { 14 }),
    );
    widgets::paint(
        &painter,
        icon,
        Rect::from_center_size(
            Pos2::new(rect.left() + 30.0, rect.center().y),
            Vec2::splat(24.0),
        ),
        WHITE,
    );
    painter.text(
        Pos2::new(rect.left() + 56.0, rect.center().y),
        egui::Align2::LEFT_CENTER,
        label,
        FontId::new(17.0, egui::FontFamily::Name("bold".into())),
        WHITE,
    );
    response
}

/// The centre of a square cover cropped to a box of `size`, in texture coordinates.
fn cover_uv(size: Vec2) -> Rect {
    let aspect = size.x / size.y.max(1.0);
    let (w, h) = if aspect >= 1.0 {
        (1.0, 1.0 / aspect)
    } else {
        (aspect, 1.0)
    };
    Rect::from_center_size(Pos2::new(0.5, 0.5), Vec2::new(w, h))
}

struct Tone {
    stripe: Color32,
    placeholder: Color32,
    highlight: Color32,
}

/// Upstream `moodTone`: YouTube's stripe colour, darkened a touch against the black page, with
/// a shadow-to-highlight ramp for the artwork behind it.
fn mood_tone(color: Option<u32>, title: &str) -> Tone {
    let base = color
        .map(|c| [(c >> 16) as u8, (c >> 8) as u8, c as u8])
        .unwrap_or_else(|| fallback_mood_color(title));
    let shadow = mix(base, [0; 3], 0.68);
    let highlight = mix(base, [255; 3], 0.32);
    let rgb = |c: [u8; 3]| Color32::from_rgb(c[0], c[1], c[2]);
    Tone {
        stripe: rgb(mix(base, [0; 3], 0.08)),
        placeholder: rgb(mix(shadow, highlight, 0.45)),
        highlight: rgb(highlight),
    }
}
fn mix(a: [u8; 3], b: [u8; 3], t: f32) -> [u8; 3] {
    std::array::from_fn(|i| {
        (f32::from(a[i]) + (f32::from(b[i]) - f32::from(a[i])) * t).round() as u8
    })
}
/// Upstream `fallbackMoodColor`: muted, never neon, picked by Java's `String.hashCode`.
fn fallback_mood_color(title: &str) -> [u8; 3] {
    const PALETTE: [u32; 8] = [
        0xCC6A55, 0xC07A92, 0x9C8AC0, 0x8090C8, 0xD0A060, 0x6A88B0, 0x7AAED0, 0x86B890,
    ];
    let hash = title
        .encode_utf16()
        .fold(0i32, |h, c| h.wrapping_mul(31).wrapping_add(i32::from(c)));
    let c = PALETTE[((hash & i32::MAX) % 8) as usize];
    [(c >> 16) as u8, (c >> 8) as u8, c as u8]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mood(title: &str, params: Option<&str>) -> MoodGenre {
        MoodGenre {
            title: title.into(),
            browse_id: "FEcat".into(),
            params: params.map(Into::into),
            ..Default::default()
        }
    }

    #[test]
    fn landing_grid_lists_each_category_once_in_server_order() {
        let sections = [
            MoodGenreSection {
                title: "Moods".into(),
                items: vec![mood("Chill", Some("a")), mood("Focus", Some("b"))],
            },
            MoodGenreSection {
                title: "Genres".into(),
                items: vec![mood("Chill again", Some("a")), mood("Pop", None)],
            },
        ];
        let titles: Vec<_> = flattened(&sections).into_iter().map(|m| m.title).collect();
        assert_eq!(titles, ["Chill", "Focus", "Pop"]);
    }

    #[test]
    fn columns_fit_the_width_between_two_and_six() {
        assert_eq!(mood_columns(0.0), 2);
        assert_eq!(mood_columns(700.0), 3);
        assert_eq!(mood_columns(5000.0), 6);
    }

    #[test]
    fn tones_follow_the_stripe_or_a_stable_fallback() {
        let tone = mood_tone(Some(0xFF64_C8FF), "x");
        assert_eq!(tone.stripe, Color32::from_rgb(92, 184, 235));
        assert_eq!(tone.highlight, Color32::from_rgb(150, 218, 255));
        // Java: "a".hashCode() == 97, 97 % 8 == 1; "".hashCode() == 0.
        assert_eq!(fallback_mood_color("a"), [0xC0, 0x7A, 0x92]);
        assert_eq!(fallback_mood_color(""), [0xCC, 0x6A, 0x55]);
        assert_eq!(mood_tone(None, "a").stripe, mood_tone(None, "a").stripe);
    }

    #[test]
    fn covers_crop_to_the_centre() {
        let wide = cover_uv(Vec2::new(200.0, 100.0));
        assert_eq!(
            wide,
            Rect::from_min_max(Pos2::new(0.0, 0.25), Pos2::new(1.0, 0.75))
        );
        let tall = cover_uv(Vec2::new(50.0, 100.0));
        assert_eq!(
            tall,
            Rect::from_min_max(Pos2::new(0.25, 0.0), Pos2::new(0.75, 1.0))
        );
    }

    #[test]
    fn stale_or_empty_answers_do_not_land() {
        let mut explore = Explore::default();
        explore.shelves_loaded(Feed::Category, Ok(vec![]));
        assert!(
            matches!(&explore.category, Loadable::Failed(m) if m == "Nothing to explore here yet")
        );
        explore.shelves_loaded(Feed::Charts, Err("offline".into()));
        assert!(matches!(&explore.charts, Loadable::Failed(m) if m == "offline"));
        let before = explore.request_for(Feed::Explore);
        explore.view = View::Moods;
        explore.reset();
        assert!(explore.request_for(Feed::Explore) > before);
        assert!(matches!(explore.charts, Loadable::Idle));
        assert!(explore.view == View::Moods, "Refresh keeps the open page");
        explore.moods = Loadable::Ready(vec![MoodGenreSection {
            title: "Moods".into(),
            items: vec![mood("Chill", Some("a"))],
        }]);
        explore.artwork_loaded("FEcat|a", "https://cover".into());
        let Loadable::Ready(sections) = &explore.moods else {
            unreachable!()
        };
        assert_eq!(
            sections[0].items[0].thumbnail.as_deref(),
            Some("https://cover")
        );
    }
}
