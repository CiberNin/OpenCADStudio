//! Full reference chains for associative dimensions.
//!
//! An `AssocDimensionReference` addresses a *feature* — an osnap point on a
//! piece of geometry — through a chain of handles:
//!
//! ```text
//! dimension -> [viewport] -> [insert ...] -> entity -> feature
//! ```
//!
//! `AssocDimensionReference::xrefs` is that chain, outermost first. AutoCAD
//! calls the same list `mainObjectIds`; `intersection_objects`
//! (`intersectObjectIds`) is the second chain an INTERSECTION / APPARENT
//! INTERSECTION osnap needs.
//!
//! The pre-PR3 resolver only ever read `xrefs[0]`, which is correct for a
//! model-space dimension on top-level geometry and wrong for everything else:
//! a paper-space dimension measuring through a layout viewport stores the
//! VIEWPORT as `xrefs[0]`, so the resolver stopped at a viewport entity, found
//! no curve on it and gave up. Nine of the ten association objects in the
//! 115448 regression drawing are of that shape.
//!
//! This module owns:
//!  * [`walk_chain`] — turn `xrefs` into a viewport, a block-instance path
//!    with its accumulated transform, and the innermost entity.
//!  * [`feature_point`] — evaluate the osnap feature on that entity, in the
//!    entity's own coordinates, then lift it through the block path into
//!    model space.
//!
//! Mapping model -> paper through the viewport frame is deliberately *not*
//! done here; the caller owns it, because only the caller knows whether the
//! association is `trans_space` and which [`ViewportFrame`] is current.
//!
//! All arithmetic is `f64`.
//!
//! [`ViewportFrame`]: super::viewport_ref::ViewportFrame

use acadrust::objects::AssocDimensionReference;
use acadrust::types::{Handle, Matrix4, Transform, Vector3};
use acadrust::{CadDocument, EntityType};
use cadkernel::geom2d::{closest_point, Curve as KernelCurve, DEFAULT_SEGMENTS_PER_RADIAN};
use cadkernel::space::Plane;

use crate::entities::curve::entity_curve;

/// `AcDb::OsnapMode`, the value stored in
/// [`AssocDimensionReference::osnap_type`].
///
/// These are the AutoCAD ObjectARX codes, not our own [`crate::snap::SnapType`]
/// discriminants. The pre-PR3 writer already emitted `END` for generic points
/// and `NEAR` for a point taken at a parameter on a circle, so those two keep
/// meaning exactly what they used to.
#[allow(dead_code)] // The full code table is kept so a reader can see what a
// stored `osnap_type` means, including the modes we pass through untouched.
pub(crate) mod osnap {
    pub const NONE: u8 = 0;
    pub const END: u8 = 1;
    pub const MID: u8 = 2;
    pub const CEN: u8 = 3;
    pub const NODE: u8 = 4;
    pub const QUAD: u8 = 5;
    pub const INTERSEC: u8 = 6;
    pub const INS: u8 = 7;
    pub const PERP: u8 = 8;
    pub const TAN: u8 = 9;
    pub const NEAR: u8 = 10;
    pub const APPARENT_INT: u8 = 11;
    pub const PARA: u8 = 12;
    pub const START_POINT: u8 = 13;
}

/// Translate a live snap mode into the `AcDb::OsnapMode` we persist.
///
/// Modes with no ObjectARX equivalent (grid, object pick) map to
/// [`osnap::NONE`]; a reference carrying `NONE` resolves by marker/parameter
/// alone, exactly like the pre-PR3 records.
pub(crate) fn osnap_type_for(snap: crate::snap::SnapType) -> u8 {
    use crate::snap::SnapType as S;
    match snap {
        S::Endpoint => osnap::END,
        S::Midpoint => osnap::MID,
        S::Center => osnap::CEN,
        S::Node => osnap::NODE,
        S::Quadrant => osnap::QUAD,
        S::Intersection => osnap::INTERSEC,
        S::Insertion => osnap::INS,
        S::Perpendicular => osnap::PERP,
        S::Tangent => osnap::TAN,
        S::Nearest | S::Extension => osnap::NEAR,
        S::ApparentIntersection => osnap::APPARENT_INT,
        S::Parallel => osnap::PARA,
        S::Grid | S::ObjectPick => osnap::NONE,
    }
}

