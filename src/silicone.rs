//! Silicone sleeve mould: a printed plug (the part), a printed sleeve in two halves that holds a
//! poured silicone liner, and a base plate that holds the plug and the sleeve while the silicone
//! is poured.
//!
//! Working frame: the open face (the face the concrete is poured into and trowelled) is the
//! part's lowest face, z = z0, and the silicone stage stands on it. The sleeve's inside is the
//! part's outline seen from above (its convex hull), grown by the silicone thickness and tapered
//! inwards going up at no more than the part's own draft, so the liner is never thinner than
//! asked for and slides out of the sleeve. A lip at the open end locates the liner and keeps
//! concrete from running behind it. The sleeve is open at the top: the silicone is poured in
//! there, level with the sleeve's rim, which leaves the liner's back flat.
//!
//! The sleeve splits on the plane through the part's axis across its narrow sides, with
//! bolting ears along both seams. Round holes across the part get a matching hole through the
//! sleeve, for a steel rod through mould and casting; holes along the part are reported, for a
//! rod pushed in from the open face.

use serde_json::{json, Value};

use crate::boxm::{Bx, EarSpec};
use crate::build::{between, created, spread};
use crate::geom::{self, grow, hull, inside_depth, simplify, AxisRays, P2, V3};
use crate::{round1, Params, Prep, Runner, HOST_CALLS};

/// A round hole through the part (frame coordinates).
#[derive(Clone, Debug)]
pub struct Hole {
    pub point: V3,
    pub axis: V3,
    pub diameter: f64,
}

fn area(poly: &[P2]) -> f64 {
    let n = poly.len();
    0.5 * (0..n).map(|i| poly[i][0] * poly[(i + 1) % n][1] - poly[(i + 1) % n][0] * poly[i][1]).sum::<f64>()
}
fn perimeter(poly: &[P2]) -> f64 {
    let n = poly.len();
    (0..n).map(|i| ((poly[(i + 1) % n][0] - poly[i][0]).powi(2) + (poly[(i + 1) % n][1] - poly[i][1]).powi(2)).sqrt()).sum()
}

/// The part's volume from its mesh (divergence theorem).
fn mesh_volume(m: &geom::Mesh) -> f64 {
    m.tri.iter().map(|t| geom::dot(m.pos[t[0]], geom::cross(m.pos[t[1]], m.pos[t[2]])) / 6.0).sum::<f64>().abs()
}

/// The tangent of the steepest inward taper the sleeve may have: every point of the part at
/// height h must lie at least h·tan inside the outline at the open face.
pub fn taper_tan(m: &geom::Mesh, outline: &[P2], z0: f64) -> f64 {
    let mut t = 5f64.to_radians().tan();
    for p in &m.pos {
        let h = p[2] - z0;
        if h > 2.0 {
            t = t.min(inside_depth(outline, [p[0], p[1]]).max(0.0) / h);
        }
    }
    // A little less than the part has, so the liner keeps its thickness at the top.
    (t.atan() - 0.1f64.to_radians()).max(0.0).tan()
}

