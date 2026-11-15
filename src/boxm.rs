//! Box mould: four walls that pull straight off sideways and a back that lifts straight off,
//! split on mitres, with a bolting ear along every joint.
//!
//! In the working frame the open face is the part's lowest face (z = z0) and the mould's outer
//! box runs `wall` beyond the part's bounding box on the four sides and `back_plate` above it.
//! Each point of the mould belongs to the piece whose face of the outer box it is relatively
//! nearest to, measured in wall thicknesses: f(-Y) = (y0 - y) / wall, ... f(+Z) = (z - z1) /
//! back_plate. The pieces are then plain shapes: the back is the outer box's top face extruded
//! down with a taper, the walls are the four regions of the outer box's straight skeleton
//! extruded up. A joint between two pieces is the plane where their scores are equal: a mitre
//! through the outer box's edge and the part's bounding box edge. Each joint gets an ear: a
//! slab in the mitre plane, outside the mould, split down the middle, with bolt and dowel holes
//! through it.
//!
//! Before touching the design the plugin checks, on a mesh of the part, that every face can let
//! go of the piece that moulds it: the piece's pull direction must not point into the face, and
//! nothing of the part may lie in the way.

use serde_json::{json, Value};

use crate::build::{between, created, spread};
use crate::geom::{self, add, cross, dot, mul, sub, unit, AxisRays, Rot, V3};
use crate::{bolt_clearance, round1, Params, Prep, Runner, HOST_CALLS};

const NAMES: [&str; 5] = ["Mould wall 1", "Mould wall 2", "Mould wall 3", "Mould wall 4", "Mould back"];
/// Pull directions in the working frame: walls -Y, +Y, -X, +X, then the back +Z.
const DIRS: [V3; 5] = [[0.0, -1.0, 0.0], [0.0, 1.0, 0.0], [-1.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 0.0, 1.0]];

/// The mould's geometry in the working frame.
#[derive(Clone, Debug)]
pub struct Frame {
    /// The part's bounding box.
    pub lo: V3,
    pub hi: V3,
    pub wall: f64,
    pub plate: f64,
}

impl Frame {
    /// Outer box: x0, x1, y0, y1, z0 (the open face), z1 (back of the back plate).
    pub fn outer(&self) -> [f64; 6] {
        [self.lo[0] - self.wall, self.hi[0] + self.wall, self.lo[1] - self.wall, self.hi[1] + self.wall, self.lo[2], self.hi[2] + self.plate]
    }
    pub fn score(&self, k: usize, p: V3) -> f64 {
        match k {
            0 => (self.lo[1] - p[1]) / self.wall,
            1 => (p[1] - self.hi[1]) / self.wall,
            2 => (self.lo[0] - p[0]) / self.wall,
            3 => (p[0] - self.hi[0]) / self.wall,
            _ => (p[2] - self.hi[2]) / self.plate,
        }
    }
    /// The piece a point of the mould belongs to.
    pub fn piece_of(&self, p: V3) -> usize {
        (0..5).max_by(|&a, &b| self.score(a, p).partial_cmp(&self.score(b, p)).unwrap()).unwrap()
    }
}

/// Where the part would lock a piece in: area and a point, in the working frame.
#[derive(Debug, Default)]
pub struct Check {
    pub locked_area: f64,
    pub worst: Option<(usize, V3)>,
    /// Area of the part's faces inside closed hollows of the open face (recesses).
    pub recess_area: f64,
}

