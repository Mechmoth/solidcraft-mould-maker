//! Small vector maths, rotations as axis and angle (what `solid.move` takes), and a part mesh
//! with ray casting along the axes for the box mould's pull check.

pub type V3 = [f64; 3];

pub fn add(a: V3, b: V3) -> V3 {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}
pub fn sub(a: V3, b: V3) -> V3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
pub fn mul(a: V3, k: f64) -> V3 {
    [a[0] * k, a[1] * k, a[2] * k]
}
pub fn dot(a: V3, b: V3) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
pub fn cross(a: V3, b: V3) -> V3 {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}
pub fn norm(a: V3) -> f64 {
    dot(a, a).sqrt()
}
pub fn unit(a: V3) -> V3 {
    let n = norm(a);
    if n > 0.0 {
        mul(a, 1.0 / n)
    } else {
        a
    }
}

/// A rotation about an axis through the origin, angle in degrees (right-handed).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rot {
    pub axis: V3,
    pub deg: f64,
}

impl Rot {
    pub fn none() -> Rot {
        Rot { axis: [0.0, 0.0, 1.0], deg: 0.0 }
    }
    pub fn is_none(&self) -> bool {
        self.deg.abs() < 1e-9
    }
    pub fn inverse(&self) -> Rot {
        Rot { axis: self.axis, deg: -self.deg }
    }
    /// Rodrigues' formula.
    pub fn apply(&self, v: V3) -> V3 {
        let k = unit(self.axis);
        let (s, c) = self.deg.to_radians().sin_cos();
        add(add(mul(v, c), mul(cross(k, v), s)), mul(k, dot(k, v) * (1.0 - c)))
    }
    /// The rotation taking unit vector `from` onto unit vector `to`.
    pub fn between(from: V3, to: V3) -> Rot {
        let (a, b) = (unit(from), unit(to));
        let c = dot(a, b).clamp(-1.0, 1.0);
        if c > 1.0 - 1e-12 {
            return Rot::none();
        }
        if c < -1.0 + 1e-12 {
            // Half a turn about any axis square to `from`.
            let side = if a[0].abs() < 0.9 { [1.0, 0.0, 0.0] } else { [0.0, 1.0, 0.0] };
            return Rot { axis: unit(cross(a, side)), deg: 180.0 };
        }
        Rot { axis: unit(cross(a, b)), deg: c.acos().to_degrees() }
    }
    /// The rotation whose matrix has these (orthonormal, right-handed) columns: it takes local
    /// x, y, z onto `cx`, `cy`, `cz`.
    pub fn from_columns(cx: V3, cy: V3, cz: V3) -> Rot {
        // m[row][col]
        let m = [[cx[0], cy[0], cz[0]], [cx[1], cy[1], cz[1]], [cx[2], cy[2], cz[2]]];
        let tr = m[0][0] + m[1][1] + m[2][2];
        let c = ((tr - 1.0) / 2.0).clamp(-1.0, 1.0);
        let angle = c.acos();
        if angle < 1e-9 {
            return Rot::none();
        }
        let axis = if angle < std::f64::consts::PI - 1e-6 {
            [m[2][1] - m[1][2], m[0][2] - m[2][0], m[1][0] - m[0][1]]
        } else {
            // Half a turn: m = 2 k k^T - I, take the largest diagonal.
            let d = [(m[0][0] + 1.0) / 2.0, (m[1][1] + 1.0) / 2.0, (m[2][2] + 1.0) / 2.0];
            let i = (0..3).max_by(|&a, &b| d[a].partial_cmp(&d[b]).unwrap()).unwrap();
            let ki = d[i].max(0.0).sqrt();
            let mut k = [0.0; 3];
            k[i] = ki;
            for j in 0..3 {
                if j != i {
                    k[j] = (m[i][j] + m[j][i]) / (4.0 * ki);
                }
            }
            k
        };
        Rot { axis: unit(axis), deg: angle.to_degrees() }
    }
}

/// A triangle mesh of the part.
#[derive(Clone, Debug, Default)]
pub struct Mesh {
    pub pos: Vec<V3>,
    pub tri: Vec<[usize; 3]>,
}

