//! Maps sketch constraints to the drawing format's native associative object graph.
//! The application record remains authoritative when both representations exist.

use acadrust::objects::{
    Assoc2dConstraintGroup, AssocAction, AssocActionDependency, AssocConstraintNode,
    AssocConstraintNodeData, AssocDependency, AssocEvalValue, AssocEvalVariant,
    AssocGeomDependency, AssocNetwork, AssocPersistentSubentId, AssocValueDependency,
    AssocVariable, AssociativeData, AssociativeObject, ObjectType,
};
use acadrust::types::{Handle, Vector3};
use acadrust::{CadDocument, EntityType};
use rustc_hash::FxHashMap;

use super::named_parameters::{DrivingValue, ParameterTable};
use super::sketch_constraints::{
    ConstraintKind, SketchConstraint, SketchConstraintSet, SketchRef, SketchScope,
};
use super::Scene;

/// Default sub-dictionary key used for an associative network.
const NETWORK_DICTIONARY_KEY: &str = "ACAD_ASSOCNETWORK";

/// `AcConstraintGroupNode::GroupNodeId::kNullGroupNodeId` — node id 0 is
/// reserved as "no node", so real node ids start at 1.
const FIRST_NODE_ID: i32 = 1;

/// Native implicit-point type values.
mod implicit_point_type {
    pub const START: u8 = 0;
    pub const END: u8 = 1;
    #[allow(dead_code)] // reserved for SketchRef marker -2, not wired up yet
    pub const MID: u8 = 2;
    pub const CENTER: u8 = 3;
}

/// One entity's constraint-group presence: its geometry node's id, plus
/// whichever `ImplicitPoint` nodes have been created for it so far, keyed
/// by our own `SketchRef::marker` convention so a second constraint
/// referencing the same point reuses the same node instead of duplicating it.
#[derive(Default)]
struct EntityNodes {
    geometry_node_id: i32,
    points: FxHashMap<i32, i32>,
    segments: FxHashMap<usize, i32>,
}

/// Builds one scope's `Assoc2dConstraintGroup` graph incrementally,
/// allocating node ids and deduplicating geometry/point nodes per entity.
struct GroupBuilder<'a> {
    document: &'a CadDocument,
    nodes: Vec<AssocConstraintNode>,
    next_node_id: i32,
    entities: FxHashMap<Handle, EntityNodes>,
    /// Real entity handles that ended up with a geometry node — what
    /// `AssocGeomDependency` objects get created for, in first-touch order.
    referenced_entities: Vec<Handle>,
    horizontal_datum: Option<i32>,
    vertical_datum: Option<i32>,
}

impl<'a> GroupBuilder<'a> {
    fn new(document: &'a CadDocument) -> Self {
        Self {
            document,
            nodes: Vec::new(),
            next_node_id: FIRST_NODE_ID,
            entities: FxHashMap::default(),
            referenced_entities: Vec::new(),
            horizontal_datum: None,
            vertical_datum: None,
        }
    }

    fn alloc_node_id(&mut self) -> i32 {
        let id = self.next_node_id;
        self.next_node_id += 1;
        id
    }

    fn push_node(&mut self, node_id: i32, class_name: &'static str, data: AssocConstraintNodeData) {
        let status = u8::from(matches!(
            &data,
            AssocConstraintNodeData::ImplicitPoint { .. }
                | AssocConstraintNodeData::Point { .. }
                | AssocConstraintNodeData::Line { .. }
                | AssocConstraintNodeData::BoundedLine { .. }
                | AssocConstraintNodeData::Circle { .. }
                | AssocConstraintNodeData::Arc { .. }
                | AssocConstraintNodeData::Ellipse { .. }
                | AssocConstraintNodeData::BoundedEllipse { .. }
        ));
        self.nodes.push(AssocConstraintNode {
            node_id,
            status,
            connections: Vec::new(),
            class_name: class_name.to_string(),
            registry_flag: false,
            data,
        });
    }

    /// Returns the whole-geometry node for an entity, creating it once.
    fn geometry_node(&mut self, handle: Handle) -> Option<i32> {
        if let Some(existing) = self.entities.get(&handle) {
            if existing.geometry_node_id != 0 {
                return Some(existing.geometry_node_id);
            }
        }
        let entity = self.document.get_entity(handle)?;
        let node_id = self.alloc_node_id();
        match entity {
            EntityType::Point(point) => {
                self.push_node(
                    node_id,
                    "AcConstrainedPoint",
                    AssocConstraintNodeData::Point {
                        geometry_dependency: Handle::NULL,
                        geometry_node_id: node_id,
                        point: Some(point.location),
                    },
                );
            }
            EntityType::Insert(insert) => self.push_node(
                node_id,
                "AcConstrainedPoint",
                AssocConstraintNodeData::Point {
                    geometry_dependency: Handle::NULL,
                    geometry_node_id: node_id,
                    point: Some(insert.insert_point),
                },
            ),
            EntityType::Text(text) => self.push_node(
                node_id,
                "AcConstrainedPoint",
                AssocConstraintNodeData::Point {
                    geometry_dependency: Handle::NULL,
                    geometry_node_id: node_id,
                    point: Some(text.insertion_point),
                },
            ),
            EntityType::MText(text) => self.push_node(
                node_id,
                "AcConstrainedPoint",
                AssocConstraintNodeData::Point {
                    geometry_dependency: Handle::NULL,
                    geometry_node_id: node_id,
                    point: Some(text.insertion_point),
                },
            ),
            EntityType::AttributeDefinition(attribute) => self.push_node(
                node_id,
                "AcConstrainedPoint",
                AssocConstraintNodeData::Point {
                    geometry_dependency: Handle::NULL,
                    geometry_node_id: node_id,
                    point: Some(attribute.insertion_point),
                },
            ),
            EntityType::AttributeEntity(attribute) => self.push_node(
                node_id,
                "AcConstrainedPoint",
                AssocConstraintNodeData::Point {
                    geometry_dependency: Handle::NULL,
                    geometry_node_id: node_id,
                    point: Some(attribute.insertion_point),
                },
            ),
            EntityType::Table(table) => self.push_node(
                node_id,
                "AcConstrainedPoint",
                AssocConstraintNodeData::Point {
                    geometry_dependency: Handle::NULL,
                    geometry_node_id: node_id,
                    point: Some(table.insertion_point),
                },
            ),
            EntityType::Line(l) => {
                let point = l.start;
                let direction = (l.end - l.start).normalize();
                self.push_node(
                    node_id,
                    "AcDbAssocGeomDependency", // placeholder, overwritten below
                    AssocConstraintNodeData::None,
                );
                let last = self.nodes.last_mut().expect("just pushed");
                last.class_name = "AcConstrainedBoundedLine".to_string();
                last.status = 1;
                last.data = AssocConstraintNodeData::BoundedLine {
                    geometry_dependency: Handle::NULL,
                    geometry_node_id: node_id,
                    point,
                    direction,
                    is_ray: false,
                    start_point: l.start,
                    end_point: l.end,
                };
            }
            EntityType::Circle(c) => {
                let (axis_x, _axis_y) =
                    crate::scene::view::transform::ocs_axes((c.normal.x, c.normal.y, c.normal.z));
                self.push_node(
                    node_id,
                    "AcConstrainedCircle",
                    AssocConstraintNodeData::Circle {
                        geometry_dependency: Handle::NULL,
                        geometry_node_id: node_id,
                        center: c.center,
                        normal: c.normal,
                        direction: Vector3::new(axis_x.0, axis_x.1, axis_x.2),
                        radius: c.radius,
                        start_parameter: 0.0,
                        end_parameter: std::f64::consts::TAU,
                        reserved: 0.0,
                    },
                );
            }
            EntityType::Arc(a) => {
                let (axis_x, _axis_y) =
                    crate::scene::view::transform::ocs_axes((a.normal.x, a.normal.y, a.normal.z));
                let direction = Vector3::new(axis_x.0, axis_x.1, axis_x.2);
                let start_point = a.center
                    + Vector3::new(axis_x.0, axis_x.1, axis_x.2) * (a.radius * a.start_angle.cos())
                    + Vector3::new(-axis_x.1, axis_x.0, 0.0) * (a.radius * a.start_angle.sin());
                let end_point = a.center
                    + Vector3::new(axis_x.0, axis_x.1, axis_x.2) * (a.radius * a.end_angle.cos())
                    + Vector3::new(-axis_x.1, axis_x.0, 0.0) * (a.radius * a.end_angle.sin());
                self.push_node(
                    node_id,
                    "AcConstrainedArc",
                    AssocConstraintNodeData::Arc {
                        geometry_dependency: Handle::NULL,
                        geometry_node_id: node_id,
                        center: a.center,
                        normal: a.normal,
                        direction,
                        radius: a.radius,
                        start_parameter: a.start_angle,
                        end_parameter: a.end_angle,
                        reserved: 0.0,
                        start_point,
                        end_point,
                    },
                );
            }
            EntityType::Ellipse(e) => {
                let major_length = e.major_axis.length();
                let major = if major_length > f64::EPSILON {
                    e.major_axis / major_length
                } else {
                    Vector3::UNIT_X
                };
                let normal = e.normal.normalize();
                let minor = Vector3::new(
                    normal.y * major.z - normal.z * major.y,
                    normal.z * major.x - normal.x * major.z,
                    normal.x * major.y - normal.y * major.x,
                );
                let point_at = |parameter: f64| {
                    e.center
                        + e.major_axis * parameter.cos()
                        + minor * (major_length * e.minor_axis_ratio * parameter.sin())
                };
                let data = if e.is_full() {
                    AssocConstraintNodeData::Ellipse {
                        geometry_dependency: Handle::NULL,
                        geometry_node_id: node_id,
                        center: e.center,
                        major_axis: e.major_axis,
                        axis_ratio: e.minor_axis_ratio,
                    }
                } else {
                    AssocConstraintNodeData::BoundedEllipse {
                        geometry_dependency: Handle::NULL,
                        geometry_node_id: node_id,
                        center: e.center,
                        major_axis: e.major_axis,
                        axis_ratio: e.minor_axis_ratio,
                        start_point: point_at(e.start_parameter),
                        end_point: point_at(e.end_parameter),
                    }
                };
                let class_name = if e.is_full() {
                    "AcConstrainedEllipse"
                } else {
                    "AcConstrainedBoundedEllipse"
                };
                self.push_node(node_id, class_name, data);
            }
            _ => return None,
        }
        self.entities.entry(handle).or_default().geometry_node_id = node_id;
        if !self.referenced_entities.contains(&handle) {
            self.referenced_entities.push(handle);
        }
        Some(node_id)
    }

    /// Returns the point-node id for a line endpoint or curve center,
    /// creating it the first time the marker is referenced.
    fn point_node(&mut self, handle: Handle, marker: i32) -> Option<i32> {
        if let Some(existing) = self
            .entities
            .get(&handle)
            .and_then(|e| e.points.get(&marker))
        {
            return Some(*existing);
        }
        let polyline = matches!(
            self.document.get_entity(handle),
            Some(EntityType::LwPolyline(_) | EntityType::Polyline2D(_))
        );
        let (curve_id, point_type) = if polyline && marker >= 0 {
            let entity = self.document.get_entity(handle)?;
            let points = super::dimension_assoc::source_points(entity);
            let closed = match entity {
                EntityType::LwPolyline(polyline) => polyline.is_closed,
                EntityType::Polyline2D(polyline) => polyline.is_closed(),
                _ => false,
            };
            let vertex = marker as usize;
            if vertex >= points.len() {
                return None;
            }
            let (segment, point_type) = if vertex + 1 < points.len() || closed {
                (vertex, implicit_point_type::START)
            } else {
                (vertex.checked_sub(1)?, implicit_point_type::END)
            };
            (self.segment_node(handle, segment)?, point_type)
        } else {
            let curve_id = self.geometry_node(handle)?;
            let point_type = match marker {
                0 => implicit_point_type::START,
                1 => implicit_point_type::END,
                -3 => implicit_point_type::CENTER,
                _ => return None,
            };
            (curve_id, point_type)
        };
        let node_id = self.alloc_node_id();
        self.push_node(
            node_id,
            "AcConstrainedImplicitPoint",
            AssocConstraintNodeData::ImplicitPoint {
                geometry_dependency: Handle::NULL,
                geometry_node_id: node_id,
                point: None,
                point_type,
                point_index: -1,
                curve_id,
            },
        );
        let relation_id = self.alloc_node_id();
        self.push_node(
            relation_id,
            if point_type == implicit_point_type::CENTER {
                "AcCenterPointConstraint"
            } else {
                "AcPointCurveConstraint"
            },
            AssocConstraintNodeData::Geometrical {
                owner_id: 0,
                is_implied: false,
                is_active: true,
            },
        );
        self.connect(relation_id, node_id);
        self.connect(relation_id, curve_id);
        self.entities
            .entry(handle)
            .or_default()
            .points
            .insert(marker, node_id);
        Some(node_id)
    }

