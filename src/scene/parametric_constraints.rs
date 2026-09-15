//! Runtime parametric-constraint data and scope management.

use super::named_parameters::DrivingValue;
use acadrust::types::{Handle, Vector3};

/// One endpoint a constraint attaches to: an entity plus which sub-element
/// of it.
///
/// Reuses the GsMarker convention `AssocDimensionReference::main_gs_marker`
/// already carries for associative-dimension endpoints
/// (`src/scene/dimension_assoc.rs`), rather than inventing a second
/// sub-element addressing scheme: `marker` indexes into
/// [`dimension_assoc::source_points`](super::dimension_assoc::source_points)'s
/// ordered per-entity-type point list when non-negative (0/1 = a line's
/// start/end, ...), or names a special case when negative (-3 = a
/// circle/arc's center; -2 = a bounded curve's midpoint). Polyline segment
/// and segment-midpoint references use private negative ranges so they stay
/// distinct from vertex markers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ParametricRef {
    pub entity: Handle,
    /// `None` addresses the entity as a whole — what a Radius, Length, or
    /// whole-curve constraint (Parallel, Perpendicular, Equal, Horizontal,
    /// Vertical) needs; a point-level constraint (Coincident, Distance
    /// between two points, Angle at a shared vertex) sets `Some(marker)`.
    pub marker: Option<i32>,
}

const POLYLINE_SEGMENT_MARKER_BASE: i32 = -1_000_000;
const POLYLINE_SEGMENT_MIDPOINT_MARKER_BASE: i32 = -2_000_000;

impl ParametricRef {
    pub fn whole(entity: Handle) -> Self {
        Self {
            entity,
            marker: None,
        }
    }

    pub fn point(entity: Handle, marker: i32) -> Self {
        Self {
            entity,
            marker: Some(marker),
        }
    }

    /// The circle/arc-center special case (`marker == -3`), broken out as
    /// its own constructor since `-3` alone reads as a magic number
    /// everywhere it would otherwise appear.
    pub fn center(entity: Handle) -> Self {
        Self {
            entity,
            marker: Some(-3),
        }
    }

    /// Select one straight segment of a polyline.
    pub fn segment(entity: Handle, index: usize) -> Self {
        Self {
            entity,
            marker: Some(POLYLINE_SEGMENT_MARKER_BASE - index as i32),
        }
    }

    pub fn segment_index(self) -> Option<usize> {
        let marker = self.marker?;
        (marker <= POLYLINE_SEGMENT_MARKER_BASE && marker > POLYLINE_SEGMENT_MIDPOINT_MARKER_BASE)
            .then(|| (POLYLINE_SEGMENT_MARKER_BASE - marker) as usize)
    }

    /// Select the midpoint of one straight polyline segment.
    pub fn segment_midpoint(entity: Handle, index: usize) -> Self {
        Self {
            entity,
            marker: Some(POLYLINE_SEGMENT_MIDPOINT_MARKER_BASE - index as i32),
        }
    }

    pub fn segment_midpoint_index(self) -> Option<usize> {
        let marker = self.marker?;
        (marker <= POLYLINE_SEGMENT_MIDPOINT_MARKER_BASE)
            .then(|| (POLYLINE_SEGMENT_MIDPOINT_MARKER_BASE - marker) as usize)
    }
}

/// The friendly, user-facing constraint types — the "what button did they
/// click" vocabulary, one layer above the `cadkernel_constraints` primitives each maps
/// onto (that mapping is `constraint_map`, a later stage; see the design
/// doc §2). Named and grouped the same way the existing one-shot ribbon
/// tools are (`crate::modules::parametric::tools`), plus the
/// endpoint-picking kinds (`Coincident`, `Radius`, `Tangent`) that one-shot
/// group never needed.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ConstraintKind {
    Coincident,
    Horizontal,
    Vertical,
    Parallel,
    Perpendicular,
    Equal,
    /// Distance between two points, or a single line's length, or a
    /// circle/arc's diameter — which reading applies depends on `refs`'
    /// shape, mirroring how `DistanceConstraintCommand`
    /// (`src/modules/draw/constrain/value.rs`) already infers it from the
    /// selected entity.
    Distance,
    Angle,
    /// Angle defined by two rays sharing the middle point.
    Angle3Point,
    Radius,
    Tangent,
    /// An open spline endpoint remains curvature-continuous with another bounded curve endpoint.
    Smooth,
    /// Two circles/arcs share a center — `refs`: `[center(a), center(b)]`.
    /// Solves identically to `Coincident` (`parametric_solve.rs` broadens that
    /// match arm rather than duplicating it) — only the DWG-native class
    /// name (`ACCONCENTRICCONSTRAINT` vs `ACPOINTCOINCIDENCECONSTRAINT`)
    /// and the UI entry point differ.
    Concentric,
    /// A point sits at a circle/arc's center — `refs`: `[point, center(circle)]`.
    /// Same solver math as `Coincident`/`Concentric`, different DWG class
    /// name (`ACCENTERPOINTCONSTRAINT`).
    CenterPoint,
    /// Two lines share the same infinite line — `refs`: `[whole(a), whole(b)]`.
    Colinear,
    /// A point sits at another line's midpoint — `refs`: `[point, whole(line)]`.
    Midpoint,
    /// Locks a whole entity at its current position — `refs`: `[whole(entity)]`.
    /// No `driving_param`: the target is the entity's own live geometry at
    /// solve time, not a typed value (see `parametric_solve.rs`'s `Fixed` arm).
    Fixed,
    /// A point lies anywhere along a line's or circle's curve (not
    /// restricted to an endpoint/center) — `refs`: `[point, whole(entity)]`.
    PointOnCurve,
    /// The distance between one point pair equals the distance between
    /// another — `refs`: `[p1, p2, p3, p4]` (`dist(p1,p2) == dist(p3,p4)`).
    /// For the supported entity types, equal curvature is already covered
    /// by the circle/circle radius branch of `Equal`.
    EqualDistance,
    /// Two circles/arcs are mirror images of each other across a line —
    /// `refs`: `[center(a), center(b), whole(mirror_line)]`. Scoped to the
    /// circle-center-pair case (unambiguous with whole-entity selection);
    /// point-symmetry about a point, and symmetry between two lines, aren't
    /// modeled.
    Symmetric,
    /// A circle/arc's diameter (twice `Radius`'s target) — `refs`:
    /// `[whole(circle_or_arc)]`. Same DWG class as `Radius`
    /// (`ACRADIUSDIAMETERCONSTRAINT`), distinguished only by the
    /// `RadiusDiameterConstrType` mode byte
    /// (`dwg_native_constraints.rs`), rather than a distinct object type.
    Diameter,
    /// The X-only (resp. Y-only) component of the distance between two
    /// points — `refs`: `[p1, p2]`, same shape as `Distance`. Same DWG
    /// class as `Distance` (`ACDISTANCECONSTRAINT`) with its
    /// `DirectionType` set to a fixed `(1,0,0)`/`(0,1,0)` direction.
    DistanceX,
    DistanceY,
    /// Signed point-to-point distance along an arbitrary fixed direction,
    /// or along a direction derived from a third line reference.
    DistanceDirected,
    /// A line perpendicular to a circle/arc's tangent at their point of
    /// contact — `refs`: `[whole(a), whole(b)]`, either order. For the
    /// Line/Circle-only entity model this is equivalent to "the line
    /// passes through the circle's center" (a circle's radius is always
    /// normal to its own tangent), so it solves via the same `PointOnLine`
    /// primitive `PointOnCurve` already uses. Distinct from
    /// `Perpendicular` (line-to-line only). Line-Line has no meaning here
    /// (that's plain `Perpendicular`) and isn't buildable.
    Normal,
    /// A standard rigid geometry container. Its referenced points keep their
    /// original relative distances while the whole set may translate or rotate.
    RigidSet,
}

