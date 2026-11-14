//! Artist pages (laid out after Apple Music's artist page, filled from YouTube Music's), the
//! "See all" grids behind their shelves, and clickable artist credits everywhere a song shows
//! its artist.
use super::explore::Loadable;
use super::*;
use crate::model::ArtistRef;
use services::artist::{ArtistPage, ArtistShelf, BrowseTarget, WatchTarget};
use std::collections::HashMap;

const CARD: f32 = 180.0;
const WIDE_CARD: f32 = 300.0;
/// Top songs shown beside the latest release; "See all" opens the rest.
const TOP_SONGS: usize = 8;
/// Biography characters shown before "More".
const ABOUT_PREVIEW: usize = 420;

/// A clicked artist name: its page when the credit links one, else a name to look up.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct ArtistLink {
    pub name: String,
    pub browse_id: Option<String>,
}

#[derive(Default)]
pub(super) struct Artists {
    pages: HashMap<String, Loadable<ArtistPage>>,
    grids: HashMap<BrowseTarget, (String, Loadable<Vec<ShelfItem>>)>,
    /// Bumped per name lookup, so only the last click opens a page.
    lookup_request: u64,
    /// Artists matching the last search, shown above its songs.
    pub(super) results: Vec<ShelfItem>,
    /// Playlists matching the last search, shown between its artists and songs.
    pub(super) playlists: Vec<ShelfItem>,
    /// Shuffle or Mix of an artist is being fetched.
    queue_loading: bool,
    queue_request: u64,
    /// A subscription change is in flight.
    subscribing: bool,
}

impl Artists {
    /// Forgets fetched pages (subscriptions and personal shelves follow the account).
    pub(super) fn reset(&mut self) {
        *self = Artists {
            lookup_request: self.lookup_request + 1,
            queue_request: self.queue_request + 1,
            ..Default::default()
        };
    }
}

/// Splits a credit line into text segments, linking each credited artist found in it. With
/// `split_unlinked`, a line without links (a track saved before links were kept, a local
/// file's tag) becomes one name per comma or ampersand, each looked up by name.
pub(super) fn credit_segments(
    line: &str,
    artists: &[ArtistRef],
    split_unlinked: bool,
) -> Vec<(String, Option<ArtistLink>)> {
    let mut out = vec![];
    if artists.is_empty() {
        if !split_unlinked {
            out.push((line.to_owned(), None));
            return out;
        }
        let mut rest = line;
        while !rest.is_empty() {
            let cut = [", ", " & "]
                .iter()
                .filter_map(|sep| rest.find(sep).map(|i| (i, sep.len())))
                .min();
            let (name, sep) = match cut {
                Some((i, len)) => (&rest[..i], &rest[i..i + len]),
                None => (rest, ""),
            };
            if !name.trim().is_empty() {
                out.push((
                    name.to_owned(),
                    Some(ArtistLink {
                        name: name.trim().to_owned(),
                        browse_id: None,
                    }),
                ));
            } else if !name.is_empty() {
                out.push((name.to_owned(), None));
            }
            if !sep.is_empty() {
                out.push((sep.to_owned(), None));
            }
            rest = &rest[name.len() + sep.len()..];
        }
        return out;
    }
    let mut at = 0;
    for artist in artists {
        let Some(found) = find_name(line, at, &artist.name) else {
            continue;
        };
        if found > at {
            out.push((line[at..found].to_owned(), None));
        }
        out.push((
            artist.name.clone(),
            Some(ArtistLink {
                name: artist.name.clone(),
                browse_id: Some(artist.browse_id.clone()),
            }),
        ));
        at = found + artist.name.len();
    }
    if at < line.len() {
        out.push((line[at..].to_owned(), None));
    }
    out
}

/// Where `name` stands as a whole name in `line` from byte `from`: not inside a longer word,
/// so an artist called "A" is not found in "Album".
fn find_name(line: &str, from: usize, name: &str) -> Option<usize> {
    if name.is_empty() {
        return None;
    }
    line[from..]
        .match_indices(name)
        .map(|(i, _)| i + from)
        .find(|&i| {
            let before = line[..i].chars().next_back();
            let after = line[i + name.len()..].chars().next();
            !before.is_some_and(char::is_alphanumeric) && !after.is_some_and(char::is_alphanumeric)
        })
}

