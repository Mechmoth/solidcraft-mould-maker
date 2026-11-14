//! Emits the SolidCraft commands that build the mould. Every step is an ordinary command, so the
//! pieces land on the timeline as native features: the saved design needs no plugin.
//!
//! Order: envelope minus part (the cavity), regions (jacket for radial pull, plate rings for Z
//! pull), sector wedges (built once, patterned), pieces (region AND wedge), then the flange
//! tabs: for the first sector of each region, built from boxes and tubes, clipped to the sector
//! and drilled, then patterned round the axis and joined to each piece. Then tidy up.
//!
//! Two host limits shape this. Calls are capped at 256, so anything repeated round the axis is
//! built once and patterned. The whole run must finish in 30 s, and a boolean on a piece with
//! the part's freeform faces costs far more than one on plain boxes and tubes (about 35 ms per
//! hole), so the tabs and holes never touch those bodies until one join per piece.

use serde_json::{json, Value};

use crate::analysis::Band;
use crate::layout::Layout;
use crate::Runner;

#[derive(Clone)]
pub struct Pin {
    /// Angle round the axis, degrees from +X.
    pub deg: f64,
    /// Absolute height of the hole's centre.
    pub z: f64,
    pub d: f64,
    /// Radius where the pin starts, inside the plug.
    pub start: f64,
}

/// Which regions one run builds: a big mould can be built in two runs.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Build {
    Everything,
    Jacket,
    Plate,
}

#[derive(Clone)]
pub struct Spec {
    pub part: String,
    pub c: [f64; 2],
    pub zmin: f64,
    pub zmax: f64,
    /// Envelope radius, tab outer radius, tab half thickness, tab width.
    pub r: f64,
    pub ro: f64,
    pub f: f64,
    pub fw: f64,
    /// Back of the plate, back of the tabs, jacket/plate split.
    pub ztop: f64,
    pub zb: f64,
    pub zj: f64,
    pub bands: Vec<Band>,
    pub layout: Layout,
    /// First sector boundary, degrees.
    pub theta0: f64,
    pub bolt_d: f64,
    pub dowel_d: f64,
    pub pin: Option<Pin>,
    pub build: Build,
    /// Bodies already in the design, so new names never clash.
    pub existing: Vec<String>,
}

pub struct Built {
    pub jacket: Vec<String>,
    pub rings: Vec<Vec<String>>,
    pub disk: Vec<String>,
    pub bolts: usize,
    pub dowels: usize,
    pub calls: usize,
    pub warnings: Vec<String>,
}

struct B<'a, R: Runner> {
    r: &'a mut R,
    s: &'a Spec,
    calls: usize,
    helpers: Vec<String>,
    /// Host compute time reported by the commands so far (their `ms`).
    ms: f64,
    /// Counter for unique body names.
    k: usize,
}

pub(crate) fn created(v: &Value) -> Vec<String> {
    v.get("created_bodies")
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect())
        .unwrap_or_default()
}