pub type ConstraintId = u32;

pub(crate) mod distance_direction_type {
    pub const UNDIRECTED: u8 = 0;
    pub const FIXED: u8 = 1;
    pub const PARALLEL_TO_LINE: u8 = 2;
    pub const PERPENDICULAR_TO_LINE: u8 = 3;
}

pub(crate) mod angle_sector {
    pub const PARALLEL_COUNTERCLOCKWISE: u8 = 0;
    pub const ANTIPARALLEL_CLOCKWISE: u8 = 1;
    pub const PARALLEL_CLOCKWISE: u8 = 2;
    pub const ANTIPARALLEL_COUNTERCLOCKWISE: u8 = 3;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct NativeConstraintOrigin {
    pub(crate) group: Handle,
    pub(crate) node: i32,
}

/// One runtime constraint: a friendly [`ConstraintKind`], the
/// entities/points it relates, and — for a dimensional kind — the value
/// driving it.
#[derive(Debug, Clone, PartialEq)]
pub struct ParametricConstraint {
    pub id: ConstraintId,
    pub kind: ConstraintKind,
    pub refs: Vec<ParametricRef>,
    /// The target for a dimensional constraint (a `Distance`'s length, an
    /// `Angle`'s degrees, a `Radius`'s radius) — a literal number or a
    /// named-parameter reference resolved through `Scene::named_parameters` at solve time by
    /// `parametric_solve::build_constraint`). `None` for every purely-geometric
    /// kind (Coincident, Horizontal, Vertical, Parallel, Perpendicular,
    /// Equal, Tangent).
    pub driving_param: Option<DrivingValue>,
    /// Lets a user suppress a constraint without losing it — a re-solve
    /// skips a disabled constraint entirely.
    pub enabled: bool,
    /// Standard graph node retained in place because its group also contains
    /// graph features that are not safe to rebuild independently.
    pub(crate) native_origin: Option<NativeConstraintOrigin>,
    /// Original point positions for a standard rigid set. This is rebuilt
    /// from the associative graph on open and never stored separately.
    pub(crate) rigid_points: Vec<(ParametricRef, Vector3)>,
    /// Standard distance direction mode and vector. A third entry in `refs`
    /// identifies the direction line for the line-relative modes.
    pub(crate) distance_direction_type: u8,
    pub(crate) distance_direction: Option<Vector3>,
    /// Which of the four directed sectors an angular constraint measures.
    pub(crate) angle_sector: u8,
}

/// A constraint scope: model space or one block definition.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ParametricScope {
    ModelSpace,
    /// A block definition's `BlockRecord` handle — matches
    /// `BlockEditSession::br_handle`
    /// (`src/modules/draw/modify/block_edit.rs`).
    Block(Handle),
}

impl ParametricScope {
    /// The owner handle under which this scope is persisted.
    pub fn owner_handle(&self, document: &acadrust::CadDocument) -> Handle {
        match self {
            ParametricScope::ModelSpace => document.header.model_space_block_handle,
            ParametricScope::Block(handle) => *handle,
        }
    }
}

/// Every constraint decoded from one standard graph scope. Solver state is rebuilt
/// on demand and is not stored here.
#[derive(Debug, Clone)]
pub struct ParametricConstraintSet {
    pub scope: ParametricScope,
    pub constraints: Vec<ParametricConstraint>,
    next_id: ConstraintId,
    /// Parameters owned by this standard block scope. Model-space and
    /// command-created constraints continue to use the drawing parameter
    /// table; block definitions need their own namespace because identical
    /// parameter names may legitimately occur in different blocks.
    pub(crate) local_parameters: super::named_parameters::ParameterTable,
    pub(crate) retained_standard_groups: Vec<Handle>,
    /// Cached total remaining degrees of freedom, summed across every
    /// independent solve partition in this scope — updated by
    /// `parametric_solve::solve_scope` each time this set is resolved. `None`
    /// until the first resolve (e.g. right after loading from disk, before
    /// any edit has touched this scope yet). The status badge reads this
    /// rather than recomputing it every frame. Not persisted:
    /// it's a derived cache, not real constraint state.
    pub dof: Option<usize>,
    /// Cached redundant/conflicting constraints found by the last resolve.
    /// Also a derived cache, not
    /// persisted; a `ConflictResolverPanel` reads this rather than calling
    /// `cadkernel_constraints::diagnosis::classify_redundant` itself.
    pub conflicts: Vec<(
        ConstraintId,
        cadkernel_constraints::diagnosis::RedundancyKind,
    )>,
}

