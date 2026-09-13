//! Render smoke test: with DIMTMOVE=1 a linear dimension whose text is dragged
//! outside the dimension line draws a leader underneath the text
//! (GB/T 4458.4 / AutoCAD behaviour).
//!
//! Same pipeline as `pdf_export_text_check.rs` (Scene::entity_wires() →
//! export_pdf()) but writes a PDF for manual/visual inspection. The file is only
//! written when `OCS_DIM_LEADER_PDF` is set, so CI stays side-effect free.
use acadrust::entities::{Dimension, DimensionLinear};
use acadrust::types::Vector3;
use acadrust::xdata::{ExtendedDataRecord, XDataValue};
use acadrust::EntityType;
use OpenCADStudio::io::pdf_export::{export_pdf, PdfPlotOptions, PlotWire};
use OpenCADStudio::scene::Scene;

/// Build a per-object DSTYLE override record (same shape as the properties
/// panel / `src/entities/dim_override.rs` write).
fn dstyle(pairs: &[(i16, XDataValue)]) -> ExtendedDataRecord {
    let mut rec = ExtendedDataRecord::new("ACAD");
    rec.add_value(XDataValue::String("DSTYLE".into()));
    rec.add_value(XDataValue::ControlString("{".into()));
    for (code, value) in pairs {
        rec.add_value(XDataValue::Integer16(*code));
        rec.add_value(value.clone());
    }
    rec.add_value(XDataValue::ControlString("}".into()));
    rec
}

fn v(x: f64, y: f64) -> Vector3 {
    Vector3::new(x, y, 0.0)
}

/// Linear dimension with the text dragged outside to the right and DIMTMOVE=1:
/// the rendered dimension line must extend underneath the text.
#[test]
fn dim_leader_under_outside_text_renders() {
    let out = std::env::var("OCS_DIM_LEADER_PDF").unwrap_or_default();

    let mut scene = Scene::new();
    let mut d = DimensionLinear::horizontal(v(20.0, 100.0), v(80.0, 100.0));
    d.definition_point = v(20.0, 86.0); // dimension line at y=86
    d.base.definition_point = d.definition_point;
    d.base.actual_measurement = 60.0;
    // Text dragged past the right end of the dimension line (centre x=98, end x=80).
    d.base.text_middle_point = v(98.0, 89.0);
    d.base.insertion_point = d.base.text_middle_point;
    d.base.text_user_positioned = true;
    // Per-object overrides, as the properties panel writes them: DIMTMOVE=1
    // plus a visible text height / arrow size / gap.
    d.base.style_name = "Standard".into();
    d.base.common.extended_data.add_record(dstyle(&[
        (279, XDataValue::Integer16(1)), // DIMTMOVE=1 → draw a leader when the text moves out
        (140, XDataValue::Real(3.5)),    // DIMTXT
        (41, XDataValue::Real(3.5)),     // DIMASZ
        (147, XDataValue::Real(1.0)),    // DIMGAP
        (271, XDataValue::Integer16(2)), // DIMDEC
    ]));
    scene.add_entity(EntityType::Dimension(Dimension::Linear(d)));

    // Control case: the same dimension with DIMTMOVE=0 (no leader).
    let mut d0 = DimensionLinear::horizontal(v(20.0, 60.0), v(80.0, 60.0));
    d0.definition_point = v(20.0, 46.0);
    d0.base.definition_point = d0.definition_point;
    d0.base.actual_measurement = 60.0;
    d0.base.text_middle_point = v(98.0, 49.0);
    d0.base.insertion_point = d0.base.text_middle_point;
    d0.base.text_user_positioned = true;
    d0.base.style_name = "Standard".into();
    d0.base.common.extended_data.add_record(dstyle(&[
        (279, XDataValue::Integer16(0)), // control: no leader
        (140, XDataValue::Real(3.5)),
        (41, XDataValue::Real(3.5)),
        (147, XDataValue::Real(1.0)),
        (271, XDataValue::Integer16(2)),
    ]));
    scene.add_entity(EntityType::Dimension(Dimension::Linear(d0)));

    let plot_wires: Vec<PlotWire> = scene
        .entity_wires()
        .iter()
        .map(|w| PlotWire {
            wire: w.clone(),
            draw_depth: 0.0,
        })
        .collect();
    assert!(!plot_wires.is_empty(), "scene should produce geometry");

    if out.trim().is_empty() {
        return; // env var unset: exercise the render path only, write nothing
    }
    let path = std::path::PathBuf::from(&out);
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).ok();
    }
    export_pdf(
        &plot_wires,
        &[],
        &[],
        210.0,
        297.0,
        0.0,
        0.0,
        0,
        1.0,
        None,
        &path,
        None,
        PdfPlotOptions::default(),
    )
    .expect("export pdf");
    println!("dim leader render smoke pdf written: {out}");
}
