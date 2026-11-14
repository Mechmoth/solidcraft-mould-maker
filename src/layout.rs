//! How the mould is cut into printable pieces: sector counts, ring radii, and whether each piece
//! fits the printer, upright and turned on the bed, or laid on its side.

/// The printer's build volume in mm.
#[derive(Clone, Copy, Debug)]
pub struct Bed {
    pub x: f64,
    pub y: f64,
    pub z: f64,
}

/// One kind of piece: an annular sector r1..r2 spanning `span` degrees, `h` tall.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Piece {
    pub r1: f64,
    pub r2: f64,
    pub span: f64,
    pub h: f64,
}

/// The smallest footprint over turns on the bed: (w, d) with the turn in degrees.
fn footprint(p: &Piece) -> Vec<(f64, f64, f64)> {
    let n = 16;
    let half = p.span.to_radians() / 2.0;
    let mut pts: Vec<[f64; 2]> = Vec::new();
    for k in 0..=n {
        let a = -half + 2.0 * half * k as f64 / n as f64;
        pts.push([p.r2 * a.cos(), p.r2 * a.sin()]);
        pts.push([p.r1 * a.cos(), p.r1 * a.sin()]);
    }
    if p.span >= 359.0 {
        // A whole ring or disk: a circle of r2.
        return vec![(2.0 * p.r2, 2.0 * p.r2, 0.0)];
    }
    (0..90)
        .map(|k| {
            let deg = 2 * k;
            let t = (deg as f64).to_radians();
            let (c, s) = (t.cos(), t.sin());
            let (mut x0, mut x1, mut y0, mut y1) = (f64::MAX, f64::MIN, f64::MAX, f64::MIN);
            for q in &pts {
                let x = q[0] * c - q[1] * s;
                let y = q[0] * s + q[1] * c;
                x0 = x0.min(x);
                x1 = x1.max(x);
                y0 = y0.min(y);
                y1 = y1.max(y);
            }
            (x1 - x0, y1 - y0, deg as f64)
        })
        .collect()
}

/// How the piece fits the bed, or None.
pub fn fit(p: &Piece, bed: &Bed) -> Option<String> {
    let fps = footprint(p);
    if p.h <= bed.z {
        if let Some((_, _, deg)) = fps.iter().find(|(w, d, _)| *w <= bed.x && *d <= bed.y) {
            return Some(if *deg == 0.0 { "upright".into() } else { format!("upright, turned {deg:.0}°") });
        }
    }
    // On its side: treat the piece as its smallest box.
    let (w, d, _) = fps.iter().cloned().min_by(|a, b| (a.0 * a.1).partial_cmp(&(b.0 * b.1)).unwrap())?;
    let mut dims = [w, d, p.h];
    dims.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let mut b = [bed.x, bed.y, bed.z];
    b.sort_by(|a, b| a.partial_cmp(b).unwrap());
    if dims.iter().zip(b.iter()).all(|(x, y)| x <= y) {
        return Some("on its side".into());
    }
    None
}

/// The mould's size, which the layout cuts up.
pub struct Envelope<'a> {
    /// Inner edge of the jacket pieces (the smallest jacket split radius).
    pub r_jacket: f64,
    /// Outer edge of the tabs.
    pub ro: f64,
    pub jacket_h: f64,
    /// Height of a plate piece spanning r1..r2.
    pub plate_h: Box<dyn Fn(f64, f64) -> f64 + 'a>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Layout {
    /// Sectors of the jacket and every plate ring.
    pub sectors: usize,
    /// Ring split radii, outermost first.
    pub rings: Vec<f64>,
    /// Sectors of the innermost disk (divides `sectors`).
    pub disk_sectors: usize,
    pub pieces: usize,
}