impl ParametricConstraintSet {
    pub fn new(scope: ParametricScope) -> Self {
        Self {
            scope,
            constraints: Vec::new(),
            next_id: 0,
            local_parameters: super::named_parameters::ParameterTable::new(),
            retained_standard_groups: Vec::new(),
            dof: None,
            conflicts: Vec::new(),
        }
    }

    /// Appends a constraint, assigning it a fresh id unique within this set.
    pub fn add(
        &mut self,
        kind: ConstraintKind,
        refs: Vec<ParametricRef>,
        driving_param: Option<DrivingValue>,
    ) -> ConstraintId {
        let id = self.next_id;
        self.next_id += 1;
        self.constraints.push(ParametricConstraint {
            id,
            kind,
            refs,
            driving_param,
            enabled: true,
            native_origin: None,
            rigid_points: Vec::new(),
            distance_direction_type: 0,
            distance_direction: None,
            angle_sector: angle_sector::PARALLEL_COUNTERCLOCKWISE,
        });
        id
    }

    /// Removes a constraint by id. Returns whether one was actually removed.
    pub fn remove(&mut self, id: ConstraintId) -> bool {
        let before = self.constraints.len();
        self.constraints.retain(|c| c.id != id);
        self.constraints.len() != before
    }

    pub fn get(&self, id: ConstraintId) -> Option<&ParametricConstraint> {
        self.constraints.iter().find(|c| c.id == id)
    }

    /// Every enabled constraint referencing `entity`.
    pub fn constraints_touching(
        &self,
        entity: Handle,
    ) -> impl Iterator<Item = &ParametricConstraint> {
        self.constraints
            .iter()
            .filter(move |c| c.enabled && c.refs.iter().any(|r| r.entity == entity))
    }

    /// Drops every constraint that references `entity` and returns the
    /// removed constraint ids.
    pub fn remove_all_touching(&mut self, entity: Handle) -> Vec<ConstraintId> {
        let (removed, kept): (Vec<_>, Vec<_>) = self
            .constraints
            .drain(..)
            .partition(|c| c.refs.iter().any(|r| r.entity == entity));
        self.constraints = kept;
        removed.into_iter().map(|c| c.id).collect()
    }
}

/// Resolves a [`ParametricRef`] to its current world-space point, for building
/// an `cadkernel_constraints` `ParamStore` from live document geometry — the constraint
/// endpoint's equivalent of `dimension_assoc::resolve_reference`, restricted
/// to the marker conventions constraint endpoints actually use (whole-entity
/// `None`, an ordinary `source_points()` index, the `-3` center case, or a
/// bounded curve/segment midpoint).
///
/// Solver-side registration reads raw entity fields directly. This helper is
/// for UI-side consumers that need the current world-space position.
pub(crate) fn resolve_point(entity: &acadrust::EntityType, marker: i32) -> Option<Vector3> {
    if marker == -3 {
        return match entity {
            acadrust::EntityType::Circle(circle) => Some(circle.center_wcs()),
            acadrust::EntityType::Arc(arc) => Some(arc.center_wcs()),
            _ => None,
        };
    }
    if marker == -2 {
        let curve = crate::entities::curve::entity_curve(entity)?;
        if curve.is_closed() {
            return None;
        }
        let point = curve.point_at(0.5);
        return Some(Vector3::new(point[0], point[1], point[2]));
    }
    let segment = ParametricRef {
        entity: Handle::NULL,
        marker: Some(marker),
    }
    .segment_midpoint_index();
    if let Some(segment) = segment {
        let points = super::dimension_assoc::source_points(entity);
        let closed = match entity {
            acadrust::EntityType::LwPolyline(polyline) => polyline.is_closed,
            acadrust::EntityType::Polyline2D(polyline) => polyline.is_closed(),
            _ => return None,
        };
        let a = *points.get(segment)?;
        let b = if segment + 1 < points.len() {
            points[segment + 1]
        } else if closed {
            *points.first()?
        } else {
            return None;
        };
        return Some((a + b) * 0.5);
    }
    if marker < 0 {
        return None;
    }
    super::dimension_assoc::source_points(entity)
        .get(marker as usize)
        .copied()
}

/// Below this squared distance (1e-6 world units), two points count as
/// already coincident for [`nearest_parametric_point`]'s purposes.
const COINCIDENT_EPSILON_SQ: f64 = 1.0e-12;

