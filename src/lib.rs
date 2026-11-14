//! Mould Maker: printed rigid moulds for round parts, a SolidCraft plugin.
//!
//! Entry `rigid-mould`: takes the selected (or named) part, which must be roughly round about a
//! vertical axis and is cast upside down with its bottom face as the open, trowelled face. It
//! builds a bolt-together mould around it: a jacket that pulls off sideways, a back plate with
//! plug that lifts off, flange tabs outside the casting faces with bolt and dowel holes, and
//! every piece sized to fit the printer. Everything is made of ordinary SolidCraft features.

pub mod analysis;
pub mod build;
pub mod layout;

use serde_json::{json, Value};

/// What the plugin needs from the host: run one command with JSON args, get the JSON reply.
pub trait Runner {
    fn run(&mut self, command: &str, args: Value) -> Result<Value, String>;
    fn log(&mut self, _message: &str) {}
    fn progress(&mut self, _fraction: f32, _message: &str) {}
}

#[derive(Clone, Debug)]
pub struct Params {
    pub part: String,
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
    pub build: String,
}

pub fn bolt_clearance(size: &str) -> Option<f64> {
    match size {
        "M4" => Some(4.5),
        "M5" => Some(5.5),
        "M6" => Some(6.6),
        "M8" => Some(9.0),
        _ => None,
    }
}

pub fn parse_params(v: &Value) -> Params {
    let num = |k: &str, d: f64| v.get(k).and_then(Value::as_f64).unwrap_or(d);
    let text = |k: &str, d: &str| v.get(k).and_then(Value::as_str).unwrap_or(d).trim().to_string();
    Params {
        part: text("part", ""),
        bed: layout::Bed { x: num("printer_width", 300.0), y: num("printer_depth", 300.0), z: num("printer_height", 300.0) },
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
        build: text("build", "everything"),
    }
}

/// (field, reason) for the first bad input.
pub fn validate(p: &Params) -> Result<(), (String, String)> {
    let bad = |f: &str, why: &str| Err((f.to_string(), why.to_string()));
    for (f, v, lo, hi) in [
        ("printer_width", p.bed.x, 50.0, 2000.0),
        ("printer_depth", p.bed.y, 50.0, 2000.0),
        ("printer_height", p.bed.z, 50.0, 2000.0),
        ("wall", p.wall, 5.0, 200.0),
        ("back_plate", p.back_plate, 5.0, 200.0),
        ("flange_width", p.flange_width, 12.0, 100.0),
        ("flange_thickness", p.flange_thickness, 6.0, 60.0),
        ("dowel_diameter", p.dowel_diameter, 0.0, 16.0),
        ("core_pin_diameter", p.pin_diameter, 0.0, 300.0),
    ] {
        if !(v.is_finite() && v >= lo && v <= hi) {
            return bad(f, &format!("must be between {lo} and {hi}"));
        }
    }
    if p.sectors == 1 || p.sectors > 64 {
        return bad("sectors", "use 0 for automatic, or 2 to 64");
    }
    let Some(bc) = bolt_clearance(&p.bolt) else { return bad("bolt", "choose M4, M5, M6 or M8") };
    if p.flange_width < 2.5 * bc {
        return bad("flange_width", &format!("needs at least {:.0} mm for {} bolts", 2.5 * bc, p.bolt));
    }
    if build_mode(&p.build).is_none() {
        return bad("build", "choose everything, jacket or plate");
    }
    if p.dowel_diameter > 0.0 && p.dowel_diameter < 2.0 {
        return bad("dowel_diameter", "use 0 for no dowels, or at least 2 mm");
    }
    Ok(())
}

pub fn build_mode(s: &str) -> Option<build::Build> {
    match s {
        "everything" => Some(build::Build::Everything),
        "jacket" => Some(build::Build::Jacket),
        "plate" => Some(build::Build::Plate),
        _ => None,
    }
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
            "solid.combine" | "solid.mirror" | "solid.remove" | "body.rename" => json!({}),
            "solid.move" if args.get("copy").and_then(Value::as_bool) != Some(true) => json!({}),
            _ => json!({"created_bodies": [fresh], "name": fresh}),
        })
    }
}

/// Host calls one run may make (the host refuses past 256; a few go to reading the part).
pub const CALL_BUDGET: usize = 248;
/// Pieces one run builds comfortably inside the host's 30 s limit on a typical desktop.
pub const PIECE_BUDGET: usize = 36;

