//! Reads a part's shape from its triangle mesh: horizontal slices, cast rays from the mould
//! axis, and the material intervals along each ray. Everything the layout needs comes from
//! these intervals: how far the part reaches, where a jacket can split from the plug, and
//! where the plug would lock in.

/// A triangle mesh in millimetres.
pub struct Mesh {
    pub pos: Vec<[f64; 3]>,
    pub tri: Vec<[usize; 3]>,
}

/// One horizontal slice: per ray angle, the material intervals [a, b] along the ray (sorted).
pub struct Level {
    pub z: f64,
    pub rays: Vec<Vec<(f64, f64)>>,
}

pub struct Profile {
    pub centre: [f64; 2],
    pub zmin: f64,
    pub zmax: f64,
    pub n_angles: usize,
    pub levels: Vec<Level>,
}

impl Level {
    /// Outermost wall at each angle that sees material: (a, b), a = 0 when the ray starts inside.
    fn outer(&self) -> impl Iterator<Item = (usize, (f64, f64))> + '_ {
        self.rays.iter().enumerate().filter_map(|(i, iv)| iv.last().map(|w| (i, *w)))
    }
    /// Smallest outer radius over all angles (the jacket must stay outside it... or inside the wall).
    pub fn r_out_min(&self) -> Option<f64> {
        self.outer().map(|(_, w)| w.1).reduce(f64::min)
    }
    /// Largest inner radius of the outer wall (the plug reaches this far).
    pub fn r_in_max(&self) -> Option<f64> {
        self.outer().map(|(_, w)| w.0).reduce(f64::max)
    }
    pub fn r_max(&self) -> Option<f64> {
        self.outer().map(|(_, w)| w.1).reduce(f64::max)
    }
    /// Is radius r at ray i inside the part?
    pub fn solid_at(&self, i: usize, r: f64) -> bool {
        self.rays[i].iter().any(|&(a, b)| r > a && r < b)
    }
}

pub fn angle_of(i: usize, n: usize) -> f64 {
    // A small offset keeps rays off exact mesh vertices on axis-aligned parts.
    (i as f64 + 0.137) * std::f64::consts::TAU / n as f64
}

/// Slice the mesh at `n_levels` heights between zmin and zmax and cast `n_angles` rays per slice.
pub fn profile(m: &Mesh, centre: [f64; 2], zmin: f64, zmax: f64, n_levels: usize, n_angles: usize) -> Profile {
    let h = zmax - zmin;
    let eps = h * 1e-4 + 1e-6;
    let mut levels = Vec::with_capacity(n_levels);
    // Triangle z ranges once.
    let ranges: Vec<(f64, f64)> = m
        .tri
        .iter()
        .map(|t| {
            let zs = [m.pos[t[0]][2], m.pos[t[1]][2], m.pos[t[2]][2]];
            (zs.iter().cloned().fold(f64::MAX, f64::min), zs.iter().cloned().fold(f64::MIN, f64::max))
        })
        .collect();
    let dirs: Vec<[f64; 2]> = (0..n_angles).map(|i| {
        let a = angle_of(i, n_angles);
        [a.cos(), a.sin()]
    }).collect();
    for k in 0..n_levels {
        // Levels sit strictly inside (zmin, zmax), nudged off round numbers.
        let z = zmin + eps + (h - 2.0 * eps) * (k as f64 + 0.5) / n_levels as f64 + eps * 0.37;
        let mut segs: Vec<([f64; 2], [f64; 2])> = Vec::new();
        for (ti, t) in m.tri.iter().enumerate() {
            let (lo, hi) = ranges[ti];
            if !(lo < z && hi > z) {
                continue;
            }
            let p = [m.pos[t[0]], m.pos[t[1]], m.pos[t[2]]];
            let mut pts: Vec<[f64; 2]> = Vec::with_capacity(2);
            for e in 0..3 {
                let a = p[e];
                let b = p[(e + 1) % 3];
                if (a[2] - z) * (b[2] - z) < 0.0 {
                    let s = (z - a[2]) / (b[2] - a[2]);
                    pts.push([a[0] + s * (b[0] - a[0]) - centre[0], a[1] + s * (b[1] - a[1]) - centre[1]]);
                }
            }
            if pts.len() == 2 {
                segs.push((pts[0], pts[1]));
            }
        }
        let mut rays = Vec::with_capacity(n_angles);
        for d in &dirs {
            let mut ts: Vec<f64> = Vec::new();
            for (p, q) in &segs {
                // Solve p + s (q - p) = t d, s in [0, 1), t > 0.
                let e = [q[0] - p[0], q[1] - p[1]];
                let den = e[0] * d[1] - e[1] * d[0];
                if den.abs() < 1e-12 {
                    continue;
                }
                let s = (p[1] * d[0] - p[0] * d[1]) / den;
                if !(0.0..1.0).contains(&s) {
                    continue;
                }
                let x = [p[0] + s * e[0], p[1] + s * e[1]];
                let t = x[0] * d[0] + x[1] * d[1];
                if t > 1e-9 {
                    ts.push(t);
                }
            }
            ts.sort_by(|a, b| a.partial_cmp(b).unwrap());
            ts.dedup_by(|a, b| (*a - *b).abs() < 1e-7);
            let mut iv = Vec::new();
            let mut i = 0;
            if ts.len() % 2 == 1 {
                iv.push((0.0, ts[0]));
                i = 1;
            }
            while i + 1 < ts.len() {
                iv.push((ts[i], ts[i + 1]));
                i += 2;
            }
            rays.push(iv);
        }
        levels.push(Level { z, rays });
    }
    Profile { centre, zmin, zmax, n_angles, levels }
}

