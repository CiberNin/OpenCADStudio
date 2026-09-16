use acadrust::{EntityType, Handle};
use glam::DVec3;

use crate::command::{CadCommand, CmdResult};
use crate::scene::parametric_constraints::{directional_axis_endpoints, ParametricRef};
use crate::t;

#[derive(Clone, Copy)]
pub struct PerpendicularPick {
    pub reference: ParametricRef,
    pub fixed_reference: ParametricRef,
    pub start_reference: ParametricRef,
}

pub struct PerpendicularConstraintCommand {
    first: Option<PerpendicularPick>,
    picked_entity: Option<EntityType>,
}

impl PerpendicularConstraintCommand {
    pub fn new() -> Self {
        Self {
            first: None,
            picked_entity: None,
        }
    }

    fn invalid_selection() -> CmdResult {
        CmdResult::ReportError(
            t!("Invalid selection for Perpendicular. Select a line segment, polyline segment, text, MText, major or minor axis of ellipse or elliptical arc.")
                .into_owned(),
        )
    }

    fn distance_to_axis(point: DVec3, endpoints: [acadrust::types::Vector3; 2]) -> f64 {
        let start = DVec3::new(endpoints[0].x, endpoints[0].y, endpoints[0].z);
        let end = DVec3::new(endpoints[1].x, endpoints[1].y, endpoints[1].z);
        let direction = end - start;
        if direction.length_squared() <= 1.0e-24 {
            return f64::INFINITY;
        }
        (point - start).cross(direction.normalize()).length()
    }

    fn ellipse_reference(entity: &EntityType, handle: Handle, point: DVec3) -> Option<ParametricRef> {
        let major = ParametricRef::ellipse_major_axis(handle);
        let minor = ParametricRef::ellipse_minor_axis(handle);
        let major_distance = Self::distance_to_axis(point, directional_axis_endpoints(entity, major)?);
        let minor_distance = Self::distance_to_axis(point, directional_axis_endpoints(entity, minor)?);
        Some(if minor_distance < major_distance { minor } else { major })
    }

    fn picked_reference(entity: &EntityType, handle: Handle, point: DVec3) -> Option<PerpendicularPick> {
        let (reference, fixed_reference, start_reference) = match entity {
            EntityType::Line(_) => (
                ParametricRef::whole(handle),
                ParametricRef::whole(handle),
                ParametricRef::point(handle, 0),
            ),
            EntityType::LwPolyline(_) | EntityType::Polyline2D(_) => {
                let (source, _, _) = crate::scene::centerline::picked_source(entity, handle, point)?;
                let index = usize::try_from(source.segment_index).ok()?;
                (
                    ParametricRef::segment(handle, index),
                    ParametricRef::segment(handle, index),
                    ParametricRef::point(handle, index as i32),
                )
            }
            EntityType::Text(_) | EntityType::MText(_) => (
                ParametricRef::text_baseline(handle),
                ParametricRef::whole(handle),
                ParametricRef::point(handle, 0),
            ),
            EntityType::Ellipse(_) => (
                Self::ellipse_reference(entity, handle, point)?,
                ParametricRef::whole(handle),
                ParametricRef::center(handle),
            ),
            _ => return None,
        };
        Some(PerpendicularPick {
            reference,
            fixed_reference,
            start_reference,
        })
    }

    pub fn preselected_reference(entity: &EntityType, handle: Handle) -> Option<PerpendicularPick> {
        match entity {
            EntityType::Line(_) | EntityType::Text(_) | EntityType::MText(_) => {
                Self::picked_reference(entity, handle, DVec3::ZERO)
            }
            EntityType::Ellipse(_) => Some(PerpendicularPick {
                reference: ParametricRef::ellipse_major_axis(handle),
                fixed_reference: ParametricRef::whole(handle),
                start_reference: ParametricRef::center(handle),
            }),
            EntityType::LwPolyline(polyline) => {
                let segment_count = if polyline.is_closed {
                    polyline.vertices.len()
                } else {
                    polyline.vertices.len().saturating_sub(1)
                };
                (segment_count == 1 && polyline.vertices.first()?.bulge.abs() <= 1.0e-12)
                    .then_some(PerpendicularPick {
                        reference: ParametricRef::segment(handle, 0),
                        fixed_reference: ParametricRef::segment(handle, 0),
                        start_reference: ParametricRef::point(handle, 0),
                    })
            }
            EntityType::Polyline2D(polyline) => {
                let segment_count = if polyline.is_closed() {
                    polyline.vertices.len()
                } else {
                    polyline.vertices.len().saturating_sub(1)
                };
                (segment_count == 1 && polyline.vertices.first()?.bulge.abs() <= 1.0e-12)
                    .then_some(PerpendicularPick {
                        reference: ParametricRef::segment(handle, 0),
                        fixed_reference: ParametricRef::segment(handle, 0),
                        start_reference: ParametricRef::point(handle, 0),
                    })
            }
            _ => None,
        }
    }
}

impl CadCommand for PerpendicularConstraintCommand {
    fn name(&self) -> &'static str {
        "GCPERPENDICULAR"
    }

    fn prompt(&self) -> String {
        if self.first.is_some() {
            t!("GCPERPENDICULAR  Select second object:").into_owned()
        } else {
            t!("GCPERPENDICULAR  Select first object:").into_owned()
        }
    }

    fn needs_entity_pick(&self) -> bool {
        true
    }

    fn entity_pick_highlights_hover(&self) -> bool {
        true
    }

    fn inject_before_entity_pick(&self) -> bool {
        true
    }

    fn inject_picked_entity(&mut self, entity: EntityType) {
        self.picked_entity = Some(entity);
    }

    fn on_entity_pick(&mut self, handle: Handle, point: DVec3) -> CmdResult {
        if handle.is_null() {
            return CmdResult::NeedPoint;
        }
        let Some(entity) = self.picked_entity.take() else {
            return CmdResult::NeedPoint;
        };
        let Some(pick) = Self::picked_reference(&entity, handle, point) else {
            return Self::invalid_selection();
        };
        let Some(first) = self.first else {
            self.first = Some(pick);
            return CmdResult::NeedPoint;
        };
        if first.reference == pick.reference {
            return Self::invalid_selection();
        }
        CmdResult::AddPerpendicularConstraint {
            first: first.reference,
            second: pick.reference,
            first_fixed: first.fixed_reference,
            second_start: pick.start_reference,
            label: "Perpendicular constraint",
        }
    }

    fn on_point(&mut self, _point: DVec3) -> CmdResult {
        CmdResult::NeedPoint
    }

    fn on_enter(&mut self) -> CmdResult {
        CmdResult::Cancel
    }

    fn on_escape(&mut self) -> CmdResult {
        CmdResult::Cancel
    }
}