/// Check every face of the part against the piece that moulds it.
pub fn check(mesh: &geom::Mesh, fr: &Frame, tol: f64) -> Check {
    let rays = AxisRays::new(mesh);
    let off = (1.5 * tol).max(0.3);
    let mut out = Check::default();
    let mut worst_area = 0.0;
    for t in 0..mesh.tri.len() {
        let (n, a) = mesh.normal_area(t);
        if a <= 0.0 {
            continue;
        }
        let c = mesh.centroid(t);
        // The open face itself.
        if n[2] < -0.95 && c[2] - fr.lo[2] < tol + 0.05 {
            continue;
        }
        let p = add(c, mul(n, off));
        // Shut in on every side but the open face: a recess in the open face, which the open
        // face's trowelling fills flat. Nothing of the mould goes there.
        if !rays.hits(p, 2, -1.0, 0.0) && rays.hits(p, 2, 1.0, 0.0) && rays.hits(p, 0, 1.0, 0.0) && rays.hits(p, 0, -1.0, 0.0) && rays.hits(p, 1, 1.0, 0.0) && rays.hits(p, 1, -1.0, 0.0) {
            out.recess_area += a;
            continue;
        }
        let k = fr.piece_of(p);
        let d = DIRS[k];
        let axis = if k < 2 { 1 } else if k < 4 { 0 } else { 2 };
        let sign = d[axis];
        // About 2° of negative draft passes: the mesh is coarse, and flat faces square to the pull
        // read a hair either way.
        if dot(n, d) < -0.035 || rays.hits(p, axis, sign, 0.0) {
            out.locked_area += a;
            if a > worst_area {
                worst_area = a;
                out.worst = Some((k, c));
            }
        }
    }
    out
}

/// The joints: (start on the outer edge, along the edge, outward in the mitre plane, length).
fn joints(fr: &Frame) -> Vec<(V3, V3, V3, f64, f64)> {
    let [x0, x1, y0, y1, z0, z1] = fr.outer();
    let (w, pl) = (fr.wall, fr.plate);
    let mut v = Vec::new();
    // Vertical corners, from the open face up.
    for (cx, sx) in [(x0, -1.0), (x1, 1.0)] {
        for (cy, sy) in [(y0, -1.0), (y1, 1.0)] {
            v.push(([cx, cy, z0], [0.0, 0.0, 1.0], unit([sx, sy, 0.0]), 0.0, z1 - z0));
        }
    }
    // Top edges, where the walls meet the back.
    v.push(([x0, y0, z1], [1.0, 0.0, 0.0], unit([0.0, -w, pl]), 0.0, x1 - x0));
    v.push(([x0, y1, z1], [1.0, 0.0, 0.0], unit([0.0, w, pl]), 0.0, x1 - x0));
    v.push(([x0, y0, z1], [0.0, 1.0, 0.0], unit([-w, 0.0, pl]), 0.0, y1 - y0));
    v.push(([x1, y0, z1], [0.0, 1.0, 0.0], unit([w, 0.0, pl]), 0.0, y1 - y0));
    v
}

pub(crate) struct Bx<'a, R: Runner> {
    pub(crate) r: &'a mut R,
    pub(crate) calls: usize,
    pub(crate) k: usize,
}

/// An ear's size: half thickness, width out from the mould, how far it reaches into the wall,
/// gap left at the split, and its holes.
pub(crate) struct EarSpec {
    pub f: f64,
    pub fw: f64,
    pub ov: f64,
    pub eps: f64,
    pub bolt_d: f64,
    pub dowel_d: f64,
    /// A square block this size in the corner where each half meets the wall, so the ear is
    /// held by more than its edge. 0 for none.
    pub root: f64,
}