/// A band of the jacket split: below `z_hi` (from the previous band's top) the jacket is r >= `r`.
#[derive(Clone, Debug, PartialEq)]
pub struct Band {
    pub z_hi: f64,
    pub r: f64,
}

/// Where the jacket (radial pull) separates from the plug (Z pull), up to `z_split`.
/// At each level the split radius must sit inside the part's outer wall, `margin` clear of both
/// faces. Bands change radius only at a level where both radii are valid, so the step's ledge
/// runs inside the part too.
pub fn jacket_bands(p: &Profile, z_split: f64, margin: f64) -> Result<Vec<Band>, String> {
    let mut iv: Vec<(f64, f64, f64)> = Vec::new(); // (z, lo, hi)
    for l in &p.levels {
        if l.z >= z_split {
            break;
        }
        if let (Some(rin), Some(rout)) = (l.r_in_max(), l.r_out_min()) {
            iv.push((l.z, rin + margin, rout - margin));
        }
    }
    if iv.is_empty() {
        return Err("Round mould couldn't find the part's outside wall. Stand the part on its open face (the face you trowel), \
                    or select that face before running."
            .into());
    }
    let mut bands = Vec::new();
    let mut start = 0usize;
    while start < iv.len() {
        let (mut lo, mut hi) = (iv[start].1, iv[start].2);
        let not_round = |k: usize| {
            format!(
                "This part isn't round enough for Round mould: {} its outside isn't round, so the jacket can't pull off \
                 sideways. Use Plugins › Box mould… for parts with flat sides.",
                if iv[k].0 - p.zmin < 1.0 { "right at the open face".to_string() } else { format!("about {:.0} mm up from the open face", iv[k].0 - p.zmin) }
            )
        };
        if let Some(k) = (start..iv.len()).find(|&k| iv[k].1 > iv[k].2) {
            if k == start {
                return Err(not_round(k));
            }
        }
        let mut end = start;
        while end + 1 < iv.len() {
            let (nlo, nhi) = (lo.max(iv[end + 1].1), hi.min(iv[end + 1].2));
            if nlo > nhi {
                break;
            }
            lo = nlo;
            hi = nhi;
            end += 1;
        }
        let r = 0.5 * (lo + hi);
        if end + 1 >= iv.len() {
            bands.push(Band { z_hi: z_split, r });
            break;
        }
        // The next band starts at this band's last level, so the ledge lies where both are valid.
        bands.push(Band { z_hi: iv[end].0, r });
        if end == start {
            // A one-level band that cannot overlap the next: the wall jumps between two slices.
            let (nlo, nhi) = (iv[end].1.max(iv[end + 1].1), iv[end].2.min(iv[end + 1].2));
            if nlo > nhi {
                if iv[end + 1].1 > iv[end + 1].2 {
                    return Err(not_round(end + 1));
                }
                return Err(format!(
                    "The part's outside steps in or out too sharply about {:.0} mm up from the open face for Round mould's \
                     jacket to pull off. Soften that step with a chamfer or draft, or use Plugins › Box mould….",
                    iv[end].0 - p.zmin
                ));
            }
            start = end + 1;
        } else {
            start = end;
        }
    }
    Ok(bands)
}