/// A resolved `xrefs` chain.
#[derive(Clone, Debug)]
pub(crate) struct ReferenceChain {
    /// The layout viewport the reference is seen through, when the chain
    /// starts with one. `None` for a plain model-space reference.
    pub viewport: Option<Handle>,
    /// INSERT handles from the outermost instance down to the instance that
    /// directly contains [`Self::entity`]. Empty for top-level geometry.
    pub block_path: Vec<Handle>,
    /// The innermost entity — the one that owns the feature.
    pub entity: Handle,
    /// Block-path transform: entity-local coordinates -> model coordinates.
    /// Identity when `block_path` is empty.
    pub transform: Transform,
}

/// Why a chain could not be walked. The data is never discarded on a failure:
/// the caller keeps the reference exactly as read and leaves the dimension
/// showing its last valid appearance.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ChainError {
    /// `xrefs` was empty — nothing to resolve.
    Empty,
    /// A handle in the chain names an object that is not in the document
    /// (erased source; restored by an undo).
    Missing(Handle),
}

/// Walk `xrefs` into a [`ReferenceChain`].
///
/// The chain is read positionally rather than by a stored kind, because that
/// is all the file gives us: a leading VIEWPORT is the trans-space hop, any
/// INSERTs after it are the block-instance path, and the first handle that is
/// neither is the geometry. A chain that is nothing but INSERTs (dimensioning
/// a block reference's insertion point) ends on the innermost INSERT.
pub(crate) fn walk_chain(
    document: &CadDocument,
    xrefs: &[Handle],
) -> Result<ReferenceChain, ChainError> {
    let mut viewport = None;
    let mut block_path: Vec<Handle> = Vec::new();
    let mut entity = None;
    let mut transform = Transform::identity();

    for (index, handle) in xrefs.iter().copied().enumerate() {
        if handle.is_null() {
            continue;
        }
        let Some(node) = document.get_entity(handle) else {
            return Err(ChainError::Missing(handle));
        };
        match node {
            // Only a *leading* viewport is the trans-space hop. A viewport
            // deeper in the chain would be geometry being dimensioned (the
            // paper-space rectangle itself), so it falls through to `entity`.
            EntityType::Viewport(_) if index == 0 && viewport.is_none() => {
                viewport = Some(handle);
            }
            EntityType::Insert(insert) => {
                block_path.push(handle);
                transform =
                    transform.compose(&crate::scene::render_graph::insert_transform(document, insert));
            }
            _ => {
                entity = Some(handle);
                break;
            }
        }
    }

    let entity = match entity {
        Some(entity) => entity,
        // Nothing but INSERTs: the innermost one is the geometry, and it is
        // no longer part of the path leading to it.
        None => match block_path.pop() {
            Some(innermost) => {
                transform = block_transform(document, &block_path);
                innermost
            }
            None => return Err(ChainError::Empty),
        },
    };
    Ok(ReferenceChain {
        viewport,
        block_path,
        entity,
        transform,
    })
}

/// Accumulated entity-local -> model transform for an ordered INSERT path.
pub(crate) fn block_transform(document: &CadDocument, block_path: &[Handle]) -> Transform {
    let mut transform = Transform::identity();
    for handle in block_path {
        if let Some(EntityType::Insert(insert)) = document.get_entity(*handle) {
            transform =
                transform.compose(&crate::scene::render_graph::insert_transform(document, insert));
        }
    }
    transform
}