impl<R: Runner> Bx<'_, R> {
    /// A bolting ear on a joint: two half slabs, one each side of the joint plane, drilled
    /// together and placed. `o` is a point on the joint's outer edge, `e` runs along the edge, `u`
    /// points out of the mould in the joint plane; the first half is on the `u × e` side.
    /// Returns the bodies of each half (slab first, then its root block) and the bolt and dowel
    /// counts.
    pub(crate) fn ear(&mut self, s: &EarSpec, o: V3, e: V3, u: V3, b0: f64, b1: f64, row: &[(bool, f64)]) -> Result<(Vec<String>, Vec<String>, usize, usize), String> {
        let (f, fw, ov, eps) = (s.f, s.fw, s.ov, s.eps);
        let n = cross(u, e);
        let ha = self.first("solid.box", json!({"corner": [-ov, b0, eps], "length": fw + ov, "width": b1 - b0, "height": f - eps, "body_name": "Ear"}))?;
        let hb = self.first("solid.box", json!({"corner": [-ov, b0, -f], "length": fw + ov, "width": b1 - b0, "height": f - eps, "body_name": "Ear"}))?;
        let mut drills = Vec::new();
        let (mut bolts, mut dowels) = (0, 0);
        for (is_bolt, at) in row {
            let dia = if *is_bolt { s.bolt_d } else { s.dowel_d };
            if dia <= 0.0 {
                continue;
            }
            drills.push(self.first(
                "solid.cylinder",
                json!({"diameter": dia, "height": 2.0 * f + 6.0, "base": [0.5 * fw, at, -f - 3.0], "axis": [0, 0, 1], "body_name": "Drill"}),
            )?);
            if *is_bolt {
                bolts += 1;
            } else {
                dowels += 1;
            }
        }
        if !drills.is_empty() {
            if drills.len() > 1 {
                self.run("solid.combine", json!({"target": drills[0], "tools": drills[1..], "operation": "join", "touching": "one_body"}))?;
            }
            self.combine(&ha, &[drills[0].clone()], "cut", true)?;
            self.combine(&hb, &[drills[0].clone()], "cut", false)?;
        }
        let (mut a, mut bb) = (vec![ha], vec![hb]);
        if s.root > 0.0 {
            let ri = 1.0_f64.min(ov);
            a.push(self.first("solid.box", json!({"corner": [-ri, b0, f - 0.5], "length": s.root + ri, "width": b1 - b0, "height": s.root + 0.5, "body_name": "Ear root"}))?);
            bb.push(self.first("solid.box", json!({"corner": [-ri, b0, -f - s.root], "length": s.root + ri, "width": b1 - b0, "height": s.root + 0.5, "body_name": "Ear root"}))?);
        }
        let rot = Rot::from_columns(u, e, n);
        let all: Vec<&String> = a.iter().chain(bb.iter()).collect();
        let mut mv = json!({"bodies": all, "translate": o});
        if !rot.is_none() {
            mv["axis"] = json!(rot.axis);
            mv["angle"] = json!(format!("{} deg", rot.deg));
            mv["origin"] = json!([0, 0, 0]);
        }
        self.run("solid.move", mv)?;
        Ok((a, bb, bolts, dowels))
    }

    pub(crate) fn run(&mut self, cmd: &str, args: Value) -> Result<Value, String> {
        self.calls += 1;
        self.r.run(cmd, args)
    }
    pub(crate) fn first(&mut self, cmd: &str, mut args: Value) -> Result<String, String> {
        if let Some(n) = args.get("body_name").and_then(Value::as_str).map(String::from) {
            self.k += 1;
            args["body_name"] = json!(format!("{n} mm{}", self.k));
        }
        let v = self.run(cmd, args)?;
        created(&v).into_iter().next().ok_or_else(|| format!("{cmd} made no body"))
    }
    pub(crate) fn named(&mut self, cmd: &str, args: Value) -> Result<String, String> {
        let v = self.run(cmd, args)?;
        created(&v).into_iter().next().ok_or_else(|| format!("{cmd} made no body"))
    }
    pub(crate) fn combine(&mut self, target: &str, tools: &[String], op: &str, keep: bool) -> Result<Vec<String>, String> {
        let v = self.run("solid.combine", json!({"target": target, "tools": tools, "operation": op, "keep_tools": keep}))?;
        Ok(created(&v))
    }
}

fn dir_word(d: V3) -> String {
    let i = (0..3).max_by(|&a, &b| d[a].abs().partial_cmp(&d[b].abs()).unwrap()).unwrap();
    format!("{}{}", if d[i] < 0.0 { "-" } else { "+" }, ["X", "Y", "Z"][i])
}