    /// The node id `SketchRef` resolves to: a point node for a marked
    /// reference, the whole geometry node otherwise.
    fn ref_node(&mut self, r: SketchRef) -> Option<i32> {
        if let Some(segment) = r.segment_index() {
            return self.segment_node(r.entity, segment);
        }
        if r.marker == Some(0)
            && matches!(
                self.document.get_entity(r.entity),
                Some(
                    EntityType::Point(_)
                        | EntityType::Insert(_)
                        | EntityType::Text(_)
                        | EntityType::MText(_)
                        | EntityType::AttributeDefinition(_)
                        | EntityType::AttributeEntity(_)
                        | EntityType::Table(_)
                )
            )
        {
            return self.geometry_node(r.entity);
        }
        match r.marker {
            Some(marker) => self.point_node(r.entity, marker),
            None => self.geometry_node(r.entity),
        }
    }

    fn segment_node(&mut self, handle: Handle, index: usize) -> Option<i32> {
        if let Some(node_id) = self
            .entities
            .get(&handle)
            .and_then(|entity| entity.segments.get(&index))
        {
            return Some(*node_id);
        }
        let entity = self.document.get_entity(handle)?;
        let points = super::dimension_assoc::source_points(entity);
        let closed = match entity {
            EntityType::LwPolyline(polyline) => {
                if polyline.vertices.get(index)?.bulge.abs() > 1e-9 {
                    return None;
                }
                polyline.is_closed
            }
            EntityType::Polyline2D(polyline) => {
                if polyline.vertices.get(index)?.bulge.abs() > 1e-9 {
                    return None;
                }
                polyline.is_closed()
            }
            _ => return None,
        };
        let start = *points.get(index)?;
        let end = if index + 1 < points.len() {
            points[index + 1]
        } else if closed {
            *points.first()?
        } else {
            return None;
        };
        let node_id = self.alloc_node_id();
        self.push_node(
            node_id,
            "AcConstrainedBoundedLine",
            AssocConstraintNodeData::BoundedLine {
                geometry_dependency: Handle::NULL,
                geometry_node_id: node_id,
                point: start,
                direction: (end - start).normalize(),
                is_ray: false,
                start_point: start,
                end_point: end,
            },
        );
        self.entities
            .entry(handle)
            .or_default()
            .segments
            .insert(index, node_id);
        if !self.referenced_entities.contains(&handle) {
            self.referenced_entities.push(handle);
        }
        Some(node_id)
    }

    fn datum_line(&mut self, horizontal: bool) -> i32 {
        let cached = if horizontal {
            self.horizontal_datum
        } else {
            self.vertical_datum
        };
        if let Some(node_id) = cached {
            return node_id;
        }
        let node_id = self.alloc_node_id();
        self.push_node(
            node_id,
            "AcConstrainedDatumLine",
            AssocConstraintNodeData::Line {
                geometry_dependency: Handle::NULL,
                geometry_node_id: node_id,
                point: Vector3::ZERO,
                direction: if horizontal {
                    Vector3::UNIT_X
                } else {
                    Vector3::UNIT_Y
                },
            },
        );
        if horizontal {
            self.horizontal_datum = Some(node_id);
        } else {
            self.vertical_datum = Some(node_id);
        }
        node_id
    }

    /// Records an undirected edge on both nodes.
    fn connect(&mut self, a: i32, b: i32) {
        if let Some(node) = self.nodes.iter_mut().find(|n| n.node_id == a) {
            node.connections.push(b);
        }
        if let Some(node) = self.nodes.iter_mut().find(|n| n.node_id == b) {
            node.connections.push(a);
        }
    }
}

/// Class name for a plain `Geometrical`-shaped constraint.
fn geometrical_class_name(kind: ConstraintKind) -> Option<&'static str> {
    match kind {
        ConstraintKind::Coincident => Some("AcPointCoincidenceConstraint"),
        ConstraintKind::Perpendicular => Some("AcPerpendicularConstraint"),
        ConstraintKind::Tangent => Some("AcTangentConstraint"),
        ConstraintKind::Smooth => Some("AcG2SmoothConstraint"),
        // the native format's own `GeomConstraintType` enum (native SDK's
        // `AcGeomConstraint.h`) lists `kNormal` alongside `kPerpendicular`
        // as a distinct, real geometric-constraint kind.
        ConstraintKind::Normal => Some("AcNormalConstraint"),
        ConstraintKind::Concentric => Some("AcConcentricConstraint"),
        ConstraintKind::CenterPoint => Some("AcCenterPointConstraint"),
        ConstraintKind::Colinear => Some("AcColinearConstraint"),
        ConstraintKind::Fixed => Some("AcFixedConstraint"),
        ConstraintKind::Midpoint => Some("AcMidPointConstraint"),
        ConstraintKind::PointOnCurve => Some("AcPointCurveConstraint"),
        ConstraintKind::Symmetric => Some("AcSymmetricConstraint"),
        // Also a plain `Geometrical`-shaped node in the native format's own class list
        // (`is_plain_geometrical_constraint`) — unlike `Equal`, its class
        // name never branches on entity type, so it belongs here rather
        // than in the caller's per-entity-type dispatch.
        ConstraintKind::EqualDistance => Some("AcEqualDistanceConstraint"),
        // `Equal` needs the resolved entity types, so the caller handles it.
        ConstraintKind::Equal
        | ConstraintKind::Horizontal
        | ConstraintKind::Vertical
        | ConstraintKind::Parallel
        | ConstraintKind::Distance
        | ConstraintKind::Angle
        | ConstraintKind::Radius
        | ConstraintKind::Diameter
        | ConstraintKind::DistanceX
        | ConstraintKind::DistanceY => None,
        // No native DWG representation exists at all: neither
        // `AcExplicitConstr.h` (Distance/Angle/RadiusDiameter — no
        // ArcLength-shaped class) nor `acadrust`'s `AssocConstraintNodeData`
        // has anything for it. `constraint_node` below falls through to
        // its own `_ => None` for this kind, same "can't persist to DWG,
        // XRecord-only" gap the dependency-chain DXF omission already
        // documents.
        ConstraintKind::ArcLength => None,
    }
}

/// `Diameter`/`DistanceX`/`DistanceY` reuse `Radius`'/`Distance`'s own DWG
/// class — native implementations represents them as the same
/// `AcRadiusDiameterConstraint`/`AcDistanceConstraint` object with a
/// different `RadiusDiameterConstrType`/`DirectionType` mode byte (set in
/// `constraint_node` below), not a distinct native class.
const fn dimensional_class_name(kind: ConstraintKind) -> &'static str {
    match kind {
        ConstraintKind::Distance | ConstraintKind::DistanceX | ConstraintKind::DistanceY => {
            "AcDistanceConstraint"
        }
        ConstraintKind::Angle => "AcAngleConstraint",
        ConstraintKind::Radius | ConstraintKind::Diameter => "AcRadiusDiameterConstraint",
        _ => "",
    }
}

/// Builds one `AssocConstraintNode` for `constraint`, returning its node id
/// so the caller can track which ones still need a `value_dependency`
/// patched in once handles can be allocated (pass 2, `Allocator` — building
/// node *shape* only needs read access to resolve `Equal`'s line-vs-circle
/// split, `AssocGeomDependency`/`AssocValueDependency` handles need mutable
/// access to the same document, so this has to be two passes). `None` if
/// this constraint's refs don't resolve to nodes this pass supports
/// (mirrors `sketch_solve::build_constraint`'s own "skip, don't error"
/// contract for an unbuildable constraint).
fn constraint_node(
    builder: &mut GroupBuilder,
    document: &CadDocument,
    constraint: &SketchConstraint,
    needs_value_dependency: &mut Vec<(i32, Option<DrivingValue>)>,
) -> Option<i32> {
    let refs: &[SketchRef] = &constraint.refs;
    if let Some(class_name) = geometrical_class_name(constraint.kind) {
        let target_refs: Vec<i32> = refs.iter().filter_map(|r| builder.ref_node(*r)).collect();
        if target_refs.len() != refs.len() {
            return None;
        }
        let owner_id = *target_refs.first()?;
        let node_id = builder.alloc_node_id();
        builder.push_node(
            node_id,
            class_name,
            AssocConstraintNodeData::Geometrical {
                owner_id,
                is_implied: false,
                is_active: true,
            },
        );
        for target in &target_refs {
            builder.connect(node_id, *target);
        }
        return Some(node_id);
    }
    match constraint.kind {
        ConstraintKind::Horizontal | ConstraintKind::Vertical => {
            if !matches!(refs.len(), 1 | 2) {
                return None;
            }
            let targets: Vec<_> = refs
                .iter()
                .map(|reference| builder.ref_node(*reference))
                .collect::<Option<_>>()?;
            let horizontal = constraint.kind == ConstraintKind::Horizontal;
            let datum = builder.datum_line(horizontal);
            let node_id = builder.alloc_node_id();
            builder.push_node(
                node_id,
                if horizontal {
                    "AcHorizontalConstraint"
                } else {
                    "AcVerticalConstraint"
                },
                AssocConstraintNodeData::Parallel {
                    owner_id: targets[0],
                    is_implied: false,
                    is_active: true,
                    datum_line_index: Some(datum),
                },
            );
            for target in targets {
                builder.connect(node_id, target);
            }
            Some(node_id)
        }
        ConstraintKind::Parallel => {
            let [a, b] = refs else { return None };
            let (na, nb) = (builder.ref_node(*a)?, builder.ref_node(*b)?);
            let node_id = builder.alloc_node_id();
            builder.push_node(
                node_id,
                "AcParallelConstraint",
                AssocConstraintNodeData::Parallel {
                    owner_id: na,
                    is_implied: false,
                    is_active: true,
                    datum_line_index: None,
                },
            );
            builder.connect(node_id, na);
            builder.connect(node_id, nb);
            Some(node_id)
        }
        ConstraintKind::Equal => {
            let [a, b] = refs else { return None };
            let (na, nb) = (builder.ref_node(*a)?, builder.ref_node(*b)?);
            let is_straight = |reference: &SketchRef| {
                matches!(
                    document.get_entity(reference.entity),
                    Some(EntityType::Line(_))
                ) || (reference.segment_index().is_some()
                    && matches!(
                        document.get_entity(reference.entity),
                        Some(EntityType::LwPolyline(_) | EntityType::Polyline2D(_))
                    ))
            };
            let both_lines = is_straight(a) && is_straight(b);
            let class_name = if both_lines {
                "AcEqualLengthConstraint"
            } else {
                "AcEqualRadiusConstraint"
            };
            let node_id = builder.alloc_node_id();
            builder.push_node(
                node_id,
                class_name,
                AssocConstraintNodeData::Geometrical {
                    owner_id: na,
                    is_implied: false,
                    is_active: true,
                },
            );
            builder.connect(node_id, na);
            builder.connect(node_id, nb);
            Some(node_id)
        }
        ConstraintKind::Distance
        | ConstraintKind::DistanceX
        | ConstraintKind::DistanceY
        | ConstraintKind::Angle
        | ConstraintKind::Radius
        | ConstraintKind::Diameter => {
            let target_refs: Vec<i32> = match constraint.kind {
                ConstraintKind::Radius | ConstraintKind::Diameter => {
                    vec![builder.ref_node(*refs.first()?)?]
                }
                _ => {
                    let [a, b] = refs else { return None };
                    vec![builder.ref_node(*a)?, builder.ref_node(*b)?]
                }
            };
            let owner_id = *target_refs.first()?;
            let node_id = builder.alloc_node_id();
            let data = match constraint.kind {
                // Plain two-point distance: undirected (`kNotDirected`).
                ConstraintKind::Distance => AssocConstraintNodeData::Distance {
                    owner_id,
                    is_implied: false,
                    is_active: true,
                    value_dependency: Handle::NULL,
                    dimension_dependency: Handle::NULL,
                    direction_type: 0,
                    distance: None,
                },
                // X-only/Y-only: `kFixedDirection` with the fixed unit
                // vector the distance is measured along — the native format's own
                // representation of DistanceX/DistanceY, not a distinct
                // constraint class.
                ConstraintKind::DistanceX => AssocConstraintNodeData::Distance {
                    owner_id,
                    is_implied: false,
                    is_active: true,
                    value_dependency: Handle::NULL,
                    dimension_dependency: Handle::NULL,
                    direction_type: 1,
                    distance: Some(Vector3::new(1.0, 0.0, 0.0)),
                },
                ConstraintKind::DistanceY => AssocConstraintNodeData::Distance {
                    owner_id,
                    is_implied: false,
                    is_active: true,
                    value_dependency: Handle::NULL,
                    dimension_dependency: Handle::NULL,
                    direction_type: 1,
                    distance: Some(Vector3::new(0.0, 1.0, 0.0)),
                },
                ConstraintKind::Angle => AssocConstraintNodeData::Angle {
                    owner_id,
                    is_implied: false,
                    is_active: true,
                    value_dependency: Handle::NULL,
                    dimension_dependency: Handle::NULL,
                    sector_type: 0,
                },
                ConstraintKind::Radius => AssocConstraintNodeData::RadiusDiameter {
                    owner_id,
                    is_implied: false,
                    is_active: true,
                    value_dependency: Handle::NULL,
                    dimension_dependency: Handle::NULL,
                    mode: 0,
                },
                // `kCircleDiameter` — same class as Radius (`mode: 0` /
                // `kCircleRadius`), different mode byte.
                ConstraintKind::Diameter => AssocConstraintNodeData::RadiusDiameter {
                    owner_id,
                    is_implied: false,
                    is_active: true,
                    value_dependency: Handle::NULL,
                    dimension_dependency: Handle::NULL,
                    mode: 1,
                },
                _ => unreachable!(),
            };
            builder.push_node(node_id, dimensional_class_name(constraint.kind), data);
            for target in &target_refs {
                builder.connect(node_id, *target);
            }
            needs_value_dependency.push((node_id, constraint.driving_param.clone()));
            Some(node_id)
        }
        _ => None,
    }
}