/// A track's artist line as segments; "Local audio" (no artist) links nowhere.
pub(super) fn track_credit(track: &Track) -> Vec<(String, Option<ArtistLink>)> {
    if track.artist.trim().is_empty() {
        return vec![("Local audio".to_owned(), None)];
    }
    credit_segments(&track.artist, &track.artists, true)
}

/// Paints a one-line credit left-aligned at `pos` (vertical centre), each linked name
/// underlined on hover, cut with "…" at `max_width`. Returns the name clicked.
pub(super) fn credit_text(
    ui: &mut egui::Ui,
    id: egui::Id,
    pos: Pos2,
    segments: &[(String, Option<ArtistLink>)],
    font: FontId,
    color: Color32,
    max_width: f32,
) -> Option<ArtistLink> {
    let mut x = pos.x;
    let right = pos.x + max_width.max(8.0);
    let mut clicked = None;
    for (i, (text, link)) in segments.iter().enumerate() {
        let room = right - x;
        if room < 8.0 {
            break;
        }
        let mut job = egui::text::LayoutJob::single_section(
            text.clone(),
            egui::TextFormat::simple(font.clone(), color),
        );
        job.wrap = egui::text::TextWrapping {
            max_width: room,
            max_rows: 1,
            break_anywhere: true,
            overflow_character: Some('…'),
        };
        let galley = ui.painter().layout_job(job);
        let rect = egui::Align2::LEFT_CENTER.anchor_size(Pos2::new(x, pos.y), galley.size());
        let elided = galley.elided;
        let mut paint = color;
        if let Some(link) = link {
            let response = ui
                .interact(rect, id.with(i), egui::Sense::click())
                .on_hover_cursor(egui::CursorIcon::PointingHand);
            if response.hovered() {
                paint = WHITE;
                ui.painter().line_segment(
                    [
                        Pos2::new(rect.left(), rect.bottom() - 1.0),
                        Pos2::new(rect.right(), rect.bottom() - 1.0),
                    ],
                    Stroke::new(1.0, WHITE),
                );
            }
            if response.clicked() {
                clicked = Some(link.clone());
            }
        }
        ui.painter().galley(rect.min, galley, paint);
        x = rect.right();
        if elided {
            break;
        }
    }
    clicked
}

/// What a card was clicked for.
pub(super) enum CardAction {
    Open(ShelfItem),
    Artist(ArtistLink),
}

/// One shelf card: square artwork, a 16:9 video frame, or a round artist photo, with its title
/// and a subtitle whose artist names link to their pages.
pub(super) fn card(ui: &mut egui::Ui, item: &ShelfItem) -> Option<CardAction> {
    let round = item.is_artist();
    let width = if item.wide { WIDE_CARD } else { CARD };
    let art_height = if item.wide {
        WIDE_CARD * 9.0 / 16.0
    } else {
        CARD
    };
    let mut action = None;
    ui.vertical(|ui| {
        ui.set_width(width);
        let (rect, response) =
            ui.allocate_exact_size(Vec2::new(width, art_height), egui::Sense::click());
        let response = response.on_hover_cursor(egui::CursorIcon::PointingHand);
        if round {
            round_image(ui, &item.thumbnail, &item.title, rect);
        } else {
            draw_image(ui, &item.thumbnail, rect, None);
        }
        if response.hovered() && !round {
            ui.painter()
                .rect_filled(rect, 10.0, Color32::from_black_alpha(35));
            let r =
                Rect::from_center_size(rect.right_bottom() - Vec2::splat(25.0), Vec2::splat(34.0));
            ui.painter()
                .circle_filled(r.center(), 20.0, Color32::from_black_alpha(140));
            let icon = if item.video_id.is_some() {
                Icon::Play
            } else {
                Icon::CaretRight
            };
            widgets::paint(ui.painter(), icon, r.shrink(7.0), WHITE);
        }
        if response.clicked() {
            action = Some(CardAction::Open(item.clone()));
        }
        let layout = if round {
            egui::Layout::top_down(Align::Center)
        } else {
            egui::Layout::top_down(Align::Min)
        };
        ui.with_layout(layout, |ui| {
            if ui
                .add(
                    egui::Label::new(heading(&item.title, 15.0))
                        .truncate()
                        .sense(egui::Sense::click()),
                )
                .on_hover_cursor(egui::CursorIcon::PointingHand)
                .clicked()
            {
                action = Some(CardAction::Open(item.clone()));
            }
        });
        if !item.subtitle.is_empty() {
            let (line, line_response) =
                ui.allocate_exact_size(Vec2::new(width, 18.0), egui::Sense::hover());
            let segments = credit_segments(&item.subtitle, &item.artists, false);
            let start = if round {
                let w = ui
                    .painter()
                    .layout_no_wrap(item.subtitle.clone(), FontId::proportional(14.0), MUTED)
                    .size()
                    .x;
                (line.center().x - w.min(width) / 2.0).max(line.left())
            } else {
                line.left()
            };
            // The line's own id is unique per card. The scope id was shared by the cards of a
            // shelf, so egui flagged "First use of widget ID" over the artist names.
            let id = line_response.id.with("credit");
            if let Some(link) = credit_text(
                ui,
                id,
                Pos2::new(start, line.center().y),
                &segments,
                FontId::proportional(14.0),
                MUTED,
                width,
            ) {
                action = Some(CardAction::Artist(link));
            }
        }
    });
    action
}