/// Round holes in the part, from its cylindrical faces: faces of one axis and radius are taken
/// together; it's a hole when their centre lies on the axis and the axis there is outside the part.
pub fn holes(faces: &Value, rays: &AxisRays, lo: V3, hi: V3) -> Vec<Hole> {
    let small = 0.3 * (hi[0] - lo[0]).min(hi[1] - lo[1]).min(hi[2] - lo[2]);
    let mut groups: Vec<(V3, V3, f64, V3, f64)> = Vec::new(); // origin, axis, radius, area-weighted centroid sum, area
    for f in faces.get("faces").and_then(Value::as_array).cloned().unwrap_or_default() {
        let Some(c) = f.get("cylinder") else { continue };
        let get = |v: Option<&Value>| -> Option<V3> {
            let a = v?.as_array()?;
            Some([a.first()?.as_f64()?, a.get(1)?.as_f64()?, a.get(2)?.as_f64()?])
        };
        let (Some(o), Some(ax), Some(r), Some(ce)) = (get(c.get("origin")), get(c.get("axis")), c.get("radius").and_then(Value::as_f64), get(f.get("centroid"))) else {
            continue;
        };
        if r > small {
            continue;
        }
        let ax = geom::unit(ax);
        let a = f.get("area").and_then(Value::as_f64).unwrap_or(1.0);
        let same = groups.iter_mut().find(|g| {
            let off = geom::sub(o, g.0);
            let along = geom::dot(off, g.1);
            geom::dot(ax, g.1).abs() > 0.999 && (g.2 - r).abs() < 0.01 && geom::norm(geom::sub(off, geom::mul(g.1, along))) < 0.01
        });
        match same {
            Some(g) => {
                g.3 = geom::add(g.3, geom::mul(ce, a));
                g.4 += a;
            }
            None => groups.push((o, ax, r, geom::mul(ce, a), a)),
        }
    }
    let mut out = Vec::new();
    for (o, ax, r, sum, a) in groups {
        let ce = geom::mul(sum, 1.0 / a);
        let foot = geom::add(o, geom::mul(ax, geom::dot(geom::sub(ce, o), ax)));
        if geom::norm(geom::sub(ce, foot)) > 0.5 * r {
            continue; // part of a round, not a whole hole
        }
        if rays.inside(foot) {
            continue; // a boss, not a hole
        }
        // Deep enough to need a rod: a shallow recess is cast as it is.
        let depth = a / (std::f64::consts::TAU * r);
        if depth < (2.0 * r).max(10.0) {
            continue;
        }
        out.push(Hole { point: foot, axis: ax, diameter: 2.0 * r });
    }
    out
}