/// Everything a scope's native graph needs beyond the group's own object:
/// the `AssocGeomDependency` and `AssocValueDependency`/`AssocVariable`
/// objects it references by handle, and the network/dictionary wiring.
struct Allocator<'a> {
    document: &'a mut CadDocument,
    /// One shared variable per distinct parameter in this scope.
    variables: FxHashMap<String, Handle>,
    variable_actions: Vec<Handle>,
}

impl Allocator<'_> {
    fn numeric_eval(value: f64) -> AssocEvalVariant {
        if value.fract() == 0.0 && value >= i32::MIN as f64 && value <= i32::MAX as f64 {
            AssocEvalVariant {
                code: 90,
                value: AssocEvalValue::Long(value as i32),
            }
        } else {
            AssocEvalVariant {
                code: 40,
                value: AssocEvalValue::Real(value),
            }
        }
    }

    fn insert_associative(
        &mut self,
        owner: Handle,
        dxf_name: &str,
        cpp_class_name: &str,
        data: AssociativeData,
    ) -> Handle {
        let handle = self.document.allocate_handle();
        self.insert_associative_at(handle, owner, dxf_name, cpp_class_name, data);
        handle
    }

    /// Like [`Allocator::insert_associative`], but for a handle already
    /// allocated up front — needed for the group/network pair, which
    /// reference each other (`AssocAction::owning_network` /
    /// `AssocNetwork::owned_actions`) and so must both have real handles
    /// before either object is actually built.
    fn insert_associative_at(
        &mut self,
        handle: Handle,
        owner: Handle,
        dxf_name: &str,
        cpp_class_name: &str,
        data: AssociativeData,
    ) {
        let object = AssociativeObject {
            handle,
            owner,
            dxf_name: dxf_name.to_string(),
            cpp_class_name: cpp_class_name.to_string(),
            data,
            ..Default::default()
        };
        self.document
            .objects
            .insert(handle, ObjectType::Associative(object));
    }

    fn geom_dependency(
        &mut self,
        group_handle: Handle,
        entity: Handle,
        dependency_id: i32,
    ) -> Handle {
        self.insert_associative(
            group_handle,
            "ACDBASSOCGEOMDEPENDENCY",
            "AcDbAssocGeomDependency",
            AssociativeData::GeomDependency(AssocGeomDependency {
                dependency: AssocDependency {
                    class_version: 2,
                    is_read_dependency: true,
                    is_write_dependency: true,
                    is_attached_to_object: true,
                    is_delegating_to_owning_action: true,
                    order: -10_000,
                    dependent_on: entity,
                    dependency_body_id: dependency_id,
                    ..Default::default()
                },
                class_version: 0,
                enabled: true,
                // Point and curve addressing lives on constraint-node data.
                persistent_subent: AssocPersistentSubentId::default(),
            }),
        )
    }

    /// Returns the `AssocVariable` handle for `name`, creating it once per scope.
    fn variable(
        &mut self,
        network_handle: Handle,
        name: &str,
        formula: &str,
        resolved: f64,
    ) -> Handle {
        if let Some(&handle) = self.variables.get(name) {
            return handle;
        }
        let handle = self.insert_associative(
            network_handle,
            "ACDBASSOCVARIABLE",
            "AcDbAssocVariable",
            AssociativeData::Variable(AssocVariable {
                action: AssocAction {
                    class_version: 2,
                    owning_network: network_handle,
                    action_index: self.variable_actions.len() as i32 + 1,
                    ..Default::default()
                },
                class_version: 2,
                name: name.to_string(),
                expression: formula.to_string(),
                evaluator: "AcDbCalc:1.0".to_string(),
                description: String::new(),
                value: Self::numeric_eval(resolved),
                has_cached_value: false,
                cached_value: String::new(),
                flag: false,
                reserved: 0,
            }),
        );
        self.variables.insert(name.to_string(), handle);
        self.variable_actions.push(handle);
        handle
    }

    fn value_dependency(
        &mut self,
        group_handle: Handle,
        variable_handle: Handle,
        resolved: f64,
        dependency_id: i32,
    ) -> Handle {
        let value = Self::numeric_eval(resolved);
        let handle = self.insert_associative(
            group_handle,
            "ACDBASSOCVALUEDEPENDENCY",
            "AcDbAssocValueDependency",
            AssociativeData::ValueDependency(AssocValueDependency {
                dependency: AssocDependency {
                    class_version: 2,
                    is_read_dependency: true,
                    is_write_dependency: false,
                    is_attached_to_object: true,
                    is_delegating_to_owning_action: true,
                    dependent_on: variable_handle,
                    dependency_body_id: dependency_id,
                    ..Default::default()
                },
                class_version: 0,
                name: String::new(),
                value,
            }),
        );
        if let Some(ObjectType::Associative(variable)) =
            self.document.objects.get_mut(&variable_handle)
        {
            variable.reactors.push(handle);
        }
        handle
    }
}

/// Sets the dependency on whole-geometry nodes. Implicit points retain a
/// null dependency and refer to their owning geometry node by `curve_id`.
fn set_geometry_dependency(data: &mut AssocConstraintNodeData, handle: Handle) {
    match data {
        AssocConstraintNodeData::Point {
            geometry_dependency,
            ..
        }
        | AssocConstraintNodeData::BoundedLine {
            geometry_dependency,
            ..
        }
        | AssocConstraintNodeData::Circle {
            geometry_dependency,
            ..
        }
        | AssocConstraintNodeData::Arc {
            geometry_dependency,
            ..
        }
        | AssocConstraintNodeData::Ellipse {
            geometry_dependency,
            ..
        }
        | AssocConstraintNodeData::BoundedEllipse {
            geometry_dependency,
            ..
        } => {
            *geometry_dependency = handle;
        }
        _ => {}
    }
}

/// Patches a dimensional-constraint node's `value_dependency` field
/// (`Distance`/`Angle`/`RadiusDiameter` — the three kinds
/// [`constraint_node`] defers to `needs_value_dependency`).
fn set_value_dependency(data: &mut AssocConstraintNodeData, handle: Handle) {
    match data {
        AssocConstraintNodeData::Distance {
            value_dependency, ..
        }
        | AssocConstraintNodeData::Angle {
            value_dependency, ..
        }
        | AssocConstraintNodeData::RadiusDiameter {
            value_dependency, ..
        } => {
            *value_dependency = handle;
        }
        _ => {}
    }
}

/// Associative classes that must have real class numbers in DWG/DXF.
const ASSOC_CLASSES: &[(&str, &str, i32)] = &[
    (
        "ACDBASSOC2DCONSTRAINTGROUP",
        "AcDbAssoc2dConstraintGroup",
        45,
    ),
    ("ACDBASSOCNETWORK", "AcDbAssocNetwork", 45),
    ("ACDBASSOCVARIABLE", "AcDbAssocVariable", 45),
    ("ACDBASSOCGEOMDEPENDENCY", "AcDbAssocGeomDependency", 29),
    ("ACDBASSOCVALUEDEPENDENCY", "AcDbAssocValueDependency", 29),
];

fn ensure_associative_classes_registered(document: &mut CadDocument) {
    for (dxf_name, cpp_class_name, maintenance_version) in ASSOC_CLASSES {
        if document.classes.get_by_name(dxf_name).is_some() {
            continue;
        }
        document.classes.add_or_update(acadrust::classes::DxfClass {
            dxf_name: dxf_name.to_string(),
            cpp_class_name: cpp_class_name.to_string(),
            application_name: "ObjectDBX Classes".to_string(),
            proxy_flags: acadrust::classes::ProxyFlags(
                acadrust::classes::ProxyFlags::ERASE_ALLOWED.0
                    | acadrust::classes::ProxyFlags::CLONING_ALLOWED.0
                    | acadrust::classes::ProxyFlags::DISABLES_PROXY_WARNING_DIALOG.0,
            ),
            instance_count: 0,
            was_zombie: false,
            is_an_entity: false,
            class_number: 0,
            // 499 = "object" (non-entity) — every class here is one.
            item_class_id: 0x1F3,
            dwg_version: 27,
            maintenance_version: *maintenance_version,
            unknown1: 0,
            unknown2: 0,
        });
    }
}

/// Ensures `owner` has an extension dictionary through the public API.
fn ensure_extension_dictionary(document: &mut CadDocument, owner: Handle) -> Handle {
    if let Some(handle) = document.extension_dictionary_handle(owner) {
        return handle;
    }
    const BOOTSTRAP_KEY: &str = "OCS_DWG_NATIVE_BOOTSTRAP";
    document.ensure_xrecord(owner, BOOTSTRAP_KEY);
    let dictionary_handle = document
        .extension_dictionary_handle(owner)
        .expect("ensure_xrecord just created the extension dictionary");
    remove_dictionary_entry(document, dictionary_handle, BOOTSTRAP_KEY);
    dictionary_handle
}

/// Removes a dictionary entry and its owned object tree.
fn remove_dictionary_entry(document: &mut CadDocument, dictionary_handle: Handle, key: &str) {
    let Some(ObjectType::Dictionary(dictionary)) = document.objects.get_mut(&dictionary_handle)
    else {
        return;
    };
    let Some(index) = dictionary
        .entries
        .iter()
        .position(|(name, _)| name.eq_ignore_ascii_case(key))
    else {
        return;
    };
    let (_, target) = dictionary.entries.remove(index);
    remove_owned_recursive(document, target);
}

fn set_dictionary_entry(
    document: &mut CadDocument,
    dictionary_handle: Handle,
    key: &str,
    target: Handle,
) {
    let Some(ObjectType::Dictionary(dictionary)) = document.objects.get_mut(&dictionary_handle)
    else {
        return;
    };
    if let Some((_, handle)) = dictionary
        .entries
        .iter_mut()
        .find(|(name, _)| name.eq_ignore_ascii_case(key))
    {
        *handle = target;
    } else {
        dictionary.add_entry(key, target);
    }
}

fn ensure_global_network_dictionary(document: &mut CadDocument) -> Handle {
    let root_handle = document.header.named_objects_dict_handle;
    if let Some(ObjectType::Dictionary(root)) = document.objects.get(&root_handle) {
        if let Some(handle) = root.get(NETWORK_DICTIONARY_KEY) {
            if matches!(
                document.objects.get(&handle),
                Some(ObjectType::Dictionary(_))
            ) {
                return handle;
            }
        }
    }

    remove_dictionary_entry(document, root_handle, NETWORK_DICTIONARY_KEY);
    let dictionary_handle = document.allocate_handle();
    let mut dictionary = acadrust::objects::Dictionary::new();
    dictionary.handle = dictionary_handle;
    dictionary.owner = root_handle;
    dictionary.reactors.push(root_handle);
    document
        .objects
        .insert(dictionary_handle, ObjectType::Dictionary(dictionary));
    set_dictionary_entry(
        document,
        root_handle,
        NETWORK_DICTIONARY_KEY,
        dictionary_handle,
    );
    dictionary_handle
}

