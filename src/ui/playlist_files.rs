//! Playlist files (a YTDugem addition): export a list to M3U8 or CSV and import an M3U/M3U8
//! file as a new local playlist, through `crate::playlist_io`. File IO runs on a worker thread.
use super::*;
use crate::playlist_io::{self, Format, Parsed};

/// A file name from a playlist name: characters Windows forbids become `_`.
fn file_stem(name: &str) -> String {
    let stem: String = name
        .trim()
        .chars()
        .map(|c| {
            if c.is_control() || r#"<>:"/\|?*"#.contains(c) {
                '_'
            } else {
                c
            }
        })
        .collect();
    let stem = stem.trim_end_matches(['.', ' ']).to_owned();
    if stem.is_empty() {
        "playlist".into()
    } else {
        stem
    }
}

/// `path` with its extension swapped for `format`'s.
fn with_format(path: &str, format: Format) -> String {
    let p = Path::new(path.trim());
    if p.as_os_str().is_empty() {
        return String::new();
    }
    p.with_extension(format.extension())
        .to_string_lossy()
        .into_owned()
}

/// A path typed or pasted into a box; Explorer's "Copy as path" adds quotes.
fn typed_path(text: &str) -> PathBuf {
    PathBuf::from(text.trim().trim_matches('"'))
}

struct ExportDialog {
    title: String,
    tracks: Vec<Track>,
    path: String,
    format: Format,
    error: String,
}

/// The export dialog and the running import or export.
#[derive(Default)]
pub(super) struct PlaylistFiles {
    export: Option<ExportDialog>,
    export_rx: Option<Receiver<Result<PathBuf, String>>>,
    import_path: String,
    import_rx: Option<Receiver<Result<(String, Parsed), String>>>,
}

impl YTDugem {
    /// Opens the export dialog for `tracks`, suggesting a file in the download folder.
    pub(super) fn open_export(&mut self, title: &str, tracks: Vec<Track>) {
        let path = self
            .library
            .download_dir
            .join(format!("{}.m3u8", file_stem(title)));
        self.playlist_files.export = Some(ExportDialog {
            title: title.to_owned(),
            tracks,
            path: path.to_string_lossy().into_owned(),
            format: Format::M3u8,
            error: String::new(),
        });
    }

