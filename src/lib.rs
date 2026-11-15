//! Mould Maker: printed rigid moulds, a SolidCraft plugin.
//!
//! Two entries share one prelude (find the part and its open face, turn a working copy so the
//! open face faces down, read a mesh of it):
//! - `round-mould`: parts roughly round about the open face's normal. A jacket that pulls off
//!   sideways in sectors, a back plate with plug in rings that lifts off, flange tabs, bolt and
//!   dowel holes, every piece sized to the printer. Big moulds take two runs, chosen by itself.
//! - `box-mould`: parts with flat or gently curved sides. Four walls that pull straight off
//!   sideways and a back that lifts off, split on mitres, with bolting ears at every joint.
//!
//! Everything is made of ordinary SolidCraft features, so a saved design needs no plugin.

pub mod analysis;
pub mod boxm;
pub mod build;
pub mod geom;
pub mod layout;
pub mod silicone;

use geom::{Rot, V3};
use serde_json::{json, Value};

/// What the plugin needs from the host: run one command with JSON args, get the JSON reply.
pub trait Runner {
    fn run(&mut self, command: &str, args: Value) -> Result<Value, String>;
    fn log(&mut self, _message: &str) {}
    fn progress(&mut self, _fraction: f32, _message: &str) {}
}

/// Host calls one run may make.
pub const HOST_CALLS: usize = 256;
/// Pieces one run builds comfortably inside the host's 30 s limit on a typical desktop.
pub const PIECE_BUDGET: usize = 36;

/// Printer presets: name and build volume (mm).
pub const PRINTERS: &[(&str, [f64; 3])] = &[("K1 Max", [300.0, 300.0, 300.0])];

#[derive(Clone, Debug)]
pub struct Params {
    pub part: String,
    pub printer: String,
    pub bed: layout::Bed,
    pub sectors: usize,
    pub wall: f64,
    pub back_plate: f64,
    pub flange_width: f64,
    pub flange_thickness: f64,
    pub bolt: String,
    pub dowel_diameter: f64,
    pub pin_diameter: f64,
    pub pin_angle: f64,
    pub pin_height: f64,
    pub silicone: f64,
}

/// Clearance hole for a bolt size; `Some(0.0)` for no bolts (clamps); `None` if unknown.
pub fn bolt_clearance(size: &str) -> Option<f64> {
    match size {
        "M4" => Some(4.5),
        "M5" => Some(5.5),
        "M6" => Some(6.6),
        "M8" => Some(9.0),
        "None" => Some(0.0),
        _ => None,
    }
}

pub fn parse_params(v: &Value) -> Params {
    let num = |k: &str, d: f64| v.get(k).and_then(Value::as_f64).unwrap_or(d);
    let text = |k: &str, d: &str| v.get(k).and_then(Value::as_str).unwrap_or(d).trim().to_string();
    let printer = text("printer", "K1 Max");
    let custom = layout::Bed { x: num("printer_width", 300.0), y: num("printer_depth", 300.0), z: num("printer_height", 300.0) };
    let bed = PRINTERS.iter().find(|(n, _)| *n == printer).map(|(_, b)| layout::Bed { x: b[0], y: b[1], z: b[2] }).unwrap_or(custom);
    Params {
        part: text("part", ""),
        printer,
        bed,
        sectors: num("sectors", 0.0).round().max(0.0) as usize,
        wall: num("wall", 20.0),
        back_plate: num("back_plate", 20.0),
        flange_width: num("flange_width", 25.0),
        flange_thickness: num("flange_thickness", 16.0),
        bolt: text("bolt", "M6"),
        dowel_diameter: num("dowel_diameter", 6.0),
        pin_diameter: num("core_pin_diameter", 0.0),
        pin_angle: num("core_pin_angle", 0.0),
        pin_height: num("core_pin_height", 0.0),
        silicone: num("silicone_thickness", 10.0),
    }
}