/// Heights (from the part's bottom) where the plug would lock in: the cavity gets narrower going
/// up at some angle, so the plug cannot lift out along +Z.
/// `skip` lists (angle deg, half-width deg, z0, z1) zones to ignore, such as a cored side hole,
/// whose edge would otherwise read as a narrowing. A narrowing must show on at least two rays.
pub fn plug_undercuts(p: &Profile, z_split: f64, tol: f64, skip: &[(f64, f64, f64, f64)]) -> Vec<(f64, f64)> {
    let mut found: Vec<(f64, f64)> = Vec::new();
    let skipped = |i: usize, z: f64| {
        let deg = angle_of(i, p.n_angles).to_degrees();
        skip.iter().any(|&(c, hw, z0, z1)| {
            let dd = (deg - c + 540.0).rem_euclid(360.0) - 180.0;
            dd.abs() <= hw && z >= z0 && z <= z1
        })
    };
    for w in p.levels.windows(2) {
        let (a, b) = (&w[0], &w[1]);
        if b.z >= z_split {
            break;
        }
        let mut hits = 0;
        for i in 0..p.n_angles {
            if skipped(i, a.z) || skipped(i, b.z) {
                continue;
            }
            let (Some(wa), Some(wb)) = (a.rays[i].last(), b.rays[i].last()) else { continue };
            if wa.0 > 0.0 && wb.0 > 0.0 && wb.0 < wa.0 - tol {
                hits += 1;
            }
        }
        if hits >= 2 {
            match found.last_mut() {
                Some(last) if (a.z - p.zmin - last.1).abs() < 1e-6 + (b.z - a.z) * 1.01 => last.1 = b.z - p.zmin,
                _ => found.push((a.z - p.zmin, b.z - p.zmin)),
            }
        }
    }
    found
}

/// Largest radius the part reaches.
pub fn r_max(p: &Profile) -> f64 {
    p.levels.iter().filter_map(Level::r_max).fold(0.0, f64::max)
}

/// Mould space inside the plug region (below the jacket split, r under the split radius), per
/// slice: the union over all angles of the radii where there is no part. Built once; plate ring
/// heights are read from it many times while the layout is chosen.
pub struct Space {
    levels: Vec<(f64, Vec<(f64, f64)>)>,
    top: f64,
}

impl Space {
    pub fn new(p: &Profile, bands: &[Band]) -> Space {
        let mut levels = Vec::new();
        for l in &p.levels {
            let Some(rs) = bands.iter().find(|b| l.z < b.z_hi).map(|b| b.r) else { break };
            let mut free: Vec<(f64, f64)> = Vec::new();
            for ray in &l.rays {
                let mut at = 0.5;
                for &(a, b) in ray {
                    if a > at {
                        free.push((at, a.min(rs)));
                    }
                    at = at.max(b);
                    if at >= rs {
                        break;
                    }
                }
                if at < rs {
                    free.push((at, rs));
                }
            }
            free.retain(|(a, b)| b > a);
            free.sort_by(|x, y| x.0.partial_cmp(&y.0).unwrap());
            let mut merged: Vec<(f64, f64)> = Vec::new();
            for (a, b) in free {
                match merged.last_mut() {
                    Some(m) if a <= m.1 => m.1 = m.1.max(b),
                    _ => merged.push((a, b)),
                }
            }
            levels.push((l.z, merged));
        }
        Space { levels, top: bands.last().map(|b| b.z_hi).unwrap_or(p.zmax) }
    }

    /// The lowest height with mould space between r1 and r2: a plate piece spanning r1..r2
    /// reaches down to here.
    pub fn lowest(&self, r1: f64, r2: f64) -> f64 {
        for (z, free) in &self.levels {
            if free.iter().any(|&(a, b)| a < r2 && b > r1) {
                return *z;
            }
        }
        self.top
    }
}

