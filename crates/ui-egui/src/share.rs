//! Share Mode: zero-server, end-to-end encrypted P2P multiplayer using `simple-sync`.
//!
//! Provides the command mutation serialization, sync loop polling, and the top toolbar
//! Share Mode controls for hosting, joining, and permissions enforcement.

use serde::{Deserialize, Serialize};
use simple_sync::{ShareLinks, ShareRole, SyncEngine};

use crate::PdfCraftApp;

/// Document mutations that are serialized and broadcast to connected peers.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum DocMutation {
    RotatePages { pages: Vec<usize>, degrees: i64 },
    DeletePages { pages: Vec<usize> },
    MovePages { pages: Vec<usize>, to: usize },
    InsertBlankPage { at: usize, width: f64, height: f64 },
    DuplicatePages { pages: Vec<usize> },
    SetInfo { key: String, value: String },
    AddNote { page: usize, at: [f64; 2], text: String, author: String },
    SetAnnotationContents { page: usize, index: usize, text: String },
    DeleteAnnotation { page: usize, index: usize },
}

/// Alias for [`DocMutation`] following the simple-sync recipe conventions.
pub type EditAction = DocMutation;

impl DocMutation {
    /// Convert this serializable mutation into an engine [`pdfcraft_engine::Edit`].
    pub fn to_edit(&self) -> pdfcraft_engine::Edit {
        match self {
            DocMutation::RotatePages { pages, degrees } => {
                pdfcraft_engine::Edit::RotatePages { pages: pages.clone(), degrees: *degrees }
            }
            DocMutation::DeletePages { pages } => {
                pdfcraft_engine::Edit::DeletePages { pages: pages.clone() }
            }
            DocMutation::MovePages { pages, to } => {
                pdfcraft_engine::Edit::MovePages { pages: pages.clone(), to: *to }
            }
            DocMutation::InsertBlankPage { at, width, height } => {
                pdfcraft_engine::Edit::InsertBlankPage { at: *at, width: *width, height: *height }
            }
            DocMutation::DuplicatePages { pages } => {
                pdfcraft_engine::Edit::DuplicatePages { pages: pages.clone() }
            }
            DocMutation::SetInfo { key, value } => {
                pdfcraft_engine::Edit::SetInfo { key: key.clone(), value: value.clone() }
            }
            DocMutation::AddNote { page, at, text, author } => {
                pdfcraft_engine::Edit::AddAnnotation(pdfcraft_engine::NewAnnotation {
                    page: *page,
                    shape: pdfcraft_engine::Shape::Note { at: *at, icon: pdfcraft_engine::NoteIcon::Comment },
                    style: Default::default(),
                    contents: text.clone(),
                    author: author.clone(),
                })
            }
            DocMutation::SetAnnotationContents { page, index, text } => {
                pdfcraft_engine::Edit::SetAnnotationContents {
                    page: *page,
                    index: *index,
                    text: text.clone(),
                }
            }
            DocMutation::DeleteAnnotation { page, index } => {
                pdfcraft_engine::Edit::DeleteAnnotation { page: *page, index: *index }
            }
        }
    }