/// (field, reason) for the first bad input. Reasons are whole sentences that say how to fix it.
pub fn validate(p: &Params) -> Result<(), (String, String)> {
    let bad = |f: &str, why: String| Err((f.to_string(), why));
    if p.printer != "Custom" && !PRINTERS.iter().any(|(n, _)| *n == p.printer) {
        return bad("printer", "Choose a printer from the list, or Custom to type its build volume.".into());
    }
    for (f, label, v, lo, hi) in [
        ("printer_width", "Bed width", p.bed.x, 50.0, 2000.0),
        ("printer_depth", "Bed depth", p.bed.y, 50.0, 2000.0),
        ("printer_height", "Bed height", p.bed.z, 50.0, 2000.0),
        ("wall", "Wall", p.wall, 5.0, 200.0),
        ("back_plate", "Back plate", p.back_plate, 5.0, 200.0),
        ("flange_width", "Flange width", p.flange_width, 12.0, 100.0),
        ("flange_thickness", "Flange thickness", p.flange_thickness, 6.0, 60.0),
        ("dowel_diameter", "Dowel pins", p.dowel_diameter, 0.0, 16.0),
        ("core_pin_diameter", "Core pin diameter", p.pin_diameter, 0.0, 300.0),
        ("silicone_thickness", "Silicone thickness", p.silicone, 5.0, 40.0),
    ] {
        if !(v.is_finite() && v >= lo && v <= hi) {
            return bad(f, format!("{label} must be between {lo} and {hi} mm."));
        }
    }
    if p.sectors == 1 || p.sectors > 64 {
        return bad("sectors", "Sectors must be 0 (the plugin chooses) or 2 to 64.".into());
    }
    let Some(bc) = bolt_clearance(&p.bolt) else { return bad("bolt", "Choose M4, M5, M6, M8 or None.".into()) };
    let hole = bc.max(if p.dowel_diameter > 0.0 { p.dowel_diameter + 0.1 } else { 0.0 });
    if hole > 0.0 && p.flange_width < 2.5 * hole {
        let what = if bc >= p.dowel_diameter { format!("{} bolts", p.bolt) } else { format!("{} mm dowels", p.dowel_diameter) };
        return bad(
            "flange_width",
            format!("Flange width must be at least {:.0} mm for {what}. Raise Flange width or pick smaller bolts or dowels.", (2.5 * hole).ceil()),
        );
    }
    if p.dowel_diameter > 0.0 && p.dowel_diameter < 2.0 {
        return bad("dowel_diameter", "Dowel pins must be 0 (no dowels) or at least 2 mm.".into());
    }
    Ok(())
}

/// Stands in for the host to count the calls a build would make, before touching the design.
struct Dry {
    calls: usize,
    next: usize,
}

impl Runner for Dry {
    fn run(&mut self, command: &str, args: Value) -> Result<Value, String> {
        self.calls += 1;
        self.next += 1;
        let fresh = format!("dry {}", self.next);
        Ok(match command {
            "solid.pattern.circular" => {
                let count = args.get("count").and_then(Value::as_u64).unwrap_or(1) as usize;
                json!({"created_bodies": (1..count).map(|i| format!("{fresh}.{i}")).collect::<Vec<_>>(), "name": fresh})
            }
            "solid.combine" | "solid.remove" | "body.rename" => json!({}),
            "solid.move" if args.get("copy").and_then(Value::as_bool) != Some(true) => json!({}),
            _ => json!({"created_bodies": [fresh], "name": fresh}),
        })
    }
}

/// The part as the mould sees it.
pub struct Prep {
    /// The user's body.
    pub source: String,
    /// The body the mould is built round: the source, or a turned working copy.
    pub part: String,
    /// The turn taking the source onto the working copy, about `origin`.
    pub turn: Option<(Rot, V3)>,
    /// Whether `part` is a working copy (turned, or placed out of a component) to remove at the end.
    pub copied: bool,
    pub lo: V3,
    pub hi: V3,
    pub mesh: geom::Mesh,
    pub tol: f64,
    pub existing: Vec<String>,
    /// Host calls made so far.
    pub calls: usize,
}

impl Prep {
    /// Host calls `finish` will make.
    pub fn finish_calls(&self) -> usize {
        self.turn.is_some() as usize + self.copied as usize
    }
    /// A world point (in the user's design) for a point of the working copy.
    pub fn to_world(&self, p: V3) -> V3 {
        match &self.turn {
            Some((rot, o)) => geom::add(rot.inverse().apply(geom::sub(p, *o)), *o),
            None => p,
        }
    }
    /// Turn the pieces back onto the user's part and drop the working copy.
    pub fn finish<R: Runner>(&self, r: &mut R, pieces: &[String]) -> Result<(), String> {
        if let Some((rot, o)) = &self.turn {
            r.run("solid.move", json!({"bodies": pieces, "axis": rot.axis, "angle": format!("{} deg", -rot.deg), "origin": o}))?;
        }
        if self.copied {
            r.run("solid.remove", json!({"bodies": [self.part]}))?;
        }
        Ok(())
    }
}

