//! Integration tests for zero-server P2P multiplayer ("Share Mode") using `simple-sync`.

use std::time::{Duration, Instant};

use pdfcraft_ui_egui::PdfCraftApp;
use pdfcraft_ui_egui::share::DocMutation;
use simple_sync::{ShareRole, SyncConfig, SyncEngine};

/// A 2-page valid synthetic PDF fixture.
fn fixture(n: usize) -> Vec<u8> {
    let mut objs: Vec<String> = vec!["<< /Type /Catalog /Pages 2 0 R >>".into()];
    let kids: Vec<String> = (0..n).map(|i| format!("{} 0 R", 4 + 2 * i)).collect();
    objs.push(format!("<< /Type /Pages /Kids [{}] /Count {n} /MediaBox [0 0 200 300] >>", kids.join(" ")));
    objs.push("<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".into());
    for i in 0..n {
        objs.push(format!("<< /Type /Page /Parent 2 0 R /Contents {} 0 R /Resources << /Font << /F1 3 0 R >> >> >>", 5 + 2 * i));
        let body = format!("BT /F1 24 Tf 20 150 Td (Page {}) Tj ET", i + 1);
        objs.push(format!("<< /Length {} >>\nstream\n{body}\nendstream", body.len()));
    }
    let mut out = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (i, o) in objs.iter().enumerate() {
        offsets.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n{o}\nendobj\n", i + 1).as_bytes());
    }
    let xref = out.len();
    out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).as_bytes());
    for o in offsets {
        out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objs.len() + 1).as_bytes());
    out
}