/// Finds the addressable entity point nearest a snapped world position.
/// Returns `None` when no point in the scope is within the coincidence
/// tolerance.
pub(crate) fn nearest_parametric_point(
    document: &acadrust::CadDocument,
    scope: ParametricScope,
    world_point: Vector3,
    exclude: Option<Handle>,
) -> Option<ParametricRef> {
    let owner = scope.owner_handle(document);
    let mut best: Option<(f64, ParametricRef)> = None;
    let mut consider = |handle: Handle, marker: i32, point: Vector3| {
        let dx = point.x - world_point.x;
        let dy = point.y - world_point.y;
        let dz = point.z - world_point.z;
        let dist_sq = dx * dx + dy * dy + dz * dz;
        if dist_sq <= COINCIDENT_EPSILON_SQ && best.as_ref().is_none_or(|(d, _)| dist_sq < *d) {
            best = Some((dist_sq, ParametricRef::point(handle, marker)));
        }
    };
    for candidate in document.entities() {
        let common = candidate.common();
        if common.owner_handle != owner || Some(common.handle) == exclude {
            continue;
        }
        for (marker, point) in super::dimension_assoc::source_points(candidate)
            .into_iter()
            .enumerate()
        {
            consider(common.handle, marker as i32, point);
        }
        match candidate {
            acadrust::EntityType::Circle(c) => consider(common.handle, -3, c.center_wcs()),
            acadrust::EntityType::Arc(a) => consider(common.handle, -3, a.center_wcs()),
            _ => {}
        }
    }
    best.map(|(_, r)| r)
}

impl ConstraintKind {
    /// The short symbol a constraint glyph shows — matches the existing
    /// ribbon icons (`crate::modules::parametric::{tools,value}`) for
    /// the kinds that have a one-click button, so the same glyph means the
    /// same thing in both places.
    pub fn glyph_symbol(&self) -> &'static str {
        match self {
            ConstraintKind::Coincident => "≡",
            ConstraintKind::Horizontal => "—",
            ConstraintKind::Vertical => "│",
            ConstraintKind::Parallel => "∥",
            ConstraintKind::Perpendicular => "⊥",
            ConstraintKind::Equal => "=",
            ConstraintKind::Distance => "↔",
            ConstraintKind::Angle => "∠",
            ConstraintKind::Angle3Point => "∠₃",
            ConstraintKind::Radius => "R",
            ConstraintKind::Tangent => "T",
            ConstraintKind::Smooth => "G²",
            ConstraintKind::Concentric => "◎",
            ConstraintKind::CenterPoint => "⊕",
            ConstraintKind::Colinear => "L",
            ConstraintKind::Midpoint => "M",
            ConstraintKind::Fixed => "F",
            ConstraintKind::PointOnCurve => "∈",
            ConstraintKind::EqualDistance => "≐",
            ConstraintKind::Symmetric => "S",
            ConstraintKind::Diameter => "⌀",
            ConstraintKind::DistanceX => "↔ₓ",
            ConstraintKind::DistanceY => "↔ᵧ",
            ConstraintKind::DistanceDirected => "↗",
            ConstraintKind::Normal => "⊾",
            ConstraintKind::RigidSet => "▣",
        }
    }
}

/// The full glyph text for one constraint: its symbol, plus the driving
/// value for a dimensional kind (Distance/Angle/Radius).
pub(crate) fn glyph_label(constraint: &ParametricConstraint) -> String {
    match (constraint.kind, &constraint.driving_param) {
        (ConstraintKind::Angle, Some(DrivingValue::Literal(value))) => {
            format!("{} {value:.1}°", constraint.kind.glyph_symbol())
        }
        (_, Some(DrivingValue::Literal(value))) => {
            format!("{} {value:.2}", constraint.kind.glyph_symbol())
        }
        // A named reference has no single resolved number to show without
        // threading `ParameterTable` into every glyph-render call site
        // (`src/ui/overlay.rs`) — showing the name itself is enough for now;
        // stage 4's parameters panel is the natural place to reconsider this
        // once a named `driving_param` can actually be authored through the
        // UI (nothing can yet — this arm exists so the match is exhaustive
        // and correct ahead of that UI, not because it's reachable today).
        (_, Some(DrivingValue::Named(name))) => {
            format!("{} {name}", constraint.kind.glyph_symbol())
        }
        (_, None) => constraint.kind.glyph_symbol().to_string(),
    }
}

/// World-space anchor and outward direction for a constraint glyph.
pub(crate) fn glyph_placement(
    document: &acadrust::CadDocument,
    constraint: &ParametricConstraint,
) -> Option<(Vector3, Vector3)> {
    let r = constraint.refs.first()?;
    let entity = document.get_entity(r.entity)?;
    let line_midpoint = |line: &acadrust::entities::Line| {
        Vector3::new(
            (line.start.x + line.end.x) * 0.5,
            (line.start.y + line.end.y) * 0.5,
            (line.start.z + line.end.z) * 0.5,
        )
    };
    let line_normal = |line: &acadrust::entities::Line| {
        let direction = Vector3::new(-(line.end.y - line.start.y), line.end.x - line.start.x, 0.0);
        (direction.length_squared() > 1e-24)
            .then_some(direction)
            .unwrap_or(Vector3::UNIT_Y)
    };
    match (entity, r.marker) {
        (acadrust::EntityType::Line(line), None) => Some((line_midpoint(line), line_normal(line))),
        (acadrust::EntityType::Circle(circle), None | Some(-3)) => {
            let center = circle.center_wcs();
            let anchor = circle.point_at_angle_wcs(0.0);
            Some((anchor, anchor - center))
        }
        (acadrust::EntityType::Arc(arc), None | Some(-3)) => {
            let center = arc.center_wcs();
            let anchor = arc.midpoint_wcs();
            Some((anchor, anchor - center))
        }
        (acadrust::EntityType::Line(line), Some(marker)) => {
            let anchor = resolve_point(entity, marker)?;
            let direction = anchor - line_midpoint(line);
            Some((
                anchor,
                (direction.length_squared() > 1e-24)
                    .then_some(direction)
                    .unwrap_or_else(|| line_normal(line)),
            ))
        }
        (acadrust::EntityType::Arc(arc), Some(marker)) => {
            let anchor = resolve_point(entity, marker)?;
            let direction = anchor - arc.center_wcs();
            Some((
                anchor,
                (direction.length_squared() > 1e-24)
                    .then_some(direction)
                    .unwrap_or(Vector3::UNIT_Y),
            ))
        }
        (_, Some(marker)) => {
            let anchor = resolve_point(entity, marker)?;
            Some((anchor, Vector3::UNIT_Y))
        }
        _ => None,
    }
}