fn names(v: &Value) -> Vec<String> {
    v.get("bodies")
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(|b| b.get("name").and_then(Value::as_str).map(String::from)).collect())
        .unwrap_or_default()
}

fn v3(v: Option<&Value>) -> Option<V3> {
    let a = v?.as_array()?;
    Some([a.first()?.as_f64()?, a.get(1)?.as_f64()?, a.get(2)?.as_f64()?])
}

fn bbox<R: Runner>(r: &mut R, body: &str, calls: &mut usize) -> Result<(V3, V3, Value), String> {
    *calls += 1;
    let m = r.run("inspect.measure", json!({"bodies": [body]}))?;
    let b = m.get("bodies").and_then(|b| b.get(0)).cloned().ok_or_else(|| format!("Couldn't measure \"{body}\"."))?;
    let bb = b.get("bbox").ok_or("The part has no size.")?;
    let (lo, hi) = (v3(bb.get("min")).ok_or("The part has no size.")?, v3(bb.get("max")).ok_or("The part has no size.")?);
    Ok((lo, hi, b))
}

/// A rigid placement: rotation columns and translation.
type Place = ([V3; 3], V3);

fn place_apply(m: &Place, p: V3) -> V3 {
    let ([a, b, c], t) = m;
    geom::add(geom::add(geom::add(geom::mul(*a, p[0]), geom::mul(*b, p[1])), geom::mul(*c, p[2])), *t)
}

/// `outer` after `inner`.
fn place_then(outer: &Place, inner: &Place) -> Place {
    let rot = |v: V3| geom::sub(place_apply(outer, v), outer.1);
    ([rot(inner.0[0]), rot(inner.0[1]), rot(inner.0[2])], place_apply(outer, inner.1))
}

/// Where a body's component puts it in the design. SolidCraft gives meshes and faces in the
/// component's own coordinates, so a part in a placed component (an imported STEP stood up, say)
/// must be copied out and placed before it is read. `None` for a body of the design itself.
fn placement<R: Runner>(r: &mut R, body: &str, calls: &mut usize) -> Result<Option<Place>, String> {
    *calls += 1;
    let list = r.run("component.list", json!({}))?;
    let comps = list.get("components").and_then(Value::as_array).cloned().unwrap_or_default();
    let has = |c: &Value| c.get("bodies").and_then(Value::as_array).is_some_and(|b| b.iter().any(|x| x.as_str() == Some(body)));
    let Some(mut comp) = comps.iter().find(|c| has(c)).cloned() else { return Ok(None) };
    let mut acc: Place = ([[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]], [0.0; 3]);
    for _ in 0..32 {
        if comp.get("parent").is_none_or(Value::is_null) {
            break;
        }
        let Some(occ) = comp.get("occurrences").and_then(Value::as_array).and_then(|o| o.first()).cloned() else { break };
        let tr = occ.get("transform").and_then(Value::as_array).cloned().unwrap_or_default();
        // Column-major: entries 0 to 2 are the axes, entry 3 the origin.
        let col = |i: usize| v3(tr.get(i)).unwrap_or(if i < 3 { let mut e = [0.0; 3]; e[i] = 1.0; e } else { [0.0; 3] });
        acc = place_then(&([col(0), col(1), col(2)], col(3)), &acc);
        let parent = occ.get("parent").and_then(Value::as_i64).or_else(|| comp.get("parent").and_then(Value::as_i64));
        let Some(next) = comps.iter().find(|c| c.get("id").and_then(Value::as_i64) == parent).cloned() else { break };
        comp = next;
    }
    let ident = (0..3).all(|i| (0..3).all(|j| (acc.0[i][j] - if i == j { 1.0 } else { 0.0 }).abs() < 1e-9)) && geom::norm(acc.1) < 1e-9;
    Ok((!ident).then_some(acc))
}

