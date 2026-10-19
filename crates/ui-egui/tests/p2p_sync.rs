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
