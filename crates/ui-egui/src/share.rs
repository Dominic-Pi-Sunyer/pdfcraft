//! Share Mode: zero-server, end-to-end encrypted P2P multiplayer using `simple-sync`.
//!
//! Provides the command mutation serialization, sync loop polling, and the top toolbar
//! Share Mode controls for hosting, joining, and permissions enforcement.

use serde::{Deserialize, Serialize};
use simple_sync::{ShareLinks, ShareRole, SyncEngine};

use crate::PdfCraftApp;

/// Document mutations that are serialized and broadcast to connected peers.
///
/// Page indices and annotation `index`es are positions in the document the peers share; every
/// peer applies the same mutations in the order they arrive.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum DocMutation {
    RotatePages { pages: Vec<usize>, degrees: i64 },
    DeletePages { pages: Vec<usize> },
    MovePages { pages: Vec<usize>, to: usize },
    InsertBlankPage { at: usize, width: f64, height: f64 },
    DuplicatePages { pages: Vec<usize> },
    SetInfo { key: String, value: String },
    /// Any new annotation: notes, highlights, shapes, ink, text boxes, stamps, signatures.
    AddAnnotation(pdfcraft_engine::NewAnnotation),
    SetAnnotationContents { page: usize, index: usize, text: String },
    DeleteAnnotation { page: usize, index: usize },
    ReplyToAnnotation { page: usize, index: usize, text: String, author: String },
    SetAnnotationStatus { page: usize, index: usize, state: pdfcraft_engine::ReviewState, author: String },
    MoveAnnotation { page: usize, index: usize, dx: f64, dy: f64 },
    ResizeAnnotation { page: usize, index: usize, rect: [f64; 4] },
    StyleAnnotation {
        page: usize,
        index: usize,
        color: Option<pdfcraft_engine::Rgb>,
        opacity: Option<f64>,
        width: Option<f64>,
        endings: Option<Vec<pdfcraft_engine::LineEnding>>,
    },
    FillAnnotation { page: usize, index: usize, fill: Option<pdfcraft_engine::Rgb> },
    FontSizeAnnotation { page: usize, index: usize, size: f64 },
    SetAnnotationInfo {
        page: usize,
        index: usize,
        author: Option<String>,
        subject: Option<String>,
        icon: Option<pdfcraft_engine::NoteIcon>,
    },
    LockAnnotation { page: usize, index: usize, locked: bool },
    MarkAnnotation { page: usize, index: usize, marked: bool, author: String },
    /// A form field's value (text, check box, radio group, choice).
    SetFieldValue { name: String, value: pdfcraft_engine::FieldValue },
}

/// Alias for [`DocMutation`] following the simple-sync recipe conventions.
pub type EditAction = DocMutation;

impl DocMutation {
    /// Convert this serializable mutation into an engine [`pdfcraft_engine::Edit`].
    pub fn to_edit(&self) -> pdfcraft_engine::Edit {
        use pdfcraft_engine::Edit;
        match self.clone() {
            DocMutation::RotatePages { pages, degrees } => Edit::RotatePages { pages, degrees },
            DocMutation::DeletePages { pages } => Edit::DeletePages { pages },
            DocMutation::MovePages { pages, to } => Edit::MovePages { pages, to },
            DocMutation::InsertBlankPage { at, width, height } => Edit::InsertBlankPage { at, width, height },
            DocMutation::DuplicatePages { pages } => Edit::DuplicatePages { pages },
            DocMutation::SetInfo { key, value } => Edit::SetInfo { key, value },
            DocMutation::AddAnnotation(a) => Edit::AddAnnotation(a),
            DocMutation::SetAnnotationContents { page, index, text } => Edit::SetAnnotationContents { page, index, text },
            DocMutation::DeleteAnnotation { page, index } => Edit::DeleteAnnotation { page, index },
            DocMutation::ReplyToAnnotation { page, index, text, author } => {
                Edit::ReplyToAnnotation { page, index, text, author }
            }
            DocMutation::SetAnnotationStatus { page, index, state, author } => {
                Edit::SetAnnotationStatus { page, index, state, author }
            }
            DocMutation::MoveAnnotation { page, index, dx, dy } => Edit::MoveAnnotation { page, index, dx, dy },
            DocMutation::ResizeAnnotation { page, index, rect } => Edit::ResizeAnnotation { page, index, rect },
            DocMutation::StyleAnnotation { page, index, color, opacity, width, endings } => {
                Edit::StyleAnnotation { page, index, color, opacity, width, endings }
            }
            DocMutation::FillAnnotation { page, index, fill } => Edit::FillAnnotation { page, index, fill },
            DocMutation::FontSizeAnnotation { page, index, size } => Edit::FontSizeAnnotation { page, index, size },
            DocMutation::SetAnnotationInfo { page, index, author, subject, icon } => {
                Edit::SetAnnotationInfo { page, index, author, subject, icon }
            }
            DocMutation::LockAnnotation { page, index, locked } => Edit::LockAnnotation { page, index, locked },
            DocMutation::MarkAnnotation { page, index, marked, author } => {
                Edit::MarkAnnotation { page, index, marked, author }
            }
            DocMutation::SetFieldValue { name, value } => Edit::SetFieldValue { name, value },
        }
    }