/// Find the part and its open face, turn a working copy so the open face faces down (-Z), and
/// read its mesh.
pub fn prepare<R: Runner>(r: &mut R, p: &Params) -> Result<Prep, String> {
    let mut calls = 0;
    calls += 1;
    let sel = r.run("selection.get", json!({"detail": true}))?;
    let items: Vec<Value> = sel.get("items").and_then(Value::as_array).cloned().unwrap_or_default();
    let kind = |it: &Value| it.get("kind").or_else(|| it.get("type")).and_then(Value::as_str).unwrap_or("").to_string();
    calls += 1;
    let existing = names(&r.run("inspect.measure", json!({"names_only": true}))?);
    let picked_body = items.iter().find_map(|it| match kind(it).as_str() {
        "body" => it.get("name").and_then(Value::as_str).map(String::from),
        "face" | "edge" | "vertex" => it.get("body").and_then(Value::as_str).map(String::from),
        _ => None,
    });
    let source = if !p.part.is_empty() {
        if !existing.iter().any(|n| *n == p.part) {
            return Err(format!("There's no body called \"{}\". Check Part body, or leave it empty and select the part.", p.part));
        }
        p.part.clone()
    } else if let Some(b) = picked_body {
        b
    } else {
        match existing.len() {
            1 => existing[0].clone(),
            0 => return Err("The design has no body to mould.".into()),
            _ => return Err("Select the part to mould (or its open face), or type its body name in Part body.".into()),
        }
    };
    // A face of the part, selected: the open face.
    let face = items.iter().find(|it| kind(it) == "face" && it.get("body").and_then(Value::as_str) == Some(source.as_str()));
    let open = match face {
        None => None,
        Some(f) => {
            if f.get("planar").and_then(Value::as_bool) != Some(true) {
                return Err("The open face must be flat. Select a flat face of the part, or select the whole part to use its bottom face.".into());
            }
            let s = f.get("surface");
            let n = v3(s.and_then(|s| s.get("normal"))).ok_or("The selected face has no direction.")?;
            let o = v3(f.get("point")).or_else(|| v3(s.and_then(|s| s.get("point")))).ok_or("The selected face has no position.")?;
            Some((geom::unit(n), o))
        }
    };
    let (lo0, hi0, body) = bbox(r, &source, &mut calls)?;
    if body.get("closed").and_then(Value::as_bool) == Some(false) {
        return Err(format!("\"{source}\" isn't a closed solid, so it has no inside to mould round."));
    }
    let mut part = source.clone();
    let (mut lo, mut hi) = (lo0, hi0);
    let mut turn = None;
    let mut copied = false;
    let place = placement(r, &source, &mut calls)?;
    let rot_t = open.map(|(n, o)| (Rot::between(n, [0.0, 0.0, -1.0]), o)).filter(|(rt, _)| !rt.is_none());
    if place.is_some() || rot_t.is_some() {
        // One copy, placed where the user sees the part and then turned open face down:
        // frame = Rt (M p - o) + o, which is rotation Rt·Rm about the origin, then a shift.
        let m: Place = place.unwrap_or(([[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]], [0.0; 3]));
        let (rt, o) = rot_t.unwrap_or((Rot::none(), [0.0; 3]));
        let total = Rot::from_columns(rt.apply(m.0[0]), rt.apply(m.0[1]), rt.apply(m.0[2]));
        let shift = geom::add(rt.apply(geom::sub(m.1, o)), o);
        let base = "Mould working copy";
        let mut name = base.to_string();
        let mut k = 2;
        while existing.iter().any(|e| *e == name) {
            name = format!("{base} {k}");
            k += 1;
        }
        let mut mv = json!({"bodies": [source], "copy": true, "translate": shift, "body_name": name});
        if !total.is_none() {
            mv["axis"] = json!(total.axis);
            mv["angle"] = json!(format!("{} deg", total.deg));
            mv["origin"] = json!([0, 0, 0]);
        }
        calls += 1;
        let v = r.run("solid.move", mv)?;
        part = build::created(&v).into_iter().next().ok_or("Couldn't copy the part.")?;
        (lo, hi, _) = bbox(r, &part, &mut calls)?;
        copied = true;
        turn = rot_t;
    }
    if let Some((_, o)) = open {
        let below = o[2] - lo[2];
        if below > 0.5 {
            return Err(format!(
                "The selected face isn't the part's open face: {below:.0} mm of the part lies beyond it. Select the face you trowel, \
                 which must be the part's lowest face when it stands on it."
            ));
        }
    }
    let h = hi[2] - lo[2];
    if h < 5.0 {
        return Err(format!("\"{source}\" is too flat to mould ({h:.1} mm thick)."));
    }
    let diag = geom::norm(geom::sub(hi, lo));
    // Coarse enough to stay inside the host's mesh budget; every split keeps a margin for it.
    let tol = (diag / 600.0).clamp(0.2, 25.0);
    calls += 1;
    let mesh = r.run("model.mesh", json!({"body": part, "tolerance": tol}))?;
    let pos: Vec<V3> = mesh
        .get("positions")
        .and_then(Value::as_array)
        .ok_or("Couldn't read the part's shape (no mesh).")?
        .iter()
        .map(|q| v3(Some(q)).unwrap_or([0.0; 3]))
        .collect();
    let tri: Vec<[usize; 3]> = mesh
        .get("triangles")
        .and_then(Value::as_array)
        .ok_or("Couldn't read the part's shape (no mesh).")?
        .iter()
        .map(|t| {
            let a = t.as_array().cloned().unwrap_or_default();
            [0, 1, 2].map(|i| a.get(i).and_then(Value::as_u64).unwrap_or(0) as usize)
        })
        .collect();
    Ok(Prep { source, part, turn, copied, lo, hi, mesh: geom::Mesh { pos, tri }, tol, existing, calls })
}