impl Mesh {
    pub fn corners(&self, t: usize) -> [V3; 3] {
        let [a, b, c] = self.tri[t];
        [self.pos[a], self.pos[b], self.pos[c]]
    }
    pub fn centroid(&self, t: usize) -> V3 {
        let [a, b, c] = self.corners(t);
        mul(add(add(a, b), c), 1.0 / 3.0)
    }
    /// Outward normal (the host meshes closed bodies counter-clockwise from outside) and area.
    pub fn normal_area(&self, t: usize) -> (V3, f64) {
        let [a, b, c] = self.corners(t);
        let n = cross(sub(b, a), sub(c, a));
        let l = norm(n);
        (if l > 0.0 { mul(n, 1.0 / l) } else { [0.0, 0.0, 0.0] }, 0.5 * l)
    }
}

/// Rays along the axes: for each axis a 2D grid of the triangles' shadows on the plane square
/// to it, so a ray only tests the triangles in its own cell.
pub struct AxisRays<'a> {
    mesh: &'a Mesh,
    grids: [Grid; 3],
}

struct Grid {
    /// The two axes the grid spans.
    a: usize,
    b: usize,
    lo: [f64; 2],
    cell: f64,
    n: [usize; 2],
    cells: Vec<Vec<u32>>,
}

impl Grid {
    fn new(mesh: &Mesh, axis: usize) -> Grid {
        let (a, b) = ((axis + 1) % 3, (axis + 2) % 3);
        let mut lo = [f64::MAX; 2];
        let mut hi = [f64::MIN; 2];
        for p in &mesh.pos {
            lo = [lo[0].min(p[a]), lo[1].min(p[b])];
            hi = [hi[0].max(p[a]), hi[1].max(p[b])];
        }
        if mesh.pos.is_empty() {
            lo = [0.0; 2];
            hi = [1.0; 2];
        }
        let span = (hi[0] - lo[0]).max(hi[1] - lo[1]).max(1e-6);
        let side = ((mesh.tri.len() as f64).sqrt() * 0.7).clamp(8.0, 160.0);
        let cell = span / side;
        let n = [((hi[0] - lo[0]) / cell) as usize + 1, ((hi[1] - lo[1]) / cell) as usize + 1];
        let mut cells = vec![Vec::new(); n[0] * n[1]];
        for (t, tri) in mesh.tri.iter().enumerate() {
            let (mut l, mut h) = ([f64::MAX; 2], [f64::MIN; 2]);
            for &i in tri {
                let p = mesh.pos[i];
                l = [l[0].min(p[a]), l[1].min(p[b])];
                h = [h[0].max(p[a]), h[1].max(p[b])];
            }
            let i0 = ((l[0] - lo[0]) / cell).floor().max(0.0) as usize;
            let j0 = ((l[1] - lo[1]) / cell).floor().max(0.0) as usize;
            let i1 = (((h[0] - lo[0]) / cell).floor() as usize).min(n[0] - 1);
            let j1 = (((h[1] - lo[1]) / cell).floor() as usize).min(n[1] - 1);
            for i in i0..=i1 {
                for j in j0..=j1 {
                    cells[i * n[1] + j].push(t as u32);
                }
            }
        }
        Grid { a, b, lo, cell, n, cells }
    }
    fn at(&self, p: V3) -> Option<&[u32]> {
        let i = ((p[self.a] - self.lo[0]) / self.cell).floor();
        let j = ((p[self.b] - self.lo[1]) / self.cell).floor();
        if i < 0.0 || j < 0.0 || i as usize >= self.n[0] || j as usize >= self.n[1] {
            return None;
        }
        Some(&self.cells[i as usize * self.n[1] + j as usize])
    }
}

impl<'a> AxisRays<'a> {
    pub fn new(mesh: &'a Mesh) -> AxisRays<'a> {
        AxisRays { mesh, grids: [Grid::new(mesh, 0), Grid::new(mesh, 1), Grid::new(mesh, 2)] }
    }
    /// Whether a ray from `p` along axis `axis` (0, 1, 2) in direction `sign` (+1 or -1) hits
    /// the part beyond `min_t`.
    pub fn hits(&self, p: V3, axis: usize, sign: f64, min_t: f64) -> bool {
        let Some(cands) = self.grids[axis].at(p) else { return false };
        let mut d = [0.0; 3];
        d[axis] = sign;
        cands.iter().any(|&t| ray_tri(p, d, self.mesh.corners(t as usize)).is_some_and(|s| s > min_t))
    }
}

impl AxisRays<'_> {
    /// All the distances at which a ray from `p` along an axis crosses the part.
    pub fn crossings(&self, p: V3, axis: usize, sign: f64) -> Vec<f64> {
        let Some(cands) = self.grids[axis].at(p) else { return vec![] };
        let mut d = [0.0; 3];
        d[axis] = sign;
        let mut t: Vec<f64> = cands.iter().filter_map(|&t| ray_tri(p, d, self.mesh.corners(t as usize))).collect();
        t.sort_by(|a, b| a.partial_cmp(b).unwrap());
        // A ray through a shared edge hits both triangles: count it once.
        t.dedup_by(|a, b| (*a - *b).abs() < 1e-6);
        t
    }
    /// Whether a point is inside the part (odd crossings going up).
    pub fn inside(&self, p: V3) -> bool {
        self.crossings(p, 2, 1.0).len() % 2 == 1
    }
}