fn find_part<R: Runner>(r: &mut R, p: &Params) -> Result<String, String> {
    if !p.part.is_empty() {
        return Ok(p.part.clone());
    }
    let sel = r.run("selection.get", json!({"detail": false}))?;
    if let Some(items) = sel.get("items").and_then(Value::as_array) {
        for it in items {
            let kind = it.get("kind").or_else(|| it.get("type")).and_then(Value::as_str).unwrap_or("");
            // A body item names itself; a face or edge item names its body.
            let body = match kind {
                "body" => it.get("name").or_else(|| it.get("body")),
                "face" | "edge" | "vertex" => it.get("body"),
                _ => None,
            };
            if let Some(body) = body.and_then(Value::as_str) {
                return Ok(body.to_string());
            }
        }
    }
    let all = r.run("inspect.measure", json!({"names_only": true}))?;
    let names: Vec<String> = all
        .get("bodies")
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(|b| b.get("name").and_then(Value::as_str).map(String::from)).collect())
        .unwrap_or_default();
    match names.len() {
        1 => Ok(names[0].clone()),
        0 => Err("the design has no body to mould".into()),
        _ => Err("select the part to mould (or type its body name in Part)".into()),
    }
}

/// Build the mould; returns what was made.
pub fn rigid_mould<R: Runner>(r: &mut R, p: &Params) -> Result<Value, String> {
    validate(p).map_err(|(f, why)| format!("{f}: {why}"))?;
    let part = find_part(r, p)?;
    r.progress(0.02, "reading the part");
    let m = r.run("inspect.measure", json!({"bodies": [part]}))?;
    let body = m.get("bodies").and_then(|b| b.get(0)).ok_or("the part was not measured")?;
    if body.get("closed").and_then(Value::as_bool) == Some(false) {
        return Err(format!("{part} is not a closed solid"));
    }
    let bb = body.get("bbox").ok_or("the part has no bounding box")?;
    let v3 = |k: &str| -> Result<[f64; 3], String> {
        let a = bb.get(k).and_then(Value::as_array).ok_or("bad bounding box")?;
        Ok([a[0].as_f64().unwrap_or(0.0), a[1].as_f64().unwrap_or(0.0), a[2].as_f64().unwrap_or(0.0)])
    };
    let (lo, hi) = (v3("min")?, v3("max")?);
    let c = [0.5 * (lo[0] + hi[0]), 0.5 * (lo[1] + hi[1])];
    let (zmin, zmax) = (lo[2], hi[2]);
    let h = zmax - zmin;
    let diag = ((hi[0] - lo[0]).powi(2) + (hi[1] - lo[1]).powi(2) + h.powi(2)).sqrt();
    if h < 5.0 {
        return Err(format!("{part} is too flat to mould ({h:.1} mm tall)"));
    }

    // Coarse enough to stay inside the host's mesh budget; the split radii keep a margin for it.
    let tol = (diag / 600.0).clamp(0.2, 25.0);
    let mesh = r.run("model.mesh", json!({"body": part, "tolerance": tol}))?;
    let pos: Vec<[f64; 3]> = mesh
        .get("positions")
        .and_then(Value::as_array)
        .ok_or("model.mesh returned no positions")?
        .iter()
        .map(|q| {
            let a = q.as_array().cloned().unwrap_or_default();
            [a.first().and_then(Value::as_f64).unwrap_or(0.0), a.get(1).and_then(Value::as_f64).unwrap_or(0.0), a.get(2).and_then(Value::as_f64).unwrap_or(0.0)]
        })
        .collect();
    let tri: Vec<[usize; 3]> = mesh
        .get("triangles")
        .and_then(Value::as_array)
        .ok_or("model.mesh returned no triangles")?
        .iter()
        .map(|t| {
            let a = t.as_array().cloned().unwrap_or_default();
            [0, 1, 2].map(|i| a.get(i).and_then(Value::as_u64).unwrap_or(0) as usize)
        })
        .collect();
    r.progress(0.04, "analysing the profile");
    let n_levels = ((h / tol).ceil() as usize).clamp(40, 240);
    let prof = analysis::profile(&analysis::Mesh { pos, tri }, c, zmin, zmax, n_levels, 180);
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
    let env = layout::Envelope {
        r_jacket,
        ro,
        jacket_h: zj - zmin,
        plate_h: Box::new(move |r1, r2| zb - space_ref.lowest(r1, r2)),
    };
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
            return Err("core_pin_height: the pin must sit within the part's height".into());
        }
        let side = (p.pin_diameter / r_jacket).to_degrees() + 3.0;
        let inner = analysis::wall_inner_near(&prof, p.pin_angle, z - p.pin_diameter / 2.0, z + p.pin_diameter / 2.0, side)
            .ok_or("core_pin_angle: no wall found there")?;
        Some(build::Pin { deg: p.pin_angle, z, d: p.pin_diameter, start: (inner - p.pin_diameter).max(0.0) })
    } else {
        None
    };
    let theta0 = if pin.is_some() { p.pin_angle - span / 2.0 } else { 0.0 };
    let spec = build::Spec {
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
        bolt_d: bolt_clearance(&p.bolt).unwrap(),
        dowel_d: if p.dowel_diameter > 0.0 { p.dowel_diameter + 0.1 } else { 0.0 },
        pin,
        build: build_mode(&p.build).unwrap(),
    };
    // Count the calls first: running out of calls or time half way discards everything.
    let mut dry = Dry { calls: 0, next: 0 };
    build::build(&mut dry, &spec)?;
    let total_pieces = lay.pieces;
    let this_run = match spec.build {
        build::Build::Everything => total_pieces,
        build::Build::Jacket => lay.sectors,
        build::Build::Plate => total_pieces - lay.sectors,
    };
    if dry.calls > CALL_BUDGET || this_run > PIECE_BUDGET {
        let why = if dry.calls > CALL_BUDGET {
            format!("{} SolidCraft commands, more than one plugin run may make ({CALL_BUDGET})", dry.calls)
        } else {
            format!("{this_run} pieces, more than one plugin run finishes inside SolidCraft's 30 s limit")
        };
        let how = match spec.build {
            build::Build::Everything => "Build it in two runs: set Build to jacket and run, then set Build to plate and run again.",
            _ => "Use a larger printer bed setting or fewer, larger pieces is not possible here; raise the bed size if your printer allows.",
        };
        return Err(format!("this mould ({total_pieces} pieces) needs {why}. {how}"));
    }
    r.log(&format!(
        "mould for {part}: {} sectors, rings at {:?}, centre in {}, jacket split {:?}",
        lay.sectors, lay.rings, lay.disk_sectors, bands
    ));
    let built = build::build(r, &spec)?;
    let mut warnings: Vec<String> = built.warnings.clone();
    for (a, b) in &undercuts {
        warnings.push(format!(
            "the plug may lock in between {a:.0} and {b:.0} mm up: the cavity narrows going up there"
        ));
    }
    if pin_needed_hint(&prof, &bands) && p.pin_diameter == 0.0 {
        warnings.push("the wall has an opening the jacket split crosses; a side hole needs a core pin".into());
    }
    Ok(json!({
        "part": part,
        "pieces": built.jacket.len() + built.rings.iter().map(Vec::len).sum::<usize>() + built.disk.len(),
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
        "analysis_tolerance_mm": (tol * 100.0).round() / 100.0,
        "host_calls": built.calls + 4,
        "warnings": warnings,
    }))
}