fn round_image(ui: &mut egui::Ui, url: &str, name: &str, rect: Rect) {
    if !ui.is_rect_visible(rect) {
        return;
    }
    let side = rect.width().min(rect.height());
    let circle = Rect::from_center_size(rect.center(), Vec2::splat(side));
    let px = (side * ui.ctx().pixels_per_point()).ceil() as u32;
    if url.starts_with("https://") {
        web_image(artwork_at(url, if px <= 240 { 240 } else { 544 }))
            .fit_to_exact_size(circle.size())
            .corner_radius(side / 2.0)
            .paint_at(ui, circle);
    } else {
        avatar(ui, url, name, circle);
    }
}

/// A row of cards that scrolls sideways.
pub(super) fn card_row(
    ui: &mut egui::Ui,
    items: &[ShelfItem],
    salt: impl std::hash::Hash + std::fmt::Debug,
) -> Option<CardAction> {
    let mut action = None;
    egui::ScrollArea::horizontal()
        .id_salt(("card-row", &salt))
        .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden)
        .show(ui, |ui| {
            ui.horizontal_top(|ui| {
                ui.spacing_mut().item_spacing.x = 16.0;
                for (i, item) in items.iter().enumerate() {
                    ui.push_id(i, |ui| {
                        if let Some(a) = card(ui, item) {
                            action = Some(a);
                        }
                    });
                }
            });
        });
    action
}

/// The image's own width over height, read from YouTube's `w<N>-h<N>` size; 12:5 (the
/// immersive banner) when the URL does not say.
fn banner_aspect(url: &str) -> f32 {
    let size = |key: char| {
        url.split(['-', '='])
            .find_map(|part| part.strip_prefix(key)?.parse::<f32>().ok())
    };
    match (size('w'), size('h')) {
        (Some(w), Some(h)) if w > 0.0 && h > 0.0 => w / h,
        _ => 2.4,
    }
}

/// Paints `url` over `rect` like CSS `object-fit: cover`.
fn cover_image(ui: &mut egui::Ui, url: &str, rect: Rect) {
    if !ui.is_rect_visible(rect) || !url.starts_with("https://") {
        ui.painter()
            .rect_filled(rect, 0.0, Color32::from_rgb(51, 53, 48));
        return;
    }
    let image = banner_aspect(url);
    let target = rect.width() / rect.height();
    let uv = if image > target {
        let w = target / image;
        Rect::from_min_max(
            Pos2::new((1.0 - w) / 2.0, 0.0),
            Pos2::new((1.0 + w) / 2.0, 1.0),
        )
    } else {
        let h = image / target;
        // Faces sit high in artist photos: keep the top rather than the middle.
        Rect::from_min_max(
            Pos2::new(0.0, 0.1 * (1.0 - h)),
            Pos2::new(1.0, 0.1 * (1.0 - h) + h),
        )
    };
    web_image(url.to_owned())
        .uv(uv)
        .fit_to_exact_size(rect.size())
        .corner_radius(14.0)
        .paint_at(ui, rect);
}

/// Darkens the lower part of the banner so the name stays readable.
fn bottom_fade(painter: &egui::Painter, rect: Rect) {
    let mut mesh = egui::Mesh::default();
    let clear = Color32::from_black_alpha(0);
    let dark = Color32::from_black_alpha(190);
    mesh.colored_vertex(rect.left_top(), clear);
    mesh.colored_vertex(rect.right_top(), clear);
    mesh.colored_vertex(rect.left_bottom(), dark);
    mesh.colored_vertex(rect.right_bottom(), dark);
    mesh.add_triangle(0, 1, 2);
    mesh.add_triangle(1, 3, 2);
    painter.add(egui::Shape::mesh(mesh));
}