/// Inner face of the outer wall near angle `deg` between heights z0 and z1 (absolute), looking
/// either side of the angle so a hole there does not hide the wall.
pub fn wall_inner_near(p: &Profile, deg: f64, z0: f64, z1: f64, side_deg: f64) -> Option<f64> {
    let n = p.n_angles;
    let mut best: Option<f64> = None;
    for l in p.levels.iter().filter(|l| l.z >= z0 && l.z <= z1) {
        for off in [-side_deg, side_deg] {
            let a = (deg + off).rem_euclid(360.0);
            let i = ((a / 360.0) * n as f64).round() as usize % n;
            if let Some(w) = l.rays[i].last() {
                best = Some(best.map_or(w.0, |b: f64| b.min(w.0)));
            }
        }
    }
    best
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A revolved polygon (r, z) as a closed mesh with `seg` segments around.
    pub fn revolve(profile: &[(f64, f64)], seg: usize) -> Mesh {
        let mut pos = Vec::new();
        for k in 0..seg {
            let a = k as f64 * std::f64::consts::TAU / seg as f64;
            for &(r, z) in profile {
                pos.push([r * a.cos(), r * a.sin(), z]);
            }
        }
        let n = profile.len();
        let mut tri = Vec::new();
        for k in 0..seg {
            let k2 = (k + 1) % seg;
            for j in 0..n {
                let j2 = (j + 1) % n;
                let (a, b, c, d) = (k * n + j, k * n + j2, k2 * n + j, k2 * n + j2);
                tri.push([a, b, d]);
                tri.push([a, d, c]);
            }
        }
        Mesh { pos, tri }
    }

    /// Bird-pool-like bowl: floor 40, wall from r 450 to 500, 240 tall, open top.
    fn bowl() -> Mesh {
        revolve(&[(0.0, 0.0), (500.0, 0.0), (500.0, 240.0), (450.0, 240.0), (450.0, 40.0), (0.0, 40.0)], 96)
    }

    #[test]
    fn bowl_profile_has_a_wall_and_a_floor() {
        let p = profile(&bowl(), [0.0, 0.0], 0.0, 240.0, 60, 90);
        let floor = &p.levels[2]; // z about 10
        assert!(floor.r_in_max().unwrap() < 1e-6, "the floor is solid from the axis");
        let wall = &p.levels[40];
        let rin = wall.r_in_max().unwrap();
        let rout = wall.r_out_min().unwrap();
        assert!((rin - 450.0).abs() < 1.0 && rout > 498.0 && rout <= 500.0 + 1e-6, "{rin} {rout}");
        assert!((r_max(&p) - 500.0).abs() < 0.1);
    }

    #[test]
    fn bowl_needs_one_band_between_wall_faces() {
        let p = profile(&bowl(), [0.0, 0.0], 0.0, 240.0, 60, 90);
        let b = jacket_bands(&p, 239.5, 3.0).unwrap();
        assert_eq!(b.len(), 1);
        assert!(b[0].r > 453.0 && b[0].r < 497.0, "{:?}", b);
        assert!(plug_undercuts(&p, 239.5, 1.0, &[]).is_empty());
    }

    #[test]
    fn a_wall_that_moves_out_gets_a_stepped_split() {
        // Wall r 450..470 below z 100, r 520..560 above: no single radius fits.
        let m = revolve(
            &[(0.0, 0.0), (470.0, 0.0), (470.0, 100.0), (560.0, 100.0), (560.0, 200.0), (520.0, 200.0),
              (520.0, 110.0), (450.0, 110.0), (450.0, 40.0), (0.0, 40.0)],
            128,
        );
        let p = profile(&m, [0.0, 0.0], 0.0, 200.0, 100, 90);
        let b = jacket_bands(&p, 199.5, 2.0).unwrap();
        assert_eq!(b.len(), 2, "{:?}", b);
        assert!(b[0].r < 470.0 && b[1].r > 520.0);
        // The ledge sits where the part is solid from 450 to 560 (z 100..110).
        assert!(b[0].z_hi > 100.0 && b[0].z_hi < 110.0, "{:?}", b);
    }

    #[test]
    fn a_cavity_narrowing_upwards_is_an_undercut() {
        // Cavity wider at the bottom (r 450) than the top (r 400).
        let m = revolve(&[(0.0, 0.0), (500.0, 0.0), (500.0, 200.0), (400.0, 200.0), (450.0, 40.0), (0.0, 40.0)], 96);
        let p = profile(&m, [0.0, 0.0], 0.0, 200.0, 50, 60);
        assert!(!plug_undercuts(&p, 199.5, 1.0, &[]).is_empty());
    }

    #[test]
    fn plate_reaches_down_to_the_bowl_floor() {
        let p = profile(&bowl(), [0.0, 0.0], 0.0, 240.0, 120, 60);
        let b = jacket_bands(&p, 239.5, 3.0).unwrap();
        let z = Space::new(&p, &b).lowest(0.0, 300.0);
        assert!((z - 40.0).abs() < 2.5, "{z}");
    }
}