/// Removes `root` and every object transitively owned by it so rebuilding the
/// graph cannot accumulate orphaned dependency or variable objects.
fn remove_owned_recursive(document: &mut CadDocument, root: Handle) {
    if root.is_null() {
        return;
    }
    let children: Vec<Handle> = document
        .objects
        .keys()
        .copied()
        .filter(|&h| document.object_owner(h) == Some(root))
        .collect();
    for child in children {
        remove_owned_recursive(document, child);
    }
    document.objects.remove(&root);
}

/// Whether `owner` already has a native graph that must stay synchronized.
fn has_native_network(document: &CadDocument, owner: Handle) -> bool {
    let Some(dictionary_handle) = document.extension_dictionary_handle(owner) else {
        return false;
    };
    let Some(ObjectType::Dictionary(dictionary)) = document.objects.get(&dictionary_handle) else {
        return false;
    };
    dictionary.get(NETWORK_DICTIONARY_KEY).is_some()
}

fn native_group(document: &CadDocument, owner: Handle) -> Option<&Assoc2dConstraintGroup> {
    let network_handle = native_scope_network_handle(document, owner)?;
    document.objects.values().find_map(|object| {
        let ObjectType::Associative(object) = object else {
            return None;
        };
        let AssociativeData::ConstraintGroup(group) = &object.data else {
            return None;
        };
        (object.owner == network_handle || group.action.owning_network == network_handle)
            .then_some(group)
    })
}

fn native_scope_network_handle(document: &CadDocument, owner: Handle) -> Option<Handle> {
    let dictionary = document.extension_dictionary_handle(owner)?;
    let ObjectType::Dictionary(dictionary) = document.objects.get(&dictionary)? else {
        return None;
    };
    let mut network_handle = dictionary.get(NETWORK_DICTIONARY_KEY)?;
    while let Some(ObjectType::Dictionary(dictionary)) = document.objects.get(&network_handle) {
        network_handle = dictionary.get(NETWORK_DICTIONARY_KEY)?;
    }
    matches!(
        document.objects.get(&network_handle),
        Some(ObjectType::Associative(AssociativeObject {
            data: AssociativeData::Network(_),
            ..
        }))
    )
    .then_some(network_handle)
}

fn geometry_dependency(data: &AssocConstraintNodeData) -> Option<Handle> {
    match data {
        AssocConstraintNodeData::Point {
            geometry_dependency,
            ..
        }
        | AssocConstraintNodeData::Line {
            geometry_dependency,
            ..
        }
        | AssocConstraintNodeData::BoundedLine {
            geometry_dependency,
            ..
        }
        | AssocConstraintNodeData::Circle {
            geometry_dependency,
            ..
        }
        | AssocConstraintNodeData::Arc {
            geometry_dependency,
            ..
        }
        | AssocConstraintNodeData::Ellipse {
            geometry_dependency,
            ..
        }
        | AssocConstraintNodeData::BoundedEllipse {
            geometry_dependency,
            ..
        } if !geometry_dependency.is_null() => Some(*geometry_dependency),
        _ => None,
    }
}

fn dependency_entity(
    document: &CadDocument,
    dependency: Handle,
    geometry: &AssocConstraintNodeData,
) -> Option<Handle> {
    let ObjectType::Associative(object) = document.objects.get(&dependency)? else {
        return None;
    };
    let AssociativeData::GeomDependency(dependency) = &object.data else {
        return None;
    };
    let entity = dependency.dependency.dependent_on;
    let supported = match (document.get_entity(entity), geometry) {
        (
            Some(
                EntityType::Point(_)
                | EntityType::Insert(_)
                | EntityType::Text(_)
                | EntityType::MText(_)
                | EntityType::AttributeDefinition(_)
                | EntityType::AttributeEntity(_)
                | EntityType::Table(_),
            ),
            AssocConstraintNodeData::Point { .. },
        ) => true,
        (
            Some(EntityType::Line(_) | EntityType::LwPolyline(_) | EntityType::Polyline2D(_)),
            AssocConstraintNodeData::Line { .. } | AssocConstraintNodeData::BoundedLine { .. },
        ) => true,
        (Some(EntityType::Circle(_)), AssocConstraintNodeData::Circle { .. }) => true,
        (Some(EntityType::Arc(_)), AssocConstraintNodeData::Arc { .. }) => true,
        (
            Some(EntityType::Ellipse(_)),
            AssocConstraintNodeData::Ellipse { .. }
            | AssocConstraintNodeData::BoundedEllipse { .. },
        ) => true,
        _ => false,
    };
    supported.then_some(entity)
}

fn native_group_requires_preservation(document: &CadDocument, owner: Handle) -> bool {
    native_group(document, owner).is_some_and(|group| {
        group.nodes.iter().any(|node| {
            if matches!(node.data, AssocConstraintNodeData::RigidSet { .. }) {
                return true;
            }
            if let Some(dependency) = geometry_dependency(&node.data) {
                if dependency_entity(document, dependency, &node.data).is_none() {
                    return true;
                }
            }
            node.class_name.to_ascii_uppercase().ends_with("CONSTRAINT")
                && constraint_kind(node).is_none()
        })
    })
}

fn polyline_segment_reference(
    document: &CadDocument,
    entity: Handle,
    data: &AssocConstraintNodeData,
) -> Option<SketchRef> {
    let AssocConstraintNodeData::BoundedLine {
        start_point,
        end_point,
        ..
    } = data
    else {
        return None;
    };
    let entity_value = document.get_entity(entity)?;
    let closed = match entity_value {
        EntityType::LwPolyline(polyline) => polyline.is_closed,
        EntityType::Polyline2D(polyline) => polyline.is_closed(),
        _ => return None,
    };
    let points = super::dimension_assoc::source_points(entity_value);
    let segment_count = points.len().saturating_sub(usize::from(!closed));
    (0..segment_count)
        .min_by(|first, second| {
            let score = |index: usize| {
                let a = points[index];
                let b = points[(index + 1) % points.len()];
                let forward =
                    (a - *start_point).length_squared() + (b - *end_point).length_squared();
                let reverse =
                    (b - *start_point).length_squared() + (a - *end_point).length_squared();
                forward.min(reverse)
            };
            score(*first).total_cmp(&score(*second))
        })
        .map(|index| SketchRef::segment(entity, index))
}

fn numeric_value(value: &AssocEvalVariant) -> Option<f64> {
    match value.value {
        AssocEvalValue::Real(value) => Some(value),
        AssocEvalValue::Long(value) => Some(value as f64),
        AssocEvalValue::Short(value) => Some(value as f64),
        AssocEvalValue::Byte(value) => Some(value as f64),
        _ => None,
    }
}

fn driving_value(
    document: &CadDocument,
    dependency: Handle,
    parameters: &mut ParameterTable,
) -> Option<DrivingValue> {
    let ObjectType::Associative(object) = document.objects.get(&dependency)? else {
        return None;
    };
    let AssociativeData::ValueDependency(value_dependency) = &object.data else {
        return None;
    };
    let literal = numeric_value(&value_dependency.value);
    let ObjectType::Associative(variable) = document
        .objects
        .get(&value_dependency.dependency.dependent_on)?
    else {
        return literal.map(DrivingValue::Literal);
    };
    let AssociativeData::Variable(variable) = &variable.data else {
        return literal.map(DrivingValue::Literal);
    };
    let source = if variable.expression.trim().is_empty() {
        numeric_value(&variable.value)?.to_string()
    } else {
        variable.expression.clone()
    };
    if parameters.set(&variable.name, &source).is_ok() {
        Some(DrivingValue::Named(variable.name.clone()))
    } else {
        literal
            .or_else(|| numeric_value(&variable.value))
            .map(DrivingValue::Literal)
    }
}

fn constraint_owner(data: &AssocConstraintNodeData) -> Option<i32> {
    match data {
        AssocConstraintNodeData::Geometrical { owner_id, .. }
        | AssocConstraintNodeData::Angle { owner_id, .. }
        | AssocConstraintNodeData::Parallel { owner_id, .. }
        | AssocConstraintNodeData::Distance { owner_id, .. }
        | AssocConstraintNodeData::RadiusDiameter { owner_id, .. } => Some(*owner_id),
        _ => None,
    }
}

fn constraint_is_active(data: &AssocConstraintNodeData) -> bool {
    match data {
        AssocConstraintNodeData::Geometrical { is_active, .. }
        | AssocConstraintNodeData::Angle { is_active, .. }
        | AssocConstraintNodeData::Parallel { is_active, .. }
        | AssocConstraintNodeData::Distance { is_active, .. }
        | AssocConstraintNodeData::RadiusDiameter { is_active, .. } => *is_active,
        _ => true,
    }
}

fn constraint_kind(node: &AssocConstraintNode) -> Option<ConstraintKind> {
    Some(match node.class_name.to_ascii_uppercase().as_str() {
        "ACPOINTCOINCIDENCECONSTRAINT" => ConstraintKind::Coincident,
        "ACHORIZONTALCONSTRAINT" => ConstraintKind::Horizontal,
        "ACVERTICALCONSTRAINT" => ConstraintKind::Vertical,
        "ACPARALLELCONSTRAINT" => ConstraintKind::Parallel,
        "ACPERPENDICULARCONSTRAINT" => ConstraintKind::Perpendicular,
        "ACEQUALLENGTHCONSTRAINT" | "ACEQUALRADIUSCONSTRAINT" => ConstraintKind::Equal,
        "ACANGLECONSTRAINT" | "AC3POINTANGLECONSTRAINT" => ConstraintKind::Angle,
        "ACTANGENTCONSTRAINT" => ConstraintKind::Tangent,
        "ACG2SMOOTHCONSTRAINT" => ConstraintKind::Smooth,
        "ACCONCENTRICCONSTRAINT" => ConstraintKind::Concentric,
        "ACCENTERPOINTCONSTRAINT" => ConstraintKind::CenterPoint,
        "ACCOLINEARCONSTRAINT" => ConstraintKind::Colinear,
        "ACMIDPOINTCONSTRAINT" => ConstraintKind::Midpoint,
        "ACFIXEDCONSTRAINT" => ConstraintKind::Fixed,
        "ACPOINTCURVECONSTRAINT" => ConstraintKind::PointOnCurve,
        "ACEQUALDISTANCECONSTRAINT" => ConstraintKind::EqualDistance,
        "ACSYMMETRICCONSTRAINT" => ConstraintKind::Symmetric,
        "ACNORMALCONSTRAINT" => ConstraintKind::Normal,
        "ACRADIUSDIAMETERCONSTRAINT" => {
            let AssocConstraintNodeData::RadiusDiameter { mode, .. } = node.data else {
                return None;
            };
            if mode == 1 {
                ConstraintKind::Diameter
            } else {
                ConstraintKind::Radius
            }
        }
        "ACDISTANCECONSTRAINT" => {
            let AssocConstraintNodeData::Distance {
                direction_type,
                distance,
                ..
            } = &node.data
            else {
                return None;
            };
            if *direction_type == 0 {
                ConstraintKind::Distance
            } else if distance.is_some_and(|direction| direction.x.abs() >= direction.y.abs()) {
                ConstraintKind::DistanceX
            } else {
                ConstraintKind::DistanceY
            }
        }
        _ => return None,
    })
}

