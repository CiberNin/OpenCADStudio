//! Object-pick resolution for paper-space dimensions taken through a layout
//! viewport.
//!
//! Paper-space hit testing only sees the sheet's own entities, so a dimension
//! command's explicit "select object" pick that lands inside a content
//! viewport finds nothing. [`Scene::dimension_pick_through_viewport`] maps the
//! click into model space through the viewport's
//! [`crate::scene::viewport_ref::ViewportFrame`], hit-tests the viewport's
//! resident model wires (descending into block instances and baking the
//! instance transform), and hands back a copy of the picked entity already
//! projected onto the sheet.
//!
//! The frame itself is built by [`Scene::viewport_frame`] (see
//! `crate::scene::mspace`), which derives it from the *live viewport camera*
//! so it always agrees with the pixels the renderer drew. It returns `None`
//! for oblique / perspective viewports, and every entry point here then
//! declines the pick rather than guessing a planar mapping.

use super::*;

use crate::command::EntityTransform;
use crate::scene::viewport_ref::ViewportFrame;
use acadrust::types::{Matrix4, Transform};
use glam::{DVec2, DVec3};

/// A model entity resolved by clicking inside a paper-space content viewport.
#[derive(Clone, Debug)]
pub struct ViewportDimPick {
    /// The viewport the click looked through.
    pub frame: ViewportFrame,
    /// Handle of the innermost model entity that owns the picked geometry.
    pub entity_handle: Handle,
    /// INSERT handles from the outermost instance down to the instance that
    /// directly contains `entity_handle`; empty for top-level model geometry.
    pub block_path: Vec<Handle>,
    /// The picked geometry with the block-instance transform *and* the
    /// viewport model→paper transform applied, so paper-space dimension
    /// commands can consume it unchanged.
    pub paper_entity: EntityType,
    /// The click position on the sheet.
    pub paper_point: DVec3,
    /// The click position in model space.
    pub model_point: DVec3,
}

impl Scene {

