use super::OpenCADStudio;
use crate::command::{DimensionAssociationSource, StepInput};
use crate::entities::dim_override;
use crate::scene::viewport_ref::{AcceptedSnap, MeasurementScale, ViewportFrame};
use crate::scene::Scene;
use crate::snap::{SnapResult, SnapType};
use acadrust::entities::{Circle, Dimension, Line, Viewport};
use acadrust::types::{Handle, Vector3};
use acadrust::{CadDocument, EntityType};
use glam::DVec3;

fn fixture() -> (OpenCADStudio, Handle, ViewportFrame) {
    let mut app = OpenCADStudio::new_for_test();
    app.automation_op(r#"{"op":"new"}"#);
    let mut scene = Scene::new();
    let line = scene.add_entity(EntityType::Line(Line::from_points(
        Vector3::ZERO,
        Vector3::new(100.0, 0.0, 0.0),
    )));
    scene.document.add_layout("Dimensions").unwrap();
    scene.set_current_layout("Dimensions".into());
    let mut viewport = Viewport::new();
    viewport.id = 2;
    viewport.center = Vector3::new(50.0, 50.0, 0.0);
    viewport.width = 100.0;
    viewport.height = 100.0;
    viewport.view_height = 1000.0;
    viewport.custom_scale = 0.1;
    viewport.view_direction = Vector3::UNIT_Z;
    viewport.status.is_on = true;
    let viewport = scene.add_entity(EntityType::Viewport(viewport));
    let frame = scene.viewport_frame(viewport).unwrap();
    scene.document.header.dimension_associativity = 2;
    let i = app.active_tab;
    app.tabs[i].scene = scene;
    (app, line, frame)
}

fn hit(frame: ViewportFrame, source: Handle, model: DVec3) -> SnapResult {
    SnapResult {
        world: frame.model_to_paper(model),
        model_point: Some(model),
        screen: iced::Point::ORIGIN,
        snap_type: SnapType::Endpoint,
        tangent_obj: None,
        extension_base: None,
        extension_base2: None,
        extension_origin: None,
        extension_dir: None,
        viewport: Some(frame.viewport),
        source: Some(DimensionAssociationSource::inferred(source)),
        secondary_source: None,
    }
}

fn point(app: &mut OpenCADStudio, frame: ViewportFrame, source: Handle, model: DVec3) {
    let i = app.active_tab;
    let hit = hit(frame, source, model);
    let paper = hit.world.with_z(0.0);
    assert!(app.record_accepted_snap(i, Some(hit), Some(frame), paper));
    let result = app.tabs[i].active_cmd.as_mut().unwrap().on_point(paper);
    app.sync_dimension_snaps(i);
    let _ = app.apply_cmd_result(result);
}

fn dimension(doc: &CadDocument) -> &Dimension {
    doc.entities()
        .find_map(|e| match e {
            EntityType::Dimension(d) => Some(d),
            _ => None,
        })
        .unwrap()
}

fn displayed(doc: &CadDocument) -> f64 {
    let dim = dimension(doc);
    if matches!(dim, Dimension::Angular2Ln(_) | Dimension::Angular3Pt(_)) {
        return dim.measurement();
    }
    let factor = dim_override::real(&dim.base().common.extended_data, dim_override::DIMLFAC)
        .or_else(|| {
            doc.dim_styles
                .get(&dim.base().style_name)
                .map(|s| s.dimlfac)
        })
        .unwrap_or(1.0);
    dim.measurement() * MeasurementScale::user_lfac_for_space(factor, true)
}

fn finish_aligned(app: &mut OpenCADStudio, line: Handle, frame: ViewportFrame) {
    let _ = app.dispatch_command("DIMALIGNED");
    point(app, frame, line, DVec3::ZERO);
    point(app, frame, line, DVec3::X * 100.0);
    let _ = app.feed_command(StepInput::Point(DVec3::new(250.0, 200.0, 0.0)));
}

#[test]
fn viewport_dimension_projected_snap_keeps_identity_and_elevation() {
    let (_, line, frame) = fixture();
    let model = DVec3::new(100.0, 0.0, 35.0);
    let hit = hit(frame, line, model);
    let accepted =
        AcceptedSnap::from_snap(&hit, Some(frame)).with_paper_point(hit.world.with_z(0.0));
    assert_eq!(accepted.model_point, model);
    assert_eq!(accepted.source.as_ref().unwrap().source.handle, line);
    let moved = accepted.with_paper_point(hit.world + DVec3::X);
    assert!(moved.source.is_none());
    assert!((moved.model_point - frame.paper_to_model(moved.paper_point)).length() < 1e-9);
}

#[test]
fn viewport_dimension_creation_preserves_active_units_and_roundtrips() {
    for factor in [25.4, -25.4] {
        let (mut app, line, frame) = fixture();
        let i = app.active_tab;
        let mut style = acadrust::tables::DimStyle::new("Millimetres");
        style.dimlfac = factor;
        app.tabs[i].scene.document.dim_styles.add(style).unwrap();
        app.tabs[i].scene.document.header.current_dimstyle_name = "Millimetres".into();
        finish_aligned(&mut app, line, frame);
        let doc = &app.tabs[i].scene.document;
        assert_eq!(dimension(doc).base().style_name, "Millimetres");
        assert!((displayed(doc) - 2540.0).abs() < 1e-5, "{}", displayed(doc));
        let data = MeasurementScale::read(&dimension(doc).base().common.extended_data).unwrap();
        assert_eq!(data.user_lfac, 25.4);
        assert_eq!(doc.header.dimension_associativity, 2);
        for ext in ["dxf", "dwg"] {
            let bytes = crate::io::save_to_bytes(doc, ext, doc.version).unwrap();
            let loaded = crate::io::load_bytes(&format!("viewport.{ext}"), bytes).unwrap();
            assert!(
                (displayed(&loaded) - 2540.0).abs() < 1e-4,
                "{ext}: {}",
                displayed(&loaded)
            );
            assert_eq!(
                MeasurementScale::read(&dimension(&loaded).base().common.extended_data),
                Some(data)
            );
        }
        app.undo_steps(1);
        assert!(!app.tabs[i]
            .scene
            .document
            .entities()
            .any(|e| matches!(e, EntityType::Dimension(_))));
        app.redo_steps(1);
        assert!((displayed(&app.tabs[i].scene.document) - 2540.0).abs() < 1e-5);
    }
}

#[test]
fn viewport_dimension_mixed_and_degenerate_picks_allow_retry() {
    let (mut app, line, frame) = fixture();
    let i = app.active_tab;
    let _ = app.dispatch_command("DIMALIGNED");
    point(&mut app, frame, line, DVec3::ZERO);
    point(&mut app, frame, line, DVec3::ZERO);
    assert_eq!(app.accepted_snaps().len(), 1);
    let _ = app.feed_command(StepInput::Point(DVec3::new(60.0, 50.0, 0.0)));
    assert_eq!(app.accepted_snaps().len(), 1);
    assert_eq!(
        app.tabs[i]
            .active_cmd
            .as_ref()
            .unwrap()
            .dimension_acquired_points()
            .len(),
        1
    );
    let other = ViewportFrame {
        viewport: Handle::new(0xFFFF),
        ..frame
    };
    assert!(!app.record_accepted_snap(
        i,
        Some(hit(other, line, DVec3::X * 100.0)),
        Some(other),
        DVec3::new(60.0, 50.0, 0.0)
    ));
    point(&mut app, frame, line, DVec3::X * 100.0);
    let _ = app.feed_command(StepInput::Point(DVec3::new(250.0, 200.0, 0.0)));
    assert!((displayed(&app.tabs[i].scene.document) - 100.0).abs() < 1e-5);
}

#[test]
fn viewport_dimension_typed_sheet_points_keep_paper_units() {
    let (mut app, _, _) = fixture();
    let result = app.automation_op(r#"{"op":"run","cmd":"DIMALIGNED 50,50 60,50 60,55"}"#);
    assert_eq!(result["ok"], true);
    let doc = &app.tabs[app.active_tab].scene.document;
    assert!((displayed(doc) - 10.0).abs() < 1e-9);
    assert!(MeasurementScale::read(&dimension(doc).base().common.extended_data).is_none());
}

#[test]
fn viewport_dimension_linear_aligned_and_radial_object_picks() {
    for (name, expected) in [
        ("DIMLINEAR", 100.0),
        ("DIMALIGNED", 100.0),
        ("DIMRADIUS", 100.0),
        ("DIMDIAMETER", 200.0),
    ] {
        let (mut app, _, frame) = fixture();
        let i = app.active_tab;
        let radial = name == "DIMRADIUS" || name == "DIMDIAMETER";
        let model = if radial {
            let mut circle = Circle::new();
            circle.center = Vector3::new(0.0, 200.0, 0.0);
            circle.radius = 100.0;
            app.tabs[i].scene.set_current_layout("Model".into());
            app.tabs[i].scene.add_entity(EntityType::Circle(circle));
            app.tabs[i].scene.set_current_layout("Dimensions".into());
            DVec3::new(100.0, 200.0, 0.0)
        } else {
            DVec3::new(50.0, 0.0, 0.0)
        };
        let _ = app.dispatch_command(name);
        if !radial {
            let _ = app.feed_command(StepInput::Enter);
        }
        let result = app
            .try_dimension_viewport_entity_pick(i, frame.model_to_paper(model), 0.5)
            .unwrap();
        let _ = app.apply_cmd_result(result);
        assert_eq!(
            app.accepted_snaps().len(),
            if radial { 1 } else { 2 },
            "{name}"
        );
        let _ = app.feed_command(StepInput::Point(DVec3::new(55.0, 80.0, 0.0)));
        assert!(
            (displayed(&app.tabs[i].scene.document) - expected).abs() < 1e-4,
            "{name}: {}",
            displayed(&app.tabs[i].scene.document)
        );
    }
}

#[test]
fn viewport_dimension_angular_preserves_angle_without_length_factor() {
    let (mut app, line, frame) = fixture();
    let _ = app.dispatch_command("DIMANGULAR");
    let _ = app.feed_command(StepInput::Enter);
    point(&mut app, frame, line, DVec3::ZERO);
    point(&mut app, frame, line, DVec3::X * 100.0);
    point(&mut app, frame, line, DVec3::Y * 100.0);
    let _ = app.feed_command(StepInput::Point(DVec3::new(70.0, 70.0, 0.0)));
    let doc = &app.tabs[app.active_tab].scene.document;
    assert!((dimension(doc).measurement() - 90.0).abs() < 1e-5);
    assert!(MeasurementScale::read(&dimension(doc).base().common.extended_data).is_none());
}

#[test]
fn viewport_dimension_snap_query_matches_pan_scale_and_twist() {
    for (height, twist, target) in [
        (1000.0, 0.0, Vector3::ZERO),
        (500.0, 0.45, Vector3::new(80.0, 20.0, 0.0)),
    ] {
        let (mut app, line, initial) = fixture();
        let i = app.active_tab;
        let Some(EntityType::Viewport(vp)) =
            app.tabs[i].scene.document.get_entity_mut(initial.viewport)
        else {
            panic!()
        };
        vp.view_height = height;
        vp.custom_scale = vp.height / height;
        vp.twist_angle = twist;
        vp.view_target = target;
        let frame = app.tabs[i].scene.viewport_frame(initial.viewport).unwrap();
        let model = DVec3::X * 100.0;
        let paper = frame.model_to_paper(model);
        let bounds = iced::Rectangle {
            x: 0.0,
            y: 0.0,
            width: 1000.0,
            height: 1000.0,
        };
        let screen = {
            let cam = app.tabs[i].scene.camera.borrow();
            let ndc = cam
                .view_proj_rte(bounds)
                .project_point3((paper - cam.eye()).as_vec3());
            iced::Point::new((ndc.x + 1.0) * 500.0, (1.0 - ndc.y) * 500.0)
        };
        app.snapper.snap_enabled = true;
        let (hit, acquired_frame) = app
            .paper_viewport_snap(i, screen, (1000.0, 1000.0), paper)
            .unwrap();
        assert_eq!(hit.source.unwrap().handle, line);
        assert!((hit.world - paper).length() < 1e-5, "{hit:?}");
        let accepted = AcceptedSnap::from_snap(&hit, Some(acquired_frame));
        assert!((accepted.model_point - model).length() < 1e-6);
    }
}

#[test]
fn viewport_dimension_eligibility_is_shared_and_uses_clipping() {
    use acadrust::entities::LwPolyline;
    use acadrust::types::Vector2;
    let (mut app, _, first) = fixture();
    let scene = &mut app.tabs[app.active_tab].scene;
    let mut overlay = scene.document.get_entity(first.viewport).unwrap().clone();
    if let EntityType::Viewport(vp) = &mut overlay {
        vp.id = 3;
        vp.width = 200.0;
    }
    overlay.common_mut().handle = Handle::NULL;
    let top = scene.add_entity(overlay);
    let paper = DVec3::new(55.0, 50.0, 0.0);
    assert_eq!(
        scene.viewport_frame_at_paper_point(paper).unwrap().viewport,
        top
    );
    assert_eq!(
        scene
            .dimension_pick_through_viewport(paper, 0.2)
            .unwrap()
            .frame
            .viewport,
        top
    );
    let mut clip = LwPolyline::from_points(vec![
        Vector2::new(0.0, 0.0),
        Vector2::new(100.0, 0.0),
        Vector2::new(0.0, 100.0),
    ]);
    clip.is_closed = true;
    let clip = scene.add_entity(EntityType::LwPolyline(clip));
    if let Some(EntityType::Viewport(vp)) = scene.document.get_entity_mut(top) {
        vp.clip_boundary_handle = clip;
    }
    assert_eq!(
        scene.viewport_frame_at_paper_point(paper).unwrap().viewport,
        first.viewport
    );
    assert_eq!(
        scene
            .dimension_pick_through_viewport(paper, 0.2)
            .unwrap()
            .frame
            .viewport,
        first.viewport
    );
    if let Some(EntityType::Viewport(vp)) = scene.document.get_entity_mut(first.viewport) {
        vp.status.is_on = false;
    }
    assert!(scene.viewport_frame_at_paper_point(paper).is_none());
    assert!(scene.dimension_pick_through_viewport(paper, 0.2).is_none());
}

#[test]
fn viewport_dimension_exploded_commit_uses_compensated_text_and_undo() {
    let (mut app, line, frame) = fixture();
    let i = app.active_tab;
    app.tabs[i].scene.document.header.dimension_associativity = 0;
    finish_aligned(&mut app, line, frame);
    let doc = &app.tabs[i].scene.document;
    assert!(!doc
        .entities()
        .any(|e| matches!(e, EntityType::Dimension(_))));
    let texts: Vec<_> = doc
        .entities()
        .filter_map(|e| match e {
            EntityType::Text(t) => Some(t.value.clone()),
            EntityType::MText(t) => Some(t.value.clone()),
            _ => None,
        })
        .collect();
    assert!(texts.iter().any(|t| t.contains("100")), "{texts:?}");
    app.undo_steps(1);
    assert_eq!(app.tabs[i].scene.document.header.dimension_associativity, 0);
    app.redo_steps(1);
    assert!(app.tabs[i]
        .scene
        .document
        .entities()
        .any(|e| matches!(e, EntityType::Text(_) | EntityType::MText(_))));
}