/// Affine inverse of `transform`.
///
/// `acadrust::types::Matrix4` has no inverse of its own, and a block path only
/// ever produces affine matrices (translate / rotate / scale), so this inverts
/// the 3x3 linear part by cofactors and back-substitutes the translation. A
/// degenerate (zero-scale) insert returns `None` rather than exploding.
pub(crate) fn invert(transform: &Transform) -> Option<Transform> {
    let m = &transform.matrix.m;
    let a = [
        [m[0][0], m[0][1], m[0][2]],
        [m[1][0], m[1][1], m[1][2]],
        [m[2][0], m[2][1], m[2][2]],
    ];
    let det = a[0][0] * (a[1][1] * a[2][2] - a[1][2] * a[2][1])
        - a[0][1] * (a[1][0] * a[2][2] - a[1][2] * a[2][0])
        + a[0][2] * (a[1][0] * a[2][1] - a[1][1] * a[2][0]);
    if !det.is_finite() || det.abs() < 1e-18 {
        return None;
    }
    let inv_det = 1.0 / det;
    let mut inverse = [[0.0f64; 3]; 3];
    for row in 0..3 {
        for col in 0..3 {
            // Cofactor of (col, row) — transposed, which is the adjugate.
            let (r0, r1) = ((col + 1) % 3, (col + 2) % 3);
            let (c0, c1) = ((row + 1) % 3, (row + 2) % 3);
            inverse[row][col] = (a[r0][c0] * a[r1][c1] - a[r0][c1] * a[r1][c0]) * inv_det;
        }
    }
    let t = [m[0][3], m[1][3], m[2][3]];
    let translation = [
        -(inverse[0][0] * t[0] + inverse[0][1] * t[1] + inverse[0][2] * t[2]),
        -(inverse[1][0] * t[0] + inverse[1][1] * t[1] + inverse[1][2] * t[2]),
        -(inverse[2][0] * t[0] + inverse[2][1] * t[1] + inverse[2][2] * t[2]),
    ];
    Some(Transform {
        matrix: Matrix4 {
            m: [
                [inverse[0][0], inverse[0][1], inverse[0][2], translation[0]],
                [inverse[1][0], inverse[1][1], inverse[1][2], translation[1]],
                [inverse[2][0], inverse[2][1], inverse[2][2], translation[2]],
                [0.0, 0.0, 0.0, 1.0],
            ],
        },
    })
}

// ── Feature evaluation ────────────────────────────────────────────────────

fn vector3(point: [f64; 3]) -> Vector3 {
    Vector3::new(point[0], point[1], point[2])
}

fn dpoint(point: Vector3) -> [f64; 3] {
    [point.x, point.y, point.z]
}

fn distance_squared(a: Vector3, b: Vector3) -> f64 {
    let (dx, dy, dz) = (a.x - b.x, a.y - b.y, a.z - b.z);
    dx * dx + dy * dy + dz * dz
}

fn lift(plane: &Plane, uv: [f64; 2]) -> Vector3 {
    vector3(plane.point_at(uv))
}

/// Everything a feature evaluation may need beyond the reference itself.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct FeatureContext {
    /// The stored osnap point, already expressed in *entity-local*
    /// coordinates. Used as the disambiguating hint for every feature that has
    /// more than one candidate (quadrant, intersection, tangent, nearest).
    pub hint: Option<Vector3>,
    /// The point the feature is measured *from*, in entity-local coordinates.
    /// PERPENDICULAR and TANGENT are two-point features: the foot / touch
    /// point only exists relative to an external point, which for a dimension
    /// is the other definition point. `None` falls back to `hint`.
    pub from: Option<Vector3>,
}

/// Evaluate the osnap feature `reference` names on `entity`, in the entity's
/// own coordinates.
///
/// Returns `None` — never a guess — when the osnap type is one we do not
/// evaluate, or when the geometry no longer supports it. The caller keeps the
/// reference data intact and leaves the dimension where it was.
pub(crate) fn feature_point(
    document: &CadDocument,
    entity: &EntityType,
    reference: &AssocDimensionReference,
    context: FeatureContext,
) -> Option<Vector3> {
    match reference.osnap_type {
        osnap::CEN => center_point(entity, reference),
        osnap::MID => mid_point(entity, reference),
        osnap::QUAD => quadrant_point(entity, context.hint?),
        osnap::NODE | osnap::INS => node_point(entity),
        osnap::INTERSEC | osnap::APPARENT_INT => intersection_point(
            document,
            entity,
            reference,
            context.hint?,
            reference.osnap_type == osnap::APPARENT_INT,
        ),
        osnap::PERP => perpendicular_point(entity, context.from.or(context.hint)?),
        osnap::TAN => tangent_point(entity, reference, context),
        osnap::NEAR => nearest_point(entity, reference, context.hint),
        // END / START_POINT / PARA / NONE all address a named vertex or a
        // stored curve parameter, which is what the legacy marker convention
        // already encodes. The caller keeps that path.
        _ => None,
    }
}

/// Whether [`feature_point`] knows how to evaluate this osnap type. A type we
/// do not handle keeps its stored data and stays unresolved.
pub(crate) fn evaluates(osnap_type: u8) -> bool {
    matches!(
        osnap_type,
        osnap::CEN
            | osnap::MID
            | osnap::QUAD
            | osnap::NODE
            | osnap::INS
            | osnap::INTERSEC
            | osnap::APPARENT_INT
            | osnap::PERP
            | osnap::TAN
            | osnap::NEAR
    )
}