    /// The smallest *on* content viewport of the current layout whose paper
    /// rectangle contains `paper`. `None` on the Model tab, while editing
    /// inside a viewport (MSPACE), or when the point is on bare sheet.
    pub fn content_viewport_at_paper_point(&self, paper: DVec3) -> Option<Handle> {
        if self.current_layout == "Model" || self.active_viewport.is_some() {
            return None;
        }
        let (_, _, handles) = self.paper_viewport_handles();
        handles
            .iter()
            .filter_map(|handle| {
                let Some(EntityType::Viewport(vp)) = self.document.get_entity(*handle) else {
                    return None;
                };
                if !vp.status.is_on || vp.width <= 0.0 || vp.height <= 0.0 {
                    return None;
                }
                let dx = (paper.x - vp.center.x).abs();
                let dy = (paper.y - vp.center.y).abs();
                (dx <= vp.width * 0.5 && dy <= vp.height * 0.5)
                    .then(|| (*handle, vp.width * vp.height))
            })
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(handle, _)| handle)
    }

    /// [`Scene::content_viewport_at_paper_point`] resolved to a frame.
    pub fn viewport_frame_at_paper_point(&self, paper: DVec3) -> Option<ViewportFrame> {
        self.viewport_frame(self.content_viewport_at_paper_point(paper)?)
    }

    /// Resolve an explicit dimension object pick made on the sheet but landing
    /// inside a content viewport, against the *model* geometry that viewport
    /// displays.
    ///
    /// `aperture_paper` is the pick aperture in paper units.
    pub fn dimension_pick_through_viewport(
        &self,
        paper: DVec3,
        aperture_paper: f64,
    ) -> Option<ViewportDimPick> {
        let frame = self.viewport_frame_at_paper_point(paper)?;
        let model_point = frame.paper_to_model(paper);
        let aperture_model =
            (aperture_paper.max(0.0) * frame.paper_to_model_length_factor()).max(1e-9);

        let top = self.nearest_model_wire_handle(frame.viewport, model_point, aperture_model)?;
        let (entity, block_path) = self.resolve_measurable_entity(top, model_point)?;

        let mut paper_entity = entity;
        crate::scene::view::dispatch::apply_transform(
            &mut paper_entity,
            &EntityTransform::Affine(viewport_model_to_paper_transform(&frame)),
        );
        let entity_handle = paper_entity.as_entity().handle();
        Some(ViewportDimPick {
            frame,
            entity_handle: if entity_handle.is_valid() { entity_handle } else { top },
            block_path,
            paper_entity,
            paper_point: paper,
            model_point,
        })
    }

    /// Nearest resident model wire of `viewport` to `model_point`, within
    /// `aperture` model units. Wires are the same tessellation the viewport
    /// draws, so this matches what the user sees.
    fn nearest_model_wire_handle(
        &self,
        viewport: Handle,
        model_point: DVec3,
        aperture: f64,
    ) -> Option<Handle> {
        let wires = self.model_wires_for_viewport_arc(viewport, 0.0);
        let mut best: Option<(f64, Handle)> = None;
        for wire in wires.iter() {
            if !wire.display_visible {
                continue;
            }
            let Some(handle) = Self::handle_from_wire_name(&wire.name) else {
                continue;
            };
            let offset = wire
                .render_instance
                .as_ref()
                .map(|inst| DVec3::from_array(inst.translation))
                .unwrap_or(DVec3::ZERO);
            let distance = wire_polyline_distance(wire, offset, model_point);
            if let Some(distance) = distance {
                if distance <= aperture && best.as_ref().is_none_or(|(d, _)| distance < *d) {
                    best = Some((distance, handle));
                }
            }
        }
        best.map(|(_, handle)| handle)
    }

    /// Turn a picked handle into a measurable planar entity in model (WCS)
    /// coordinates, descending through block instances and baking the
    /// instance transform into the returned clone.
    fn resolve_measurable_entity(
        &self,
        handle: Handle,
        model_point: DVec3,
    ) -> Option<(EntityType, Vec<Handle>)> {
        let entity = self.document.get_entity(handle)?;
        match entity {
            EntityType::Insert(_) => {
                let mut path = Vec::new();
                let found = self.descend_block_instance(
                    entity.clone(),
                    Transform::identity(),
                    model_point,
                    &mut path,
                    0,
                )?;
                Some((found.0, found.1))
            }
            other => {
                planar_pick_distance(other, model_point)?;
                Some((other.clone(), Vec::new()))
            }
        }
    }

    /// Depth-first search for the nearest measurable entity inside an INSERT.
    /// Returns the entity already transformed into WCS plus the INSERT path.
    fn descend_block_instance(
        &self,
        insert_entity: EntityType,
        outer: Transform,
        model_point: DVec3,
        path: &mut Vec<Handle>,
        depth: usize,
    ) -> Option<(EntityType, Vec<Handle>)> {
        const MAX_DEPTH: usize = 8;
        if depth > MAX_DEPTH {
            return None;
        }
        let EntityType::Insert(insert) = &insert_entity else {
            return None;
        };
        let insert_handle = insert.common.handle;
        let local = crate::scene::render_graph::insert_transform(&self.document, insert);
        let combined = local.then(&outer);
        let block = self.document.block_records.get(&insert.block_name)?;
        let children: Vec<Handle> = block.entity_handles.clone();

        let mut best: Option<(f64, EntityType, Vec<Handle>)> = None;
        for child in children {
            let Some(child_entity) = self.document.get_entity(child) else {
                continue;
            };
            if matches!(child_entity, EntityType::Insert(_)) {
                let mut nested = Vec::new();
                if let Some((entity, mut nested_path)) = self.descend_block_instance(
                    child_entity.clone(),
                    combined,
                    model_point,
                    &mut nested,
                    depth + 1,
                ) {
                    if let Some(distance) = planar_pick_distance(&entity, model_point) {
                        if best.as_ref().is_none_or(|(d, _, _)| distance < *d) {
                            let mut full = vec![insert_handle];
                            full.append(&mut nested_path);
                            best = Some((distance, entity, full));
                        }
                    }
                }
                continue;
            }
            let mut placed = child_entity.clone();
            crate::scene::view::dispatch::apply_transform(
                &mut placed,
                &EntityTransform::Affine(combined),
            );
            let Some(distance) = planar_pick_distance(&placed, model_point) else {
                continue;
            };
            if best.as_ref().is_none_or(|(d, _, _)| distance < *d) {
                best = Some((distance, placed, vec![insert_handle]));
            }
        }
        let (_, entity, found_path) = best?;
        path.clear();
        path.extend(found_path.iter().copied());
        Some((entity, found_path))
    }
}

/// Model→paper for a viewport, as an affine entity transform. Z passes
/// through unchanged so planar geometry keeps its elevation.
pub fn viewport_model_to_paper_transform(frame: &ViewportFrame) -> Transform {
    let (sin, cos) = frame.twist.sin_cos();
    let s = frame.scale;
    let a = cos * s;
    let b = sin * s;
    // t = paper_center - R*s*model_target
    let tx = frame.paper_center.x - (a * frame.model_target.x - b * frame.model_target.y);
    let ty = frame.paper_center.y - (b * frame.model_target.x + a * frame.model_target.y);
    Transform::from_matrix(Matrix4 {
        m: [
            [a, -b, 0.0, tx],
            [b, a, 0.0, ty],
            [0.0, 0.0, 1.0, 0.0],
            [0.0, 0.0, 0.0, 1.0],
        ],
    })
}