impl super::Scene {
    pub fn is_parametric_constraint_visible(
        &self,
        scope: ParametricScope,
        id: ConstraintId,
    ) -> bool {
        !self.hidden_parametric_constraints.contains(&(scope, id))
    }

    pub fn set_parametric_constraint_visibility(
        &mut self,
        scope: ParametricScope,
        handles: Option<&[Handle]>,
        dimensional: bool,
        visible: bool,
    ) -> usize {
        let ids: Vec<_> = self
            .parametric_constraint_set(scope)
            .into_iter()
            .flat_map(|set| set.constraints.iter())
            .filter(|constraint| constraint.driving_param.is_some() == dimensional)
            .filter(|constraint| {
                handles.is_none_or(|handles| {
                    constraint
                        .refs
                        .iter()
                        .any(|reference| handles.contains(&reference.entity))
                })
            })
            .map(|constraint| constraint.id)
            .collect();
        for id in &ids {
            if visible {
                self.hidden_parametric_constraints.remove(&(scope, *id));
            } else {
                self.hidden_parametric_constraints.insert((scope, *id));
            }
        }
        ids.len()
    }

    /// Infers relations already present in the selected geometry.
    pub fn inferred_parametric_constraints(
        &self,
        scope: ParametricScope,
        handles: &[Handle],
    ) -> Vec<(ConstraintKind, Vec<ParametricRef>)> {
        use cadkernel::geom2d::{
            infer_constraints, Arc, Circle, ConstraintEndpoint, InferredConstraint, Line,
            ParametricPrimitive, Tolerance,
        };
        let mut sources = Vec::new();
        for handle in handles {
            let primitive = match self.document.get_entity(*handle) {
                Some(acadrust::EntityType::Line(line)) => ParametricPrimitive::Line(Line {
                    start: [line.start.x, line.start.y],
                    end: [line.end.x, line.end.y],
                }),
                Some(acadrust::EntityType::Circle(circle)) => ParametricPrimitive::Circle(Circle {
                    centre: [circle.center.x, circle.center.y],
                    radius: circle.radius,
                }),
                Some(acadrust::EntityType::Arc(arc)) => ParametricPrimitive::Arc(Arc {
                    centre: [arc.center.x, arc.center.y],
                    radius: arc.radius,
                    start_angle: arc.start_angle,
                    end_angle: arc.end_angle,
                }),
                _ => continue,
            };
            sources.push((*handle, primitive));
        }
        let primitives: Vec<_> = sources.iter().map(|(_, primitive)| *primitive).collect();
        let marker = |endpoint| match endpoint {
            ConstraintEndpoint::Start => 0,
            ConstraintEndpoint::End => 1,
        };
        let mut mapped: Vec<_> =
            infer_constraints(&primitives, Tolerance::new(1e-6), 0.5_f64.to_radians())
                .into_iter()
                .map(|relation| match relation {
                    InferredConstraint::Coincident {
                        first,
                        first_endpoint,
                        second,
                        second_endpoint,
                    } => (
                        ConstraintKind::Coincident,
                        vec![
                            ParametricRef::point(sources[first].0, marker(first_endpoint)),
                            ParametricRef::point(sources[second].0, marker(second_endpoint)),
                        ],
                    ),
                    InferredConstraint::Collinear { first, second } => (
                        ConstraintKind::Colinear,
                        vec![
                            ParametricRef::whole(sources[first].0),
                            ParametricRef::whole(sources[second].0),
                        ],
                    ),
                    InferredConstraint::Concentric { first, second } => (
                        ConstraintKind::Concentric,
                        vec![
                            ParametricRef::center(sources[first].0),
                            ParametricRef::center(sources[second].0),
                        ],
                    ),
                    InferredConstraint::Parallel { first, second } => (
                        ConstraintKind::Parallel,
                        vec![
                            ParametricRef::whole(sources[first].0),
                            ParametricRef::whole(sources[second].0),
                        ],
                    ),
                    InferredConstraint::Perpendicular { first, second } => (
                        ConstraintKind::Perpendicular,
                        vec![
                            ParametricRef::whole(sources[first].0),
                            ParametricRef::whole(sources[second].0),
                        ],
                    ),
                    InferredConstraint::Horizontal { entity } => (
                        ConstraintKind::Horizontal,
                        vec![ParametricRef::whole(sources[entity].0)],
                    ),
                    InferredConstraint::Vertical { entity } => (
                        ConstraintKind::Vertical,
                        vec![ParametricRef::whole(sources[entity].0)],
                    ),
                    InferredConstraint::Tangent { first, second } => (
                        ConstraintKind::Tangent,
                        vec![
                            ParametricRef::whole(sources[first].0),
                            ParametricRef::whole(sources[second].0),
                        ],
                    ),
                })
                .collect();
        if let Some(existing) = self.parametric_constraint_set(scope) {
            mapped.retain(|(kind, refs)| {
                !existing.constraints.iter().any(|constraint| {
                    constraint.kind == *kind
                        && (constraint.refs == *refs
                            || (constraint.refs.len() == 2
                                && refs.len() == 2
                                && constraint.refs[0] == refs[1]
                                && constraint.refs[1] == refs[0]))
                })
            });
        }
        mapped.retain(|(kind, refs)| {
            self.validate_parametric_constraint(*kind, refs, None)
                .is_ok()
        });
        mapped
    }