pub fn build<R: Runner>(r: &mut R, p: &Params, prep: &Prep) -> Result<Value, String> {
    let (lo, hi) = (prep.lo, prep.hi);
    let (z0, z1) = (lo[2], hi[2]);
    let t = p.silicone;
    let w = p.wall;
    let f = p.flange_thickness / 2.0;
    let fw = p.flange_width;
    let eps = 0.05;
    let bolt_d = crate::bolt_clearance(&p.bolt).unwrap_or(0.0);
    let dowel_d = if p.dowel_diameter > 0.0 { p.dowel_diameter + 0.1 } else { 0.0 };

    // ---------- the sleeve's inside ----------
    r.progress(0.05, "outline and taper");
    let pts: Vec<P2> = prep.mesh.pos.iter().map(|q| [q[0], q[1]]).collect();
    let outline = hull(&pts);
    if outline.len() < 3 {
        return Err("Couldn't read the part's outline.".into());
    }
    let tan_a = taper_tan(&prep.mesh, &outline, z0);
    let taper_deg = tan_a.atan().to_degrees();
    let zc = z1 + t;
    let height = zc - z0;
    let lip = (w - 3.0).clamp(0.0, 6.0);
    let lip_h = if lip > 0.0 { 4.0 } else { 0.0 };
    // Sketches start 1 mm below the open face, so no face of the cut lies on the sleeve's ends.
    let cav = simplify(&grow(&outline, t + tan_a, 8.0), 0.05);
    let lip_line = simplify(&grow(&outline, t + lip, 8.0), 0.05);
    let at_open = grow(&outline, t, 8.0);
    let (mut bx0, mut bx1, mut by0, mut by1) = (f64::MAX, f64::MIN, f64::MAX, f64::MIN);
    for q in &at_open {
        bx0 = bx0.min(q[0]);
        bx1 = bx1.max(q[0]);
        by0 = by0.min(q[1]);
        by1 = by1.max(q[1]);
    }
    let (x0, x1, y0, y1) = (bx0 - w, bx1 + w, by0 - w, by1 + w);
    // Split across the narrow sides: the split plane holds the long side.
    let split_y = (x1 - x0) >= (y1 - y0);
    let (xc, yc) = (0.5 * (x0 + x1), 0.5 * (y0 + y1));

    // Silicone needed: the tapered inside (Steiner: area of an outline grown by d is A + P d + π d²)
    // and the lip, less the part.
    let (a0, p0) = (area(&outline), perimeter(&outline));
    let n = 200;
    let cav_v: f64 = (0..n)
        .map(|i| {
            let h = (i as f64 + 0.5) * height / n as f64;
            let d = t - h * tan_a;
            (a0 + p0 * d + std::f64::consts::PI * d * d) * height / n as f64
        })
        .sum();
    let ring = |d: f64| a0 + p0 * d + std::f64::consts::PI * d * d;
    let lip_v = lip_h * (ring(t + lip) - ring(t));
    let silicone_ml = (cav_v + lip_v - mesh_volume(&prep.mesh)) / 1000.0;

    // ---------- holes ----------
    let rays = AxisRays::new(&prep.mesh);
    let mut calls = 0;
    calls += 1;
    let faces = r.run("model.faces", json!({"body": prep.part}))?;
    let found = holes(&faces, &rays, lo, hi);
    let across: Vec<Hole> = found.iter().filter(|h| h.axis[2].abs() < 0.1).cloned().collect();
    let along: Vec<Hole> = found.iter().filter(|h| h.axis[2].abs() > 0.99).cloned().collect();
    let slanted = found.len() - across.len() - along.len();

    // Rods along the part that come out of the open face: the base plate gets a socket for each,
    // so the rod stands the plug in place while the silicone is poured.
    let sockets: Vec<&Hole> = along.iter().filter(|h| inside_depth(&outline, [h.point[0], h.point[1]]) > 0.5 * h.diameter).collect();

    // ---------- names ----------
    let existing = &prep.existing;
    let free = |base: &str| -> String {
        let clash = |c: &str| existing.iter().any(|e| e == c || e.starts_with(&format!("{c} (")));
        if !clash(base) {
            return base.to_string();
        }
        (2..).map(|k| format!("{base} #{k}")).find(|c| !clash(c)).unwrap()
    };
    let (n_plug, n_a, n_b, n_base) = (free("Mould plug"), free("Mould sleeve A"), free("Mould sleeve B"), free("Mould base"));

    // Seams and their holes.
    let seams: Vec<(V3, V3, V3)> = if split_y {
        vec![([x1, yc, z0], [0.0, 0.0, 1.0], [1.0, 0.0, 0.0]), ([x0, yc, z0], [0.0, 0.0, 1.0], [-1.0, 0.0, 0.0])]
    } else {
        vec![([xc, y1, z0], [0.0, 0.0, 1.0], [0.0, 1.0, 0.0]), ([xc, y0, z0], [0.0, 0.0, 1.0], [0.0, -1.0, 0.0])]
    };
    let rim_h = 6.0;
    let (b0, b1) = (rim_h + 1.0, height - eps);
    let mut pos = spread(b0 + 15.0, b1 - 15.0, 150.0, 2);
    if pos.is_empty() {
        pos = vec![0.5 * (b0 + b1)];
    }
    let mut row: Vec<(bool, f64)> = if bolt_d > 0.0 { pos.iter().map(|b| (true, *b)).collect() } else { vec![] };
    if dowel_d > 0.0 {
        row.extend(between(&pos).into_iter().take(1).map(|b| (false, b)));
    }
    let planned = prep.calls + calls + 30 + 2 * (5 + row.len()) + 2 * across.len() + sockets.len() + prep.finish_calls();
    if planned > HOST_CALLS {
        return Err(format!("This mould needs about {planned} SolidCraft commands, more than one plugin run may make ({HOST_CALLS})."));
    }

    let mut b = Bx { r, calls, k: 0 };

    // ---------- plug ----------
    b.r.progress(0.1, "plug");
    let plug = b.named("solid.move", json!({"bodies": [prep.part], "copy": true, "translate": [0, 0, 0], "body_name": n_plug}))?;

    // ---------- sleeve ----------
    b.r.progress(0.2, "sleeve");
    let sleeve = b.named("solid.box", json!({"corner": [x0, y0, z0], "length": x1 - x0, "width": y1 - y0, "height": height, "body_name": n_a}))?;
    b.k += 1;
    let sk = format!("Mould silicone outline mm{}", b.k);
    b.run("sketch.create", json!({"plane": "XY", "offset": z0 - 1.0, "name": sk}))?;
    b.run("sketch.line", json!({"points": cav, "closed": true}))?;
    b.run("sketch.finish", json!({}))?;
    let cavity = b.first("solid.extrude", json!({"sketch": sk, "distance": height + 2.0, "taper": -taper_deg, "body_name": "Silicone space"}))?;
    let mut tools = vec![cavity];
    if lip > 0.0 {
        b.k += 1;
        let sl = format!("Mould silicone lip mm{}", b.k);
        b.run("sketch.create", json!({"plane": "XY", "offset": z0 - 1.0, "name": sl}))?;
        b.run("sketch.line", json!({"points": lip_line, "closed": true}))?;
        b.run("sketch.finish", json!({}))?;
        tools.push(b.first("solid.extrude", json!({"sketch": sl, "distance": 1.0 + lip_h, "body_name": "Silicone lip"}))?);
    }
    b.combine(&sleeve, &tools, "cut", false)?;
    let plane = if split_y { json!({"origin": [xc, yc, z0], "normal": [0, 1, 0]}) } else { json!({"origin": [xc, yc, z0], "normal": [1, 0, 0]}) };
    // The body keeps the side the normal points to; the new body is the other side.
    let v = b.run("solid.split_body", json!({"body": sleeve, "plane": plane, "body_names": [n_b]}))?;
    let other = created(&v).into_iter().next().ok_or("Couldn't split the sleeve.")?;
    let (pos_half, neg_half) = (sleeve.clone(), other);

    // ---------- rod holes across the part ----------
    let mut rods = Vec::new();
    for h in &across {
        let reach = (x1 - x0).max(y1 - y0) + 2.0 * fw + 20.0;
        let base = geom::sub(h.point, geom::mul(h.axis, reach / 2.0));
        let drill = b.first("solid.cylinder", json!({"diameter": h.diameter, "height": reach, "base": base, "axis": h.axis, "body_name": "Rod hole"}))?;
        b.combine(&pos_half, &[drill.clone()], "cut", true)?;
        b.combine(&neg_half, &[drill], "cut", false)?;
        let span = if h.axis[0].abs() > h.axis[1].abs() { x1 - x0 } else { y1 - y0 };
        rods.push(json!({"diameter_mm": round1(h.diameter), "across": true, "length_mm": (span + 40.0).ceil(),
                         "at_mm_from_open_face": round1(h.point[2] - z0)}));
    }
    for h in &along {
        rods.push(json!({"diameter_mm": round1(h.diameter), "across": false, "length_mm": (z1 - z0 + 40.0).ceil()}));
    }

    // ---------- ears along both seams ----------
    b.r.progress(0.5, "bolting ears");
    let es = EarSpec { f, fw, ov: 2.0, eps, bolt_d, dowel_d };
    let (mut ears_pos, mut ears_neg) = (Vec::new(), Vec::new());
    let (mut bolts, mut dowels) = (0, 0);
    for (o, e, u) in &seams {
        let (ha, hb, nb, nd) = b.ear(&es, *o, *e, *u, b0, b1, &row)?;
        bolts += nb;
        dowels += nd;
        // `ha` lies on the u × e side.
        let side = geom::cross(*u, *e);
        let a_on_pos = if split_y { side[1] > 0.0 } else { side[0] > 0.0 };
        if a_on_pos {
            ears_pos.push(ha);
            ears_neg.push(hb);
        } else {
            ears_neg.push(ha);
            ears_pos.push(hb);
        }
    }
    b.combine(&pos_half, &ears_pos, "join", false)?;
    b.combine(&neg_half, &ears_neg, "join", false)?;

    // ---------- base plate ----------
    b.r.progress(0.7, "base plate");
    let (g, rw, bt) = (0.3, 4.0, 8.0);
    let base = b.named(
        "solid.box",
        json!({"corner": [x0 - g - rw, y0 - g - rw, z0 - bt], "length": x1 - x0 + 2.0 * (g + rw), "width": y1 - y0 + 2.0 * (g + rw), "height": bt + rim_h, "body_name": n_base}),
    )?;
    let pocket = b.first("solid.box", json!({"corner": [x0 - g, y0 - g, z0], "length": x1 - x0 + 2.0 * g, "width": y1 - y0 + 2.0 * g, "height": rim_h + 1.0, "body_name": "Base pocket"}))?;
    b.combine(&base, &[pocket], "cut", false)?;
    // The plug isn't drilled: imported parts can carry gaps SolidCraft's booleans refuse.
    for h in &sockets {
        let drill = b.first(
            "solid.cylinder",
            json!({"diameter": h.diameter + 0.2, "height": 7.0, "base": [h.point[0], h.point[1], z0 - 6.0], "axis": [0, 0, 1], "body_name": "Rod socket"}),
        )?;
        b.combine(&base, &[drill], "cut", false)?;
    }

    // ---------- does every piece fit the printer? ----------
    let all = vec![plug.clone(), pos_half.clone(), neg_half.clone(), base.clone()];
    let m = b.run("inspect.measure", json!({"bodies": all}))?;
    let mut bed = [p.bed.x, p.bed.y, p.bed.z];
    bed.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let mut sizes = Vec::new();
    for body in m.get("bodies").and_then(Value::as_array).cloned().unwrap_or_default() {
        let name = body.get("name").and_then(Value::as_str).unwrap_or("").to_string();
        let bb = body.get("bbox");
        let get = |k: &str, i: usize| bb.and_then(|x| x.get(k)).and_then(|x| x.get(i)).and_then(Value::as_f64).unwrap_or(0.0);
        let mut d: Vec<f64> = (0..3).map(|i| get("max", i) - get("min", i)).collect();
        let shown = d.iter().map(|x| format!("{:.0}", x)).collect::<Vec<_>>().join(" × ");
        d.sort_by(|a, b| a.partial_cmp(b).unwrap());
        if d.iter().zip(&bed).any(|(x, y)| *x > *y + 0.01) {
            return Err(format!(
                "\"{name}\" would be {shown} mm, too big for your printer ({:.0} × {:.0} × {:.0} mm) in any direction. Choose a bigger \
                 printer (Custom), or a thinner Wall or Silicone thickness.",
                p.bed.x, p.bed.y, p.bed.z
            ));
        }
        sizes.push(json!({"piece": name, "size_mm": d.iter().map(|x| round1(*x)).collect::<Vec<_>>()}));
    }
    let calls = b.calls;
    prep.finish(r, &all)?;

    let mut warnings = Vec::new();
    if taper_deg < 0.5 {
        warnings.push(format!(
            "The part has almost no draft ({taper_deg:.1}°), so the sleeve's inside is nearly straight: the liner will be stiff to pull out \
             of the sleeve. Split the halves first, then peel."
        ));
    }
    if slanted > 0 {
        warnings.push(format!("{slanted} slanted hole(s) got no rod hole in the sleeve: drill them in the casting afterwards."));
    }
    if sockets.is_empty() && across.is_empty() {
        warnings.push("The part has no holes to locate the plug with rods: centre it on the base plate by eye, using the sleeve to check the gap all round, and glue it down.".into());
    }
    for w_ in &warnings {
        r.log(w_);
    }
    Ok(json!({
        "part": prep.source,
        "plug": plug,
        "sleeve": [pos_half, neg_half],
        "base": base,
        "pieces": 4,
        "silicone": {"thickness_mm": t, "volume_ml": silicone_ml.round(), "mass_g_at_sg_1_05": (silicone_ml * 1.05).round(),
                     "pour_to_mm_above_open_face": round1(height)},
        "taper_deg": (taper_deg * 100.0).round() / 100.0,
        "split": if split_y { "across Y" } else { "across X" },
        "rods": rods,
        "bolts": {"size": p.bolt, "count": bolts},
        "dowel_holes": {"diameter_mm": p.dowel_diameter, "count": dowels},
        "base_rod_sockets": sockets.len(),
        "sizes": sizes,
        "turned": prep.turn.is_some(),
        "host_calls": calls + prep.calls + prep.finish_calls(),
        "warnings": warnings,
    }))
}