/// Round mould. Builds everything in one run if it fits the host's limits; if not, the jacket
/// now and the back plate on the next run (which finds the jacket already there).
pub fn round_mould<R: Runner>(r: &mut R, p: &Params) -> Result<Value, String> {
    validate(p).map_err(|(_, why)| why)?;
    r.progress(0.02, "reading the part");
    let prep = prepare(r, p)?;
    let (lo, hi) = (prep.lo, prep.hi);
    let part = prep.part.clone();
    let c = [0.5 * (lo[0] + hi[0]), 0.5 * (lo[1] + hi[1])];
    let (zmin, zmax) = (lo[2], hi[2]);
    let h = zmax - zmin;
    let tol = prep.tol;
    r.progress(0.04, "analysing the profile");
    let n_levels = ((h / tol).ceil() as usize).clamp(40, 240);
    let amesh = analysis::Mesh { pos: prep.mesh.pos.clone(), tri: prep.mesh.tri.clone() };
    let prof = analysis::profile(&amesh, c, zmin, zmax, n_levels, 180);
    let zj = zmax - 0.5f64.min(h * 0.01);
    let margin = tol + 1.5;
    let bands = analysis::jacket_bands(&prof, zj, margin)?;
    let pin_zone: Vec<(f64, f64, f64, f64)> = if p.pin_diameter > 0.0 {
        let z = zmin + p.pin_height;
        let r_ref = analysis::r_max(&prof).max(1.0);
        let hw = (p.pin_diameter / r_ref).to_degrees() + 6.0;
        vec![(p.pin_angle, hw, z - p.pin_diameter, z + p.pin_diameter)]
    } else {
        vec![]
    };
    let undercuts = analysis::plug_undercuts(&prof, zj, 2.0 * tol + 0.5, &pin_zone);
    let half_w = 0.5 * (hi[0] - lo[0]).max(hi[1] - lo[1]);
    let r_part = analysis::r_max(&prof).max(half_w);
    let r_env = r_part + p.wall;
    let ro = r_env + p.flange_width;
    let ztop = zmax + p.back_plate;
    let zb = ztop + p.flange_width;
    let r_jacket = bands.iter().map(|b| b.r).fold(f64::MAX, f64::min);

    let space = analysis::Space::new(&prof, &bands);
    let space_ref = &space;
    let env = layout::Envelope { r_jacket, ro, jacket_h: zj - zmin, plate_h: Box::new(move |r1, r2| zb - space_ref.lowest(r1, r2)) };
    let lay = layout::choose(&env, &p.bed, if p.sectors == 0 { None } else { Some(p.sectors) })?;
    let span = 360.0 / lay.sectors as f64;
    let piece_kinds: Vec<Value> = {
        let mut v = vec![];
        let jp = layout::Piece { r1: r_jacket, r2: ro, span, h: env.jacket_h };
        v.push(json!({"kind": "jacket", "count": lay.sectors, "radial_mm": [round1(jp.r1), round1(jp.r2)],
                      "height_mm": round1(jp.h), "print": layout::fit(&jp, &p.bed)}));
        let mut edges = vec![ro];
        edges.extend(lay.rings.iter().cloned());
        for (i, w) in edges.windows(2).enumerate() {
            let pc = layout::Piece { r1: w[1], r2: w[0], span, h: (env.plate_h)(w[1], w[0]) };
            v.push(json!({"kind": format!("plate ring {}", i + 1), "count": lay.sectors, "radial_mm": [round1(pc.r1), round1(pc.r2)],
                          "height_mm": round1(pc.h), "print": layout::fit(&pc, &p.bed)}));
        }
        let r_disk = *edges.last().unwrap();
        let dc = layout::Piece { r1: 0.0, r2: r_disk, span: 360.0 / lay.disk_sectors as f64, h: (env.plate_h)(0.0, r_disk) };
        v.push(json!({"kind": "plate centre", "count": lay.disk_sectors, "radial_mm": [0.0, round1(r_disk)],
                      "height_mm": round1(dc.h), "print": layout::fit(&dc, &p.bed)}));
        v
    };

    let pin = if p.pin_diameter > 0.0 {
        let z = zmin + p.pin_height;
        if p.pin_height - p.pin_diameter / 2.0 < 0.0 || z + p.pin_diameter / 2.0 > zmax {
            return Err(format!(
                "The core pin doesn't fit within the part's height ({:.0} mm). Core pin height is measured from the open face \
                 to the hole's centre.",
                h
            ));
        }
        let side = (p.pin_diameter / r_jacket).to_degrees() + 3.0;
        let inner = analysis::wall_inner_near(&prof, p.pin_angle, z - p.pin_diameter / 2.0, z + p.pin_diameter / 2.0, side)
            .ok_or_else(|| {
                format!(
                    "There's no wall at Core pin angle {:.0}° and height {:.0} mm. The angle is measured from +X, counter-clockwise \
                     seen from the back of the mould.",
                    p.pin_angle, p.pin_height
                )
            })?;
        Some(build::Pin { deg: p.pin_angle, z, d: p.pin_diameter, start: (inner - p.pin_diameter).max(0.0) })
    } else {
        None
    };
    let theta0 = if pin.is_some() { p.pin_angle - span / 2.0 } else { 0.0 };

    // Which run is this? A jacket already in the design means the back plate is next.
    let has = |base: &str| prep.existing.iter().any(|n| n.starts_with(base));
    if has("Mould jacket") && has("Mould plate") {
        return Err("This design already has a round mould (Mould jacket and Mould plate pieces). Undo it, or delete those \
                    pieces, to build a new one."
            .into());
    }
    let first_mode = if has("Mould jacket") { build::Build::Plate } else { build::Build::Everything };
    let mut spec = build::Spec {
        part: part.clone(),
        c,
        zmin,
        zmax,
        r: r_env,
        ro,
        f: p.flange_thickness / 2.0,
        fw: p.flange_width,
        ztop,
        zb,
        zj,
        bands: bands.clone(),
        layout: lay.clone(),
        theta0,
        bolt_d: bolt_clearance(&p.bolt).unwrap_or(0.0),
        dowel_d: if p.dowel_diameter > 0.0 { p.dowel_diameter + 0.1 } else { 0.0 },
        pin,
        build: first_mode,
        existing: prep.existing.clone(),
    };
    // Count the calls first: running out of calls or time half way discards everything.
    let budget = HOST_CALLS - prep.calls - prep.finish_calls() - 1;
    let fits = |spec: &build::Spec| -> Result<(bool, usize), String> {
        let mut dry = Dry { calls: 0, next: 0 };
        build::build(&mut dry, spec)?;
        let pieces = match spec.build {
            build::Build::Everything => lay.pieces,
            build::Build::Jacket => lay.sectors,
            build::Build::Plate => lay.pieces - lay.sectors,
        };
        Ok((dry.calls <= budget && pieces <= PIECE_BUDGET, dry.calls))
    };
    let (mut ok, mut calls) = fits(&spec)?;
    if !ok && spec.build == build::Build::Everything {
        spec.build = build::Build::Jacket;
        (ok, calls) = fits(&spec)?;
    }
    if !ok {
        let what = match spec.build {
            build::Build::Plate => "back plate",
            _ => "jacket",
        };
        return Err(format!(
            "This mould has {} pieces and its {what} alone is too big for one plugin run ({calls} SolidCraft commands; the limit is \
             {HOST_CALLS} commands and 30 seconds). Choose a printer with a bigger bed, or a thinner Back plate.",
            lay.pieces
        ));
    }
    r.log(&format!("mould for {}: {} sectors, rings at {:?}, centre in {}, building {:?}", prep.source, lay.sectors, lay.rings, lay.disk_sectors, spec.build));
    let built = build::build(r, &spec)?;
    let mut all: Vec<String> = built.jacket.clone();
    all.extend(built.rings.iter().flatten().cloned());
    all.extend(built.disk.iter().cloned());
    prep.finish(r, &all)?;
    let mut warnings: Vec<String> = built.warnings.clone();
    for (a, b) in &undercuts {
        warnings.push(format!("The plug may lock in between {a:.0} and {b:.0} mm up: the cavity narrows going up there. Add draft to the part."));
    }
    if pin_needed_hint(&prof, &bands) && p.pin_diameter == 0.0 {
        warnings.push("The wall has an opening the jacket split crosses: a side hole needs a core pin.".into());
    }
    let next = (spec.build == build::Build::Jacket)
        .then_some("This mould is too big for one run: the jacket is built. Run Round mould again with the same part selected to build the back plate.");
    if let Some(n) = next {
        r.log(n);
    }
    for w in &warnings {
        r.log(w);
    }
    Ok(json!({
        "part": prep.source,
        "built": format!("{:?}", spec.build).to_lowercase(),
        "next": next,
        "pieces": all.len(),
        "jacket": built.jacket,
        "plate_rings": built.rings,
        "plate_centre": built.disk,
        "layout": {
            "sectors": lay.sectors,
            "ring_split_radii_mm": lay.rings.iter().map(|x| round1(*x)).collect::<Vec<_>>(),
            "centre_sectors": lay.disk_sectors,
            "jacket_split": bands.iter().map(|b| json!({"up_to_mm": round1(b.z_hi - zmin), "radius_mm": round1(b.r)})).collect::<Vec<_>>(),
            "envelope_radius_mm": round1(r_env),
            "tab_outer_radius_mm": round1(ro),
        },
        "piece_kinds": piece_kinds,
        "bolts": {"size": p.bolt, "count": built.bolts},
        "dowel_holes": {"diameter_mm": p.dowel_diameter, "count": built.dowels},
        "core_pin": p.pin_diameter > 0.0,
        "turned": prep.turn.is_some(),
        "analysis_tolerance_mm": (tol * 100.0).round() / 100.0,
        "host_calls": built.calls + prep.calls + prep.finish_calls(),
        "warnings": warnings,
    }))
}