    pub fn smooth_constraint_refs(&self, handles: &[Handle]) -> Option<Vec<ParametricRef>> {
        let [first, second] = handles else {
            return None;
        };
        let first_entity = self.document.get_entity(*first)?;
        let second_entity = self.document.get_entity(*second)?;
        let (spline_handle, spline, target_handle, target) = match (first_entity, second_entity) {
            (acadrust::EntityType::Spline(spline), target) => (*first, spline, *second, target),
            (target, acadrust::EntityType::Spline(spline)) => (*second, spline, *first, target),
            _ => return None,
        };
        if spline.flags.closed || spline.flags.periodic {
            return None;
        }
        let spline_points =
            super::dimension_assoc::source_points(&acadrust::EntityType::Spline(spline.clone()));
        let target_points = super::dimension_assoc::source_points(target);
        let spline_ends = [*spline_points.first()?, *spline_points.last()?];
        let target_ends = [*target_points.first()?, *target_points.last()?];
        let mut best = (f64::INFINITY, 0, 0);
        for (source_marker, source) in spline_ends.iter().enumerate() {
            for (target_marker, target) in target_ends.iter().enumerate() {
                let distance = (*source - *target).length_squared();
                if distance < best.0 {
                    best = (distance, source_marker, target_marker);
                }
            }
        }
        Some(vec![
            ParametricRef::point(spline_handle, best.1 as i32),
            ParametricRef::point(target_handle, best.2 as i32),
        ])
    }

    /// The constraint set for `scope`, if one has been created.
    pub fn parametric_constraint_set(
        &self,
        scope: ParametricScope,
    ) -> Option<&ParametricConstraintSet> {
        self.parametric_constraints
            .iter()
            .find(|s| s.scope == scope)
    }

    /// The constraint set for `scope`, creating an empty one on first use.
    /// The `pub(crate)` `parametric_constraints` field itself stays private so
    /// nothing outside this module can end up with two sets for the same
    /// scope — this is the one way to reach a scope's set for both reading
    /// and mutating.
    pub fn parametric_constraint_set_mut(
        &mut self,
        scope: ParametricScope,
    ) -> &mut ParametricConstraintSet {
        if let Some(index) = self
            .parametric_constraints
            .iter()
            .position(|s| s.scope == scope)
        {
            &mut self.parametric_constraints[index]
        } else {
            self.parametric_constraints
                .push(ParametricConstraintSet::new(scope));
            self.parametric_constraints.last_mut().expect("just pushed")
        }
    }

    /// Handle remapping lives in each command that duplicates
    /// entities — `Scene::copy_entities`' `handle_map` (COPY/ARRAY/MIRROR,
    /// `src/scene/modify.rs`) and `OpenCADStudio::finalize_paste`'s own
    /// (clipboard paste, `src/app/command_driver.rs`) — rather than in one
    /// shared table, so each call site passes its own `handle_map` here
    /// after adding the duplicated entities.
    ///
    /// For every enabled constraint whose *every* referenced entity was
    /// duplicated (a constraint straddling a duplicated and a
    /// non-duplicated entity can't sensibly follow — only one side moved),
    /// adds an equivalent constraint over the new handles to the same
    /// scope, then triggers a solve for the newly duplicated geometry the
    /// same way any other edit would. A no-op when `handle_map` is empty or
    /// nothing constrained was duplicated.
    pub fn duplicate_parametric_constraints_for(
        &mut self,
        handle_map: &rustc_hash::FxHashMap<Handle, Handle>,
    ) {
        if handle_map.is_empty() {
            return;
        }
        let mut to_add: Vec<(
            usize,
            ConstraintKind,
            Vec<ParametricRef>,
            Option<DrivingValue>,
        )> = Vec::new();
        for (scope_index, set) in self.parametric_constraints.iter().enumerate() {
            for c in &set.constraints {
                if !c.enabled || !c.refs.iter().all(|r| handle_map.contains_key(&r.entity)) {
                    continue;
                }
                let new_refs: Vec<ParametricRef> = c
                    .refs
                    .iter()
                    .map(|r| ParametricRef {
                        entity: handle_map[&r.entity],
                        marker: r.marker,
                    })
                    .collect();
                to_add.push((scope_index, c.kind, new_refs, c.driving_param.clone()));
            }
        }
        if to_add.is_empty() {
            return;
        }
        let mut touched: Vec<Handle> = Vec::new();
        for (scope_index, kind, refs, driving_param) in to_add {
            touched.extend(refs.iter().map(|r| r.entity));
            self.parametric_constraints[scope_index].add(kind, refs, driving_param);
        }
        touched.sort();
        touched.dedup();
        let changes: Vec<(Handle, super::ChangeKind)> = touched
            .into_iter()
            .map(|h| (h, super::ChangeKind::Modified))
            .collect();
        self.bump_entities(&changes);
    }

    /// Every persistent constraint, in any scope, currently driven by the
    /// named parameter `name` — what the Named Parameters panel's "used by"
    /// column shows. `entities` is the constraint's own referenced handles
    /// (deduplicated; a two-point constraint on the same entity's own two
    /// markers would otherwise list it twice), not resolved against the
    /// live document — a caller wanting an entity's current type/position
    /// still needs `Scene::document.get_entity`.
    pub fn parameter_usage(&self, name: &str) -> Vec<ParameterUsage> {
        let mut out = Vec::new();
        for set in &self.parametric_constraints {
            for c in &set.constraints {
                let Some(DrivingValue::Named(n)) = &c.driving_param else {
                    continue;
                };
                if n != name {
                    continue;
                }
                let mut entities: Vec<Handle> = c.refs.iter().map(|r| r.entity).collect();
                entities.sort();
                entities.dedup();
                out.push(ParameterUsage {
                    scope: set.scope,
                    constraint_id: c.id,
                    kind: c.kind,
                    entities,
                });
            }
        }
        out
    }
}

/// One persistent constraint driven by a named parameter — [`Scene::parameter_usage`]'s
/// result type.
#[derive(Debug, Clone)]
pub struct ParameterUsage {
    pub scope: ParametricScope,
    pub constraint_id: ConstraintId,
    pub kind: ConstraintKind,
    pub entities: Vec<Handle>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn h(v: u64) -> Handle {
        Handle::new(v)
    }