/// Whether this osnap type needs the *other* definition point to evaluate.
pub(crate) fn needs_from_point(osnap_type: u8) -> bool {
    matches!(osnap_type, osnap::PERP | osnap::TAN)
}

fn center_point(entity: &EntityType, reference: &AssocDimensionReference) -> Option<Vector3> {
    match entity {
        EntityType::Circle(circle) => Some(circle.center_wcs()),
        EntityType::Arc(arc) => Some(arc.center_wcs()),
        EntityType::Ellipse(ellipse) => Some(ellipse.center),
        _ => {
            // A polyline's bulge segment: `main_gs_marker` names the segment.
            let planar = entity_curve(entity)?;
            let KernelCurve::Polyline(polyline) = &planar.curve else {
                return None;
            };
            let segment = reference.main_gs_marker.max(0) as usize;
            let arc = polyline.segment_arc(segment)?;
            Some(lift(&planar.plane, arc.center))
        }
    }
}

fn mid_point(entity: &EntityType, reference: &AssocDimensionReference) -> Option<Vector3> {
    let planar = entity_curve(entity)?;
    match &planar.curve {
        // For a polyline the marker names which segment's midpoint it is;
        // without one, the midpoint of the whole run.
        KernelCurve::Polyline(_) if reference.main_gs_marker >= 0 => {
            let segments = planar.curve.segments();
            let segment = segments.get(reference.main_gs_marker as usize)?;
            Some(lift(&planar.plane, segment.point_at(0.5)))
        }
        _ => Some(lift(&planar.plane, planar.curve.point_at(0.5))),
    }
}

fn quadrant_point(entity: &EntityType, hint: Vector3) -> Option<Vector3> {
    let planar = entity_curve(entity)?;
    let (centre, radius) = match &planar.curve {
        KernelCurve::Circle(circle) => (circle.centre, circle.radius),
        KernelCurve::Arc(arc) => (arc.centre, arc.radius),
        _ => return None,
    };
    // Four candidates on the curve's own plane; the stored point picks one, so
    // a rotated source keeps the quadrant the user actually snapped to.
    (0..4)
        .map(|index| {
            let angle = std::f64::consts::FRAC_PI_2 * index as f64;
            lift(
                &planar.plane,
                [
                    centre[0] + radius * angle.cos(),
                    centre[1] + radius * angle.sin(),
                ],
            )
        })
        .min_by(|first, second| {
            distance_squared(*first, hint).total_cmp(&distance_squared(*second, hint))
        })
}

fn node_point(entity: &EntityType) -> Option<Vector3> {
    match entity {
        EntityType::Point(point) => Some(point.location),
        EntityType::Insert(insert) => Some(insert.insert_point),
        EntityType::Text(text) => Some(text.insertion_point),
        EntityType::MText(text) => Some(text.insertion_point),
        EntityType::Shape(shape) => Some(shape.insertion_point),
        _ => None,
    }
}

fn perpendicular_point(entity: &EntityType, from: Vector3) -> Option<Vector3> {
    let planar = entity_curve(entity)?;
    let uv = planar.plane.project(dpoint(from))?;
    Some(lift(&planar.plane, closest_point(&planar.curve, uv).point))
}

fn nearest_point(
    entity: &EntityType,
    reference: &AssocDimensionReference,
    hint: Option<Vector3>,
) -> Option<Vector3> {
    let planar = entity_curve(entity)?;
    // Circles and arcs carry an *angle* in `osnap_distance` — that is the
    // convention the pre-PR3 writer established and imported files share it.
    // Every other curve carries the kernel's normalised 0..1 parameter.
    match &planar.curve {
        KernelCurve::Circle(_) | KernelCurve::Arc(_) => match entity {
            EntityType::Circle(circle) => {
                Some(circle.point_at_angle_wcs(reference.osnap_distance))
            }
            EntityType::Arc(arc) => Some(arc.point_at_angle_wcs(reference.osnap_distance)),
            _ => None,
        },
        _ => {
            let parameter = reference.osnap_distance;
            if parameter.is_finite() && (0.0..=1.0).contains(&parameter) {
                return Some(lift(&planar.plane, planar.curve.point_at(parameter)));
            }
            // No usable parameter: fall back to the point on the curve nearest
            // the stored osnap point, which is what NEAR means anyway.
            let uv = planar.plane.project(dpoint(hint?))?;
            Some(lift(&planar.plane, closest_point(&planar.curve, uv).point))
        }
    }
}