/// Distance from `point` to a wire's polyline, in XY. `None` when the wire has
/// no usable segment.
fn wire_polyline_distance(
    wire: &crate::scene::WireModel,
    offset: DVec3,
    point: DVec3,
) -> Option<f64> {
    let count = wire.points.len();
    if count == 0 {
        return None;
    }
    let at = |i: usize| -> DVec3 {
        let hi = wire.points[i];
        let lo = wire.points_low.get(i).copied().unwrap_or([0.0; 3]);
        DVec3::new(
            hi[0] as f64 + lo[0] as f64,
            hi[1] as f64 + lo[1] as f64,
            hi[2] as f64 + lo[2] as f64,
        ) + offset
    };
    let mut best = f64::INFINITY;
    let mut previous: Option<DVec3> = None;
    for i in 0..count {
        let current = at(i);
        if !current.x.is_finite() || !current.y.is_finite() {
            previous = None;
            continue;
        }
        match previous {
            Some(start) => best = best.min(segment_distance_xy(start, current, point)),
            None => best = best.min((current.truncate() - point.truncate()).length()),
        }
        previous = Some(current);
    }
    best.is_finite().then_some(best)
}

fn segment_distance_xy(a: DVec3, b: DVec3, p: DVec3) -> f64 {
    let ab = b.truncate() - a.truncate();
    let ap = p.truncate() - a.truncate();
    let len2 = ab.length_squared();
    if len2 < 1e-24 {
        return ap.length();
    }
    let t = (ap.dot(ab) / len2).clamp(0.0, 1.0);
    (ap - ab * t).length()
}

/// Distance from `point` to the planar geometry object-pick dimensioning
/// supports. `None` for entity types that cannot be dimensioned by picking.
pub fn planar_pick_distance(entity: &EntityType, point: DVec3) -> Option<f64> {
    let p = point.truncate();
    match entity {
        EntityType::Line(line) => Some(segment_distance_xy(
            DVec3::new(line.start.x, line.start.y, line.start.z),
            DVec3::new(line.end.x, line.end.y, line.end.z),
            point,
        )),
        EntityType::Circle(circle) => {
            let c = DVec2::new(circle.center.x, circle.center.y);
            Some(((p - c).length() - circle.radius).abs())
        }
        EntityType::Arc(arc) => {
            let c = DVec2::new(arc.center.x, arc.center.y);
            Some(((p - c).length() - arc.radius).abs())
        }
        EntityType::LwPolyline(polyline) => polyline_distance(
            polyline
                .vertices
                .iter()
                .map(|v| DVec3::new(v.location.x, v.location.y, polyline.elevation)),
            polyline.is_closed,
            point,
        ),
        EntityType::Polyline2D(polyline) => polyline_distance(
            polyline
                .vertices
                .iter()
                .map(|v| DVec3::new(v.location.x, v.location.y, polyline.elevation)),
            polyline.is_closed(),
            point,
        ),
        _ => None,
    }
}

fn polyline_distance(
    points: impl IntoIterator<Item = DVec3>,
    closed: bool,
    point: DVec3,
) -> Option<f64> {
    let points: Vec<DVec3> = points.into_iter().collect();
    if points.len() < 2 {
        return None;
    }
    let count = if closed { points.len() } else { points.len() - 1 };
    (0..count)
        .map(|i| segment_distance_xy(points[i], points[(i + 1) % points.len()], point))
        .reduce(f64::min)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame() -> ViewportFrame {
        ViewportFrame {
            viewport: Handle::NULL,
            paper_center: DVec2::new(100.0, 50.0),
            model_target: DVec2::new(1000.0, 2000.0),
            scale: 0.1,
            twist: 0.4,
            locked: false,
        }
    }

    #[test]
    fn affine_transform_matches_the_frame_mapping() {
        let f = frame();
        let t = viewport_model_to_paper_transform(&f);
        for model in [
            DVec3::new(1000.0, 2000.0, 0.0),
            DVec3::new(1100.0, 2000.0, 0.0),
            DVec3::new(940.5, 2113.25, 0.0),
        ] {
            let expected = f.model_to_paper(model);
            let actual = t.apply(acadrust::types::Vector3::new(model.x, model.y, model.z));
            assert!(
                (actual.x - expected.x).abs() < 1e-9 && (actual.y - expected.y).abs() < 1e-9,
                "{actual:?} != {expected:?}"
            );
        }
    }
}