    #[test]
    fn glyph_placement_points_away_from_its_geometry() {
        let mut document = acadrust::CadDocument::new();
        let mut line = acadrust::entities::Line::from_points(
            Vector3::new(0.0, 0.0, 0.0),
            Vector3::new(10.0, 0.0, 0.0),
        );
        line.common.handle = h(1);
        document
            .add_entity(acadrust::EntityType::Line(line))
            .unwrap();
        let mut circle = acadrust::entities::Circle::from_center_radius(Vector3::ZERO, 5.0);
        circle.common.handle = h(2);
        document
            .add_entity(acadrust::EntityType::Circle(circle))
            .unwrap();

        let constraint = |reference| ParametricConstraint {
            id: 0,
            kind: ConstraintKind::Fixed,
            refs: vec![reference],
            driving_param: None,
            enabled: true,
            native_origin: None,
            rigid_points: Vec::new(),
            distance_direction_type: 0,
            distance_direction: None,
            angle_sector: angle_sector::PARALLEL_COUNTERCLOCKWISE,
        };
        let (anchor, direction) =
            glyph_placement(&document, &constraint(ParametricRef::whole(h(1)))).unwrap();
        assert_eq!(anchor, Vector3::new(5.0, 0.0, 0.0));
        assert_eq!(direction, Vector3::new(0.0, 10.0, 0.0));
        let (anchor, direction) =
            glyph_placement(&document, &constraint(ParametricRef::point(h(1), 0))).unwrap();
        assert_eq!(anchor, Vector3::ZERO);
        assert_eq!(direction, Vector3::new(-5.0, 0.0, 0.0));
        let (anchor, direction) =
            glyph_placement(&document, &constraint(ParametricRef::center(h(2)))).unwrap();
        assert_eq!(anchor, Vector3::new(5.0, 0.0, 0.0));
        assert_eq!(direction, Vector3::new(5.0, 0.0, 0.0));
    }

    #[test]
    fn add_assigns_increasing_ids_and_get_finds_them() {
        let mut set = ParametricConstraintSet::new(ParametricScope::ModelSpace);
        let a = set.add(
            ConstraintKind::Horizontal,
            vec![ParametricRef::whole(h(1))],
            None,
        );
        let b = set.add(
            ConstraintKind::Distance,
            vec![ParametricRef::whole(h(1))],
            Some(DrivingValue::Literal(25.0)),
        );
        assert_ne!(a, b);
        assert_eq!(set.get(a).unwrap().kind, ConstraintKind::Horizontal);
        assert_eq!(
            set.get(b).unwrap().driving_param,
            Some(DrivingValue::Literal(25.0))
        );
    }

    #[test]
    fn remove_drops_only_the_matching_id() {
        let mut set = ParametricConstraintSet::new(ParametricScope::ModelSpace);
        let a = set.add(
            ConstraintKind::Horizontal,
            vec![ParametricRef::whole(h(1))],
            None,
        );
        let b = set.add(
            ConstraintKind::Vertical,
            vec![ParametricRef::whole(h(2))],
            None,
        );
        assert!(set.remove(a));
        assert!(
            !set.remove(a),
            "removing twice should report nothing removed the second time"
        );
        assert!(set.get(a).is_none());
        assert!(set.get(b).is_some());
    }

    #[test]
    fn constraints_touching_finds_entity_regardless_of_marker() {
        let mut set = ParametricConstraintSet::new(ParametricScope::ModelSpace);
        set.add(
            ConstraintKind::Coincident,
            vec![ParametricRef::point(h(1), 0), ParametricRef::point(h(2), 1)],
            None,
        );
        set.add(
            ConstraintKind::Horizontal,
            vec![ParametricRef::whole(h(3))],
            None,
        );

        let touching_1: Vec<_> = set.constraints_touching(h(1)).collect();
        assert_eq!(touching_1.len(), 1);
        let touching_3: Vec<_> = set.constraints_touching(h(3)).collect();
        assert_eq!(touching_3.len(), 1);
        assert_eq!(set.constraints_touching(h(99)).count(), 0);
    }

    #[test]
    fn constraints_touching_skips_disabled() {
        let mut set = ParametricConstraintSet::new(ParametricScope::ModelSpace);
        let id = set.add(
            ConstraintKind::Horizontal,
            vec![ParametricRef::whole(h(1))],
            None,
        );
        set.constraints
            .iter_mut()
            .find(|c| c.id == id)
            .unwrap()
            .enabled = false;
        assert_eq!(set.constraints_touching(h(1)).count(), 0);
    }

    #[test]
    fn remove_all_touching_drops_every_constraint_referencing_the_entity() {
        let mut set = ParametricConstraintSet::new(ParametricScope::ModelSpace);
        let coincident = set.add(
            ConstraintKind::Coincident,
            vec![ParametricRef::point(h(1), 0), ParametricRef::point(h(2), 1)],
            None,
        );
        let horizontal_other = set.add(
            ConstraintKind::Horizontal,
            vec![ParametricRef::whole(h(3))],
            None,
        );

        let removed = set.remove_all_touching(h(1));
        assert_eq!(removed, vec![coincident]);
        assert!(set.get(coincident).is_none());
        assert!(
            set.get(horizontal_other).is_some(),
            "unrelated entity's constraint must survive"
        );
    }

    #[test]
    fn scope_owner_handle_resolves_block_directly() {
        let block_handle = h(42);
        let scope = ParametricScope::Block(block_handle);
        let doc = acadrust::CadDocument::new();
        assert_eq!(scope.owner_handle(&doc), block_handle);
    }

    #[test]
    fn ref_center_constructor_matches_the_dash_three_convention() {
        let r = ParametricRef::center(h(7));
        assert_eq!(
            r,
            ParametricRef {
                entity: h(7),
                marker: Some(-3)
            }
        );
    }