pub(super) fn native_constraint_set(
    document: &CadDocument,
    owner: Handle,
    parameters: &mut ParameterTable,
) -> Option<SketchConstraintSet> {
    let group = native_group(document, owner)?;
    let mut refs = FxHashMap::default();
    for node in &group.nodes {
        let Some(dependency) = geometry_dependency(&node.data) else {
            continue;
        };
        if let Some(entity) = dependency_entity(document, dependency, &node.data) {
            let reference = polyline_segment_reference(document, entity, &node.data)
                .unwrap_or_else(|| {
                    if matches!(node.data, AssocConstraintNodeData::Point { .. }) {
                        SketchRef::point(entity, 0)
                    } else {
                        SketchRef::whole(entity)
                    }
                });
            refs.insert(node.node_id, reference);
        }
    }
    for node in &group.nodes {
        let AssocConstraintNodeData::ImplicitPoint {
            point_type,
            curve_id,
            ..
        } = node.data
        else {
            continue;
        };
        let Some(curve) = refs.get(&curve_id).copied() else {
            continue;
        };
        let marker = if let Some(segment) = curve.segment_index() {
            let point_count =
                super::dimension_assoc::source_points(document.get_entity(curve.entity)?).len();
            (match point_type {
                implicit_point_type::START => segment,
                implicit_point_type::END if point_count > 0 => (segment + 1) % point_count,
                _ => continue,
            }) as i32
        } else {
            match point_type {
                implicit_point_type::START => 0,
                implicit_point_type::END => 1,
                implicit_point_type::CENTER => -3,
                _ => continue,
            }
        };
        refs.insert(node.node_id, SketchRef::point(curve.entity, marker));
    }

    let scope = if owner == document.header.model_space_block_handle {
        SketchScope::ModelSpace
    } else {
        SketchScope::Block(owner)
    };
    let mut set = SketchConstraintSet::new(scope);
    for node in &group.nodes {
        let Some(kind) = constraint_kind(node) else {
            continue;
        };
        if !constraint_is_active(&node.data) {
            continue;
        }
        let mut targets: Vec<SketchRef> = node
            .connections
            .iter()
            .filter_map(|id| refs.get(id).copied())
            .collect();
        if let Some(owner) = constraint_owner(&node.data).and_then(|id| refs.get(&id).copied()) {
            if !targets.contains(&owner) {
                targets.insert(0, owner);
            }
        }
        if matches!(
            kind,
            ConstraintKind::PointOnCurve | ConstraintKind::CenterPoint
        ) && targets.len() == 2
            && targets[0].entity == targets[1].entity
        {
            continue;
        }
        let driving = match &node.data {
            AssocConstraintNodeData::Angle {
                value_dependency, ..
            }
            | AssocConstraintNodeData::Distance {
                value_dependency, ..
            }
            | AssocConstraintNodeData::RadiusDiameter {
                value_dependency, ..
            } => driving_value(document, *value_dependency, parameters),
            _ => None,
        };
        let expected = match kind {
            ConstraintKind::Fixed | ConstraintKind::Radius | ConstraintKind::Diameter => 1,
            ConstraintKind::Symmetric => 3,
            ConstraintKind::EqualDistance => 4,
            _ => 2,
        };
        if (matches!(kind, ConstraintKind::Horizontal | ConstraintKind::Vertical)
            && !matches!(targets.len(), 1 | 2))
            || (!matches!(kind, ConstraintKind::Horizontal | ConstraintKind::Vertical)
                && targets.len() != expected)
        {
            continue;
        }
        if matches!(
            kind,
            ConstraintKind::Distance
                | ConstraintKind::DistanceX
                | ConstraintKind::DistanceY
                | ConstraintKind::Angle
                | ConstraintKind::Radius
                | ConstraintKind::Diameter
        ) && driving.is_none()
        {
            continue;
        }
        set.add(kind, targets, driving);
    }
    (!set.constraints.is_empty()).then_some(set)
}

/// One scope's worth of
/// [`Scene::materialize_dwg_native_constraints_for_save`] — see that
/// method's doc comment for the save-time contract this implements.
fn materialize_scope(
    document: &mut CadDocument,
    owner: Handle,
    set: &SketchConstraintSet,
    parameters: &ParameterTable,
    root_network_handle: Handle,
    action_index: i32,
) -> Option<Handle> {
    let dictionary_handle = ensure_extension_dictionary(document, owner);
    remove_dictionary_entry(document, dictionary_handle, NETWORK_DICTIONARY_KEY);

    // Pass 1 (read-only): constraint-node shapes.
    let mut needs_value_dependency = Vec::new();
    let (mut nodes, entities, referenced_entities) = {
        let mut builder = GroupBuilder::new(document);
        for constraint in &set.constraints {
            if !constraint.enabled {
                continue;
            }
            constraint_node(
                &mut builder,
                document,
                constraint,
                &mut needs_value_dependency,
            );
        }
        let GroupBuilder {
            nodes,
            entities,
            referenced_entities,
            ..
        } = builder;
        (nodes, entities, referenced_entities)
    };
    if nodes.is_empty() {
        return None;
    }

    // The stream starts with one synthetic root before the registered nodes.
    nodes.insert(
        0,
        AssocConstraintNode {
            node_id: 0,
            status: 0,
            connections: Vec::new(),
            class_name: String::new(),
            registry_flag: false,
            data: AssocConstraintNodeData::None,
        },
    );

    // Pass 2 (mutable): allocate handles for everything the node shapes
    // above still reference as `Handle::NULL`. The group and network
    // reference each other, so both handles are reserved up front.
    let group_handle = document.allocate_handle();
    let network_handle = document.allocate_handle();
    let mut allocator = Allocator {
        document,
        variables: FxHashMap::default(),
        variable_actions: Vec::new(),
    };

    let mut geometry_dependencies = Vec::with_capacity(referenced_entities.len());
    for (index, entity_handle) in referenced_entities.iter().enumerate() {
        let Some(entity_nodes) = entities.get(entity_handle) else {
            continue;
        };
        if entity_nodes.geometry_node_id == 0 {
            if entity_nodes.segments.is_empty() {
                continue;
            }
        }
        let dep_handle = allocator.geom_dependency(group_handle, *entity_handle, index as i32 + 1);
        geometry_dependencies.push(dep_handle);
        for node_id in std::iter::once(entity_nodes.geometry_node_id)
            .filter(|node_id| *node_id != 0)
            .chain(entity_nodes.segments.values().copied())
        {
            if let Some(node) = nodes.iter_mut().find(|node| node.node_id == node_id) {
                set_geometry_dependency(&mut node.data, dep_handle);
            }
        }
    }

    let mut group_dependencies = geometry_dependencies;
    for (node_id, driving) in needs_value_dependency {
        let Some(driving) = driving else { continue };
        let Ok(resolved) = driving.resolve(parameters) else {
            continue;
        };
        let (name, formula) = match &driving {
            DrivingValue::Named(name) => {
                let formula = parameters
                    .get(name)
                    .map(|p| p.source.clone())
                    .unwrap_or_default();
                (name.clone(), formula)
            }
            DrivingValue::Literal(_) => (format!("d{node_id}"), resolved.to_string()),
        };
        let variable_handle = allocator.variable(network_handle, &name, &formula, resolved);
        let dependency_id = group_dependencies.len() as i32 + 1;
        let dep_handle =
            allocator.value_dependency(group_handle, variable_handle, resolved, dependency_id);
        group_dependencies.push(dep_handle);
        if let Some(node) = nodes.iter_mut().find(|n| n.node_id == node_id) {
            set_value_dependency(&mut node.data, dep_handle);
        }
    }

    let group = Assoc2dConstraintGroup {
        action: AssocAction {
            class_version: 2,
            owning_network: network_handle,
            action_index: allocator.variable_actions.len() as i32 + 1,
            max_dependency_index: group_dependencies.len() as i32 + 1,
            ..Default::default()
        },
        version: 2,
        flag: false,
        // Only world-XY entities reach native graph materialization.
        work_plane: [Vector3::ZERO, Vector3::UNIT_X, Vector3::UNIT_Y],
        dependency: Handle::NULL,
        actions: group_dependencies,
        nodes,
    };
    allocator.insert_associative_at(
        group_handle,
        network_handle,
        "ASSOC2DCONSTRAINTGROUP",
        "AcDbAssoc2dConstraintGroup",
        AssociativeData::ConstraintGroup(group),
    );

    let network = AssocNetwork {
        action: AssocAction {
            class_version: 2,
            owning_network: root_network_handle,
            action_index,
            ..Default::default()
        },
        network_version: 0,
        network_action_index: allocator.variable_actions.len() as i32 + 1,
        actions: allocator
            .variable_actions
            .iter()
            .copied()
            .chain(std::iter::once(group_handle))
            .map(|dependency| AssocActionDependency {
                is_owned: true,
                dependency,
            })
            .collect(),
        owned_actions: Vec::new(),
    };
    allocator.insert_associative_at(
        network_handle,
        dictionary_handle,
        "ASSOCNETWORK",
        "AcDbAssocNetwork",
        AssociativeData::Network(network),
    );
    if let Some(ObjectType::Associative(network)) =
        allocator.document.objects.get_mut(&network_handle)
    {
        network.reactors.push(dictionary_handle);
    }

    set_dictionary_entry(
        allocator.document,
        dictionary_handle,
        NETWORK_DICTIONARY_KEY,
        network_handle,
    );
    Some(network_handle)
}