/// A rounded action button: icon and label, filled for the main action.
fn pill(ui: &mut egui::Ui, icon: Icon, label: &str, filled: bool, enabled: bool) -> egui::Response {
    let font = FontId::new(15.0, egui::FontFamily::Name("bold".into()));
    let text_width = ui
        .painter()
        .layout_no_wrap(label.to_owned(), font.clone(), WHITE)
        .size()
        .x;
    let size = Vec2::new(text_width + 58.0, 40.0);
    let sense = if enabled {
        egui::Sense::click()
    } else {
        egui::Sense::hover()
    };
    let (rect, response) = ui.allocate_exact_size(size, sense);
    let (fill, ink) = match (filled, response.hovered() && enabled) {
        (true, false) => (ACCENT, BG),
        (true, true) => (Color32::from_rgb(196, 224, 178), BG),
        (false, false) => (widgets::white_alpha(18), WHITE),
        (false, true) => (widgets::white_alpha(32), WHITE),
    };
    let ink = if enabled { ink } else { MUTED };
    ui.painter().rect_filled(rect, 20.0, fill);
    let icon_rect = Rect::from_center_size(
        Pos2::new(rect.left() + 26.0, rect.center().y),
        Vec2::splat(18.0),
    );
    widgets::paint(ui.painter(), icon, icon_rect, ink);
    ui.painter().text(
        Pos2::new(rect.left() + 42.0, rect.center().y),
        egui::Align2::LEFT_CENTER,
        label,
        font,
        ink,
    );
    if enabled {
        response.on_hover_cursor(egui::CursorIcon::PointingHand)
    } else {
        response
    }
}