    #[test]
    fn parameter_usage_finds_every_constraint_driven_by_the_named_parameter() {
        let mut scene = super::super::Scene::new();
        scene
            .parametric_constraint_set_mut(ParametricScope::ModelSpace)
            .add(
                ConstraintKind::Distance,
                vec![ParametricRef::point(h(1), 0), ParametricRef::point(h(1), 1)],
                Some(DrivingValue::Named("gap".to_string())),
            );
        scene
            .parametric_constraint_set_mut(ParametricScope::ModelSpace)
            .add(
                ConstraintKind::Radius,
                vec![ParametricRef::whole(h(2))],
                Some(DrivingValue::Named("gap".to_string())),
            );
        // Unrelated: a literal-driven constraint and one driven by a
        // different name must not show up.
        scene
            .parametric_constraint_set_mut(ParametricScope::ModelSpace)
            .add(
                ConstraintKind::Radius,
                vec![ParametricRef::whole(h(3))],
                Some(DrivingValue::Literal(5.0)),
            );
        scene
            .parametric_constraint_set_mut(ParametricScope::ModelSpace)
            .add(
                ConstraintKind::Distance,
                vec![ParametricRef::point(h(4), 0), ParametricRef::point(h(4), 1)],
                Some(DrivingValue::Named("other".to_string())),
            );

        let usage = scene.parameter_usage("gap");
        assert_eq!(
            usage.len(),
            2,
            "exactly the two constraints driven by 'gap', got {usage:?}"
        );
        assert!(usage
            .iter()
            .any(|u| u.kind == ConstraintKind::Distance && u.entities == vec![h(1)]));
        assert!(usage
            .iter()
            .any(|u| u.kind == ConstraintKind::Radius && u.entities == vec![h(2)]));

        assert_eq!(scene.parameter_usage("nonexistent").len(), 0);
    }

    #[test]
    fn parameter_usage_searches_every_scope_not_just_model_space() {
        let mut scene = super::super::Scene::new();
        let block = h(99);
        scene
            .parametric_constraint_set_mut(ParametricScope::Block(block))
            .add(
                ConstraintKind::Radius,
                vec![ParametricRef::whole(h(1))],
                Some(DrivingValue::Named("r".to_string())),
            );
        let usage = scene.parameter_usage("r");
        assert_eq!(usage.len(), 1);
        assert_eq!(usage[0].scope, ParametricScope::Block(block));
    }

    #[test]
    fn parameter_usage_deduplicates_an_entity_referenced_by_two_markers() {
        let mut scene = super::super::Scene::new();
        // A Distance constraint whose two points are both on the same
        // entity (e.g. a line's own start and end) must list that entity
        // once, not twice.
        scene
            .parametric_constraint_set_mut(ParametricScope::ModelSpace)
            .add(
                ConstraintKind::Distance,
                vec![ParametricRef::point(h(1), 0), ParametricRef::point(h(1), 1)],
                Some(DrivingValue::Named("len".to_string())),
            );
        let usage = scene.parameter_usage("len");
        assert_eq!(usage.len(), 1);
        assert_eq!(usage[0].entities, vec![h(1)]);
    }

    #[test]
    fn automatic_inference_maps_relations_and_skips_existing_constraints() {
        let mut scene = super::super::Scene::new();
        let first = scene.add_entity(acadrust::EntityType::Line(
            acadrust::entities::Line::from_points(
                Vector3::new(0.0, 0.0, 0.0),
                Vector3::new(5.0, 0.0, 0.0),
            ),
        ));
        let second = scene.add_entity(acadrust::EntityType::Line(
            acadrust::entities::Line::from_points(
                Vector3::new(5.0, 0.0, 0.0),
                Vector3::new(10.0, 0.0, 0.0),
            ),
        ));
        scene
            .parametric_constraint_set_mut(ParametricScope::ModelSpace)
            .add(
                ConstraintKind::Horizontal,
                vec![ParametricRef::whole(first)],
                None,
            );

        let inferred =
            scene.inferred_parametric_constraints(ParametricScope::ModelSpace, &[first, second]);

        assert!(!inferred.iter().any(|(kind, refs)| {
            *kind == ConstraintKind::Horizontal && *refs == [ParametricRef::whole(first)]
        }));
        assert!(inferred.iter().any(|(kind, refs)| {
            *kind == ConstraintKind::Horizontal && *refs == [ParametricRef::whole(second)]
        }));
        assert!(inferred
            .iter()
            .any(|(kind, _)| *kind == ConstraintKind::Coincident));
        assert!(inferred
            .iter()
            .any(|(kind, _)| *kind == ConstraintKind::Colinear));
    }

    #[test]
    fn visibility_toggles_geometric_and_dimensional_independently() {
        let mut scene = super::super::Scene::new();
        let line = scene.add_entity(acadrust::EntityType::Line(
            acadrust::entities::Line::from_points(
                Vector3::new(0.0, 0.0, 0.0),
                Vector3::new(5.0, 0.0, 0.0),
            ),
        ));
        let set = scene.parametric_constraint_set_mut(ParametricScope::ModelSpace);
        let geometric = set.add(
            ConstraintKind::Horizontal,
            vec![ParametricRef::whole(line)],
            None,
        );
        let dimensional = set.add(
            ConstraintKind::Distance,
            vec![ParametricRef::point(line, 0), ParametricRef::point(line, 1)],
            Some(DrivingValue::Literal(5.0)),
        );

        assert_eq!(
            scene.set_parametric_constraint_visibility(
                ParametricScope::ModelSpace,
                None,
                false,
                false,
            ),
            1
        );
        assert!(!scene.is_parametric_constraint_visible(ParametricScope::ModelSpace, geometric));
        assert!(scene.is_parametric_constraint_visible(ParametricScope::ModelSpace, dimensional));
    }
}