/// Möller–Trumbore: the distance along `d` to the triangle, if the ray hits it.
pub fn ray_tri(o: V3, d: V3, [a, b, c]: [V3; 3]) -> Option<f64> {
    let e1 = sub(b, a);
    let e2 = sub(c, a);
    let p = cross(d, e2);
    let det = dot(e1, p);
    if det.abs() < 1e-12 {
        return None;
    }
    let inv = 1.0 / det;
    let s = sub(o, a);
    let u = dot(s, p) * inv;
    if !(0.0..=1.0).contains(&u) {
        return None;
    }
    let q = cross(s, e1);
    let v = dot(d, q) * inv;
    if v < 0.0 || u + v > 1.0 {
        return None;
    }
    let t = dot(e2, q) * inv;
    (t > 0.0).then_some(t)
}


pub type P2 = [f64; 2];

/// Convex hull, counter-clockwise, no repeated end point (Andrew's monotone chain).
pub fn hull(points: &[P2]) -> Vec<P2> {
    let mut p: Vec<P2> = points.to_vec();
    p.sort_by(|a, b| a.partial_cmp(b).unwrap());
    p.dedup_by(|a, b| (a[0] - b[0]).abs() < 1e-9 && (a[1] - b[1]).abs() < 1e-9);
    if p.len() < 3 {
        return p;
    }
    let turn = |o: P2, a: P2, b: P2| (a[0] - o[0]) * (b[1] - o[1]) - (a[1] - o[1]) * (b[0] - o[0]);
    let mut h: Vec<P2> = Vec::new();
    for pass in 0..2 {
        let start = h.len();
        let it: Box<dyn Iterator<Item = &P2>> = if pass == 0 { Box::new(p.iter()) } else { Box::new(p.iter().rev()) };
        for &q in it {
            while h.len() >= start + 2 && turn(h[h.len() - 2], h[h.len() - 1], q) <= 1e-12 {
                h.pop();
            }
            h.push(q);
        }
        h.pop();
    }
    h
}

/// How far a point lies inside a convex counter-clockwise polygon (negative outside).
pub fn inside_depth(poly: &[P2], q: P2) -> f64 {
    let n = poly.len();
    (0..n)
        .map(|i| {
            let (a, b) = (poly[i], poly[(i + 1) % n]);
            let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
            let l = (dx * dx + dy * dy).sqrt().max(1e-12);
            // Left of the edge is inside for counter-clockwise order.
            (dx * (q[1] - a[1]) - dy * (q[0] - a[0])) / l
        })
        .fold(f64::MAX, f64::min)
}

/// A convex counter-clockwise polygon grown by `d`, its corners rounded with arcs of at most
/// `step_deg`.
pub fn grow(poly: &[P2], d: f64, step_deg: f64) -> Vec<P2> {
    let n = poly.len();
    let mut out = Vec::new();
    for i in 0..n {
        let (prev, cur, next) = (poly[(i + n - 1) % n], poly[i], poly[(i + 1) % n]);
        let normal = |a: P2, b: P2| {
            let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
            let l = (dx * dx + dy * dy).sqrt().max(1e-12);
            [dy / l, -dx / l]
        };
        let (n0, n1) = (normal(prev, cur), normal(cur, next));
        let (a0, mut a1) = (n0[1].atan2(n0[0]), n1[1].atan2(n1[0]));
        while a1 < a0 {
            a1 += std::f64::consts::TAU;
        }
        let k = (((a1 - a0).to_degrees() / step_deg).ceil() as usize).max(1);
        for j in 0..=k {
            let a = a0 + (a1 - a0) * j as f64 / k as f64;
            out.push([cur[0] + d * a.cos(), cur[1] + d * a.sin()]);
        }
    }
    // Drop points closer than 0.2 mm to the last kept one.
    let mut kept: Vec<P2> = Vec::new();
    for q in out {
        if kept.last().is_none_or(|l: &P2| ((q[0] - l[0]).powi(2) + (q[1] - l[1]).powi(2)).sqrt() > 0.2) {
            kept.push(q);
        }
    }
    kept
}

