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
    fn rays_hit_a_box_face() {
        // A unit square in the plane z = 5.
        let m = Mesh { pos: vec![[0.0, 0.0, 5.0], [1.0, 0.0, 5.0], [1.0, 1.0, 5.0], [0.0, 1.0, 5.0]], tri: vec![[0, 1, 2], [0, 2, 3]] };
        let r = AxisRays::new(&m);
        assert!(r.hits([0.5, 0.5, 0.0], 2, 1.0, 0.0));
        assert!(!r.hits([0.5, 0.5, 0.0], 2, -1.0, 0.0));
        assert!(!r.hits([2.5, 0.5, 0.0], 2, 1.0, 0.0));
    }
}