// ── Tangent ───────────────────────────────────────────────────────────────

/// Tangent point on `entity` for the touch line through `context.from`.
///
/// Circles and arcs have a closed form. A spline does not, so the stored
/// parameter (`osnap_distance`) seeds a Newton refinement of the tangency
/// condition `(C(u) - P) · C'(u) = 0`; with no usable parameter the seed is
/// the point on the spline nearest the stored osnap point. That is what makes
/// the 115448 drawing's tangent-to-spline reference survive an edit to the
/// spline instead of silently freezing at its imported location.
fn tangent_point(
    entity: &EntityType,
    reference: &AssocDimensionReference,
    context: FeatureContext,
) -> Option<Vector3> {
    if let EntityType::Spline(spline) = entity {
        return spline_tangent_point(spline, reference, context);
    }
    let planar = entity_curve(entity)?;
    let (centre, radius) = match &planar.curve {
        KernelCurve::Circle(circle) => (circle.centre, circle.radius),
        KernelCurve::Arc(arc) => (arc.centre, arc.radius),
        _ => {
            // Generic curve: no closed form and no spline evaluator. Hold the
            // stored parameter if there is one, else leave it unresolved.
            return nearest_point(entity, reference, context.hint);
        }
    };
    let from = context.from.or(context.hint)?;
    let uv = planar.plane.project(dpoint(from))?;
    let (dx, dy) = (uv[0] - centre[0], uv[1] - centre[1]);
    let distance = (dx * dx + dy * dy).sqrt();
    if !distance.is_finite() || distance <= radius + 1e-12 || radius <= 1e-12 {
        return None;
    }
    // Two touch points, symmetric about the centre-to-`from` line.
    let base = dy.atan2(dx);
    let offset = (radius / distance).clamp(-1.0, 1.0).acos();
    let candidates = [base + offset, base - offset].map(|angle| {
        lift(
            &planar.plane,
            [
                centre[0] + radius * angle.cos(),
                centre[1] + radius * angle.sin(),
            ],
        )
    });
    let hint = context.hint.unwrap_or(candidates[0]);
    candidates
        .into_iter()
        .min_by(|first, second| {
            distance_squared(*first, hint).total_cmp(&distance_squared(*second, hint))
        })
}

fn spline_tangent_point(
    spline: &acadrust::entities::Spline,
    reference: &AssocDimensionReference,
    context: FeatureContext,
) -> Option<Vector3> {
    let curve = crate::entities::spline::nurbs3(spline)?;
    let (start, end) = curve.domain();
    let stored = reference.osnap_distance;
    let mut u = if stored.is_finite() && stored > start - 1e-9 && stored < end + 1e-9 {
        stored.clamp(start, end)
    } else {
        let hint = context.hint.or(context.from)?;
        curve.parameter_at(dpoint(hint)).clamp(start, end)
    };
    let Some(from) = context.from.or(context.hint) else {
        // No external point: the stored parameter is the whole answer.
        return Some(vector3(curve.point_at_knot(u)));
    };
    let p = dpoint(from);
    // Newton on f(u) = (C(u) - P) · C'(u). f'(u) is approximated by a central
    // difference of f, which converges in a handful of steps from a seed that
    // is already on the right lobe and never leaves the domain.
    let f = |u: f64| {
        let c = curve.point_at_knot(u);
        let d = curve.derivative_at_knot(u);
        (c[0] - p[0]) * d[0] + (c[1] - p[1]) * d[1] + (c[2] - p[2]) * d[2]
    };
    let span = (end - start).abs().max(1e-12);
    let step = span * 1e-6;
    for _ in 0..24 {
        let value = f(u);
        if value.abs() <= span * 1e-12 {
            break;
        }
        let slope = (f((u + step).min(end)) - f((u - step).max(start))) / (2.0 * step);
        if !slope.is_finite() || slope.abs() < 1e-18 {
            break;
        }
        let next = (u - value / slope).clamp(start, end);
        if !next.is_finite() || (next - u).abs() <= span * 1e-14 {
            u = next;
            break;
        }
        u = next;
    }
    Some(vector3(curve.point_at_knot(u)))
}