    /// Try to convert an engine [`pdfcraft_engine::Edit`] into a serializable [`DocMutation`].
    pub fn from_edit(edit: &pdfcraft_engine::Edit) -> Option<Self> {
        match edit {
            pdfcraft_engine::Edit::RotatePages { pages, degrees } => {
                Some(DocMutation::RotatePages { pages: pages.clone(), degrees: *degrees })
            }
            pdfcraft_engine::Edit::DeletePages { pages } => {
                Some(DocMutation::DeletePages { pages: pages.clone() })
            }
            pdfcraft_engine::Edit::MovePages { pages, to } => {
                Some(DocMutation::MovePages { pages: pages.clone(), to: *to })
            }
            pdfcraft_engine::Edit::InsertBlankPage { at, width, height } => {
                Some(DocMutation::InsertBlankPage { at: *at, width: *width, height: *height })
            }
            pdfcraft_engine::Edit::DuplicatePages { pages } => {
                Some(DocMutation::DuplicatePages { pages: pages.clone() })
            }
            pdfcraft_engine::Edit::SetInfo { key, value } => {
                Some(DocMutation::SetInfo { key: key.clone(), value: value.clone() })
            }
            pdfcraft_engine::Edit::AddAnnotation(a) => match a.shape {
                // Only a plain comment note round-trips losslessly; other shapes stay local.
                pdfcraft_engine::Shape::Note { at, icon: pdfcraft_engine::NoteIcon::Comment }
                    if a.style == Default::default() =>
                {
                    Some(DocMutation::AddNote {
                    page: a.page,
                    at,
                    text: a.contents.clone(),
                    author: a.author.clone(),
                    })
                }
                _ => None,
            },
            pdfcraft_engine::Edit::SetAnnotationContents { page, index, text } => {
                Some(DocMutation::SetAnnotationContents {
                    page: *page,
                    index: *index,
                    text: text.clone(),
                })
            }
            pdfcraft_engine::Edit::DeleteAnnotation { page, index } => {
                Some(DocMutation::DeleteAnnotation { page: *page, index: *index })
            }
            _ => None,
        }
    }
}

/// What the Share Mode toolbar should render this frame.
#[derive(Debug, PartialEq)]
pub enum ToolbarView {
    /// `[Share Document]` button + `[Join via Link]` input.
    Offline,
    /// `🟢 Live (N Peers)`, `[Copy Edit Link]`, `[Copy View-Only Link]`, red `[Stop Sharing]`.
    Hosting { label: String },
    /// `🟢 Live (N Peers)` + `[Leave]`. Local mutation broadcasting stays enabled.
    JoinedCanEdit { label: String },
    /// `🔒 View Only` badge. Local mutations must NOT be broadcast.
    JoinedViewOnly { label: String },
}

/// State tracking for the Share Mode toolbar controls.
pub struct ShareToolbar {
    pub join_input: String,
    pub links: Option<ShareLinks>,
    pub error: Option<String>,
}

impl Default for ShareToolbar {
    fn default() -> Self {
        Self::new()
    }
}

impl ShareToolbar {
    pub fn new() -> Self {
        Self { join_input: String::new(), links: None, error: None }
    }

    pub fn view(&self, sync: &SyncEngine) -> ToolbarView {
        let n = sync.connected_peers_count();
        let live = format!("🟢 Live ({n} Peer{})", if n == 1 { "" } else { "s" });
        match sync.role() {
            ShareRole::Offline => ToolbarView::Offline,
            ShareRole::Hosting => ToolbarView::Hosting { label: live },
            ShareRole::JoinedCanEdit => ToolbarView::JoinedCanEdit { label: live },
            ShareRole::JoinedViewOnly => ToolbarView::JoinedViewOnly { label: "🔒 View Only".into() },
        }
    }

    /// `[Share Document]`
    pub fn on_share(&mut self, sync: &mut SyncEngine, filename: &str, current_file_bytes: Vec<u8>) {
        match sync.enable_share_mode(filename, current_file_bytes) {
            Ok(links) => {
                self.links = Some(links);
                self.error = None;
            }
            Err(e) => {
                self.error = Some(e.to_string());
            }
        }
    }

    /// `[Join via Link]`
    pub fn on_join(&mut self, sync: &mut SyncEngine) {
        let trimmed = self.join_input.trim();
        if trimmed.is_empty() {
            return;
        }
        match sync.join_via_ticket(trimmed) {
            Ok(_) => {
                self.join_input.clear();
                self.error = None;
            }
            Err(e) => {
                self.error = Some(e.to_string());
            }
        }
    }

    /// `[Stop Sharing]` (host) or `[Leave]` (joiner): the kill switch.
    pub fn on_stop(&mut self, sync: &mut SyncEngine) {
        if let Err(e) = sync.disable_share_mode() {
            self.error = Some(e.to_string());
        }
        self.links = None;
    }

    pub fn edit_link(&self) -> Option<&str> {
        self.links.as_ref().map(|l| l.edit_uri.as_str())
    }