/// The 0.1 entry's name.
pub fn rigid_mould<R: Runner>(r: &mut R, p: &Params) -> Result<Value, String> {
    round_mould(r, p)
}

/// Box mould: see [`boxm`].
pub fn box_mould<R: Runner>(r: &mut R, p: &Params) -> Result<Value, String> {
    validate(p).map_err(|(_, why)| why)?;
    r.progress(0.02, "reading the part");
    let prep = prepare(r, p)?;
    boxm::build(r, p, &prep)
}

/// Silicone sleeve mould: see [`silicone`].
pub fn silicone_mould<R: Runner>(r: &mut R, p: &Params) -> Result<Value, String> {
    validate(p).map_err(|(_, why)| why)?;
    r.progress(0.02, "reading the part");
    let prep = prepare(r, p)?;
    silicone::build(r, p, &prep)
}

/// Run an entry by name.
pub fn run_entry<R: Runner>(r: &mut R, entry: &str, p: &Params) -> Result<Value, String> {
    match entry {
        "round-mould" | "rigid-mould" => round_mould(r, p),
        "box-mould" => box_mould(r, p),
        "silicone-mould" => silicone_mould(r, p),
        other => Err(format!("unknown entry {other}")),
    }
}

/// An angle where the outer wall is missing over some heights (a side hole or slot).
fn pin_needed_hint(p: &analysis::Profile, bands: &[analysis::Band]) -> bool {
    let top = bands.last().map(|b| b.z_hi).unwrap_or(p.zmax);
    p.levels.iter().filter(|l| l.z < top).any(|l| {
        let seen = l.rays.iter().filter(|r| !r.is_empty()).count();
        seen > 0 && seen < p.n_angles
    })
}