impl<'a, R: Runner> B<'a, R> {
    fn run(&mut self, cmd: &str, args: Value) -> Result<Value, String> {
        self.calls += 1;
        let v = self.r.run(cmd, args)?;
        self.ms += v.get("ms").and_then(Value::as_f64).unwrap_or(0.0);
        Ok(v)
    }
    fn stage(&mut self, fraction: f32, what: &str) {
        let msg = format!("{what} (after {} calls, {:.0} ms of host work)", self.calls, self.ms);
        self.r.log(&msg);
        self.r.progress(fraction, what);
    }
    /// A body name no other body has. The host gives a taken name a counter name instead
    /// ("Body18"), and its counters differ between the plugin's scratch session and the design
    /// the result is adopted into, so every body this plugin makes gets its own unique name.
    fn uname(&mut self, base: &str) -> String {
        self.k += 1;
        format!("{base} mm{}", self.k)
    }
    /// A body under exactly this name (the caller made sure no other body has it).
    fn named(&mut self, cmd: &str, args: Value) -> Result<String, String> {
        let v = self.run(cmd, args)?;
        created(&v).into_iter().next().ok_or_else(|| format!("{cmd} made no body"))
    }
    fn first(&mut self, cmd: &str, mut args: Value) -> Result<String, String> {
        if let Some(n) = args.get("body_name").and_then(Value::as_str).map(String::from) {
            args["body_name"] = json!(self.uname(&n));
        }
        let v = self.run(cmd, args)?;
        created(&v).into_iter().next().ok_or_else(|| format!("{cmd} made no body"))
    }
    fn axis(&self) -> Value {
        json!({"origin": [self.s.c[0], self.s.c[1], 0.0], "dir": [0.0, 0.0, 1.0]})
    }
    /// A cylinder on the mould axis.
    fn cyl(&mut self, name: &str, radius: f64, z0: f64, z1: f64) -> Result<String, String> {
        let c = self.s.c;
        self.first("solid.cylinder", json!({"radius": radius, "height": z1 - z0, "base": [c[0], c[1], z0], "body_name": name}))
    }
    /// A tube on the mould axis.
    fn tube(&mut self, name: &str, r_in: f64, r_out: f64, z0: f64, z1: f64) -> Result<String, String> {
        let o = self.cyl(name, r_out, z0, z1)?;
        let i = self.cyl(&format!("{name} bore"), r_in, z0 - 5.0, z1 + 5.0)?;
        self.combine(&o, &[i], "cut", false)?;
        Ok(o)
    }
    fn combine(&mut self, target: &str, tools: &[String], op: &str, keep: bool) -> Result<Value, String> {
        self.run("solid.combine", json!({"target": target, "tools": tools, "operation": op, "keep_tools": keep}))
    }
    /// Join into one body even where the pieces do not touch (a set of separate holes).
    fn join_all(&mut self, target: &str, tools: &[String]) -> Result<(), String> {
        if !tools.is_empty() {
            self.run("solid.combine", json!({"target": target, "tools": tools, "operation": "join", "touching": "one_body"}))?;
        }
        Ok(())
    }
    fn copy(&mut self, body: &str) -> Result<String, String> {
        self.first("solid.move", json!({"bodies": [body], "translate": [0, 0, 0], "copy": true}))
    }
    fn turn(&mut self, bodies: &[String], deg: f64) -> Result<(), String> {
        if deg.abs() > 1e-9 {
            let c = self.s.c;
            self.run("solid.move", json!({"bodies": bodies, "axis": [0, 0, 1], "origin": [c[0], c[1], 0.0], "angle": format!("{deg} deg")}))?;
        }
        Ok(())
    }
    /// Copies round the axis; returns the original and its copies.
    fn pattern(&mut self, body: &str, count: usize) -> Result<Vec<String>, String> {
        let mut all = vec![body.to_string()];
        if count > 1 {
            let v = self.run("solid.pattern.circular", json!({"bodies": [body], "axis": self.axis(), "count": count}))?;
            all.extend(created(&v));
        }
        Ok(all)
    }
    /// A box in the frame of the mould axis: x along the ray at 0°, y to its left.
    fn local_box(&mut self, name: &str, x0: f64, y0: f64, z0: f64, l: f64, w: f64, h: f64) -> Result<String, String> {
        let c = self.s.c;
        self.first("solid.box", json!({"length": l, "width": w, "height": h, "corner": [c[0] + x0, c[1] + y0, z0], "body_name": name}))
    }
    /// A world point at radius r, angle deg, height z.
    fn at(&self, r: f64, deg: f64, z: f64) -> [f64; 3] {
        let a = deg.to_radians();
        [self.s.c[0] + r * a.cos(), self.s.c[1] + r * a.sin(), z]
    }
}

/// Evenly spaced positions in [a, b], about `pitch` apart, at least `min`.
pub(crate) fn spread(a: f64, b: f64, pitch: f64, min: usize) -> Vec<f64> {
    if b < a {
        return vec![];
    }
    let n = (((b - a) / pitch).ceil() as usize + 1).max(min);
    if n == 1 {
        return vec![0.5 * (a + b)];
    }
    (0..n).map(|i| a + (b - a) * i as f64 / (n - 1) as f64).collect()
}