    /// A path box and "Import playlist file" button, for the Library page.
    pub(super) fn playlist_import_row(&mut self, ui: &mut egui::Ui) {
        let busy = self.playlist_files.import_rx.is_some();
        ui.horizontal(|ui| {
            let r = ui.add(
                egui::TextEdit::singleline(&mut self.playlist_files.import_path)
                    .margin(TEXT_MARGIN)
                    .desired_width(420.0)
                    .hint_text("Path of an .m3u or .m3u8 playlist file"),
            );
            let go = ui
                .add_enabled(!busy, egui::Button::new("Import playlist file"))
                .clicked()
                || (!busy && r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)));
            if busy {
                ui.spinner();
            }
            if go {
                let path = typed_path(&self.playlist_files.import_path);
                let (tx, rx) = mpsc::channel();
                let repaint = ui.ctx().clone();
                std::thread::spawn(move || {
                    let _ = tx.send(playlist_io::import(&path).map_err(err));
                    repaint.request_repaint();
                });
                self.playlist_files.import_rx = Some(rx);
            }
        });
    }

    /// The export dialog, and the results of a running import or export.
    pub(super) fn playlist_files_step(&mut self, ctx: &egui::Context) {
        let imported = (self.playlist_files.import_rx.as_ref()).and_then(|rx| rx.try_recv().ok());
        if let Some(result) = imported {
            self.playlist_files.import_rx = None;
            self.imported_playlist(result);
        }
        let exported = (self.playlist_files.export_rx.as_ref()).and_then(|rx| rx.try_recv().ok());
        if let Some(result) = exported {
            self.playlist_files.export_rx = None;
            match result {
                Ok(path) => {
                    self.playlist_files.export = None;
                    self.toast_success(format!("Exported to {}", path.display()));
                }
                Err(e) => match self.playlist_files.export.as_mut() {
                    Some(dialog) => dialog.error = e,
                    None => self.toast_error(e),
                },
            }
        }
        let busy = self.playlist_files.export_rx.is_some();
        let Some(dialog) = self.playlist_files.export.as_mut() else {
            return;
        };
        let mut close = false;
        let mut start = false;
        egui::Window::new("Export playlist")
            .collapsible(false)
            .resizable(false)
            .frame(widgets::dialog_card())
            .anchor(egui::Align2::CENTER_CENTER, Vec2::ZERO)
            .show(ctx, |ui| {
                ui.set_width(460.0);
                ui.label(format!(
                    "Save “{}” ({} songs) as a new file. Local songs are written as file paths, online songs as YouTube Music links. An existing file is never replaced.",
                    dialog.title,
                    dialog.tracks.len()
                ));
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    for format in [Format::M3u8, Format::Csv] {
                        if ui
                            .selectable_label(dialog.format == format, format.label())
                            .clicked()
                        {
                            dialog.format = format;
                            dialog.path = with_format(&dialog.path, format);
                        }
                    }
                });
                ui.add(
                    egui::TextEdit::singleline(&mut dialog.path)
                        .margin(TEXT_MARGIN)
                        .desired_width(f32::INFINITY)
                        .hint_text("File to create"),
                );
                if !dialog.error.is_empty() {
                    ui.label(RichText::new(&dialog.error).color(ERROR));
                }
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    ui.add_enabled_ui(!busy, |ui| {
                        if widgets::action(ui, widgets::Action::Primary, "Export").clicked() {
                            start = true;
                        }
                    });
                    if busy {
                        ui.spinner();
                    }
                    if widgets::action(ui, widgets::Action::Ghost, "Cancel").clicked() {
                        close = true;
                    }
                });
            });
        if start {
            dialog.error.clear();
            let path = typed_path(&dialog.path);
            let tracks = dialog.tracks.clone();
            let (tx, rx) = mpsc::channel();
            let repaint = ctx.clone();
            std::thread::spawn(move || {
                let _ = tx.send(playlist_io::export(&path, &tracks).map_err(err));
                repaint.request_repaint();
            });
            self.playlist_files.export_rx = Some(rx);
        }
        if close {
            // A running export still finishes; its result then shows as a toast.
            self.playlist_files.export = None;
        }
    }

    /// Turns an imported file into a new local playlist and opens it.
    fn imported_playlist(&mut self, result: Result<(String, Parsed), String>) {
        let (name, parsed) = match result {
            Ok(r) => r,
            Err(e) => {
                self.toast_error(e);
                return;
            }
        };
        if parsed.tracks.is_empty() {
            self.toast_error(format!(
                "No playable songs in “{name}” ({} entries skipped)",
                parsed.skipped
            ));
            return;
        }
        let index = match self.library.create_playlist(&name, None) {
            Ok(i) => i,
            Err(e) => {
                self.toast_error(format!("{e:#}"));
                return;
            }
        };
        let added = parsed
            .tracks
            .into_iter()
            .filter(|t| self.library.add_to_playlist(index, t.clone()))
            .count();
        self.save();
        self.playlist_files.import_path.clear();
        let skipped = if parsed.skipped > 0 {
            format!(" ({} entries skipped)", parsed.skipped)
        } else {
            String::new()
        };
        self.toast_success(format!("Imported {added} songs into “{name}”{skipped}"));
        self.navigate(Page::Playlist(index));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_stems_drop_forbidden_characters() {
        assert_eq!(file_stem("Road trip"), "Road trip");
        assert_eq!(file_stem("a/b:c*?"), "a_b_c__");
        assert_eq!(file_stem("  ..  "), "playlist");
        assert_eq!(file_stem("mix."), "mix");
    }

    #[test]
    fn format_swaps_the_extension_and_quotes_are_dropped() {
        assert_eq!(
            with_format(
                &format!("x{}mix.m3u8", std::path::MAIN_SEPARATOR),
                Format::Csv
            ),
            format!("x{}mix.csv", std::path::MAIN_SEPARATOR)
        );
        assert_eq!(with_format("", Format::Csv), "");
        assert_eq!(typed_path(" \"C:/a b.m3u\" "), PathBuf::from("C:/a b.m3u"));
    }
}