pub(crate) fn round1(x: f64) -> f64 {
    (x * 10.0).round() / 10.0
}

#[cfg(target_arch = "wasm32")]
mod guest {
    wit_bindgen::generate!({ path: "wit", world: "command-plugin" });

    use super::*;
    use craft::plugin::{
        commands, control, log,
        types::{CommandFailure, FieldFailure, PluginError},
    };

    struct Host {
        last_error: Option<PluginError>,
    }
    impl Runner for Host {
        fn run(&mut self, command: &str, args: Value) -> Result<Value, String> {
            if control::cancelled() {
                self.last_error = Some(PluginError::Cancelled);
                return Err("cancelled".into());
            }
            match commands::run(command, &args.to_string()) {
                Ok(s) => Ok(serde_json::from_str(&s).unwrap_or(Value::Null)),
                Err(e) => {
                    let a = args.to_string();
                    let msg = format!("SolidCraft refused a step ({command}): {e:?}. Args: {}", &a[..a.len().min(500)]);
                    self.last_error = Some(e);
                    Err(msg)
                }
            }
        }
        fn log(&mut self, message: &str) {
            log::log(log::Level::Info, message);
        }
        fn progress(&mut self, fraction: f32, message: &str) {
            control::progress(fraction, message);
        }
    }

    struct MouldMaker;
    impl exports::craft::plugin::entry::Guest for MouldMaker {
        fn execute(entry: String, params: String) -> Result<String, PluginError> {
            if !matches!(entry.as_str(), "round-mould" | "rigid-mould" | "box-mould" | "silicone-mould") {
                return Err(PluginError::InvalidInput(FieldFailure { field: "entry".into(), reason: format!("unknown entry {entry}") }));
            }
            let v: Value = serde_json::from_str(&params).unwrap_or(json!({}));
            let p = parse_params(&v);
            if let Err((field, reason)) = validate(&p) {
                return Err(PluginError::InvalidInput(FieldFailure { field, reason }));
            }
            let mut host = Host { last_error: None };
            match run_entry(&mut host, &entry, &p) {
                Ok(v) => Ok(v.to_string()),
                // Cancellation and limits keep the host's own error; anything else is the
                // plugin's message, which says what to change.
                Err(e) => Err(match host.last_error.take() {
                    Some(PluginError::Cancelled) => PluginError::Cancelled,
                    Some(PluginError::LimitExceeded(m)) => PluginError::LimitExceeded(m),
                    Some(PluginError::PermissionDenied(m)) => PluginError::PermissionDenied(m),
                    _ => PluginError::CommandFailed(CommandFailure { command: format!("mould-maker.{entry}"), reason: e }),
                }),
            }
        }
    }
    export!(MouldMaker);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_valid() {
        let p = parse_params(&json!({}));
        assert!(validate(&p).is_ok());
        assert_eq!(p.bolt, "M6");
        assert_eq!(p.bed.x, 300.0);
    }

    #[test]
    fn custom_printer_uses_the_fields() {
        let p = parse_params(&json!({"printer": "Custom", "printer_width": 250}));
        assert_eq!(p.bed.x, 250.0);
        let k = parse_params(&json!({"printer": "K1 Max", "printer_width": 250}));
        assert_eq!(k.bed.x, 300.0);
    }

    #[test]
    fn bad_inputs_name_their_field() {
        assert_eq!(validate(&parse_params(&json!({"bolt": "M7"}))).unwrap_err().0, "bolt");
        assert_eq!(validate(&parse_params(&json!({"sectors": 1}))).unwrap_err().0, "sectors");
        let e = validate(&parse_params(&json!({"flange_width": 14, "bolt": "M8"}))).unwrap_err();
        assert_eq!(e.0, "flange_width");
        assert!(e.1.contains("at least 23 mm for M8 bolts"), "{}", e.1);
        assert!(validate(&parse_params(&json!({"bolt": "None", "dowel_diameter": 0, "flange_width": 12}))).is_ok());
    }
}