/// Midpoints between consecutive positions (one dowel per gap), or the middle if only one.
pub(crate) fn between(v: &[f64]) -> Vec<f64> {
    if v.len() < 2 {
        return v.to_vec();
    }
    v.windows(2).map(|w| 0.5 * (w[0] + w[1])).collect()
}

pub fn build<R: Runner>(r: &mut R, s: &Spec) -> Result<Built, String> {
    let mut b = B { r, s, calls: 0, helpers: Vec::new(), ms: 0.0, k: 0 };
    let mut warnings: Vec<String> = Vec::new();
    let l = &s.layout;
    let n = l.sectors;
    let d = l.disk_sectors;
    let span = 360.0 / n as f64;
    let (f, fw) = (s.f, s.fw);
    let zlo = s.zmin;
    let t0 = s.theta0;

    // ---------- names ----------
    // Every region is made under its final name, so sector copies come out as `Name (2)`,
    // `Name (3)`… by themselves. (body.rename inside a plugin run, after an earlier plugin result
    // was adopted, trips a name-following bug in SolidCraft 0.41, so nothing here renames.)
    let existing = &s.existing;
    let free = |base: &str| -> String {
        let clash = |c: &str| existing.iter().any(|e| e == c || e.starts_with(&format!("{c} (")));
        if !clash(base) {
            return base.to_string();
        }
        (2..).map(|k| format!("{base} #{k}")).find(|c| !clash(c)).unwrap_or_else(|| base.to_string())
    };
    let want_jacket = s.build != Build::Plate;
    let want_plate = s.build != Build::Jacket;

    // ---------- regions ----------
    b.stage(0.1, "jacket and plate regions");
    let far = s.ro + 20.0;
    let mut jt_parts = Vec::new();
    let mut z0 = zlo - 10.0;
    for (i, band) in s.bands.iter().enumerate() {
        let last = i + 1 == s.bands.len();
        let top = if last { band.z_hi } else { band.z_hi + 0.25 };
        jt_parts.push(b.tube(&format!("Jacket region {i}"), band.r, far + 2.0 * i as f64, z0, top)?);
        z0 = band.z_hi - 0.25;
    }
    if let Some(p) = &s.pin {
        let a = p.deg.to_radians();
        let dir = [a.cos(), a.sin(), 0.0];
        let base = b.at(p.start, p.deg, p.z);
        jt_parts.push(b.first(
            "solid.cylinder",
            json!({"diameter": p.d + 0.4, "height": far - p.start, "base": base, "axis": dir, "body_name": "Core pin"}),
        )?);
    }
    let jt = jt_parts[0].clone();
    if jt_parts.len() > 1 {
        b.combine(&jt, &jt_parts[1..], "join", false)?;
    }
    b.helpers.push(jt.clone());
    // A region: the envelope between radii r0..r1 under its final name, minus the part, then
    // inside (jacket) or outside (plate) the jacket tool.
    let region = |b: &mut B<R>, name: &str, r0: f64, r1: f64, jacket: bool| -> Result<String, String> {
        let c = s.c;
        let body = b.named("solid.cylinder", json!({"radius": r1, "height": s.ztop - zlo, "base": [c[0], c[1], zlo], "body_name": name}))?;
        if r0 > 0.0 {
            let bore = b.cyl("Region bore", r0, zlo - 5.0, s.ztop + 5.0)?;
            b.combine(&body, &[bore], "cut", false)?;
        }
        b.combine(&body, &[s.part.clone()], "cut", true)?;
        let loose = created(&b.combine(&body, &[jt.clone()], if jacket { "intersect" } else { "cut" }, true)?);
        if !loose.is_empty() {
            return Err(if jacket {
                "The jacket would come apart in separate rings because the part's outside isn't one continuous wall. \
                 Use Plugins › Box mould…, or fill the gap in the part's outside."
                    .to_string()
            } else {
                format!(
                    "The back plate would come apart in {} pieces, so it couldn't lift off. The open face is probably \
                     wrong: select the face you trowel before running, or stand the part on it.",
                    loose.len() + 1
                )
            });
        }
        Ok(body)
    };
    let jacket = if want_jacket { region(&mut b, &free("Mould jacket"), 0.0, s.r, true)? } else { String::new() };
    let mut ring_bodies = Vec::new();
    let mut disk = String::new();
    if want_plate {
        let mut edges = vec![s.r];
        edges.extend(l.rings.iter().cloned());
        for i in 0..l.rings.len() {
            ring_bodies.push(region(&mut b, &free(&format!("Mould plate ring {}", i + 1)), edges[i + 1], edges[i], false)?);
        }
        disk = region(&mut b, &free("Mould plate centre"), 0.0, *edges.last().unwrap(), false)?;
    }

    // ---------- wedges and pieces ----------
    let wedges = |b: &mut B<R>, count: usize| -> Result<Vec<String>, String> {
        if count == 1 {
            return Ok(vec![]);
        }
        let big = s.ro + 200.0;
        let (z0, h) = (zlo - 60.0, s.zb - zlo + 120.0);
        let w_span = 360.0 / count as f64;
        let a = b.local_box("Wedge", -big, 0.0, z0, 2.0 * big, big, h)?;
        b.turn(&[a.clone()], t0)?;
        if count > 2 {
            let c = b.local_box("Wedge side", -big, -big, z0, 2.0 * big, big, h)?;
            b.turn(&[c.clone()], t0 + w_span)?;
            b.combine(&a, &[c], "intersect", false)?;
        }
        let all = b.pattern(&a, count)?;
        b.helpers.extend(all.iter().cloned());
        Ok(all)
    };
    // One entry per sector, in sector order. A sector that comes out in several lumps (a slot or
    // hole on its edge) keeps every lump as its own piece, with a warning.
    let cut_up = |b: &mut B<R>, region: &str, ws: &[String], warnings: &mut Vec<String>| -> Result<Vec<Vec<String>>, String> {
        if ws.is_empty() {
            return Ok(vec![vec![region.to_string()]]);
        }
        let mut out = Vec::new();
        for (i, w) in ws.iter().enumerate() {
            let piece = if i + 1 == ws.len() { region.to_string() } else { b.copy(region)? };
            let extra = created(&b.combine(&piece, &[w.clone()], "intersect", true)?);
            if !extra.is_empty() {
                warnings.push(format!("a sector came out in {} lumps; each is its own piece", extra.len() + 1));
            }
            let mut v = vec![piece];
            v.extend(extra);
            out.push(v);
        }
        Ok(out)
    };
    b.stage(0.25, "jacket pieces");
    let need_n = want_jacket || !ring_bodies.is_empty() || (want_plate && d == n);
    let w_n = if need_n { wedges(&mut b, n)? } else { vec![] };
    let jacket_pieces = if want_jacket { cut_up(&mut b, &jacket, &w_n, &mut warnings)? } else { vec![] };
    let mut ring_pieces = Vec::new();
    for (i, rb) in ring_bodies.iter().enumerate() {
        b.stage(0.35 + 0.1 * i as f32 / l.rings.len().max(1) as f32, "plate ring pieces");
        ring_pieces.push(cut_up(&mut b, rb, &w_n, &mut warnings)?);
    }
    b.stage(0.45, "centre pieces");
    let w_d = if !want_plate { vec![] } else if d == n { w_n.clone() } else { wedges(&mut b, d)? };
    let disk_pieces = if want_plate { cut_up(&mut b, &disk, &w_d, &mut warnings)? } else { vec![] };

    // ---------- flange tabs with bolt and dowel holes ----------
    // Each region's tabs are built for its first sector from boxes and tubes, clipped to the
    // sector and drilled, then copied round the axis. (Building half a sector and mirroring it
    // would halve this, but solid.mirror makes invalid bodies of these shapes in SolidCraft 0.41.)
    b.stage(0.55, "flange tabs");
    let rt = s.r + fw / 2.0;
    let zt = s.ztop + fw / 2.0;
    let ov = 2.0; // tabs bite into the mould wall so each join overlaps
    let ray_n = |deg: f64| {
        let a = deg.to_radians();
        [-a.sin(), a.cos(), 0.0]
    };
    let rad = |deg: f64| {
        let a = deg.to_radians();
        [a.cos(), a.sin(), 0.0]
    };
    type Hole = (bool, [f64; 3], [f64; 3]);
    let (cx, cy) = (s.c[0], s.c[1]);
    let at = move |r_: f64, deg: f64, z: f64| -> [f64; 3] {
        let a = deg.to_radians();
        [cx + r_ * a.cos(), cy + r_ * a.sin(), z]
    };
    // One dowel per joint, between its first two bolts.
    let dowel_of = |v: &[f64]| -> Vec<f64> { if s.dowel_d > 0.0 { between(v).into_iter().take(1).collect() } else { vec![] } };
    // Bolts and a dowel at `pos` along the edge at angle `edge`.
    let edge_row = |pos: &[f64], edge: f64, place: &dyn Fn(f64, f64) -> [f64; 3]| -> Vec<Hole> {
        let mut v: Vec<Hole> = pos.iter().map(|x| (true, place(*x, edge), ray_n(edge))).collect();
        v.extend(dowel_of(pos).into_iter().map(|x| (false, place(x, edge), ray_n(edge))));
        v
    };
    let arc = |radius: f64| (35.0 / radius).to_degrees().min(span / 3.0);
    let ring_tab_holes = |c: f64| -> Vec<Hole> {
        let mut v = vec![(true, at(rt, c, s.zj), [0.0, 0.0, 1.0])];
        if s.dowel_d > 0.0 {
            v.push((false, at(rt, c + arc(rt), s.zj), [0.0, 0.0, 1.0]));
        }
        v
    };
    let split_holes = |rs: f64, c: f64| -> Vec<Hole> {
        let mut v = vec![(true, at(rs, c, zt), rad(c))];
        if s.dowel_d > 0.0 {
            let a = c + arc(rs);
            v.push((false, at(rs, a, zt), rad(a)));
        }
        v
    };
    // A half-slab beside the sector edge at angle `edge`, on the sector's side (`left`: the
    // first edge, whose sector lies counter-clockwise of it).
    let half_slab = |b: &mut B<R>, edge: f64, left: bool, r0: f64, r1: f64, z0: f64, z1: f64| -> Result<String, String> {
        let y0 = if left { 0.0 } else { -f };
        let bx = b.local_box("Tab", r0, y0, z0, r1 - r0, f, z1 - z0)?;
        b.turn(&[bx.clone()], edge)?;
        Ok(bx)
    };
    // Join parts, clip to the first sector, drill, and copy round the axis.
    let finish = |b: &mut B<R>, parts: Vec<String>, wedge: Option<String>, holes: Vec<Hole>, count: usize| -> Result<Vec<String>, String> {
        let t = parts[0].clone();
        b.join_all(&t, &parts[1..])?;
        if let Some(w) = wedge {
            b.combine(&t, &[w], "intersect", true)?;
        }
        // All holes as one tool and one cut: drilling them one by one gets slower with every
        // hole already in the body.
        let len = 2.0 * s.f + 6.0;
        let mut drills = Vec::new();
        for (is_bolt, p, ax) in &holes {
            let dia = if *is_bolt { s.bolt_d } else { s.dowel_d };
            if dia <= 0.0 {
                continue;
            }
            let base = [p[0] - ax[0] * len / 2.0, p[1] - ax[1] * len / 2.0, p[2] - ax[2] * len / 2.0];
            drills.push(b.first("solid.cylinder", json!({"diameter": dia, "height": len, "base": base, "axis": ax, "body_name": "Drill"}))?);
        }
        if !drills.is_empty() {
            b.join_all(&drills[0], &drills[1..])?;
            b.combine(&t, &[drills[0].clone()], "cut", false)?;
        }
        b.pattern(&t, count)
    };
    // Tab surfaces stay a hair inside the piece's own boundaries (split planes, split cylinders,
    // the jacket/plate plane, the open face): faces that coincide exactly with the piece's make
    // the kernel's join fragile (zero-area loops). The only contact is the 2 mm bite into the wall.
    let eps = 0.05;
    let tab_wedge = |b: &mut B<R>, count: usize| -> Result<Option<String>, String> {
        if count == 1 {
            return Ok(None);
        }
        let big = s.ro + 200.0;
        let (z0, h) = (zlo - 60.0, s.zb - zlo + 120.0);
        let w = 360.0 / count as f64;
        let inset = 0.15;
        let a = b.local_box("Tab clip", -big, 0.0, z0, 2.0 * big, big, h)?;
        b.turn(&[a.clone()], t0 + inset)?;
        if w < 180.0 {
            let c = b.local_box("Tab clip side", -big, -big, z0, 2.0 * big, big, h)?;
            b.turn(&[c.clone()], t0 + w - inset)?;
            b.combine(&a, &[c], "intersect", false)?;
        } else {
            let c = b.local_box("Tab clip side", -big, -big, z0, 2.0 * big, big, h)?;
            b.turn(&[c.clone()], t0 + w - inset)?;
            b.combine(&a, &[c], "intersect", false)?;
        }
        b.helpers.push(a.clone());
        Ok(Some(a))
    };
    let clip_n = if need_n { tab_wedge(&mut b, n)? } else { None };
    let mut bolts = 0usize;
    let mut dowels = 0usize;
    // Count each joint once: an edge row is shared by two pieces, a ring or split tab's holes
    // by the pieces either side of it.
    let has_bolts = s.bolt_d > 0.0;
    let mut count = |holes: &[Hole], joints: usize| {
        bolts += if has_bolts { joints * holes.iter().filter(|h| h.0).count() } else { 0 };
        dowels += joints * holes.iter().filter(|h| !h.0).count();
    };
    let jz = spread(zlo + 25.0, s.zj - f - 15.0, 200.0, 2);
    let mut edges = vec![s.ro];
    edges.extend(l.rings.iter().cloned());
    let r_disk = *edges.last().unwrap();
    let mid = t0 + span / 2.0;

    // Jacket: tabs along both edges below the ring tab, and the ring tab's lower half.
    let mut tabs_j = Vec::new();
    if want_jacket {
        let mut parts = Vec::new();
        let mut holes = Vec::new();
        if n > 1 {
            parts.push(half_slab(&mut b, t0, true, s.r - ov, s.ro, zlo + eps, s.zj - eps)?);
            parts.push(half_slab(&mut b, t0 + span, false, s.r - ov, s.ro, zlo + eps, s.zj - eps)?);
            let e = edge_row(&jz, t0, &|z, a| at(rt, a, z));
            count(&e, n);
            holes.extend(e);
            holes.extend(edge_row(&jz, t0 + span, &|z, a| at(rt, a, z)));
        }
        parts.push(b.tube("Tab ring", s.r - ov, s.ro, s.zj - f, s.zj - eps)?);
        let h = ring_tab_holes(mid);
        count(&h, n);
        holes.extend(h);
        tabs_j = finish(&mut b, parts, clip_n.clone(), holes, n)?;
    }

    // Plate rings, outermost first: back tabs along both edges, the ring tab's upper half and the
    // outer tabs above it (outermost), and half of each cylindrical split tab they meet.
    let mut tabs_r = Vec::new();
    for i in 0..ring_bodies.len() {
        let (r_out, r_in) = (edges[i], edges[i + 1]);
        let mut parts = Vec::new();
        let mut holes = Vec::new();
        if n > 1 {
            // Ends half a tab thickness into each split's tube: a box ending at the split radius
            // would poke its corners past the curved split into the next ring, and one ending
            // exactly on the tube would only touch it along a line.
            let r_end = if i == 0 { s.ro } else { r_out - f / 2.0 };
            for (edge, left) in [(t0, true), (t0 + span, false)] {
                parts.push(half_slab(&mut b, edge, left, r_in + f / 2.0, r_end, s.ztop - ov, s.zb)?);
                if i == 0 {
                    parts.push(half_slab(&mut b, edge, left, s.r - ov, s.ro, s.zj + eps, s.ztop)?);
                }
            }
            let pos = spread(r_in + f + 15.0, r_out - 15.0, 150.0, 2);
            let e = edge_row(&pos, t0, &|r_, a| at(r_, a, zt));
            count(&e, n);
            holes.extend(e);
            holes.extend(edge_row(&pos, t0 + span, &|r_, a| at(r_, a, zt)));
        }
        if i == 0 {
            parts.push(b.tube("Tab ring", s.r - ov, s.ro, s.zj + eps, s.zj + f)?);
            holes.extend(ring_tab_holes(mid));
        } else {
            parts.push(b.tube("Tab split", r_out - f, r_out - eps, s.ztop - ov, s.zb)?);
            holes.extend(split_holes(r_out, mid));
        }
        parts.push(b.tube("Tab split", r_in + eps, r_in + f, s.ztop - ov, s.zb)?);
        let h = split_holes(r_in, mid);
        count(&h, n);
        holes.extend(h);
        tabs_r.push(finish(&mut b, parts, clip_n.clone(), holes, n)?);
    }

    // Centre: back tabs along its own edges, and its half of the innermost split tab (or, with no
    // rings, everything the plate carries at the jacket: outer tabs and the ring tab's upper half).
    let mut tabs_d = Vec::new();
    if want_plate {
        let dw = 360.0 / d as f64;
        let mut parts = Vec::new();
        let mut holes = Vec::new();
        if d > 1 {
            let r_end = if l.rings.is_empty() { s.ro } else { r_disk - f / 2.0 };
            for (edge, left) in [(t0, true), (t0 + dw, false)] {
                parts.push(half_slab(&mut b, edge, left, -f, r_end, s.ztop - ov, s.zb)?);
                if l.rings.is_empty() {
                    parts.push(half_slab(&mut b, edge, left, s.r - ov, s.ro, s.zj + eps, s.ztop)?);
                }
            }
            let pos = spread(40.0, r_disk - f - 15.0, 150.0, 2);
            let e = edge_row(&pos, t0, &|r_, a| at(r_, a, zt));
            count(&e, d);
            holes.extend(e);
            holes.extend(edge_row(&pos, t0 + dw, &|r_, a| at(r_, a, zt)));
        }
        match l.rings.last() {
            Some(rs) => parts.push(b.tube("Tab split", rs - f, *rs - eps, s.ztop - ov, s.zb)?),
            None => parts.push(b.tube("Tab ring", s.r - ov, s.ro, s.zj + eps, s.zj + f)?),
        }
        for k in 0..(n / d) {
            let c = mid + k as f64 * span;
            let h = match l.rings.last() {
                Some(rs) => split_holes(*rs, c),
                None => ring_tab_holes(c),
            };
            // Counted already with the jacket (ring tab) or the outer ring (split tab), unless
            // this run builds only the plate.
            if !want_jacket && l.rings.is_empty() {
                count(&h, d);
            }
            holes.extend(h);
        }
        let clip_d = if d == n { clip_n.clone() } else { tab_wedge(&mut b, d)? };
        tabs_d = finish(&mut b, parts, clip_d, holes, d)?;
    }

    // Join each piece's tabs.
    b.stage(0.8, "joining tabs");
    let join = |b: &mut B<R>, piece: &str, tab: &str| -> Result<(), String> {
        b.combine(piece, &[tab.to_string()], "join", false).map(|_| ())
    };
    for (k, lumps) in jacket_pieces.iter().enumerate() {
        join(&mut b, &lumps[0], &tabs_j[k % tabs_j.len()])?;
    }
    for (i, rp) in ring_pieces.iter().enumerate() {
        for (k, lumps) in rp.iter().enumerate() {
            join(&mut b, &lumps[0], &tabs_r[i][k % tabs_r[i].len()])?;
        }
    }
    for (j, lumps) in disk_pieces.iter().enumerate() {
        join(&mut b, &lumps[0], &tabs_d[j % tabs_d.len()])?;
    }

    // ---------- tidy ----------
    b.stage(0.95, "naming pieces");
    let helpers = std::mem::take(&mut b.helpers);
    if !helpers.is_empty() {
        b.run("solid.remove", json!({"bodies": helpers}))?;
    }
    let flat = |pieces: &[Vec<String>]| -> Vec<String> { pieces.iter().flatten().cloned().collect() };
    let jacket_named = flat(&jacket_pieces);
    let rings_named: Vec<Vec<String>> = ring_pieces.iter().map(|rp| flat(rp)).collect();
    let disk_named = flat(&disk_pieces);
    b.stage(1.0, "done");
    Ok(Built { jacket: jacket_named, rings: rings_named, disk: disk_named, bolts, dowels, calls: b.calls, warnings })
}