// ── Intersection ──────────────────────────────────────────────────────────

/// Intersection of `entity` with the reference's second object chain.
///
/// Both curves are tessellated on the first curve's plane and crossed
/// pairwise; the crossing nearest the stored osnap point wins, so a pair of
/// curves that meet more than once keeps the corner the user picked. With
/// `apparent` the segment parameters are not clamped, which recovers the
/// crossing of the two *extended* nearest segments — the best-effort AutoCAD
/// calls an apparent intersection in a plan view.
fn intersection_point(
    document: &CadDocument,
    entity: &EntityType,
    reference: &AssocDimensionReference,
    hint: Vector3,
    apparent: bool,
) -> Option<Vector3> {
    let other = walk_chain(document, &reference.intersection_objects).ok()?;
    let other_entity = document.get_entity(other.entity)?;
    let first = entity_curve(entity)?;
    let second = entity_curve(other_entity)?;
    // The first curve's plane is the working frame. The second curve is
    // evaluated through its *own* block path, so each of its points is lifted
    // to model space and then projected back onto that frame before crossing.
    let a = first.curve.tessellate(DEFAULT_SEGMENTS_PER_RADIAN);
    let b = second
        .curve
        .tessellate(DEFAULT_SEGMENTS_PER_RADIAN)
        .into_iter()
        .map(|uv| {
            let world = other.transform.apply(lift(&second.plane, uv));
            first.plane.project(dpoint(world)).unwrap_or(uv)
        })
        .collect::<Vec<_>>();
    let hint_uv = first.plane.project(dpoint(hint))?;
    let mut best: Option<([f64; 2], f64)> = None;
    for pair_a in a.windows(2) {
        for pair_b in b.windows(2) {
            let Some(point) = segment_cross(pair_a[0], pair_a[1], pair_b[0], pair_b[1], false)
            else {
                continue;
            };
            let error = (point[0] - hint_uv[0]).powi(2) + (point[1] - hint_uv[1]).powi(2);
            if best.is_none_or(|(_, previous)| error < previous) {
                best = Some((point, error));
            }
        }
    }
    if best.is_none() && apparent {
        // Nothing actually crosses: take the two segments nearest the stored
        // point and cross their infinite extensions.
        let nearest = |run: &[[f64; 2]]| -> Option<([f64; 2], [f64; 2])> {
            run.windows(2)
                .map(|pair| {
                    let mid = [
                        (pair[0][0] + pair[1][0]) * 0.5,
                        (pair[0][1] + pair[1][1]) * 0.5,
                    ];
                    let error =
                        (mid[0] - hint_uv[0]).powi(2) + (mid[1] - hint_uv[1]).powi(2);
                    ((pair[0], pair[1]), error)
                })
                .min_by(|first, second| first.1.total_cmp(&second.1))
                .map(|(pair, _)| pair)
        };
        let (a0, a1) = nearest(&a)?;
        let (b0, b1) = nearest(&b)?;
        best = segment_cross(a0, a1, b0, b1, true).map(|point| (point, 0.0));
    }
    best.map(|(point, _)| lift(&first.plane, point))
}

/// Crossing of segments `a0->a1` and `b0->b1`. With `extend` the parameters
/// are not clamped, giving the crossing of the two infinite lines.
fn segment_cross(
    a0: [f64; 2],
    a1: [f64; 2],
    b0: [f64; 2],
    b1: [f64; 2],
    extend: bool,
) -> Option<[f64; 2]> {
    let d = [a1[0] - a0[0], a1[1] - a0[1]];
    let e = [b1[0] - b0[0], b1[1] - b0[1]];
    let denominator = d[0] * e[1] - d[1] * e[0];
    if !denominator.is_finite() || denominator.abs() < 1e-18 {
        return None;
    }
    let delta = [b0[0] - a0[0], b0[1] - a0[1]];
    let t = (delta[0] * e[1] - delta[1] * e[0]) / denominator;
    let s = (delta[0] * d[1] - delta[1] * d[0]) / denominator;
    if !extend && !((-1e-9..=1.0 + 1e-9).contains(&t) && (-1e-9..=1.0 + 1e-9).contains(&s)) {
        return None;
    }
    Some([a0[0] + d[0] * t, a0[1] + d[1] * t])
}