impl Scene {
    /// Rebuilds every sketch scope's native `AssocNetwork`/
    /// `Assoc2dConstraintGroup` graph from `self.sketch_constraints`,
    /// replacing whatever this module wrote on a previous save.
    ///
    /// `enabled` is the app's `write_dwg_native_constraints` Options toggle
    /// (off by default — this graph adds file weight every save, whether or
    /// not compatible applications interop is wanted). It only gates *creating* the
    /// graph in a scope that doesn't have one yet: a scope that already
    /// carries one — from an earlier save with the setting on, or from a
    /// file that came from native implementations — keeps getting it rebuilt in sync
    /// regardless of the current setting ([`has_native_network`]), so
    /// toggling this off never leaves a stale graph sitting in the file.
    pub(crate) fn materialize_dwg_native_constraints_for_save(&mut self, enabled: bool) {
        let mut scopes = Vec::new();
        let mut preserved_networks = Vec::new();
        for index in 0..self.sketch_constraints.len() {
            let set = self.sketch_constraints[index].clone();
            let owner = set.scope.owner_handle(&self.document);
            if owner.is_null() {
                continue;
            }
            if !enabled && !has_native_network(&self.document, owner) {
                continue;
            }
            if native_group_requires_preservation(&self.document, owner) {
                if let Some(network) = native_scope_network_handle(&self.document, owner) {
                    preserved_networks.push(network);
                }
                continue;
            }
            scopes.push((owner, set));
        }
        if scopes.is_empty() {
            return;
        }

        ensure_associative_classes_registered(&mut self.document);
        let global_dictionary = ensure_global_network_dictionary(&mut self.document);
        remove_dictionary_entry(
            &mut self.document,
            global_dictionary,
            NETWORK_DICTIONARY_KEY,
        );
        let root_network_handle = self.document.allocate_handle();
        let mut child_networks = preserved_networks;
        for (index, (owner, set)) in scopes.into_iter().enumerate() {
            if let Some(handle) = materialize_scope(
                &mut self.document,
                owner,
                &set,
                &self.named_parameters,
                root_network_handle,
                index as i32 + 1,
            ) {
                child_networks.push(handle);
            }
        }
        if child_networks.is_empty() {
            return;
        }

        let root_network = AssociativeObject {
            handle: root_network_handle,
            owner: global_dictionary,
            reactors: vec![global_dictionary],
            dxf_name: "ASSOCNETWORK".to_string(),
            cpp_class_name: "AcDbAssocNetwork".to_string(),
            data: AssociativeData::Network(AssocNetwork {
                action: AssocAction {
                    class_version: 2,
                    ..Default::default()
                },
                network_version: 0,
                network_action_index: child_networks.len() as i32,
                actions: child_networks
                    .into_iter()
                    .map(|dependency| AssocActionDependency {
                        is_owned: false,
                        dependency,
                    })
                    .collect(),
                owned_actions: Vec::new(),
            }),
            ..Default::default()
        };
        self.document
            .objects
            .insert(root_network_handle, ObjectType::Associative(root_network));
        set_dictionary_entry(
            &mut self.document,
            global_dictionary,
            NETWORK_DICTIONARY_KEY,
            root_network_handle,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::super::sketch_constraints::SketchScope;
    use super::*;
    use acadrust::entities::{Arc, Circle, Insert, Line, LwPolyline};
    use acadrust::tables::BlockRecord;
    use acadrust::types::Vector2;

    fn line_entity(scene: &mut Scene, start: (f64, f64), end: (f64, f64)) -> Handle {
        scene.add_entity(EntityType::Line(Line::from_points(
            Vector3::new(start.0, start.1, 0.0),
            Vector3::new(end.0, end.1, 0.0),
        )))
    }

    fn circle_entity(scene: &mut Scene, center: (f64, f64), radius: f64) -> Handle {
        scene.add_entity(EntityType::Circle(Circle::from_center_radius(
            Vector3::new(center.0, center.1, 0.0),
            radius,
        )))
    }

    fn arc_entity(scene: &mut Scene, center: (f64, f64), radius: f64) -> Handle {
        scene.add_entity(EntityType::Arc(Arc::from_center_radius_angles(
            Vector3::new(center.0, center.1, 0.0),
            radius,
            0.0,
            std::f64::consts::PI,
        )))
    }

    fn native_group_handle(document: &CadDocument, owner: Handle) -> Handle {
        let dict = document
            .extension_dictionary_handle(owner)
            .expect("extension dictionary should exist");
        let Some(ObjectType::Dictionary(dictionary)) = document.objects.get(&dict) else {
            panic!("expected owner's extension dictionary object to exist");
        };
        let network_handle = dictionary
            .get(NETWORK_DICTIONARY_KEY)
            .expect("ACAD_ASSOCNETWORK entry should exist");
        let Some(ObjectType::Associative(AssociativeObject {
            data: AssociativeData::Network(network),
            ..
        })) = document.objects.get(&network_handle)
        else {
            panic!("expected an AssocNetwork object at the ACAD_ASSOCNETWORK handle");
        };
        network
            .actions
            .iter()
            .filter(|dependency| dependency.is_owned)
            .map(|dependency| dependency.dependency)
            .find(|handle| {
                matches!(
                    document.objects.get(handle),
                    Some(ObjectType::Associative(AssociativeObject {
                        data: AssociativeData::ConstraintGroup(_),
                        ..
                    }))
                )
            })
            .expect("network should own the constraint group")
    }

    fn native_group(document: &CadDocument, owner: Handle) -> &Assoc2dConstraintGroup {
        let group_handle = native_group_handle(document, owner);
        let Some(ObjectType::Associative(AssociativeObject {
            data: AssociativeData::ConstraintGroup(group),
            ..
        })) = document.objects.get(&group_handle)
        else {
            panic!(
                "expected an Assoc2dConstraintGroup object at the network's owned action handle"
            );
        };
        group
    }

    #[test]
    fn horizontal_and_vertical_constraints_round_trip_through_dwg_and_dxf() {
        for ext in ["dwg", "dxf"] {
            let mut scene = Scene::new();
            let a = line_entity(&mut scene, (0.0, 0.0), (10.0, 0.0));
            let b = line_entity(&mut scene, (10.0, 0.0), (10.0, 5.0));
            scene
                .sketch_constraint_set_mut(SketchScope::ModelSpace)
                .add(ConstraintKind::Horizontal, vec![SketchRef::whole(a)], None);
            scene
                .sketch_constraint_set_mut(SketchScope::ModelSpace)
                .add(ConstraintKind::Vertical, vec![SketchRef::whole(b)], None);

            scene.materialize_dwg_native_constraints_for_save(true);
            let owner = scene.document.header.model_space_block_handle;
            // Root + two geometry nodes + two datums + two constraints.
            let before = native_group(&scene.document, owner);
            assert_eq!(
                before.nodes.len(),
                7,
                "expected root + 2 geometry + 2 datums + 2 constraints, got {:#?}",
                before.nodes
            );

            let bytes = crate::io::save_to_bytes(&scene.document, ext, scene.document.version)
                .unwrap_or_else(|e| panic!("save to {ext}: {e}"));
            let reloaded = crate::io::load_bytes(&format!("dwg_native_poc.{ext}"), bytes)
                .unwrap_or_else(|e| panic!("reload {ext}: {e}"));

            let group = native_group(&reloaded, owner);
            let class_names: Vec<&str> =
                group.nodes.iter().map(|n| n.class_name.as_str()).collect();
            assert!(
                class_names.contains(&"AcHorizontalConstraint"),
                "{ext}: missing horizontal constraint node, got {class_names:?}"
            );
            assert!(
                class_names.contains(&"AcVerticalConstraint"),
                "{ext}: missing vertical constraint node, got {class_names:?}"
            );
            assert_eq!(
                class_names
                    .iter()
                    .filter(|&&n| n == "AcConstrainedBoundedLine")
                    .count(),
                2,
                "{ext}: expected both lines' geometry nodes to survive, got {class_names:?}"
            );
            let datums: Vec<i32> = group
                .nodes
                .iter()
                .filter_map(|node| match node.data {
                    AssocConstraintNodeData::Parallel {
                        datum_line_index: Some(datum),
                        ..
                    } if matches!(
                        node.class_name.as_str(),
                        "AcHorizontalConstraint" | "AcVerticalConstraint"
                    ) =>
                    {
                        Some(datum)
                    }
                    _ => None,
                })
                .collect();
            assert_eq!(datums.len(), 2, "{ext}: both constraints need a datum");
            assert!(datums.iter().all(|datum| group.nodes.iter().any(|node| {
                node.node_id == *datum && node.class_name == "AcConstrainedDatumLine"
            })));
        }
    }

    #[test]
    fn native_only_constraints_and_parameters_are_imported() {
        let mut scene = Scene::new();
        let a = line_entity(&mut scene, (0.0, 0.0), (10.0, 0.0));
        let b = line_entity(&mut scene, (0.0, 5.0), (10.0, 5.0));
        scene.named_parameters.set("gap", "5").unwrap();
        let set = scene.sketch_constraint_set_mut(SketchScope::ModelSpace);
        set.add(ConstraintKind::Horizontal, vec![SketchRef::whole(a)], None);
        set.add(
            ConstraintKind::Parallel,
            vec![SketchRef::whole(a), SketchRef::whole(b)],
            None,
        );
        set.add(
            ConstraintKind::DistanceY,
            vec![SketchRef::point(a, 0), SketchRef::point(b, 0)],
            Some(DrivingValue::Named("gap".to_string())),
        );

        scene.materialize_dwg_native_constraints_for_save(true);
        let bytes = crate::io::save_to_bytes(&scene.document, "dwg", scene.document.version)
            .expect("save native-only graph");
        let mut restored = Scene::new();
        restored.document = crate::io::load_bytes("native_only.dwg", bytes).unwrap();
        restored.load_named_parameters_from_document();
        restored.load_sketch_constraints_from_document();

        let set = restored
            .sketch_constraint_set(SketchScope::ModelSpace)
            .expect("native group should become an editable constraint set");
        let kinds: Vec<_> = set
            .constraints
            .iter()
            .map(|constraint| constraint.kind)
            .collect();
        assert_eq!(
            kinds,
            vec![
                ConstraintKind::Horizontal,
                ConstraintKind::Parallel,
                ConstraintKind::DistanceY,
            ]
        );
        assert_eq!(restored.named_parameters.resolve("gap"), Ok(5.0));
        assert_eq!(
            set.constraints[2].driving_param,
            Some(DrivingValue::Named("gap".to_string()))
        );
    }

    #[test]
    fn point_and_two_point_horizontal_constraints_round_trip_natively() {
        let mut scene = Scene::new();
        let mut block = BlockRecord::new("fixture");
        block.handle = scene.document.allocate_handle();
        scene.document.block_records.add(block).unwrap();
        let point = scene.add_entity(EntityType::Point(acadrust::entities::Point::at(
            Vector3::new(1.0, 2.0, 0.0),
        )));
        let insert = scene.add_entity(EntityType::Insert(Insert::new(
            "fixture",
            Vector3::new(3.0, 4.0, 0.0),
        )));
        let line = line_entity(&mut scene, (5.0, 7.0), (10.0, 9.0));
        let set = scene.sketch_constraint_set_mut(SketchScope::ModelSpace);
        set.add(
            ConstraintKind::Horizontal,
            vec![SketchRef::point(point, 0), SketchRef::point(insert, 0)],
            None,
        );
        set.add(
            ConstraintKind::Midpoint,
            vec![SketchRef::point(point, 0), SketchRef::whole(line)],
            None,
        );

        scene.materialize_dwg_native_constraints_for_save(true);
        let bytes = crate::io::save_to_bytes(&scene.document, "dwg", scene.document.version)
            .expect("save point graph");
        let mut restored = Scene::new();
        restored.document = crate::io::load_bytes("point_constraints.dwg", bytes).unwrap();
        restored.load_sketch_constraints_from_document();

        let constraints = &restored
            .sketch_constraint_set(SketchScope::ModelSpace)
            .expect("native point constraints should be imported")
            .constraints;
        assert_eq!(constraints.len(), 2);
        assert_eq!(constraints[0].kind, ConstraintKind::Horizontal);
        assert_eq!(constraints[0].refs.len(), 2);
        assert_eq!(constraints[1].kind, ConstraintKind::Midpoint);
    }

    #[test]
    fn polyline_segment_constraints_round_trip_through_the_native_graph() {
        let mut scene = Scene::new();
        let polyline = scene.add_entity(EntityType::LwPolyline(LwPolyline::from_points(vec![
            Vector2::new(0.0, 0.0),
            Vector2::new(5.0, 2.0),
            Vector2::new(10.0, 7.0),
        ])));
        scene
            .sketch_constraint_set_mut(SketchScope::ModelSpace)
            .add(
                ConstraintKind::Horizontal,
                vec![SketchRef::segment(polyline, 1)],
                None,
            );

        scene.materialize_dwg_native_constraints_for_save(true);
        let bytes = crate::io::save_to_bytes(&scene.document, "dwg", scene.document.version)
            .expect("save polyline segment graph");
        let mut restored = Scene::new();
        restored.document = crate::io::load_bytes("polyline_segment.dwg", bytes).unwrap();
        restored.load_sketch_constraints_from_document();

        let set = restored
            .sketch_constraint_set(SketchScope::ModelSpace)
            .expect("native segment constraint should be imported");
        assert_eq!(set.constraints.len(), 1);
        assert_eq!(set.constraints[0].kind, ConstraintKind::Horizontal);
        assert_eq!(
            set.constraints[0].refs,
            vec![SketchRef::segment(polyline, 1)]
        );
    }

    fn distance_with_named_parameter_scene() -> (Scene, Handle) {
        let mut scene = Scene::new();
        let a = line_entity(&mut scene, (0.0, 0.0), (10.0, 0.0));
        let b = line_entity(&mut scene, (20.0, 0.0), (30.0, 0.0));
        scene
            .named_parameters
            .set("gap", "5")
            .expect("defining the parameter should succeed");
        scene
            .sketch_constraint_set_mut(SketchScope::ModelSpace)
            .add(
                ConstraintKind::Distance,
                vec![SketchRef::point(a, 1), SketchRef::point(b, 0)],
                Some(DrivingValue::Named("gap".to_string())),
            );
        let owner = scene.document.header.model_space_block_handle;
        (scene, owner)
    }

    fn has_variable_named(document: &CadDocument, name: &str) -> bool {
        document.objects.values().any(|obj| {
            matches!(
                obj,
                ObjectType::Associative(AssociativeObject { data: AssociativeData::Variable(v), .. })
                    if v.name == name
            )
        })
    }

    /// DWG is the format this module's dependency chain (`AssocGeomDependency`
    /// / `AssocValueDependency` / `AssocVariable`) actually survives a save
    /// through — see [`a_dxf_save_keeps_only_the_constraint_group_shell`] for
    /// the DXF side of this same scene and why it's different, not a bug here.
    #[test]
    fn a_dwg_save_keeps_the_full_dependency_chain_including_the_named_variable() {
        let (mut scene, owner) = distance_with_named_parameter_scene();
        scene.materialize_dwg_native_constraints_for_save(true);
        assert!(
            has_variable_named(&scene.document, "gap"),
            "expected an AssocVariable BEFORE serialization"
        );

        let bytes = crate::io::save_to_bytes(&scene.document, "dwg", scene.document.version)
            .unwrap_or_else(|e| panic!("save to dwg: {e}"));
        let reloaded = crate::io::load_bytes("dwg_native_named_param.dwg", bytes)
            .unwrap_or_else(|e| panic!("reload dwg: {e}"));

        let group_handle = native_group_handle(&reloaded, owner);
        let group = native_group(&reloaded, owner);
        assert!(
            group
                .nodes
                .iter()
                .any(|n| n.class_name == "AcDistanceConstraint"),
            "expected a distance constraint node, got {:?}",
            group
                .nodes
                .iter()
                .map(|n| &n.class_name)
                .collect::<Vec<_>>()
        );
        assert!(
            has_variable_named(&reloaded, "gap"),
            "expected the AssocVariable \"gap\" to survive a DWG round trip"
        );

        assert_eq!(group.action.class_version, 2);
        assert_eq!(group.action.action_index, 2);
        assert_eq!(group.action.max_dependency_index, 4);
        assert_eq!(group.actions.len(), 3);

        let value_dependency = group
            .nodes
            .iter()
            .find_map(|node| match &node.data {
                AssocConstraintNodeData::Distance {
                    value_dependency, ..
                } => Some(*value_dependency),
                _ => None,
            })
            .expect("distance node should reference its value dependency");
        let Some(ObjectType::Associative(AssociativeObject {
            owner: value_owner,
            data: AssociativeData::ValueDependency(value),
            ..
        })) = reloaded.objects.get(&value_dependency)
        else {
            panic!("value dependency should exist");
        };
        assert_eq!(*value_owner, group_handle);
        assert_eq!(value.dependency.dependency_body_id, 3);
        assert!(value.dependency.is_read_dependency);
        assert!(!value.dependency.is_write_dependency);

        let variable_handle = value.dependency.dependent_on;
        let Some(ObjectType::Associative(AssociativeObject {
            owner: network_handle,
            reactors,
            data: AssociativeData::Variable(variable),
            ..
        })) = reloaded.objects.get(&variable_handle)
        else {
            panic!("named variable should exist");
        };
        assert_eq!(group.action.owning_network, *network_handle);
        assert_eq!(variable.action.owning_network, *network_handle);
        assert_eq!(variable.action.action_index, 1);
        assert!(reactors.contains(&value_dependency));

        let Some(ObjectType::Associative(AssociativeObject {
            data: AssociativeData::Network(network),
            ..
        })) = reloaded.objects.get(network_handle)
        else {
            panic!("scope network should exist");
        };
        assert_eq!(network.network_action_index, 2);
        assert_eq!(network.actions.len(), 2);
        assert_eq!(network.actions[0].dependency, variable_handle);
        assert_eq!(network.actions[1].dependency, group_handle);
        assert!(network.actions.iter().all(|action| action.is_owned));
    }

    /// DXF currently keeps the graph shell but omits dependency objects.
    #[test]
    fn a_dxf_save_keeps_only_the_constraint_group_shell() {
        let (mut scene, owner) = distance_with_named_parameter_scene();
        scene.materialize_dwg_native_constraints_for_save(true);

        let bytes = crate::io::save_to_bytes(&scene.document, "dxf", scene.document.version)
            .unwrap_or_else(|e| panic!("save to dxf: {e}"));
        let reloaded = crate::io::load_bytes("dwg_native_named_param.dxf", bytes)
            .unwrap_or_else(|e| panic!("reload dxf: {e}"));

        let group = native_group(&reloaded, owner);
        assert!(
            group
                .nodes
                .iter()
                .any(|n| n.class_name == "AcDistanceConstraint"),
            "expected the distance constraint node's shell to survive, got {:?}",
            group
                .nodes
                .iter()
                .map(|n| &n.class_name)
                .collect::<Vec<_>>()
        );
        assert!(
            !has_variable_named(&reloaded, "gap"),
            "acadrust's DXF writer should not carry AssocVariable; update the native materializer if this changes"
        );
    }

    #[test]
    fn resaving_replaces_rather_than_accumulates_native_objects() {
        let mut scene = Scene::new();
        let a = line_entity(&mut scene, (0.0, 0.0), (10.0, 0.0));
        scene
            .sketch_constraint_set_mut(SketchScope::ModelSpace)
            .add(ConstraintKind::Horizontal, vec![SketchRef::whole(a)], None);

        scene.materialize_dwg_native_constraints_for_save(true);
        let first_count = scene.document.objects.len();
        scene.materialize_dwg_native_constraints_for_save(true);
        let second_count = scene.document.objects.len();
        assert_eq!(
            first_count, second_count,
            "a resave with the same constraints must not accumulate new objects"
        );
    }

    #[test]
    fn resaving_preserves_a_native_group_with_uneditable_geometry() {
        let mut scene = Scene::new();
        let a = line_entity(&mut scene, (0.0, 0.0), (10.0, 0.0));
        scene
            .sketch_constraint_set_mut(SketchScope::ModelSpace)
            .add(ConstraintKind::Horizontal, vec![SketchRef::whole(a)], None);
        scene.materialize_dwg_native_constraints_for_save(true);

        let owner = scene.document.header.model_space_block_handle;
        let group_handle = native_group_handle(&scene.document, owner);
        let Some(ObjectType::Associative(AssociativeObject {
            data: AssociativeData::ConstraintGroup(group),
            ..
        })) = scene.document.objects.get_mut(&group_handle)
        else {
            panic!("expected a native constraint group");
        };
        group.nodes.push(AssocConstraintNode {
            node_id: 99,
            class_name: "AcConstrainedRigidSet".to_string(),
            data: AssocConstraintNodeData::RigidSet {
                geometry_dependency: Handle::NULL,
                geometry_node_id: 99,
                reserved: false,
                transform: [0.0; 16],
                geometry_ids: vec![1],
            },
            ..Default::default()
        });

        scene
            .sketch_constraint_set_mut(SketchScope::ModelSpace)
            .add(ConstraintKind::Vertical, vec![SketchRef::whole(a)], None);
        scene.materialize_dwg_native_constraints_for_save(true);

        let group = native_group(&scene.document, owner);
        assert!(group
            .nodes
            .iter()
            .any(|node| matches!(node.data, AssocConstraintNodeData::RigidSet { .. })));
        assert!(!group
            .nodes
            .iter()
            .any(|node| node.class_name == "AcVerticalConstraint"));
    }

    #[test]
    fn an_empty_constraint_set_leaves_no_native_network() {
        let mut scene = Scene::new();
        let _ = scene.sketch_constraint_set_mut(SketchScope::ModelSpace);
        scene.materialize_dwg_native_constraints_for_save(true);

        let owner = scene.document.header.model_space_block_handle;
        let dict = scene.document.extension_dictionary_handle(owner);
        if let Some(dict) = dict {
            if let Some(ObjectType::Dictionary(dictionary)) = scene.document.objects.get(&dict) {
                assert!(
                    dictionary.get(NETWORK_DICTIONARY_KEY).is_none(),
                    "an unconstrained scope should not materialize a native network"
                );
            }
        }
    }

    fn model_space_has_native_network(scene: &Scene) -> bool {
        has_native_network(
            &scene.document,
            scene.document.header.model_space_block_handle,
        )
    }

    #[test]
    fn the_setting_off_skips_a_scope_with_no_existing_native_graph() {
        let mut scene = Scene::new();
        let a = line_entity(&mut scene, (0.0, 0.0), (10.0, 0.0));
        scene
            .sketch_constraint_set_mut(SketchScope::ModelSpace)
            .add(ConstraintKind::Horizontal, vec![SketchRef::whole(a)], None);

        scene.materialize_dwg_native_constraints_for_save(false);
        assert!(
            !model_space_has_native_network(&scene),
            "a scope with no prior native graph should stay untouched while the setting is off"
        );
    }

    /// The "sync-if-present, create-only-if-enabled" rule from
    /// `Scene::materialize_dwg_native_constraints_for_save`'s doc comment:
    /// once a scope has the native graph (here, from an earlier save with
    /// the setting on), turning the setting off must not freeze that graph
    /// out of date — it should keep tracking the current constraints.
    #[test]
    fn the_setting_off_still_resyncs_a_scope_that_already_has_a_native_graph() {
        let mut scene = Scene::new();
        let a = line_entity(&mut scene, (0.0, 0.0), (10.0, 0.0));
        scene
            .sketch_constraint_set_mut(SketchScope::ModelSpace)
            .add(ConstraintKind::Horizontal, vec![SketchRef::whole(a)], None);
        scene.materialize_dwg_native_constraints_for_save(true);
        assert!(
            model_space_has_native_network(&scene),
            "sanity: the graph should exist after the first, enabled save"
        );

        let b = line_entity(&mut scene, (0.0, 0.0), (0.0, 10.0));
        scene
            .sketch_constraint_set_mut(SketchScope::ModelSpace)
            .add(ConstraintKind::Vertical, vec![SketchRef::whole(b)], None);
        scene.materialize_dwg_native_constraints_for_save(false);

        let owner = scene.document.header.model_space_block_handle;
        let group = native_group(&scene.document, owner);
        assert!(
            group.nodes.iter().any(|n| n.class_name == "AcVerticalConstraint"),
            "the second, disabled-setting save should still pick up the new Vertical constraint, got {:?}",
            group.nodes.iter().map(|n| &n.class_name).collect::<Vec<_>>()
        );
    }

    /// The other half of the same rule: disabling the setting after a scope
    /// already has the graph must not make that graph disappear either —
    /// only remove it by clearing the scope's constraints (existing
    /// `an_empty_constraint_set_leaves_no_native_network` behavior), not by
    /// toggling the setting off.
    #[test]
    fn the_setting_off_does_not_remove_an_existing_native_graph() {
        let mut scene = Scene::new();
        let a = line_entity(&mut scene, (0.0, 0.0), (10.0, 0.0));
        scene
            .sketch_constraint_set_mut(SketchScope::ModelSpace)
            .add(ConstraintKind::Horizontal, vec![SketchRef::whole(a)], None);
        scene.materialize_dwg_native_constraints_for_save(true);
        scene.materialize_dwg_native_constraints_for_save(false);
        assert!(
            model_space_has_native_network(&scene),
            "an existing native graph must survive a disabled-setting resave, not be silently dropped"
        );
    }

    /// These `ConstraintKind`s all reuse the same generic
    /// `Geometrical`-shaped node path `geometrical_class_name` drives (see
    /// its own doc comment) — one representative mix of ref shapes (2 whole
    /// entities, an asymmetric point+whole pair, and a 3-ref case) is
    /// enough to confirm that path handles them all, without one dedicated
    /// test per kind.
    #[test]
    fn new_constraint_kinds_round_trip_with_their_own_dwg_native_class_names() {
        for ext in ["dxf", "dwg"] {
            let mut scene = Scene::new();
            let circle_a = circle_entity(&mut scene, (0.0, 0.0), 3.0);
            let circle_b = circle_entity(&mut scene, (5.0, 5.0), 1.0);
            let line_a = line_entity(&mut scene, (0.0, 0.0), (10.0, 0.0));
            let line_b = line_entity(&mut scene, (3.0, 4.0), (7.0, 6.0));
            let marker = line_entity(&mut scene, (20.0, 20.0), (21.0, 21.0));
            let axis = line_entity(&mut scene, (0.0, 0.0), (0.0, 10.0));

            let set = scene.sketch_constraint_set_mut(SketchScope::ModelSpace);
            set.add(
                ConstraintKind::Concentric,
                vec![SketchRef::center(circle_a), SketchRef::center(circle_b)],
                None,
            );
            set.add(
                ConstraintKind::Colinear,
                vec![SketchRef::whole(line_a), SketchRef::whole(line_b)],
                None,
            );
            set.add(ConstraintKind::Fixed, vec![SketchRef::whole(line_a)], None);
            set.add(
                ConstraintKind::CenterPoint,
                vec![SketchRef::point(marker, 0), SketchRef::center(circle_a)],
                None,
            );
            set.add(
                ConstraintKind::Midpoint,
                vec![SketchRef::point(marker, 1), SketchRef::whole(line_a)],
                None,
            );
            set.add(
                ConstraintKind::PointOnCurve,
                vec![SketchRef::point(marker, 0), SketchRef::whole(circle_a)],
                None,
            );
            set.add(
                ConstraintKind::EqualDistance,
                vec![
                    SketchRef::point(line_b, 0),
                    SketchRef::point(line_b, 1),
                    SketchRef::point(line_a, 0),
                    SketchRef::point(line_a, 1),
                ],
                None,
            );
            set.add(
                ConstraintKind::Symmetric,
                vec![
                    SketchRef::center(circle_a),
                    SketchRef::center(circle_b),
                    SketchRef::whole(axis),
                ],
                None,
            );
            set.add(
                ConstraintKind::Normal,
                vec![SketchRef::whole(circle_a), SketchRef::whole(line_a)],
                None,
            );

            scene.materialize_dwg_native_constraints_for_save(true);
            let owner = scene.document.header.model_space_block_handle;

            let bytes = crate::io::save_to_bytes(&scene.document, ext, scene.document.version)
                .unwrap_or_else(|e| panic!("save to {ext}: {e}"));
            let reloaded = crate::io::load_bytes(&format!("new_kinds.{ext}"), bytes)
                .unwrap_or_else(|e| panic!("reload {ext}: {e}"));

            let group = native_group(&reloaded, owner);
            let class_names: Vec<&str> =
                group.nodes.iter().map(|n| n.class_name.as_str()).collect();

            for expected in [
                "AcConcentricConstraint",
                "AcColinearConstraint",
                "AcFixedConstraint",
                "AcCenterPointConstraint",
                "AcMidPointConstraint",
                "AcPointCurveConstraint",
                "AcEqualDistanceConstraint",
                "AcSymmetricConstraint",
                "AcNormalConstraint",
            ] {
                assert!(
                    class_names.contains(&expected),
                    "{ext}: missing {expected} node, got {class_names:?}"
                );
            }
        }
    }

    /// Constraint-parity Phase 1: an Arc registers in the solver as its own
    /// center/radius (`sketch_solve.rs`'s `EntityGeom::Circle` reuse), and
    /// this module already had constrained-arc geometry-dependency
    /// support in place before that. This confirms the two sides actually
    /// meet: a Concentric/Tangent/Radius constraint referencing an arc
    /// still gets its expected class names AND its geometry node is the
    /// arc-specific one, through a real DWG/DXF byte round trip — not just
    /// the in-memory graph this test module builds directly.
    #[test]
    fn constraints_referencing_an_arc_round_trip_with_the_arc_geometry_node() {
        for ext in ["dxf", "dwg"] {
            let mut scene = Scene::new();
            let arc = arc_entity(&mut scene, (0.0, 0.0), 4.0);
            let circle = circle_entity(&mut scene, (10.0, -6.0), 2.0);

            let set = scene.sketch_constraint_set_mut(SketchScope::ModelSpace);
            set.add(
                ConstraintKind::Concentric,
                vec![SketchRef::center(arc), SketchRef::center(circle)],
                None,
            );
            set.add(
                ConstraintKind::Radius,
                vec![SketchRef::whole(arc)],
                Some(crate::scene::named_parameters::DrivingValue::Literal(9.0)),
            );

            scene.materialize_dwg_native_constraints_for_save(true);
            let owner = scene.document.header.model_space_block_handle;

            let bytes = crate::io::save_to_bytes(&scene.document, ext, scene.document.version)
                .unwrap_or_else(|e| panic!("save to {ext}: {e}"));
            let reloaded = crate::io::load_bytes(&format!("arc_ref.{ext}"), bytes)
                .unwrap_or_else(|e| panic!("reload {ext}: {e}"));

            let group = native_group(&reloaded, owner);
            let class_names: Vec<&str> =
                group.nodes.iter().map(|n| n.class_name.as_str()).collect();
            assert!(
                class_names.contains(&"AcConcentricConstraint"),
                "{ext}: missing concentric constraint, got {class_names:?}"
            );
            assert!(
                class_names.contains(&"AcRadiusDiameterConstraint"),
                "{ext}: missing radius/diameter constraint, got {class_names:?}"
            );
            assert!(
                group
                    .nodes
                    .iter()
                    .any(|n| n.class_name == "AcConstrainedArc"),
                "{ext}: the arc should have its own geometry node, got {class_names:?}"
            );
        }
    }

    /// Constraint-parity Phase 4b: a Coincident constraint referencing an
    /// arc's actual *endpoint* (marker 0, not its center via `-3` or its
    /// whole curve) round-trips correctly too — `SketchRef`'s marker is
    /// serialized generically regardless of what entity type or point it
    /// addresses, so this needs no dedicated DWG-side wiring beyond what
    /// already existed; this test is here to actually confirm that rather
    /// than assume it.
    #[test]
    fn a_coincident_constraint_on_an_arcs_endpoint_round_trips() {
        for ext in ["dxf", "dwg"] {
            let mut scene = Scene::new();
            let arc = arc_entity(&mut scene, (0.0, 0.0), 4.0);
            let line = line_entity(&mut scene, (20.0, 20.0), (21.0, 20.0));

            scene
                .sketch_constraint_set_mut(SketchScope::ModelSpace)
                .add(
                    ConstraintKind::Coincident,
                    vec![SketchRef::point(arc, 0), SketchRef::point(line, 0)],
                    None,
                );

            scene.materialize_dwg_native_constraints_for_save(true);
            let owner = scene.document.header.model_space_block_handle;

            let bytes = crate::io::save_to_bytes(&scene.document, ext, scene.document.version)
                .unwrap_or_else(|e| panic!("save to {ext}: {e}"));
            let reloaded = crate::io::load_bytes(&format!("arc_endpoint_ref.{ext}"), bytes)
                .unwrap_or_else(|e| panic!("reload {ext}: {e}"));

            let group = native_group(&reloaded, owner);
            let class_names: Vec<&str> =
                group.nodes.iter().map(|n| n.class_name.as_str()).collect();
            assert!(
                class_names.contains(&"AcPointCoincidenceConstraint"),
                "{ext}: missing point-coincidence constraint, got {class_names:?}"
            );
            assert!(
                group
                    .nodes
                    .iter()
                    .any(|n| n.class_name == "AcConstrainedArc"),
                "{ext}: the arc should still have its own geometry node, got {class_names:?}"
            );
        }
    }

    #[test]
    fn an_ellipse_constraint_round_trips_in_the_native_graph() {
        for ext in ["dxf", "dwg"] {
            let mut scene = Scene::new();
            let ellipse = scene.add_entity(EntityType::Ellipse(
                acadrust::entities::Ellipse::from_center_axes(
                    Vector3::new(0.0, 0.0, 0.0),
                    Vector3::new(4.0, 0.0, 0.0),
                    0.5,
                ),
            ));
            let circle = circle_entity(&mut scene, (10.0, 10.0), 2.0);

            let set = scene.sketch_constraint_set_mut(SketchScope::ModelSpace);
            set.add(
                ConstraintKind::Concentric,
                vec![SketchRef::center(ellipse), SketchRef::center(circle)],
                None,
            );
            set.add(ConstraintKind::Fixed, vec![SketchRef::whole(circle)], None);

            scene.materialize_sketch_constraints_for_save();
            scene.materialize_dwg_native_constraints_for_save(true);
            let owner = scene.document.header.model_space_block_handle;

            let bytes = crate::io::save_to_bytes(&scene.document, ext, scene.document.version)
                .unwrap_or_else(|e| panic!("save to {ext}: {e}"));
            let reloaded_doc = crate::io::load_bytes(&format!("ellipse_ref.{ext}"), bytes)
                .unwrap_or_else(|e| panic!("reload {ext}: {e}"));

            let group = native_group(&reloaded_doc, owner);
            let class_names: Vec<&str> =
                group.nodes.iter().map(|n| n.class_name.as_str()).collect();
            assert!(
                class_names.contains(&"AcFixedConstraint"),
                "{ext}: the other constraint should still persist, got {class_names:?}"
            );
            assert!(
                class_names.contains(&"AcConcentricConstraint"),
                "{ext}: missing ellipse-referencing constraint, got {class_names:?}"
            );
            assert!(
                class_names.contains(&"AcConstrainedEllipse"),
                "{ext}: missing ellipse geometry node, got {class_names:?}"
            );

            let mut reloaded_scene = Scene::new();
            reloaded_scene.document = reloaded_doc;
            reloaded_scene.load_sketch_constraints_from_document();
            let restored = reloaded_scene
                .sketch_constraint_set(SketchScope::ModelSpace)
                .unwrap_or_else(|| {
                    panic!("{ext}: no ModelSpace constraint set survived the round trip")
                });
            let kinds: Vec<ConstraintKind> = restored.constraints.iter().map(|c| c.kind).collect();
            assert!(
                kinds.contains(&ConstraintKind::Concentric),
                "{ext}: the ellipse constraint should still round-trip via XRecord, got {kinds:?}"
            );
        }
    }

    /// Constraint-parity Phase 4a: `ArcLength` has no native SDK class at
    /// all (confirmed against `AcExplicitConstr.h`) — this asserts the
    /// degradation is exactly as documented: the native DWG/DXF graph
    /// gets every *other* constraint on the arc (Radius, here) but no
    /// ArcLength-shaped node, while the app's own constraint set (what
    /// XRecord persistence actually serializes, per `sketch_persist.rs`'s
    /// fully-generic encode/decode) still carries it untouched.
    #[test]
    fn arc_length_has_no_native_class_but_survives_in_the_apps_own_constraint_set() {
        for ext in ["dxf", "dwg"] {
            let mut scene = Scene::new();
            let arc = arc_entity(&mut scene, (0.0, 0.0), 4.0);

            let set = scene.sketch_constraint_set_mut(SketchScope::ModelSpace);
            set.add(
                ConstraintKind::Radius,
                vec![SketchRef::whole(arc)],
                Some(DrivingValue::Literal(4.0)),
            );
            set.add(
                ConstraintKind::ArcLength,
                vec![SketchRef::whole(arc)],
                Some(DrivingValue::Literal(6.0)),
            );

            // Both materializers, "called alongside, not instead of" each
            // other (`materialize_dwg_native_constraints_for_save`'s own
            // doc comment) — the native graph and this app's own XRecord
            // format are independent, additive persistence paths.
            scene.materialize_sketch_constraints_for_save();
            scene.materialize_dwg_native_constraints_for_save(true);
            let owner = scene.document.header.model_space_block_handle;

            let bytes = crate::io::save_to_bytes(&scene.document, ext, scene.document.version)
                .unwrap_or_else(|e| panic!("save to {ext}: {e}"));
            let reloaded_doc = crate::io::load_bytes(&format!("arc_length.{ext}"), bytes)
                .unwrap_or_else(|e| panic!("reload {ext}: {e}"));

            let group = native_group(&reloaded_doc, owner);
            let class_names: Vec<&str> =
                group.nodes.iter().map(|n| n.class_name.as_str()).collect();
            assert!(
                class_names.contains(&"AcRadiusDiameterConstraint"),
                "{ext}: missing radius/diameter constraint, got {class_names:?}"
            );
            assert!(
                !class_names.iter().any(|n| n.contains("ARCLENGTH")),
                "{ext}: ArcLength has no native DWG class and must not appear here, got {class_names:?}"
            );

            // Still present in this app's own (XRecord-backed) constraint
            // set — reload into a fresh `Scene` and read it back the same
            // way `sketch_persist.rs`'s own round-trip tests do.
            let mut reloaded_scene = Scene::new();
            reloaded_scene.document = reloaded_doc;
            reloaded_scene.load_sketch_constraints_from_document();
            let restored = reloaded_scene
                .sketch_constraint_set(SketchScope::ModelSpace)
                .unwrap_or_else(|| {
                    panic!("{ext}: no ModelSpace constraint set survived the round trip")
                });
            let kinds: Vec<ConstraintKind> = restored.constraints.iter().map(|c| c.kind).collect();
            assert!(
                kinds.contains(&ConstraintKind::ArcLength),
                "{ext}: ArcLength should still round-trip via XRecord, got {kinds:?}"
            );
        }
    }

    /// Constraint-parity Phase 2: `Diameter`/`DistanceX`/`DistanceY` reuse
    /// `Radius`'/`Distance`'s own DWG class with a different
    /// `RadiusDiameterConstrType`/`DirectionType` mode byte, matching real
    /// the native format's representation (this module's own research, folded into
    /// `dimensional_class_name`'s doc comment). Confirms that byte actually
    /// survives a real DWG/DXF write+read, not just the in-memory node this
    /// module built.
    #[test]
    fn diameter_and_directed_distance_constraints_round_trip_with_their_mode_byte() {
        for ext in ["dxf", "dwg"] {
            let mut scene = Scene::new();
            let circle = circle_entity(&mut scene, (0.0, 0.0), 3.0);
            let line = line_entity(&mut scene, (0.0, 0.0), (10.0, 0.0));

            let set = scene.sketch_constraint_set_mut(SketchScope::ModelSpace);
            set.add(
                ConstraintKind::Diameter,
                vec![SketchRef::whole(circle)],
                Some(DrivingValue::Literal(16.0)),
            );
            set.add(
                ConstraintKind::DistanceX,
                vec![SketchRef::point(line, 0), SketchRef::point(line, 1)],
                Some(DrivingValue::Literal(10.0)),
            );
            set.add(
                ConstraintKind::DistanceY,
                vec![SketchRef::point(line, 0), SketchRef::point(line, 1)],
                Some(DrivingValue::Literal(3.0)),
            );

            scene.materialize_dwg_native_constraints_for_save(true);
            let owner = scene.document.header.model_space_block_handle;

            let bytes = crate::io::save_to_bytes(&scene.document, ext, scene.document.version)
                .unwrap_or_else(|e| panic!("save to {ext}: {e}"));
            let reloaded = crate::io::load_bytes(&format!("diameter_directed.{ext}"), bytes)
                .unwrap_or_else(|e| panic!("reload {ext}: {e}"));

            let group = native_group(&reloaded, owner);
            let radius_diameter_nodes: Vec<_> = group
                .nodes
                .iter()
                .filter_map(|n| match &n.data {
                    AssocConstraintNodeData::RadiusDiameter { mode, .. } => Some(*mode),
                    _ => None,
                })
                .collect();
            assert_eq!(radius_diameter_nodes, vec![1], "{ext}: Diameter should round-trip as mode=1 (kCircleDiameter), got {radius_diameter_nodes:?}");

            let distance_nodes: Vec<_> = group
                .nodes
                .iter()
                .filter_map(|n| match &n.data {
                    AssocConstraintNodeData::Distance {
                        direction_type,
                        distance,
                        ..
                    } => Some((*direction_type, *distance)),
                    _ => None,
                })
                .collect();
            assert_eq!(
                distance_nodes.len(),
                2,
                "{ext}: expected DistanceX and DistanceY nodes, got {distance_nodes:?}"
            );
            assert!(
                distance_nodes.contains(&(1, Some(Vector3::new(1.0, 0.0, 0.0)))),
                "{ext}: DistanceX should round-trip as direction_type=1 with a (1,0,0) direction, got {distance_nodes:?}"
            );
            assert!(
                distance_nodes.contains(&(1, Some(Vector3::new(0.0, 1.0, 0.0)))),
                "{ext}: DistanceY should round-trip as direction_type=1 with a (0,1,0) direction, got {distance_nodes:?}"
            );
        }
    }
}
