//! Connected selection contours. Input bands use the same visual-row rectangles as carets.
use vel_platform::{Point, Rect};
/// Corner treatment independent of scheme parsing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Corners {
    /// Circular-looking quadratic corner joins.
    Round,
    /// Diagonal joins.
    Cut,
    /// Orthogonal joins.
    Square,
}
/// A complete connected outline, including concave wrap/line joins.
#[derive(Clone, Debug)]
pub struct Contour {
    /// Clockwise boundary in scene coordinates.
    pub points: std::sync::Arc<[Point]>,
    /// Fill triangles, indexed into points. Computed once when the contour is made.
    pub triangles: std::sync::Arc<[[usize; 3]]>,
    /// Selection fill.
    pub fill: crate::scene::Color,
    /// One continuous selection border.
    pub border: crate::scene::Color,
    /// Scene clipping bounds.
    pub clip: Rect,
    /// Translation applied without copying shared cached geometry.
    pub offset: Point,
    /// Border thickness in logical points.
    pub border_width: f64,
}
fn cross(a: Point, b: Point, c: Point) -> f64 {
    (b.x - a.x) * (c.y - a.y) - (b.y - a.y) * (c.x - a.x)
}
fn distance(a: Point, b: Point) -> f64 {
    (b.x - a.x).hypot(b.y - a.y)
}
/// Returns the union perimeter of adjacent overlapping bands, without internal seams.
/// Non-overlapping rows start another connected component.
pub fn outlines(bands: &[Rect]) -> Vec<Vec<Point>> {
    let mut result = Vec::new();
    let mut first = 0;
    while first < bands.len() {
        let mut end = first + 1;
        while end < bands.len()
            && (bands[end - 1].y + bands[end - 1].height - bands[end].y).abs() < 0.001
            && bands[end - 1].x < bands[end].x + bands[end].width
            && bands[end].x < bands[end - 1].x + bands[end - 1].width
        {
            end += 1;
        }
        let rows = &bands[first..end];
        let mut points = vec![
            Point {
                x: rows[0].x,
                y: rows[0].y,
            },
            Point {
                x: rows[0].x + rows[0].width,
                y: rows[0].y,
            },
        ];
        for (i, r) in rows.iter().enumerate() {
            points.push(Point {
                x: r.x + r.width,
                y: r.y + r.height,
            });
            if let Some(next) = rows.get(i + 1) {
                points.push(Point {
                    x: next.x + next.width,
                    y: r.y + r.height,
                });
            }
        }
        let last = rows.last().expect("nonempty bands");
        points.push(Point {
            x: last.x,
            y: last.y + last.height,
        });
        for i in (0..rows.len()).rev() {
            let r = rows[i];
            points.push(Point { x: r.x, y: r.y });
            if i > 0 {
                points.push(Point {
                    x: rows[i - 1].x,
                    y: r.y,
                });
            }
        }
        points.dedup();
        if points.first() == points.last() {
            points.pop();
        }
        // Collapse collinear chains with a stack; every point is pushed/popped once.
        let mut simplified = Vec::with_capacity(points.len());
        for point in points {
            while simplified.len() >= 2
                && cross(
                    simplified[simplified.len() - 2],
                    simplified[simplified.len() - 1],
                    point,
                )
                .abs()
                    <= 0.00001
            {
                simplified.pop();
            }
            simplified.push(point);
        }
        while simplified.len() >= 3
            && cross(
                simplified[simplified.len() - 2],
                *simplified.last().unwrap(),
                simplified[0],
            )
            .abs()
                <= 0.00001
        {
            simplified.pop();
        }
        let points = simplified;
        if points.len() >= 3 {
            result.push(points);
        }
        first = end;
    }
    result
}
/// Rounds or cuts each convex/concave corner with a radius clamped to its neighbors.
pub fn corners(points: &[Point], style: Corners, radius: f64) -> Vec<Point> {
    if radius <= 0. || style == Corners::Square {
        return points.to_vec();
    }
    let mut result = Vec::new();
    for (i, &p) in points.iter().enumerate() {
        let previous = points[(i + points.len() - 1) % points.len()];
        let next = points[(i + 1) % points.len()];
        let a = distance(previous, p);
        let b = distance(p, next);
        if a <= 0. || b <= 0. {
            continue;
        }
        let r = radius.min(a / 2.).min(b / 2.);
        let enter = Point {
            x: p.x + (previous.x - p.x) * r / a,
            y: p.y + (previous.y - p.y) * r / a,
        };
        let exit = Point {
            x: p.x + (next.x - p.x) * r / b,
            y: p.y + (next.y - p.y) * r / b,
        };
        result.push(enter);
        if style == Corners::Round {
            for step in 1..4 {
                let t = step as f64 / 4.;
                let u = 1. - t;
                result.push(Point {
                    x: u * u * enter.x + 2. * u * t * p.x + t * t * exit.x,
                    y: u * u * enter.y + 2. * u * t * p.y + t * t * exit.y,
                });
            }
        }
        result.push(exit);
    }
    result.dedup();
    if result.first() == result.last() {
        result.pop();
    }
    result
}
/// Triangulates a y-monotone clockwise contour in linear time. Each boundary
/// chain is already ordered by y: merge the chains, then consume a vertex stack.
/// Horizontal ties retain boundary order; zero-area triangles need no primitive.
pub fn triangulate(points: &[Point]) -> Vec<[usize; 3]> {
    if points.len() < 3 {
        return Vec::new();
    }
    let n = points.len();
    let order = |a: usize, b: usize| {
        points[a]
            .y
            .total_cmp(&points[b].y)
            .then(points[a].x.total_cmp(&points[b].x))
    };
    let first = (0..n).min_by(|a, b| order(*a, *b)).unwrap();
    let last = (0..n).max_by(|a, b| order(*a, *b)).unwrap();
    let mut right = Vec::new();
    let mut i = (first + 1) % n;
    while i != last {
        right.push(i);
        i = (i + 1) % n;
    }
    let mut left = Vec::new();
    i = (first + n - 1) % n;
    while i != last {
        left.push(i);
        i = (i + n - 1) % n;
    }
    let mut vertices = vec![(first, false)];
    let (mut r, mut l) = (0, 0);
    while r < right.len() || l < left.len() {
        if l == left.len() || r < right.len() && order(right[r], left[l]).is_le() {
            vertices.push((right[r], true));
            r += 1;
        } else {
            vertices.push((left[l], false));
            l += 1;
        }
    }
    vertices.push((last, false));
    let mut stack = vec![vertices[0], vertices[1]];
    let mut triangles = Vec::with_capacity(n - 2);
    let mut emit = |a, b, c| {
        let area = cross(points[a], points[b], points[c]);
        if area > 1e-10 {
            triangles.push([a, b, c]);
        } else if area < -1e-10 {
            triangles.push([c, b, a]);
        }
    };
    for index in 2..vertices.len() - 1 {
        let current = vertices[index];
        if current.1 != stack.last().unwrap().1 {
            let mut previous = stack.pop().unwrap();
            while let Some(next) = stack.pop() {
                emit(current.0, previous.0, next.0);
                previous = next;
            }
            stack.push(vertices[index - 1]);
            stack.push(current);
        } else {
            let mut previous = stack.pop().unwrap();
            while let Some(next) = stack.last() {
                let area = cross(points[current.0], points[previous.0], points[next.0]);
                if if current.1 { area > 0. } else { area < 0. } {
                    break;
                }
                emit(current.0, previous.0, next.0);
                previous = stack.pop().unwrap();
            }
            stack.push(previous);
            stack.push(current);
        }
    }
    let mut previous = stack.pop().unwrap();
    while let Some(next) = stack.pop() {
        emit(last, previous.0, next.0);
        previous = next;
    }
    triangles
}
impl Contour {
    /// Applies corner geometry once; rendering and headless tests consume the same points.
    pub fn new(
        points: &[Point],
        style: Corners,
        radius: f64,
        fill: crate::scene::Color,
        border: crate::scene::Color,
        clip: Rect,
    ) -> Self {
        let points = corners(points, style, radius);
        let triangles = triangulate(&points);
        Self {
            points: points.into(),
            triangles: triangles.into(),
            offset: Point::default(),
            border_width: 1.,
            fill,
            border,
            clip,
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn connected_outline_corner_geometry_table() {
        for widths in [[40., 40., 40.], [30., 60., 20.], [80., 10., 80.]] {
            let bands: Vec<_> = widths
                .iter()
                .enumerate()
                .map(|(i, w)| Rect {
                    x: 0.,
                    y: i as f64 * 20.,
                    width: *w,
                    height: 20.,
                })
                .collect();
            let outlines = outlines(&bands);
            assert_eq!(outlines.len(), 1);
            for style in [Corners::Round, Corners::Cut, Corners::Square] {
                for radius in [0., 2., 200.] {
                    let points = corners(&outlines[0], style, radius);
                    let triangles = triangulate(&points);
                    // Horizontal ties may eliminate zero-area triangles. Validate
                    // exact filled area, rather than requiring degenerate primitives.
                    assert!(triangles.len() <= points.len() - 2);
                    let expected = points
                        .iter()
                        .zip(points.iter().cycle().skip(1))
                        .take(points.len())
                        .map(|(a, b)| a.x * b.y - b.x * a.y)
                        .sum::<f64>()
                        / 2.;
                    let area: f64 = triangles
                        .iter()
                        .map(|[a, b, c]| cross(points[*a], points[*b], points[*c]) / 2.)
                        .sum();
                    assert!((area - expected).abs() < 1e-6);
                    assert!(area > 0. && area <= widths.iter().sum::<f64>() * 20. + 0.001);
                    assert!(points.iter().all(|p| p.x.is_finite() && p.y.is_finite()));
                }
            }
        }
        assert_eq!(
            outlines(&[
                Rect {
                    x: 0.,
                    y: 0.,
                    width: 2.,
                    height: 20.
                },
                Rect {
                    x: 3.,
                    y: 20.,
                    width: 2.,
                    height: 20.
                }
            ])
            .len(),
            2
        );
    }
}

#[cfg(test)]
mod ragged_tests {
    use super::*;
    #[test]
    fn monotone_triangulation_matches_ragged_outline_area_in_a_randomized_sweep() {
        let mut seed = 81u64;
        for rows in [1, 2, 3, 10, 50, 200, 1000] {
            for _ in 0..8 {
                let bands = (0..rows)
                    .map(|i| {
                        seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
                        Rect {
                            x: (seed % 7) as f64,
                            y: i as f64 * 16.,
                            width: 8. + (seed % 131) as f64,
                            height: 16.,
                        }
                    })
                    .collect::<Vec<_>>();
                for style in [Corners::Round, Corners::Cut, Corners::Square] {
                    for radius in [0., 2., 100.] {
                        for outline in outlines(&bands) {
                            let points = corners(&outline, style, radius);
                            let triangles = triangulate(&points);
                            let area = points
                                .iter()
                                .zip(points.iter().cycle().skip(1))
                                .take(points.len())
                                .map(|(a, b)| a.x * b.y - b.x * a.y)
                                .sum::<f64>()
                                / 2.;
                            let fill = triangles
                                .iter()
                                .map(|[a, b, c]| cross(points[*a], points[*b], points[*c]) / 2.)
                                .sum::<f64>();
                            assert!(
                                (fill - area).abs() < 1e-6 * area.max(1.),
                                "rows={rows} {style:?} radius={radius} area={area} fill={fill} points={points:?} triangles={triangles:?}"
                            );
                        }
                    }
                }
            }
        }
    }
    #[test]
    #[ignore = "explicit linear triangulation timing on ragged rows"]
    fn ragged_triangulation_50_200_1000_lines() {
        for rows in [50, 200, 1000] {
            let bands = (0..rows)
                .map(|i| Rect {
                    x: 0.,
                    y: i as f64 * 16.,
                    width: 20. + ((i * 73) % 131) as f64,
                    height: 16.,
                })
                .collect::<Vec<_>>();
            let points = corners(&outlines(&bands)[0], Corners::Round, 2.);
            let start = std::time::Instant::now();
            for _ in 0..1000 {
                std::hint::black_box(triangulate(&points));
            }
            let micros = start.elapsed().as_secs_f64() * 1e6 / 1000.;
            println!(
                "RAGGED_TRIANGULATION rows={rows} points={} us={micros:.3}",
                points.len()
            );
            assert!(
                micros < rows as f64 * 2.,
                "linear triangulation budget: {rows} rows {micros} us"
            );
        }
    }
}