/// Fewer points: drop any whose removal moves the outline by less than `tol` (convex input).
pub fn simplify(poly: &[P2], tol: f64) -> Vec<P2> {
    let mut p = poly.to_vec();
    loop {
        let n = p.len();
        if n <= 8 {
            return p;
        }
        let mut removed = false;
        let mut i = 0;
        while i < p.len() && p.len() > 8 {
            let n = p.len();
            let (a, q, b) = (p[(i + n - 1) % n], p[i], p[(i + 1) % n]);
            let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
            let l = (dx * dx + dy * dy).sqrt().max(1e-12);
            let dist = ((dx * (q[1] - a[1]) - dy * (q[0] - a[0])) / l).abs();
            if dist < tol {
                p.remove(i);
                removed = true;
            } else {
                i += 1;
            }
        }
        if !removed {
            return p;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: V3, b: V3) -> bool {
        norm(sub(a, b)) < 1e-9
    }

    #[test]
    fn between_takes_from_onto_to() {
        for from in [[0.0, -1.0, 0.0], [0.0, 0.0, 1.0], [1.0, 2.0, -3.0], [0.0, 0.0, -1.0]] {
            let r = Rot::between(from, [0.0, 0.0, -1.0]);
            assert!(close(r.apply(unit(from)), [0.0, 0.0, -1.0]), "{from:?}");
            assert!(close(r.inverse().apply(r.apply(from)), from));
        }
    }

    #[test]
    fn from_columns_round_trips() {
        let s = 0.5f64.sqrt();
        let cases = [
            ([1.0, 0.0, 0.0], [0.0, 1.0, 0.0]),
            ([-s, -s, 0.0], [0.0, 0.0, 1.0]),
            ([0.0, -0.6, 0.8], [1.0, 0.0, 0.0]),
            ([-1.0, 0.0, 0.0], [0.0, -1.0, 0.0]),
            ([0.0, 1.0, 0.0], [1.0, 0.0, 0.0]),
        ];
        for (u, e) in cases {
            let n = cross(u, e);
            let r = Rot::from_columns(u, e, n);
            assert!(close(r.apply([1.0, 0.0, 0.0]), u), "{u:?} {e:?} -> {r:?}");
            assert!(close(r.apply([0.0, 1.0, 0.0]), e));
            assert!(close(r.apply([0.0, 0.0, 1.0]), n));
        }
    }

    #[test]
    fn hull_and_grow() {
        let pts = [[0.0, 0.0], [10.0, 0.0], [10.0, 5.0], [0.0, 5.0], [5.0, 2.0], [3.0, 4.0]];
        let h = hull(&pts);
        assert_eq!(h.len(), 4);
        assert!((inside_depth(&h, [5.0, 2.0]) - 2.0).abs() < 1e-9);
        assert!(inside_depth(&h, [12.0, 2.0]) < 0.0);
        let g = grow(&h, 3.0, 10.0);
        for q in &g {
            let d = inside_depth(&h, *q);
            assert!(d <= -2.0 && d >= -3.0001, "{d}");
        }
        assert!((inside_depth(&simplify(&g, 0.05), [5.0, 2.0]) - 5.0).abs() < 0.1);
    }

    #[test]
    fn rays_hit_a_box_face() {
        // A unit square in the plane z = 5.
        let m = Mesh { pos: vec![[0.0, 0.0, 5.0], [1.0, 0.0, 5.0], [1.0, 1.0, 5.0], [0.0, 1.0, 5.0]], tri: vec![[0, 1, 2], [0, 2, 3]] };
        let r = AxisRays::new(&m);
        assert!(r.hits([0.5, 0.5, 0.0], 2, 1.0, 0.0));
        assert!(!r.hits([0.5, 0.5, 0.0], 2, -1.0, 0.0));
        assert!(!r.hits([2.5, 0.5, 0.0], 2, 1.0, 0.0));
    }
}