/// An angle where the outer wall is missing over some heights (a side hole or slot).
fn pin_needed_hint(p: &analysis::Profile, bands: &[analysis::Band]) -> bool {
    let top = bands.last().map(|b| b.z_hi).unwrap_or(p.zmax);
    p.levels.iter().filter(|l| l.z < top).any(|l| {
        let seen = l.rays.iter().filter(|r| !r.is_empty()).count();
        seen > 0 && seen < p.n_angles
    })
}

fn round1(x: f64) -> f64 {
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
                    let msg = format!("{command} {}: {e:?}", &a[..a.len().min(700)]);
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
            match entry.as_str() {
                "rigid-mould" => {
                    let v: Value = serde_json::from_str(&params).unwrap_or(json!({}));
                    let p = parse_params(&v);
                    if let Err((field, reason)) = validate(&p) {
                        return Err(PluginError::InvalidInput(FieldFailure { field, reason }));
                    }
                    let mut host = Host { last_error: None };
                    match rigid_mould(&mut host, &p) {
                        Ok(v) => Ok(v.to_string()),
                        // A host refusal goes back as the host's own typed error.
                        // Cancellation and limits keep the host's own error; anything else says
                        // which step failed.
                        Err(e) => Err(match host.last_error.take() {
                            Some(PluginError::Cancelled) => PluginError::Cancelled,
                            Some(PluginError::LimitExceeded(m)) => PluginError::LimitExceeded(m),
                            Some(PluginError::PermissionDenied(m)) => PluginError::PermissionDenied(m),
                            _ => PluginError::CommandFailed(CommandFailure { command: "mould-maker.rigid-mould".into(), reason: e }),
                        }),
                    }
                }
                other => Err(PluginError::InvalidInput(FieldFailure {
                    field: "entry".into(),
                    reason: format!("unknown entry {other}"),
                })),
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
    }

    #[test]
    fn bad_inputs_name_their_field() {
        assert_eq!(validate(&parse_params(&json!({"bolt": "M7"}))).unwrap_err().0, "bolt");
        assert_eq!(validate(&parse_params(&json!({"sectors": 1}))).unwrap_err().0, "sectors");
        assert_eq!(validate(&parse_params(&json!({"flange_width": 14, "bolt": "M8"}))).unwrap_err().0, "flange_width");
    }
}