    pub fn view_only_link(&self) -> Option<&str> {
        self.links.as_ref().map(|l| l.view_only_uri.as_str())
    }
}

/// Gate for local edit paths. Disallow local edits when in a View-Only session.
pub fn local_editing_allowed(sync: &SyncEngine) -> bool {
    sync.role() != ShareRole::JoinedViewOnly
}

/// Render the Share Mode controls in the top toolbar.
pub fn show_share_toolbar(app: &mut PdfCraftApp, ui: &mut egui::Ui) {
    let Some(sync) = &app.sync else {
        return;
    };
    let view = app.share_toolbar.view(sync);

    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 4.0;
        match view {
            ToolbarView::Offline => {
                let has_doc = app.active.is_some();
                let share_btn = ui.add_enabled(has_doc, egui::Button::new(tl!("Share Document")));
                if share_btn.clicked() {
                    if let Some((_, id)) = app.active_ids() {
                        let filename = app
                            .session
                            .get(id)
                            .map(|d| d.display_name())
                            .unwrap_or_else(|| "document.pdf".to_string());
                        let bytes = app
                            .session
                            .save_bytes(id)
                            .map(|b| (*b).clone())
                            .unwrap_or_else(|_| {
                                app.session
                                    .get(id)
                                    .map(|d| (*d.bytes).clone())
                                    .unwrap_or_default()
                            });
                        if !bytes.is_empty() {
                            if let Some(sync) = &mut app.sync {
                                app.share_toolbar.on_share(sync, &filename, bytes);
                            }
                        } else {
                            app.share_toolbar.error = Some(tl!("No document data to share").to_string());
                        }
                    }
                }

                ui.add(
                    egui::TextEdit::singleline(&mut app.share_toolbar.join_input)
                        .hint_text("ssync://...")
                        .desired_width(110.0),
                );

                let has_input = !app.share_toolbar.join_input.trim().is_empty();
                if ui.add_enabled(has_input, egui::Button::new(tl!("Join via Link"))).clicked() {
                    if let Some(sync) = &mut app.sync {
                        app.share_toolbar.on_join(sync);
                    }
                }
            }
            ToolbarView::Hosting { label } => {
                ui.label(egui::RichText::new(&label).strong());
                if ui.button(tl!("Copy Edit Link")).clicked() {
                    if let Some(link) = app.share_toolbar.edit_link() {
                        ui.ctx().copy_text(link.to_string());
                        app.notify(tl!("Edit link copied to clipboard"));
                    }
                }
                if ui.button(tl!("Copy View-Only Link")).clicked() {
                    if let Some(link) = app.share_toolbar.view_only_link() {
                        ui.ctx().copy_text(link.to_string());
                        app.notify(tl!("View-only link copied to clipboard"));
                    }
                }
                let stop_button = egui::Button::new(
                    egui::RichText::new(tl!("Stop Sharing")).color(egui::Color32::WHITE),
                )
                .fill(egui::Color32::from_rgb(200, 40, 40));
                if ui.add(stop_button).clicked() {
                    if let Some(sync) = &mut app.sync {
                        app.share_toolbar.on_stop(sync);
                    }
                }
            }
            ToolbarView::JoinedCanEdit { label } => {
                ui.label(egui::RichText::new(&label).strong());
                if ui.button(tl!("Leave")).clicked() {
                    if let Some(sync) = &mut app.sync {
                        app.share_toolbar.on_stop(sync);
                    }
                }
            }
            ToolbarView::JoinedViewOnly { label } => {
                ui.label(
                    egui::RichText::new(&label)
                        .color(egui::Color32::from_rgb(220, 160, 40))
                        .strong(),
                );
                if ui.button(tl!("Leave")).clicked() {
                    if let Some(sync) = &mut app.sync {
                        app.share_toolbar.on_stop(sync);
                    }
                }
            }
        }
    });

    if let Some(err) = &app.share_toolbar.error {
        ui.colored_label(egui::Color32::from_rgb(220, 60, 60), err);
    }
}