#[test]
fn test_p2p_share_mode_transfer_and_edit_sync() {
    let mut host = PdfCraftApp::new();
    host.sync = Some(SyncEngine::with_config("test-host", SyncConfig::local_only()).unwrap());

    let mut joiner = PdfCraftApp::new();
    joiner.sync = Some(SyncEngine::with_config("test-joiner", SyncConfig::local_only()).unwrap());

    let pdf = fixture(2);
    host.open_bytes("contract.pdf", None, pdf.clone()).expect("host opens fixture PDF");
    assert_eq!(host.active_document().unwrap().info.pages.len(), 2);
    assert_eq!(host.active_document().unwrap().info.pages[0].rotation, 0);

    // Host enables Share Mode
    let links = host.enable_share_mode("contract.pdf", pdf.clone()).expect("enable share mode");
    assert_eq!(host.share_role(), ShareRole::Hosting);
    assert!(links.edit_uri.starts_with("ssync://"));
    assert!(links.view_only_uri.starts_with("ssync://"));

    // Joiner joins via edit ticket
    let role = joiner.join_session(&links.edit_uri).expect("joiner connects with edit ticket");
    assert_eq!(role, ShareRole::JoinedCanEdit);
    assert_eq!(joiner.share_role(), ShareRole::JoinedCanEdit);

    // Pump until base document is transferred and loaded on joiner
    let deadline = Instant::now() + Duration::from_secs(30);
    while joiner.active_document().is_none() {
        host.pump_sync(None);
        joiner.pump_sync(None);
        assert!(Instant::now() < deadline, "timed out waiting for base PDF transfer to joiner");
        std::thread::sleep(Duration::from_millis(10));
    }

    assert_eq!(joiner.active_document().unwrap().info.pages.len(), 2);
    assert_eq!(joiner.active_document().unwrap().info.pages[0].rotation, 0);

    // Host dispatches a document edit (e.g. RotatePages)
    let mutation = DocMutation::RotatePages { pages: vec![0], degrees: 90 };
    let applied = host.apply_local_mutation(mutation);
    assert!(applied, "host applied mutation");
    assert_eq!(host.active_document().unwrap().info.pages[0].rotation, 90);

    // Pump until joiner receives and applies the remote mutation
    let deadline = Instant::now() + Duration::from_secs(30);
    while joiner.active_document().unwrap().info.pages[0].rotation != 90 {
        host.pump_sync(None);
        joiner.pump_sync(None);
        assert!(Instant::now() < deadline, "timed out waiting for remote mutation to arrive at joiner");
        std::thread::sleep(Duration::from_millis(10));
    }

    assert_eq!(
        joiner.active_document().unwrap().info.pages[0].rotation,
        90,
        "joiner applied remote rotation mutation"
    );

    // Editor joiner can also broadcast a mutation back to host
    let editor_mutation = DocMutation::RotatePages { pages: vec![1], degrees: 180 };
    let editor_applied = joiner.apply_local_mutation(editor_mutation);
    assert!(editor_applied, "joiner applied local mutation");
    assert_eq!(joiner.active_document().unwrap().info.pages[1].rotation, 180);

    let deadline = Instant::now() + Duration::from_secs(30);
    while host.active_document().unwrap().info.pages[1].rotation != 180 {
        host.pump_sync(None);
        joiner.pump_sync(None);
        assert!(Instant::now() < deadline, "timed out waiting for joiner mutation at host");
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(host.active_document().unwrap().info.pages[1].rotation, 180);
}

#[test]
fn test_p2p_share_mode_view_only_permissions() {
    let mut host = PdfCraftApp::new();
    host.sync = Some(SyncEngine::with_config("test-host-vo", SyncConfig::local_only()).unwrap());

    let mut viewer = PdfCraftApp::new();
    viewer.sync = Some(SyncEngine::with_config("test-viewer-vo", SyncConfig::local_only()).unwrap());

    let pdf = fixture(2);
    host.open_bytes("report.pdf", None, pdf.clone()).expect("open host report");
    let links = host.enable_share_mode("report.pdf", pdf.clone()).expect("enable share mode");

    let role = viewer.join_session(&links.view_only_uri).expect("join view-only");
    assert_eq!(role, ShareRole::JoinedViewOnly);
    assert_eq!(viewer.share_role(), ShareRole::JoinedViewOnly);
    assert!(!viewer.local_editing_allowed());

    // Wait for base document
    let deadline = Instant::now() + Duration::from_secs(30);
    while viewer.active_document().is_none() {
        host.pump_sync(None);
        viewer.pump_sync(None);
        assert!(Instant::now() < deadline, "timed out waiting for viewer document");
        std::thread::sleep(Duration::from_millis(10));
    }

    // Viewer cannot apply local mutations
    let local_edit = DocMutation::RotatePages { pages: vec![0], degrees: 90 };
    let refused = viewer.apply_local_mutation(local_edit);
    assert!(!refused, "view-only peer must refuse local mutation");
    assert_eq!(viewer.active_document().unwrap().info.pages[0].rotation, 0);

    // Host can still edit and viewer receives the update
    host.apply_local_mutation(DocMutation::RotatePages { pages: vec![0], degrees: 90 });
    assert_eq!(host.active_document().unwrap().info.pages[0].rotation, 90);

    let deadline = Instant::now() + Duration::from_secs(30);
    while viewer.active_document().unwrap().info.pages[0].rotation != 90 {
        host.pump_sync(None);
        viewer.pump_sync(None);
        assert!(Instant::now() < deadline, "timed out waiting for viewer update");
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(viewer.active_document().unwrap().info.pages[0].rotation, 90);
}

#[test]
fn test_p2p_share_mode_kill_switch() {
    let mut host = PdfCraftApp::new();
    host.sync = Some(SyncEngine::with_config("test-kill", SyncConfig::local_only()).unwrap());

    let pdf = fixture(1);
    host.open_bytes("doc.pdf", None, pdf.clone()).unwrap();
    let _links = host.enable_share_mode("doc.pdf", pdf).unwrap();
    assert_eq!(host.share_role(), ShareRole::Hosting);
    assert!(host.share_toolbar.links.is_some());

    // Host hits kill switch
    host.disable_share_mode().expect("disable share mode");
    assert_eq!(host.share_role(), ShareRole::Offline);
    assert!(host.share_toolbar.links.is_none());
}

/// A peer who joins after edits were made must end up with the same document as everyone else,
/// not the bytes that were shared when Share Mode started.
#[test]
fn test_p2p_late_joiner_receives_earlier_edits() {
    let mut host = PdfCraftApp::new();
    host.sync = Some(SyncEngine::with_config("test-late-host", SyncConfig::local_only()).unwrap());
    let mut late = PdfCraftApp::new();
    late.sync = Some(SyncEngine::with_config("test-late-joiner", SyncConfig::local_only()).unwrap());

    let pdf = fixture(3);
    host.open_bytes("late.pdf", None, pdf.clone()).expect("host opens fixture");
    let links = host.enable_share_mode("late.pdf", pdf).expect("enable share mode");

    // Edits made while nobody else is connected yet.
    assert!(host.apply_local_mutation(DocMutation::RotatePages { pages: vec![0], degrees: 90 }));
    assert!(host.apply_local_mutation(DocMutation::RotatePages { pages: vec![2], degrees: 180 }));

    late.join_session(&links.edit_uri).expect("late joiner connects");

    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        host.pump_sync(None);
        late.pump_sync(None);
        if let Some(doc) = late.active_document() {
            let pages = &doc.info.pages;
            if pages.len() == 3 && pages[0].rotation == 90 && pages[2].rotation == 180 {
                break;
            }
        }
        assert!(Instant::now() < deadline, "late joiner never caught up with the host's earlier edits");
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(late.active_document().unwrap().info.pages[1].rotation, 0);
}

/// Highlights, shapes and other annotations are shared, and so are changes to them.
#[test]
fn test_p2p_annotation_edits_sync() {
    use pdfcraft_engine::{Edit, NewAnnotation, Shape, Style};

    let mut host = PdfCraftApp::new();
    host.sync = Some(SyncEngine::with_config("test-annot-host", SyncConfig::local_only()).unwrap());
    let mut peer = PdfCraftApp::new();
    peer.sync = Some(SyncEngine::with_config("test-annot-peer", SyncConfig::local_only()).unwrap());

    let pdf = fixture(1);
    host.open_bytes("annot.pdf", None, pdf.clone()).unwrap();
    let links = host.enable_share_mode("annot.pdf", pdf).unwrap();
    peer.join_session(&links.edit_uri).unwrap();

    let pump_until = |host: &mut PdfCraftApp, peer: &mut PdfCraftApp, what: &str, done: &dyn Fn(&PdfCraftApp) -> bool| {
        let deadline = Instant::now() + Duration::from_secs(15);
        while !done(peer) {
            host.pump_sync(None);
            peer.pump_sync(None);
            assert!(Instant::now() < deadline, "timed out: {what}");
            std::thread::sleep(Duration::from_millis(10));
        }
    };
    pump_until(&mut host, &mut peer, "base document", &|p| p.active_document().is_some());

    let shape = Shape::Rectangle { rect: [20.0, 20.0, 120.0, 90.0] };
    let style = Style::default_for(&shape);
    assert!(host.apply_edit(Edit::AddAnnotation(NewAnnotation {
        page: 0,
        shape,
        style,
        contents: "check this".into(),
        author: "Host".into(),
    })));
    pump_until(&mut host, &mut peer, "annotation added", &|p| {
        p.active_document().is_some_and(|d| d.info.annotations.len() == 1)
    });
    assert_eq!(peer.active_document().unwrap().info.annotations[0].contents.as_deref(), Some("check this"));

    // The note keeps the position it was placed at (it must not be reset by the sharing path).
    let placed = host.active_document().unwrap().info.annotations[0].rect;
    assert_eq!(peer.active_document().unwrap().info.annotations[0].rect, placed);

    assert!(host.apply_edit(Edit::MoveAnnotation { page: 0, index: 0, dx: 30.0, dy: 0.0 }));
    let moved = host.active_document().unwrap().info.annotations[0].rect;
    assert_ne!(moved, placed);
    pump_until(&mut host, &mut peer, "annotation moved", &|p| {
        p.active_document().is_some_and(|d| d.info.annotations[0].rect == moved)
    });

    assert!(host.apply_edit(Edit::DeleteAnnotation { page: 0, index: 0 }));
    pump_until(&mut host, &mut peer, "annotation deleted", &|p| {
        p.active_document().is_some_and(|d| d.info.annotations.is_empty())
    });
}

/// A peer's edit goes to the shared document even when another tab is showing, and an edit made
/// in that other tab is not sent to the peers at all.
#[test]
fn test_p2p_edits_follow_the_shared_document_not_the_active_tab() {
    let mut host = PdfCraftApp::new();
    host.sync = Some(SyncEngine::with_config("test-tab-host", SyncConfig::local_only()).unwrap());
    let mut peer = PdfCraftApp::new();
    peer.sync = Some(SyncEngine::with_config("test-tab-peer", SyncConfig::local_only()).unwrap());

    let pdf = fixture(2);
    host.open_bytes("shared.pdf", None, pdf.clone()).unwrap();
    let links = host.enable_share_mode("shared.pdf", pdf).unwrap();
    peer.join_session(&links.edit_uri).unwrap();
    let deadline = Instant::now() + Duration::from_secs(15);
    while peer.shared_document().is_none() {
        host.pump_sync(None);
        peer.pump_sync(None);
        assert!(Instant::now() < deadline, "peer never received the document");
        std::thread::sleep(Duration::from_millis(10));
    }

    // The peer opens another document: it is now the active tab.
    peer.open_bytes("other.pdf", None, fixture(2)).unwrap();
    assert!(!peer.active_is_shared());
    // The host does the same and edits that other document: nothing may be sent.
    host.open_bytes("scratch.pdf", None, fixture(2)).unwrap();
    assert!(!host.active_is_shared());
    assert!(host.apply_local_mutation(DocMutation::RotatePages { pages: vec![1], degrees: 90 }));

    // An edit to the shared document, made after switching the host back to it.
    host.active = Some(0);
    assert!(host.active_is_shared());
    assert!(host.apply_local_mutation(DocMutation::RotatePages { pages: vec![0], degrees: 180 }));

    let deadline = Instant::now() + Duration::from_secs(15);
    while peer.shared_document().unwrap().info.pages[0].rotation != 180 {
        host.pump_sync(None);
        peer.pump_sync(None);
        assert!(Instant::now() < deadline, "peer never received the edit");
        std::thread::sleep(Duration::from_millis(10));
    }
    // The edit landed on the shared document, not on the tab the peer had in front.
    assert_eq!(peer.shared_document().unwrap().info.pages[1].rotation, 0, "the scratch edit was not shared");
    assert_eq!(peer.active_document().unwrap().info.pages[0].rotation, 0, "the other tab is untouched");
}