pub fn build<R: Runner>(r: &mut R, p: &Params, prep: &Prep) -> Result<Value, String> {
    let fr = Frame { lo: prep.lo, hi: prep.hi, wall: p.wall, plate: p.back_plate };
    let [x0, x1, y0, y1, z0, ztop] = fr.outer();
    let f = p.flange_thickness / 2.0;
    let fw = p.flange_width;
    let ov = 2.0;
    let eps = 0.05;
    let bolt_d = bolt_clearance(&p.bolt).unwrap_or(0.0);
    let dowel_d = if p.dowel_diameter > 0.0 { p.dowel_diameter + 0.1 } else { 0.0 };

    // ---------- can every piece let go? ----------
    r.progress(0.05, "checking the part releases");
    let chk = check(&prep.mesh, &fr, prep.tol);
    if chk.locked_area > 20.0 {
        let (k, c) = chk.worst.unwrap();
        let at = prep.to_world(c);
        let pull = match &prep.turn {
            Some((rot, _)) => rot.inverse().apply(DIRS[k]),
            None => DIRS[k],
        };
        let piece = if k == 4 { "the back".to_string() } else { format!("the wall that pulls off toward {}", dir_word(pull)) };
        return Err(format!(
            "Box mould can't release the part near x {:.0}, y {:.0}, z {:.0} mm: a face there would lock {piece} in \
             (about {:.0} mm² of the part's faces would). Box mould pulls four walls straight off sideways and the back straight \
             off, so the part can't have overhangs or side holes across those directions. Remove the overhang, or try Round mould.",
            at[0],
            at[1],
            at[2],
            chk.locked_area
        ));
    }

    // ---------- names and calls ----------
    let existing = &prep.existing;
    let free = |base: &str| -> String {
        let clash = |c: &str| existing.iter().any(|e| e == c || e.starts_with(&format!("{c} (")));
        if !clash(base) {
            return base.to_string();
        }
        (2..).map(|k| format!("{base} #{k}")).find(|c| !clash(c)).unwrap()
    };
    let names: Vec<String> = NAMES.iter().map(|n| free(n)).collect();
    let js = joints(&fr);
    let rows: Vec<Vec<(bool, f64)>> = js
        .iter()
        .enumerate()
        .map(|(i, (_, _, _, _, len))| {
            let (b0, b1) = if i < 4 { (eps, len - f - 2.0) } else { (f + 2.0, len - f - 2.0) };
            let mut pos = spread(b0 + 15.0, b1 - 15.0, 150.0, 2);
            if pos.is_empty() {
                pos = vec![0.5 * (b0 + b1)];
            }
            let mut v: Vec<(bool, f64)> = if bolt_d > 0.0 { pos.iter().map(|b| (true, *b)).collect() } else { vec![] };
            if dowel_d > 0.0 {
                v.extend(between(&pos).into_iter().take(1).map(|b| (false, b)));
            }
            v
        })
        .collect();
    let holes: usize = rows.iter().map(Vec::len).sum();
    let planned = prep.calls + 26 + js.len() * 5 + holes + js.iter().filter(|_| holes > 0).count() + prep.finish_calls();
    if planned > HOST_CALLS {
        return Err(format!(
            "This box mould needs about {planned} SolidCraft commands, more than one plugin run may make ({HOST_CALLS}). \
             Use fewer holes: a bigger bolt size, or Dowel pins 0."
        ));
    }
    let mut b = Bx { r, calls: 0, k: 0 };
    let mut warnings: Vec<String> = Vec::new();

    // ---------- blank: outer box minus the part ----------
    b.r.progress(0.15, "mould blank");
    let blank = b.first("solid.box", json!({"corner": [x0, y0, z0], "length": x1 - x0, "width": y1 - y0, "height": ztop - z0, "body_name": "Mould blank"}))?;
    let hollows = b.combine(&blank, &[prep.part.clone()], "cut", true)?;
    if !hollows.is_empty() {
        b.run("solid.remove", json!({"bodies": hollows}))?;
        warnings.push(format!(
            "{} hollow{} in the open face (or closed void{}) can't be cast with an open mould: they come out solid. Drill or rout them afterwards.",
            hollows.len(),
            if hollows.len() == 1 { "" } else { "s" },
            if hollows.len() == 1 { "" } else { "s" },
        ));
    }
    let mut pieces: Vec<Vec<String>> = vec![Vec::new(); 5];

    // ---------- the back: the top face extruded down with a taper ----------
    b.r.progress(0.25, "back");
    b.k += 1;
    let sk_top = format!("Mould back outline mm{}", b.k);
    b.run("sketch.create", json!({"plane": "XY", "offset": ztop, "name": sk_top}))?;
    b.run("sketch.line", json!({"points": [[x0, y0], [x1, y0], [x1, y1], [x0, y1]], "closed": true}))?;
    b.run("sketch.finish", json!({}))?;
    let collapse = 0.5 * (x1 - x0).min(y1 - y0) * fr.plate / fr.wall - 0.5;
    let depth = collapse.min(ztop - z0 + 1.0);
    let taper = -(fr.wall / fr.plate).atan().to_degrees();
    let back = b.named(
        "solid.extrude",
        json!({"sketch": sk_top, "direction": "negative", "distance": depth, "taper": taper, "body_name": names[4]}),
    )?;
    let extra = b.combine(&back, &[blank.clone()], "intersect", true)?;
    pieces[4].push(back.clone());
    pieces[4].extend(extra);
    b.combine(&blank, &[back.clone()], "cut", true)?;

    // ---------- the walls: the outer box's straight skeleton extruded up ----------
    b.r.progress(0.35, "walls");
    let e = 5.0;
    let (a0, a1, c0, c1) = (x0 - e, x1 + e, y0 - e, y1 + e);
    let (wd, dp) = (a1 - a0, c1 - c0);
    let (xm, ym) = (0.5 * (a0 + a1), 0.5 * (c0 + c1));
    b.k += 1;
    let sk_side = format!("Mould wall outline mm{}", b.k);
    b.run("sketch.create", json!({"plane": "XY", "offset": z0 - 1.0, "name": sk_side}))?;
    b.run("sketch.line", json!({"points": [[a0, c0], [a1, c0], [a1, c1], [a0, c1]], "closed": true}))?;
    if wd <= dp {
        let (lo_, hi_) = ([xm, c0 + wd / 2.0], [xm, c1 - wd / 2.0]);
        b.run("sketch.line", json!({"points": [[a0, c0], lo_, [a1, c0]]}))?;
        b.run("sketch.line", json!({"points": [[a0, c1], hi_, [a1, c1]]}))?;
        if hi_[1] - lo_[1] > 0.01 {
            b.run("sketch.line", json!({"points": [lo_, hi_]}))?;
        }
    } else {
        let (l_, r_) = ([a0 + dp / 2.0, ym], [a1 - dp / 2.0, ym]);
        b.run("sketch.line", json!({"points": [[a0, c0], l_, [a0, c1]]}))?;
        b.run("sketch.line", json!({"points": [[a1, c0], r_, [a1, c1]]}))?;
        if r_[0] - l_[0] > 0.01 {
            b.run("sketch.line", json!({"points": [l_, r_]}))?;
        }
    }
    b.run("sketch.finish", json!({}))?;
    let picks = [[xm, c0 + 1.0], [xm, c1 - 1.0], [a0 + 1.0, ym], [a1 - 1.0, ym]];
    for k in 0..4 {
        let wall = b.named(
            "solid.extrude",
            json!({"sketch": sk_side, "profiles": [{"point": picks[k]}], "distance": ztop - z0 + 2.0, "body_name": names[k]}),
        )?;
        let extra = b.combine(&wall, &[blank.clone()], "intersect", true)?;
        pieces[k].push(wall);
        pieces[k].extend(extra);
    }
    b.run("solid.remove", json!({"bodies": [blank]}))?;
    for (k, lumps) in pieces.iter().enumerate() {
        if lumps.len() > 1 {
            warnings.push(format!("{} came out in {} separate lumps; each is its own piece.", NAMES[k], lumps.len()));
        }
    }

    // ---------- ears ----------
    b.r.progress(0.5, "bolting ears");
    let mut ears: Vec<Vec<String>> = vec![Vec::new(); 5];
    let (mut bolts, mut dowels) = (0usize, 0usize);
    for (i, ((o, ed, u, _, len), row)) in js.iter().zip(&rows).enumerate() {
        let (b0, b1) = if i < 4 { (eps, len - f - 2.0) } else { (f + 2.0, len - f - 2.0) };
        let n = cross(*u, *ed);
        // Which piece is on which side of the mitre.
        let dl = 0.5 * fr.wall.min(fr.plate);
        let mid = add(*o, mul(*ed, 0.5 * (b0 + b1)));
        let inside = sub(mid, mul(*u, dl));
        let (ka, kb) = (fr.piece_of(add(inside, mul(n, 0.5 * dl))), fr.piece_of(sub(inside, mul(n, 0.5 * dl))));
        if ka == kb {
            return Err(format!("internal: joint {i} has {} on both sides", NAMES[ka]));
        }
        let es = EarSpec { f, fw, ov, eps, bolt_d, dowel_d, root: 0.0 };
        let (ha, hb, nb, nd) = b.ear(&es, *o, *ed, *u, b0, b1, row)?;
        let (ha, hb) = (ha[0].clone(), hb[0].clone());
        bolts += nb;
        dowels += nd;
        ears[ka].push(ha);
        ears[kb].push(hb);
    }

    // ---------- join each piece's ears ----------
    b.r.progress(0.75, "joining ears");
    for k in 0..5 {
        if !ears[k].is_empty() {
            b.combine(&pieces[k][0], &ears[k], "join", false)?;
        }
    }

    // ---------- does every piece fit the printer? ----------
    let all: Vec<String> = pieces.iter().flatten().cloned().collect();
    let m = b.run("inspect.measure", json!({"bodies": all}))?;
    let mut bed = [p.bed.x, p.bed.y, p.bed.z];
    bed.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let mut sizes = Vec::new();
    for body in m.get("bodies").and_then(Value::as_array).cloned().unwrap_or_default() {
        let name = body.get("name").and_then(Value::as_str).unwrap_or("").to_string();
        let bb = body.get("bbox");
        let lo = bb.and_then(|x| x.get("min")).and_then(Value::as_array).cloned().unwrap_or_default();
        let hi = bb.and_then(|x| x.get("max")).and_then(Value::as_array).cloned().unwrap_or_default();
        let mut d: Vec<f64> = (0..3).map(|i| hi.get(i).and_then(Value::as_f64).unwrap_or(0.0) - lo.get(i).and_then(Value::as_f64).unwrap_or(0.0)).collect();
        let shown = d.iter().map(|x| format!("{:.0}", x)).collect::<Vec<_>>().join(" × ");
        d.sort_by(|a, b| a.partial_cmp(b).unwrap());
        if d.iter().zip(&bed).any(|(x, y)| *x > *y + 0.01) {
            return Err(format!(
                "\"{name}\" would be {shown} mm, too big for your printer ({:.0} × {:.0} × {:.0} mm) in any direction. Box mould \
                 0.2 doesn't split walls into strips yet: choose a bigger printer (Custom), or use thinner Wall, Back plate or \
                 Flange width.",
                p.bed.x, p.bed.y, p.bed.z
            ));
        }
        sizes.push(json!({"piece": name, "size_mm": d.iter().map(|x| round1(*x)).collect::<Vec<_>>()}));
    }

    let calls = b.calls;
    prep.finish(r, &all)?;
    for w in &warnings {
        r.log(w);
    }
    Ok(json!({
        "part": prep.source,
        "pieces": all.len(),
        "walls": pieces[..4].iter().flatten().cloned().collect::<Vec<_>>(),
        "back": pieces[4],
        "sizes": sizes,
        "bolts": {"size": p.bolt, "count": bolts},
        "dowel_holes": {"diameter_mm": p.dowel_diameter, "count": dowels},
        "turned": prep.turn.is_some(),
        "recess_area_mm2": round1(chk.recess_area),
        "host_calls": calls + prep.calls + prep.finish_calls(),
        "warnings": warnings,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A closed box mesh from lo to hi, outward normals.
    fn box_mesh(lo: V3, hi: V3) -> geom::Mesh {
        let p = |i: usize| [if i & 1 == 0 { lo[0] } else { hi[0] }, if i & 2 == 0 { lo[1] } else { hi[1] }, if i & 4 == 0 { lo[2] } else { hi[2] }];
        let pos = (0..8).map(p).collect();
        let quads = [[0, 2, 3, 1], [4, 5, 7, 6], [0, 1, 5, 4], [2, 6, 7, 3], [0, 4, 6, 2], [1, 3, 7, 5]];
        let tri = quads.iter().flat_map(|q| [[q[0], q[1], q[2]], [q[0], q[2], q[3]]]).collect();
        geom::Mesh { pos, tri }
    }

    #[test]
    fn a_plain_block_releases() {
        let m = box_mesh([0.0, 0.0, 0.0], [100.0, 120.0, 160.0]);
        for t in 0..m.tri.len() {
            let (n, _) = m.normal_area(t);
            assert!(dot(n, sub(m.centroid(t), [50.0, 60.0, 80.0])) > 0.0, "normals point out");
        }
        let fr = Frame { lo: [0.0, 0.0, 0.0], hi: [100.0, 120.0, 160.0], wall: 20.0, plate: 20.0 };
        let c = check(&m, &fr, 0.4);
        assert_eq!(c.locked_area, 0.0);
        assert_eq!(fr.piece_of([50.0, -5.0, 80.0]), 0);
        assert_eq!(fr.piece_of([50.0, 60.0, 170.0]), 4);
    }

    /// Add a box's faces turned inside out: a cavity.
    fn add_cavity(m: &mut geom::Mesh, lo: V3, hi: V3) {
        let c = box_mesh(lo, hi);
        let off = m.pos.len();
        m.pos.extend(c.pos);
        m.tri.extend(c.tri.iter().map(|t| [t[0] + off, t[2] + off, t[1] + off]));
    }

    #[test]
    fn a_cap_and_a_bell_lock() {
        // A T: the back's mitre reaches down under the cap, so the back would lock. (Walls that
        // slid out from under the cap would release it; the mitre split doesn't make those.)
        let mut t = box_mesh([40.0, 0.0, 0.0], [60.0, 100.0, 100.0]);
        let cap = box_mesh([0.0, 0.0, 100.0], [100.0, 100.0, 120.0]);
        let off = t.pos.len();
        t.pos.extend(cap.pos);
        t.tri.extend(cap.tri.iter().map(|q| [q[0] + off, q[1] + off, q[2] + off]));
        let fr = Frame { lo: [0.0, 0.0, 0.0], hi: [100.0, 100.0, 120.0], wall: 20.0, plate: 20.0 };
        assert!(check(&t, &fr, 0.4).locked_area > 1000.0);
        // A chamber wider than its mouth in the back: the plug can't lift out.
        let mut m = box_mesh([0.0, 0.0, 0.0], [100.0, 100.0, 100.0]);
        add_cavity(&mut m, [20.0, 20.0, 40.0], [80.0, 80.0, 80.0]);
        add_cavity(&mut m, [40.0, 40.0, 80.0], [60.0, 60.0, 100.0]);
        let fr = Frame { lo: [0.0, 0.0, 0.0], hi: [100.0, 100.0, 100.0], wall: 20.0, plate: 20.0 };
        let c = check(&m, &fr, 0.4);
        assert!(c.locked_area > 1000.0, "{c:?}");
    }

    #[test]
    fn a_recess_in_the_open_face_is_skipped() {
        // Without bottom faces: the open face has a hole where the recess opens.
        let mut m = box_mesh([0.0, 0.0, 0.0], [100.0, 100.0, 100.0]);
        m.tri.drain(0..2);
        let mut r = box_mesh([30.0, 30.0, 0.0], [50.0, 50.0, 2.0]);
        r.tri.drain(0..2);
        let off = m.pos.len();
        m.pos.extend(r.pos);
        m.tri.extend(r.tri.iter().map(|t| [t[0] + off, t[2] + off, t[1] + off]));
        let fr = Frame { lo: [0.0, 0.0, 0.0], hi: [100.0, 100.0, 100.0], wall: 20.0, plate: 20.0 };
        let c = check(&m, &fr, 0.4);
        assert_eq!(c.locked_area, 0.0, "{c:?}");
        assert!(c.recess_area > 400.0, "{c:?}");
    }

    #[test]
    fn joints_have_two_pieces() {
        let fr = Frame { lo: [-50.0, -60.0, 0.0], hi: [50.0, 60.0, 160.0], wall: 20.0, plate: 30.0 };
        for (o, e, u, _, len) in joints(&fr) {
            let n = cross(u, e);
            let inside = sub(add(o, mul(e, len / 2.0)), mul(u, 10.0));
            assert_ne!(fr.piece_of(add(inside, mul(n, 5.0))), fr.piece_of(sub(inside, mul(n, 5.0))));
        }
    }
}
