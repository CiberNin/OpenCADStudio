use cadkernel::geom2d::{triangulate_rings, Curve};
use cadkernel::tessellation::DEFAULT_ANGLE;

#[cfg(test)]
mod tests;

const MAX_RECOVERY_POINTS: usize = 512;
const MAX_EXTRA_POINTS: usize = 128;
const MAX_PASSES: usize = 16;
const MAX_INTERSECTION_CHECKS: usize = 1_000_000;

/// Resample a crossed spline boundary without changing ordinary boundaries.
/// `project` must use the same coordinate frame as the eventual fill mesh.
pub(super) fn refine(
    ring: &[[f64; 2]],
    curves: impl FnOnce() -> Vec<Curve>,
    project: impl Fn([f64; 2]) -> [f64; 2],
) -> Option<Vec<[f64; 2]>> {
    if ring.len() > MAX_RECOVERY_POINTS {
        return None;
    }
    let mut work_left = MAX_INTERSECTION_CHECKS;
    if !crossings(&[ring], &project, &mut work_left)?[0] {
        return None;
    }
    let projected = ring.iter().copied().map(&project).collect();
    if !triangulate_rings(&[projected]).1.is_empty() {
        return None;
    }
    let limit = (ring.len() + MAX_EXTRA_POINTS).min(MAX_RECOVERY_POINTS);
    let mut spans = Vec::new();
    for curve in curves() {
        if let Curve::Nurbs(spline) = curve {
            let (start, end) = spline.domain();
            if end <= start || spline.control_points().len() > limit {
                return None;
            }
            for pair in spline.knots().windows(2) {
                if pair[0] >= start && pair[1] <= end && pair[0] < pair[1] {
                    spans.push(Curve::Nurbs(spline.trimmed(
                        (pair[0] - start) / (end - start),
                        (pair[1] - start) / (end - start),
                    )?));
                }
            }
        } else {
            spans.push(curve);
        }
    }
    let mut remaining = limit;
    let mut points: Vec<_> = spans
        .iter()
        .map(|curve| sample(curve, &mut remaining))
        .collect::<Option<_>>()?;
    for _ in 0..MAX_PASSES {
        let marked = crossings(&points, &project, &mut work_left)?;
        if !marked.iter().any(|marked| *marked) {
            let ring = super::entity::chain_path_edges(points);
            let projected = ring.iter().copied().map(&project).collect();
            return (!triangulate_rings(&[projected]).1.is_empty()).then_some(ring);
        }
        let mut next = Vec::new();
        let mut sampled = Vec::new();
        let mut changed = false;
        let mut remaining = limit;
        for ((curve, points), marked) in spans.into_iter().zip(points).zip(marked) {
            if marked {
                if let Curve::Nurbs(spline) = &curve {
                    let (a, b) = spline.split_at(0.5)?;
                    for part in [Curve::Nurbs(a), Curve::Nurbs(b)] {
                        sampled.push(sample(&part, &mut remaining)?);
                        next.push(part);
                    }
                    changed = true;
                    continue;
                }
            }
            remaining = remaining.checked_sub(points.len())?;
            next.push(curve);
            sampled.push(points);
        }
        if !changed {
            return None;
        }
        spans = next;
        points = sampled;
    }
    None
}

fn sample(curve: &Curve, remaining: &mut usize) -> Option<Vec<[f64; 2]>> {
    let Curve::Nurbs(spline) = curve else {
        let points = curve.tessellate_angle(DEFAULT_ANGLE);
        *remaining = remaining.checked_sub(points.len())?;
        return Some(points);
    };
    let mut min = [f64::INFINITY; 2];
    let mut max = [f64::NEG_INFINITY; 2];
    for point in spline.control_points() {
        for axis in 0..2 {
            min[axis] = min[axis].min(point[axis]);
            max[axis] = max[axis].max(point[axis]);
        }
    }
    // Scale chord error to this span; splitting tightens only crossed spans.
    let tolerance = (max[0] - min[0]).hypot(max[1] - min[1]) * (1.0 - (DEFAULT_ANGLE * 0.5).cos());
    if !tolerance.is_finite() || tolerance <= 0.0 {
        return None;
    }
    let points = curve.tessellate_within(tolerance);
    *remaining = remaining.checked_sub(points.len())?;
    Some(points)
}

fn crossings(
    edges: &[impl AsRef<[[f64; 2]]>],
    project: &impl Fn([f64; 2]) -> [f64; 2],
    work_left: &mut usize,
) -> Option<Vec<bool>> {
    let mut marked = vec![false; edges.len()];
    for projected in [false, true] {
        let mut segments = Vec::new();
        for (edge, points) in edges.iter().enumerate() {
            for pair in points.as_ref().windows(2) {
                let [a, b] = [pair[0], pair[1]].map(|p| if projected { project(p) } else { p });
                if !a.into_iter().chain(b).all(f64::is_finite) {
                    return None;
                }
                segments.push((a[0].min(b[0]), a[0].max(b[0]), edge, a, b));
            }
        }
        segments.sort_unstable_by(|a, b| a.0.total_cmp(&b.0));
        for (index, &(_, right, edge, a, b)) in segments.iter().enumerate() {
            for &(left, _, other, c, d) in &segments[index + 1..] {
                if left > right {
                    break;
                }
                *work_left = work_left.checked_sub(1)?;
                if a[1].max(b[1]) < c[1].min(d[1]) || c[1].max(d[1]) < a[1].min(b[1]) {
                    continue;
                }
                let cross = |a: [f64; 2], b: [f64; 2], c: [f64; 2]| {
                    (b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0])
                };
                let opposite = |a: f64, b: f64| (a < 0.0 && b > 0.0) || (a > 0.0 && b < 0.0);
                let inside = |p: [f64; 2], a: [f64; 2], b: [f64; 2]| {
                    cross(a, b, p) == 0.0
                        && (0..2).any(|axis| {
                            p[axis] > a[axis].min(b[axis]) && p[axis] < a[axis].max(b[axis])
                        })
                };
                if opposite(cross(a, b, c), cross(a, b, d))
                    && opposite(cross(c, d, a), cross(c, d, b))
                    || (inside(a, c, d)
                        || inside(b, c, d)
                        || inside(c, a, b)
                        || inside(d, a, b)
                        || a != b && (a == c && b == d || a == d && b == c))
                {
                    marked[edge] = true;
                    marked[other] = true;
                }
            }
        }
    }
    Some(marked)
}