    /// Try to convert an engine [`pdfcraft_engine::Edit`] into a serializable [`DocMutation`].
    /// `None`: this kind of edit is not shared live (it stays on this peer).
    pub fn from_edit(edit: &pdfcraft_engine::Edit) -> Option<Self> {
        use pdfcraft_engine::Edit;
        Some(match edit.clone() {
            Edit::RotatePages { pages, degrees } => DocMutation::RotatePages { pages, degrees },
            Edit::DeletePages { pages } => DocMutation::DeletePages { pages },
            Edit::MovePages { pages, to } => DocMutation::MovePages { pages, to },
            Edit::InsertBlankPage { at, width, height } => DocMutation::InsertBlankPage { at, width, height },
            Edit::DuplicatePages { pages } => DocMutation::DuplicatePages { pages },
            Edit::SetInfo { key, value } => DocMutation::SetInfo { key, value },
            // A custom stamp points at an object number in this peer's copy of the file.
            Edit::AddAnnotation(a) if matches!(a.shape, pdfcraft_engine::Shape::CustomStamp { .. }) => {
                let _ = a;
                return None;
            }
            Edit::AddAnnotation(a) => DocMutation::AddAnnotation(a),
            Edit::SetAnnotationContents { page, index, text } => DocMutation::SetAnnotationContents { page, index, text },
            Edit::DeleteAnnotation { page, index } => DocMutation::DeleteAnnotation { page, index },
            Edit::ReplyToAnnotation { page, index, text, author } => {
                DocMutation::ReplyToAnnotation { page, index, text, author }
            }
            Edit::SetAnnotationStatus { page, index, state, author } => {
                DocMutation::SetAnnotationStatus { page, index, state, author }
            }
            Edit::MoveAnnotation { page, index, dx, dy } => DocMutation::MoveAnnotation { page, index, dx, dy },
            Edit::ResizeAnnotation { page, index, rect } => DocMutation::ResizeAnnotation { page, index, rect },
            Edit::StyleAnnotation { page, index, color, opacity, width, endings } => {
                DocMutation::StyleAnnotation { page, index, color, opacity, width, endings }
            }
            Edit::FillAnnotation { page, index, fill } => DocMutation::FillAnnotation { page, index, fill },
            Edit::FontSizeAnnotation { page, index, size } => DocMutation::FontSizeAnnotation { page, index, size },
            Edit::SetAnnotationInfo { page, index, author, subject, icon } => {
                DocMutation::SetAnnotationInfo { page, index, author, subject, icon }
            }
            Edit::LockAnnotation { page, index, locked } => DocMutation::LockAnnotation { page, index, locked },
            Edit::MarkAnnotation { page, index, marked, author } => {
                DocMutation::MarkAnnotation { page, index, marked, author }
            }
            Edit::SetFieldValue { name, value } => DocMutation::SetFieldValue { name, value },
            _ => return None,
        })
    }
}

/// What a background share/join start produced.
pub enum Started {
    Hosting(ShareLinks),
    Joined,
}

/// Sent back by the thread that starts sharing: the engine (so the app owns it again) and the result.
pub struct ShareOutcome {
    pub engine: SyncEngine,
    pub result: Result<Started, String>,
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
    pub(crate) fn on_stop(&mut self, sync: &mut SyncEngine) {
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
        if app.share_pending_now() {
            ui.spinner();
        }
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
                            app.begin_share(filename, bytes);
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
                    let ticket = app.share_toolbar.join_input.trim().to_string();
                    app.begin_join(ticket);
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
                    app.clear_shared_doc();
                }
            }
            ToolbarView::JoinedCanEdit { label } => {
                ui.label(egui::RichText::new(&label).strong());
                if ui.button(tl!("Leave")).clicked() {
                    if let Some(sync) = &mut app.sync {
                        app.share_toolbar.on_stop(sync);
                    }
                    app.clear_shared_doc();
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
                    app.clear_shared_doc();
                }
            }
        }
    });

    if let Some(err) = &app.share_toolbar.error {
        ui.colored_label(egui::Color32::from_rgb(220, 60, 60), err);
    }
}