impl YTDugem {
    /// Opens an artist page, fetching it unless it is already loaded.
    pub(super) fn open_artist(&mut self, browse_id: String) {
        self.navigate(Page::Artist(browse_id.clone()));
        let stale = !matches!(
            self.artists.pages.get(&browse_id),
            Some(Loadable::Ready(_) | Loadable::Loading)
        );
        if stale {
            self.load_artist(browse_id);
        }
    }
    fn load_artist(&mut self, browse_id: String) {
        self.artists
            .pages
            .insert(browse_id.clone(), Loadable::Loading);
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let result = services::artist::artist_page(&browse_id).map_err(err);
            let _ = tx.send(Event::Artist(browse_id, result));
        });
    }
    pub(super) fn artist_loaded(&mut self, browse_id: String, result: Result<ArtistPage, String>) {
        let state = match result {
            Ok(page) => Loadable::Ready(page),
            Err(e) => Loadable::Failed(e),
        };
        self.artists.pages.insert(browse_id, state);
    }

    /// Opens a clicked credit: its page, or the page of the artist with exactly that name.
    pub(super) fn open_artist_link(&mut self, link: ArtistLink) {
        if let Some(id) = link.browse_id {
            self.open_artist(id);
            return;
        }
        self.artists.lookup_request += 1;
        let request = self.artists.lookup_request;
        self.message = format!("Looking for {}…", link.name);
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let result = services::artist::find_artist_id(&link.name).map_err(err);
            let _ = tx.send(Event::ArtistLookup(request, link.name, result));
        });
    }
    pub(super) fn artist_found(
        &mut self,
        request: u64,
        name: String,
        result: Result<Option<String>, String>,
    ) {
        if request != self.artists.lookup_request {
            return;
        }
        match result {
            Ok(Some(id)) => {
                self.message.clear();
                self.open_artist(id);
            }
            Ok(None) => self.message = format!("No artist page found for {name}"),
            Err(e) => self.message = e,
        }
    }

    /// Opens a clicked card: artists open their page, everything else as before.
    pub(super) fn card_clicked(&mut self, action: CardAction) {
        match action {
            CardAction::Open(item) => self.open_item(item),
            CardAction::Artist(link) => self.open_artist_link(link),
        }
    }

    /// Searches artists and playlists alongside songs; `request` is the song search's.
    pub(super) fn search_artists(&mut self, request: u64, query: String) {
        self.artists.results.clear();
        self.artists.playlists.clear();
        let tx = self.tx.clone();
        let q = query.clone();
        std::thread::spawn(move || {
            let result = services::artist::search_artists(&q).map_err(err);
            let _ = tx.send(Event::ArtistResults(request, result));
        });
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let result = services::search_playlists(&query).map_err(err);
            let _ = tx.send(Event::PlaylistResults(request, result));
        });
    }

    fn open_shelf(&mut self, title: String, target: BrowseTarget) {
        // Song and video lists are playlists: they open as one, with Play all and paging.
        if target.browse_id.starts_with("VL") {
            self.open_item(ShelfItem {
                title,
                browse_id: Some(target.browse_id),
                params: target.params,
                ..Default::default()
            });
            return;
        }
        let stale = !matches!(
            self.artists.grids.get(&target),
            Some((_, Loadable::Ready(_) | Loadable::Loading))
        );
        if stale {
            self.artists
                .grids
                .insert(target.clone(), (title, Loadable::Loading));
            let tx = self.tx.clone();
            let t = target.clone();
            std::thread::spawn(move || {
                let result = services::artist::shelf_items(&t).map_err(err);
                let _ = tx.send(Event::ArtistGrid(t, result));
            });
        }
        self.navigate(Page::ArtistGrid(target));
    }
    pub(super) fn grid_loaded(
        &mut self,
        target: BrowseTarget,
        result: Result<Vec<ShelfItem>, String>,
    ) {
        if let Some((_, state)) = self.artists.grids.get_mut(&target) {
            *state = match result {
                Ok(items) if items.is_empty() => Loadable::Failed("Nothing here yet".into()),
                Ok(items) => Loadable::Ready(items),
                Err(e) => Loadable::Failed(e),
            };
        }
    }

    /// Plays the header's Shuffle or Mix queue, falling back to the top songs.
    fn play_artist_queue(&mut self, page: &ArtistPage, target: Option<WatchTarget>, label: &str) {
        let Some(target) = target else {
            let mut songs = page.songs.clone();
            songs.shuffle(&mut rand::rng());
            if !songs.is_empty() {
                self.start_tracks(songs, 0, &page.name);
            }
            return;
        };
        self.artists.queue_request += 1;
        self.artists.queue_loading = true;
        let request = self.artists.queue_request;
        let origin = format!("{} · {label}", page.name);
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let result = services::artist::watch(&target).map_err(err);
            let _ = tx.send(Event::ArtistQueue(request, origin, result));
        });
    }
    pub(super) fn artist_queue_loaded(
        &mut self,
        request: u64,
        origin: String,
        result: Result<Vec<Track>, String>,
    ) {
        if request != self.artists.queue_request {
            return;
        }
        self.artists.queue_loading = false;
        match result {
            Ok(tracks) => self.start_tracks(tracks, 0, &origin),
            Err(e) => self.message = e,
        }
    }

    fn toggle_subscription(&mut self, browse_id: &str, channel_id: String, subscribe: bool) {
        self.artists.subscribing = true;
        // Optimistic, as upstream: the button flips now and flips back on failure.
        self.set_subscribed(browse_id, Some(subscribe));
        let tx = self.tx.clone();
        let id = browse_id.to_owned();
        std::thread::spawn(move || {
            let result = services::artist::set_subscribed(&channel_id, subscribe).map_err(err);
            let _ = tx.send(Event::ArtistSubscribed(id, subscribe, result));
        });
    }
    fn set_subscribed(&mut self, browse_id: &str, subscribed: Option<bool>) {
        if let Some(Loadable::Ready(page)) = self.artists.pages.get_mut(browse_id) {
            page.subscribed = subscribed;
        }
    }
    pub(super) fn subscription_changed(
        &mut self,
        browse_id: String,
        subscribed: bool,
        result: Result<(), String>,
    ) {
        self.artists.subscribing = false;
        if let Err(e) = result {
            self.set_subscribed(&browse_id, Some(!subscribed));
            self.message = e;
        }
    }

    pub(super) fn artist_view(&mut self, ui: &mut egui::Ui, browse_id: &str) {
        let state = self.artists.pages.get(browse_id).cloned();
        let page = match state {
            Some(Loadable::Ready(page)) => page,
            Some(Loadable::Failed(e)) => {
                ui.label(heading("Could not load this artist", 22.0));
                ui.label(RichText::new(e).color(MUTED));
                ui.add_space(10.0);
                if ui.button("Try again").clicked() {
                    self.load_artist(browse_id.to_owned());
                }
                return;
            }
            Some(Loadable::Loading) => {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label(RichText::new("Loading artist…").color(MUTED));
                });
                return;
            }
            Some(Loadable::Idle) | None => {
                self.load_artist(browse_id.to_owned());
                return;
            }
        };
        self.artist_header(ui, &page);
        ui.add_space(26.0);
        self.artist_highlights(ui, &page);
        for (i, shelf) in page.shelves.iter().enumerate() {
            self.artist_shelf(ui, &page, shelf, i);
        }
        if !page.description.is_empty() {
            artist_about(ui, &page);
        }
        if page.songs.is_empty() && page.shelves.is_empty() {
            ui.label(RichText::new("YouTube Music has no music for this artist yet").color(MUTED));
        }
        ui.add_space(20.0);
    }

    /// The banner with the name and counts over it, then Play, Shuffle, Mix and Subscribe.
    fn artist_header(&mut self, ui: &mut egui::Ui, page: &ArtistPage) {
        let width = ui.available_width();
        let stats: Vec<&str> = [page.subscribers.as_deref(), page.monthly.as_deref()]
            .into_iter()
            .flatten()
            .collect();
        if page.banner {
            let height = (width / 2.4).clamp(220.0, 400.0);
            let (rect, _) = ui.allocate_exact_size(Vec2::new(width, height), egui::Sense::hover());
            cover_image(ui, &page.thumbnail, rect);
            bottom_fade(
                ui.painter(),
                Rect::from_min_max(Pos2::new(rect.left(), rect.top() + height * 0.35), rect.max),
            );
            let name_y = if stats.is_empty() {
                rect.bottom() - 38.0
            } else {
                rect.bottom() - 62.0
            };
            fit_text(
                ui.painter(),
                Pos2::new(rect.left() + 28.0, name_y),
                egui::Align2::LEFT_CENTER,
                &page.name,
                FontId::new(46.0, egui::FontFamily::Name("bold".into())),
                WHITE,
                width - 56.0,
            );
            if !stats.is_empty() {
                fit_text(
                    ui.painter(),
                    Pos2::new(rect.left() + 30.0, rect.bottom() - 24.0),
                    egui::Align2::LEFT_CENTER,
                    &stats.join("  ·  "),
                    FontId::proportional(14.0),
                    Color32::from_white_alpha(200),
                    width - 60.0,
                );
            }
        } else {
            ui.horizontal(|ui| {
                let (rect, _) = ui.allocate_exact_size(Vec2::splat(180.0), egui::Sense::hover());
                round_image(ui, &page.thumbnail, &page.name, rect);
                ui.add_space(20.0);
                ui.vertical(|ui| {
                    ui.add_space(60.0);
                    ui.label(heading(&page.name, 40.0));
                    if !stats.is_empty() {
                        ui.label(RichText::new(stats.join("  ·  ")).color(MUTED));
                    }
                });
            });
        }
        ui.add_space(18.0);
        let busy = self.artists.queue_loading;
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 10.0;
            if pill(ui, Icon::Play, "Play", true, !page.songs.is_empty()).clicked() {
                self.start_tracks(page.songs.clone(), 0, &page.name);
            }
            if pill(ui, Icon::Shuffle, "Shuffle", false, !busy).clicked() {
                self.play_artist_queue(page, page.shuffle.clone(), "Shuffle");
            }
            if page.radio.is_some() && pill(ui, Icon::Radio, "Mix", false, !busy).clicked() {
                self.play_artist_queue(page, page.radio.clone(), "Mix");
            }
            if let (Some(subscribed), Some(channel)) = (page.subscribed, page.channel_id.clone()) {
                let (icon, label) = if subscribed {
                    (Icon::Check, "Subscribed")
                } else {
                    (Icon::Plus, "Subscribe")
                };
                if pill(ui, icon, label, false, !self.artists.subscribing).clicked() {
                    self.toggle_subscription(&page.browse_id, channel, !subscribed);
                }
            }
            if busy {
                ui.spinner();
            }
        });
    }

    /// Apple Music's top row: the latest release beside the top songs.
    fn artist_highlights(&mut self, ui: &mut egui::Ui, page: &ArtistPage) {
        let latest = page.latest_release().cloned();
        if latest.is_none() && page.songs.is_empty() {
            return;
        }
        let width = ui.available_width();
        let side_by_side = latest.is_some() && width >= 760.0;
        let latest_card = |ui: &mut egui::Ui, this: &mut Self| {
            if let Some(item) = &latest {
                ui.label(heading("Latest Release", 22.0));
                ui.add_space(10.0);
                if let Some(action) = latest_release_card(ui, item) {
                    this.card_clicked(action);
                }
            }
        };
        if side_by_side {
            ui.horizontal_top(|ui| {
                ui.vertical(|ui| {
                    ui.set_width(250.0);
                    latest_card(ui, self);
                });
                ui.add_space(26.0);
                ui.vertical(|ui| self.top_songs(ui, page));
            });
        } else {
            latest_card(ui, self);
            ui.add_space(20.0);
            self.top_songs(ui, page);
        }
        ui.add_space(30.0);
    }

    fn top_songs(&mut self, ui: &mut egui::Ui, page: &ArtistPage) {
        if page.songs.is_empty() {
            return;
        }
        ui.horizontal(|ui| {
            ui.label(heading("Top Songs", 22.0));
            if let Some(more) = page.songs_more.clone() {
                ui.with_layout(egui::Layout::right_to_left(Align::Center), |ui| {
                    if ui.button("See all").clicked() {
                        self.open_shelf(format!("{} · Top songs", page.name), more);
                    }
                });
            }
        });
        ui.add_space(6.0);
        let songs = &page.songs[..page.songs.len().min(TOP_SONGS)];
        let width = ui.available_width();
        let columns = if width >= 860.0 && songs.len() > 4 {
            2
        } else {
            1
        };
        let per_column = songs.len().div_ceil(columns);
        let column_width = (width - 16.0 * (columns - 1) as f32) / columns as f32;
        ui.horizontal_top(|ui| {
            ui.spacing_mut().item_spacing.x = 16.0;
            for (c, chunk) in songs.chunks(per_column).enumerate() {
                ui.vertical(|ui| {
                    ui.set_width(column_width);
                    for (i, track) in chunk.iter().enumerate() {
                        let index = c * per_column + i;
                        ui.push_id(("top-song", index), |ui| {
                            if self.song_row(ui, track, None, column_width) {
                                self.start_tracks(page.songs.clone(), index, &page.name);
                            }
                        });
                    }
                });
            }
        });
    }

    fn artist_shelf(
        &mut self,
        ui: &mut egui::Ui,
        page: &ArtistPage,
        shelf: &ArtistShelf,
        i: usize,
    ) {
        ui.horizontal(|ui| {
            ui.label(heading(&shelf.title, 22.0));
            if let Some(more) = shelf.more.clone() {
                ui.with_layout(egui::Layout::right_to_left(Align::Center), |ui| {
                    if ui.button("See all").clicked() {
                        self.open_shelf(format!("{} · {}", page.name, shelf.title), more);
                    }
                });
            }
        });
        ui.add_space(10.0);
        if let Some(action) = card_row(ui, &shelf.items, (&page.browse_id, i)) {
            self.card_clicked(action);
        }
        ui.add_space(30.0);
    }

    pub(super) fn artist_grid_view(&mut self, ui: &mut egui::Ui, target: &BrowseTarget) {
        let Some((title, state)) = self.artists.grids.get(target).cloned() else {
            self.page = Page::Home;
            return;
        };
        ui.label(heading(&title, 26.0));
        ui.add_space(16.0);
        match state {
            Loadable::Ready(items) => {
                let mut action = None;
                // egui cannot wrap a row of vertical cards, so the columns are counted here.
                let gap = 16.0;
                let card_width = if items.iter().any(|i| i.wide) {
                    WIDE_CARD
                } else {
                    CARD
                };
                let columns = (((ui.available_width() + gap) / (card_width + gap)) as usize).max(1);
                for (row, chunk) in items.chunks(columns).enumerate() {
                    ui.horizontal_top(|ui| {
                        ui.spacing_mut().item_spacing.x = gap;
                        for (i, item) in chunk.iter().enumerate() {
                            ui.push_id(row * columns + i, |ui| {
                                if let Some(a) = card(ui, item) {
                                    action = Some(a);
                                }
                            });
                        }
                    });
                    ui.add_space(22.0);
                }
                if let Some(action) = action {
                    self.card_clicked(action);
                }
            }
            Loadable::Failed(e) => {
                ui.label(RichText::new(e).color(MUTED));
            }
            Loadable::Loading | Loadable::Idle => {
                ui.spinner();
            }
        }
    }

    /// Search's Artists and Playlists rows, above the songs.
    pub(super) fn artist_results(&mut self, ui: &mut egui::Ui) {
        if self.artists.results.is_empty() && self.artists.playlists.is_empty() {
            return;
        }
        let rows = [
            ("Artists", self.artists.results.clone()),
            ("Playlists", self.artists.playlists.clone()),
        ];
        for (title, items) in rows {
            if items.is_empty() {
                continue;
            }
            ui.add_space(10.0);
            ui.label(heading(title, 20.0));
            ui.add_space(8.0);
            if let Some(action) = card_row(ui, &items[..items.len().min(10)], ("search", title)) {
                self.card_clicked(action);
            }
            ui.add_space(8.0);
        }
        ui.add_space(10.0);
        ui.label(heading("Songs", 20.0));
        ui.add_space(4.0);
    }
}

