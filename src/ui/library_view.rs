//! Filter and sort for the Local music, History and local playlist lists (a YTDugem addition).
use super::*;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum SortKey {
    /// The list's own order: import or add order, or most recently played first for History.
    #[default]
    Default,
    /// The stored order reversed: the last added first.
    Recent,
    Title,
    Artist,
    Duration,
}

/// What a list is filtered and sorted by; kept per list in egui's memory for the session.
#[derive(Clone, Default)]
struct ListView {
    query: String,
    sort: SortKey,
}

/// Case-insensitive match of every word of `query` against title, artist or album.
pub(super) fn matches(track: &Track, query: &str) -> bool {
    let haystack = format!("{}\n{}\n{}", track.title, track.artist, track.album).to_lowercase();
    query
        .to_lowercase()
        .split_whitespace()
        .all(|word| haystack.contains(word))
}

/// The tracks that match `query`, in `sort` order. Sorting is stable, so equal entries keep
/// the list's own order; unknown durations sort last.
pub(super) fn filter_sort(tracks: &[Track], query: &str, sort: SortKey) -> Vec<Track> {
    let mut out: Vec<Track> = tracks
        .iter()
        .filter(|t| matches(t, query))
        .cloned()
        .collect();
    let fold = |s: &str| s.to_lowercase();
    match sort {
        SortKey::Default => {}
        SortKey::Recent => out.reverse(),
        SortKey::Title => out.sort_by_cached_key(|t| fold(&t.title)),
        SortKey::Artist => out.sort_by_cached_key(|t| (fold(&t.artist), fold(&t.title))),
        SortKey::Duration => out.sort_by(|a, b| {
            let key =
                |t: &Track| (t.duration.is_finite() && t.duration > 0.0).then_some(t.duration);
            match (key(a), key(b)) {
                (Some(x), Some(y)) => x.total_cmp(&y),
                (Some(_), None) => std::cmp::Ordering::Less,
                (None, Some(_)) => std::cmp::Ordering::Greater,
                (None, None) => std::cmp::Ordering::Equal,
            }
        }),
    }
    out
}

impl YTDugem {
    /// A filter box and sort menu over `tracks`, then the matching rows. `recent_first` lists
    /// (History) are already newest first, so they offer no "Recently added" order.
    pub(super) fn filtered_track_list(
        &mut self,
        ui: &mut egui::Ui,
        tracks: &[Track],
        origin: &str,
        header: bool,
        recent_first: bool,
    ) {
        if tracks.is_empty() {
            self.track_list_with(ui, Vec::new(), origin, header);
            return;
        }
        let id = egui::Id::unique(("list-view", origin));
        let mut view: ListView = ui.data(|d| d.get_temp(id)).unwrap_or_default();
        let default_label = if recent_first {
            "Recently played"
        } else {
            "Order added"
        };
        let label = |sort: SortKey| match sort {
            SortKey::Default => default_label,
            SortKey::Recent => "Recently added",
            SortKey::Title => "Title",
            SortKey::Artist => "Artist",
            SortKey::Duration => "Duration",
        };
        ui.horizontal(|ui| {
            ui.add(
                egui::TextEdit::singleline(&mut view.query)
                    .margin(TEXT_MARGIN)
                    .desired_width(320.0)
                    .hint_text("Filter by title, artist or album")
                    .id(id.with("query")),
            );
            if !view.query.is_empty()
                && widgets::button(ui, Icon::Close, 28.0, false, "Clear filter").clicked()
            {
                view.query.clear();
            }
            ui.add_space(8.0);
            ui.label(RichText::new("Sort").color(MUTED));
            egui::ComboBox::from_id_salt(id.with("sort"))
                .selected_text(label(view.sort))
                .show_ui(ui, |ui| {
                    for sort in [
                        SortKey::Default,
                        SortKey::Recent,
                        SortKey::Title,
                        SortKey::Artist,
                        SortKey::Duration,
                    ] {
                        if sort == SortKey::Recent && recent_first {
                            continue;
                        }
                        ui.selectable_value(&mut view.sort, sort, label(sort));
                    }
                });
        });
        ui.add_space(10.0);
        let shown = filter_sort(tracks, &view.query, view.sort);
        let empty = shown.is_empty();
        ui.data_mut(|d| d.insert_temp(id, view));
        if empty {
            ui.add_space(20.0);
            ui.label(RichText::new("No songs match the filter").color(MUTED));
            return;
        }
        self.track_list_with(ui, shown, origin, header);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn track(title: &str, artist: &str, album: &str, duration: f64) -> Track {
        Track {
            id: title.into(),
            title: title.into(),
            artist: artist.into(),
            album: album.into(),
            duration,
            ..Default::default()
        }
    }

    fn titles(tracks: &[Track]) -> Vec<&str> {
        tracks.iter().map(|t| t.title.as_str()).collect()
    }

    #[test]
    fn filter_is_case_insensitive_over_title_artist_and_album() {
        let list = [
            track("Blue Monday", "New Order", "Power", 448.0),
            track("Song 2", "Blur", "Blur", 122.0),
            track("Hey", "Pixies", "Doolittle", 0.0),
        ];
        let filter = |q: &str| filter_sort(&list, q, SortKey::Default);
        assert_eq!(filter("").len(), 3);
        assert_eq!(titles(&filter("BLU")), ["Blue Monday", "Song 2"]);
        assert_eq!(titles(&filter("doolittle")), ["Hey"]);
        assert_eq!(titles(&filter("new power")), ["Blue Monday"]);
        assert!(filter("nothing").is_empty());
    }

    #[test]
    fn sorts_by_title_artist_duration_and_recent() {
        let list = [
            track("b", "Zed", "", 200.0),
            track("A", "zed", "", 0.0),
            track("c", "Abe", "", 100.0),
        ];
        let sort = |s: SortKey| filter_sort(&list, "", s);
        assert_eq!(titles(&sort(SortKey::Default)), ["b", "A", "c"]);
        assert_eq!(titles(&sort(SortKey::Recent)), ["c", "A", "b"]);
        assert_eq!(titles(&sort(SortKey::Title)), ["A", "b", "c"]);
        assert_eq!(titles(&sort(SortKey::Artist)), ["c", "A", "b"]);
        assert_eq!(titles(&sort(SortKey::Duration)), ["c", "b", "A"]);
    }
}