/// Cut the plate into rings that fit, outermost first, then a centre disk.
pub fn rings_for(env: &Envelope, n: usize, bed: &Bed) -> Option<(Vec<f64>, usize)> {
    let span = 360.0 / n as f64;
    let mut r2 = env.ro;
    let mut rings = Vec::new();
    for _ in 0..6 {
        // A centre disk with the fewest sectors that divide n.
        for d in (1..=n).filter(|d| n % d == 0) {
            let p = Piece { r1: 0.0, r2, span: 360.0 / d as f64, h: (env.plate_h)(0.0, r2) };
            if fit(&p, bed).is_some() {
                return Some((rings, d));
            }
        }
        // Otherwise the widest ring that fits (binary search; a narrower ring always fits if a
        // wider one does). Radii sit on x.5 mm so they miss round-number faces.
        let grid = |k: usize| ((r2 - 20.0) / 5.0).floor() * 5.0 + 2.5 - 5.0 * k as f64;
        let fits_k = |k: usize| {
            let r1 = grid(k);
            r1 > 10.0 && fit(&Piece { r1, r2, span, h: (env.plate_h)(r1, r2) }, bed).is_some()
        };
        let best = if !fits_k(0) {
            None
        } else {
            let (mut lo, mut hi) = (0usize, ((r2 - 30.0) / 5.0).max(1.0) as usize);
            while lo < hi {
                let mid = (lo + hi + 1) / 2;
                if fits_k(mid) { lo = mid } else { hi = mid - 1 }
            }
            Some(grid(lo))
        };
        rings.push(best?);
        r2 = best?;
    }
    None
}

/// The layout with the fewest pieces (fewer sectors on a tie). `forced` fixes the sector count.
pub fn choose(env: &Envelope, bed: &Bed, forced: Option<usize>) -> Result<Layout, String> {
    let candidates: Vec<usize> = match forced {
        Some(n) => vec![n],
        None => (2..=64).collect(),
    };
    let mut best: Option<Layout> = None;
    for n in candidates {
        if best.as_ref().is_some_and(|b| n + 1 >= b.pieces) {
            break;
        }
        let jacket = Piece { r1: env.r_jacket, r2: env.ro, span: 360.0 / n as f64, h: env.jacket_h };
        if fit(&jacket, bed).is_none() {
            continue;
        }
        let Some((rings, d)) = rings_for(env, n, bed) else { continue };
        let pieces = n * (1 + rings.len()) + d;
        let l = Layout { sectors: n, rings, disk_sectors: d, pieces };
        if best.as_ref().map_or(true, |b| l.pieces < b.pieces) {
            best = Some(l);
        }
    }
    best.ok_or_else(|| match forced {
        Some(n) => format!("with {n} sectors some pieces do not fit the printer; set sectors to 0 to choose automatically"),
        None => "no layout of up to 64 sectors fits this printer; the part is too tall or the bed too small".into(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bird_pool() -> Envelope<'static> {
        Envelope { r_jacket: 487.0, ro: 592.5, jacket_h: 239.5, plate_h: Box::new(|_, _| 245.0) }
    }

    #[test]
    fn a_45_degree_wedge_of_310_fits_300_turned() {
        let bed = Bed { x: 300.0, y: 300.0, z: 300.0 };
        let p = Piece { r1: 0.0, r2: 310.0, span: 45.0, h: 245.0 };
        assert!(fit(&p, &bed).unwrap().contains("turned"));
        assert!(fit(&Piece { r1: 0.0, r2: 420.0, span: 45.0, h: 245.0 }, &bed).is_none());
    }

    #[test]
    fn bird_pool_on_a_k1_max() {
        let bed = Bed { x: 300.0, y: 300.0, z: 300.0 };
        let l = choose(&bird_pool(), &bed, None).unwrap();
        assert!(l.sectors >= 14 && l.sectors <= 20, "{l:?}");
        assert_eq!(l.rings.len(), 1, "{l:?}");
        assert!(l.pieces <= 48, "{l:?}");
        // Every piece of the layout fits.
        let env = bird_pool();
        let span = 360.0 / l.sectors as f64;
        assert!(fit(&Piece { r1: env.r_jacket, r2: env.ro, span, h: env.jacket_h }, &bed).is_some());
        assert!(fit(&Piece { r1: l.rings[0], r2: env.ro, span, h: 245.0 }, &bed).is_some());
        assert!(fit(&Piece { r1: 0.0, r2: l.rings[0], span: 360.0 / l.disk_sectors as f64, h: 245.0 }, &bed).is_some());
    }

    #[test]
    fn a_small_part_is_one_ring_with_few_pieces() {
        let bed = Bed { x: 300.0, y: 300.0, z: 300.0 };
        let env = Envelope { r_jacket: 60.0, ro: 110.0, jacket_h: 120.0, plate_h: Box::new(|_, _| 140.0) };
        let l = choose(&env, &bed, None).unwrap();
        assert_eq!((l.sectors, l.rings.len(), l.disk_sectors), (2, 0, 1), "{l:?}");
    }

    #[test]
    fn a_forced_count_that_cannot_fit_says_so() {
        let bed = Bed { x: 300.0, y: 300.0, z: 300.0 };
        assert!(choose(&bird_pool(), &bed, Some(4)).is_err());
    }
}