fn latest_release_card(ui: &mut egui::Ui, item: &ShelfItem) -> Option<CardAction> {
    let mut action = None;
    let (rect, response) = ui.allocate_exact_size(Vec2::splat(220.0), egui::Sense::click());
    let response = response.on_hover_cursor(egui::CursorIcon::PointingHand);
    draw_image(ui, &item.thumbnail, rect, None);
    if response.hovered() {
        ui.painter()
            .rect_filled(rect, 8.0, Color32::from_black_alpha(35));
    }
    if response.clicked() {
        action = Some(CardAction::Open(item.clone()));
    }
    ui.add_space(4.0);
    ui.label(heading(&item.title, 16.0));
    if !item.subtitle.is_empty() {
        ui.label(RichText::new(&item.subtitle).size(14.0).color(MUTED));
    }
    action
}

fn artist_about(ui: &mut egui::Ui, page: &ArtistPage) {
    let id = ui.scope_id().with(("about", &page.browse_id));
    let open = ui.data(|d| d.get_temp::<bool>(id)).unwrap_or(false);
    ui.label(heading(format!("About {}", page.name), 22.0));
    ui.add_space(8.0);
    let long = page.description.chars().count() > ABOUT_PREVIEW;
    let text = if long && !open {
        truncate(&page.description, ABOUT_PREVIEW)
    } else {
        page.description.clone()
    };
    ui.scope(|ui| {
        ui.set_max_width(ui.available_width().min(760.0));
        ui.label(
            RichText::new(text)
                .size(15.0)
                .color(Color32::from_white_alpha(210)),
        );
    });
    if long && ui.button(if open { "Less" } else { "More" }).clicked() {
        ui.data_mut(|d| d.insert_temp(id, !open));
    }
    ui.add_space(30.0);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn linked(segments: &[(String, Option<ArtistLink>)]) -> Vec<(&str, Option<&str>)> {
        segments
            .iter()
            .map(|(t, l)| {
                (
                    t.as_str(),
                    l.as_ref().map(|l| l.browse_id.as_deref().unwrap_or("?")),
                )
            })
            .collect()
    }

    #[test]
    fn credits_link_every_credited_artist_and_keep_the_rest_as_text() {
        let artists = [
            ArtistRef {
                name: "A".into(),
                browse_id: "UCa".into(),
            },
            ArtistRef {
                name: "B".into(),
                browse_id: "UCb".into(),
            },
        ];
        let segments = credit_segments("A, Unlinked & B", &artists, true);
        assert_eq!(
            linked(&segments),
            [
                ("A", Some("UCa")),
                (", Unlinked & ", None),
                ("B", Some("UCb"))
            ]
        );
        // A credited name missing from the line is skipped, not invented.
        let segments = credit_segments("Album • A • 2020", &artists, false);
        assert_eq!(
            linked(&segments),
            [("Album • ", None), ("A", Some("UCa")), (" • 2020", None)]
        );
    }

    #[test]
    fn unlinked_credits_split_into_names_to_look_up() {
        let segments = credit_segments("Simon & Garfunkel, Paul", &[], true);
        assert_eq!(
            linked(&segments),
            [
                ("Simon", Some("?")),
                (" & ", None),
                ("Garfunkel", Some("?")),
                (", ", None),
                ("Paul", Some("?"))
            ]
        );
        assert_eq!(
            linked(&credit_segments("Album • 2020", &[], false)),
            [("Album • 2020", None)]
        );
        let local = Track::default();
        assert_eq!(linked(&track_credit(&local)), [("Local audio", None)]);
    }

    #[test]
    fn banner_aspect_reads_the_url_size() {
        assert_eq!(
            banner_aspect("https://lh3.googleusercontent.com/x=w1440-h600-p-l90-rj"),
            2.4
        );
        assert_eq!(
            banner_aspect("https://yt3.googleusercontent.com/x=w544-h544-l90-rj"),
            1.0
        );
        assert_eq!(banner_aspect("https://example.com/x.jpg"), 2.4);
    }
}
