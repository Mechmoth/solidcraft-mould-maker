"""Integration test: run the built plugin on a real SolidCraft host (headless MCP) and check the mould.

    SOLIDCRAFT_CLI=/path/to/solidcraft-cli python3 tests/cli_test.py [design.sc part-body params-json]

params-json may hold "entry" (round-mould, the default, or box-mould), "runs" (how many times to
run it: a big round mould builds its jacket, then its plate), "select" (select.set items in place
of the part body) and "expect_error" (a piece of the message the run must fail with).

Without arguments it builds a test bowl itself. Checks: one undo step, every piece a closed solid,
nothing of the mould inside the part, pieces do not overlap, pieces + part + holes fill the
envelope, every piece fits the printer.
"""
import json, os, subprocess, sys, time, itertools

HERE = os.path.dirname(os.path.abspath(__file__))
PKG = os.path.join(HERE, "..", "dist", "mould-maker")


class MCP:
    def __init__(self, cli, design=None, allow=()):
        args = [cli, "mcp", "--headless"] + (["--in", design] if design else [])
        for a in allow:
            args += ["--allow-path", a]
        self.p = subprocess.Popen(args, stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, text=True, bufsize=1)
        self.ids = itertools.count(1)
        self.rpc("initialize", {"protocolVersion": "2024-11-05", "capabilities": {}, "clientInfo": {"name": "mould-test", "version": "1"}})
        self.send({"jsonrpc": "2.0", "method": "notifications/initialized"})

    def send(self, m):
        self.p.stdin.write(json.dumps(m) + "\n"); self.p.stdin.flush()

    def rpc(self, method, params):
        i = next(self.ids)
        self.send({"jsonrpc": "2.0", "id": i, "method": method, "params": params})
        while True:
            line = self.p.stdout.readline()
            if not line:
                raise RuntimeError("host exited")
            m = json.loads(line)
            if m.get("id") == i:
                if "error" in m:
                    raise RuntimeError(m["error"])
                return m["result"]

    def tool(self, name, **args):
        r = self.rpc("tools/call", {"name": name, "arguments": args})
        out = next((c["text"] for c in r.get("content", []) if c.get("type") == "text"), None)
        try:
            out = json.loads(out)
        except Exception:
            pass
        if r.get("isError"):
            raise RuntimeError(f"{name}: {out}")
        return out

    def ex(self, command, **params):
        return self.tool("execute", command=command, params=params)


def check(cond, msg):
    print(("PASS " if cond else "FAIL ") + msg, flush=True)
    if not cond:
        check.failed = True
check.failed = False


def main():
    cli = os.environ.get("SOLIDCRAFT_CLI", "solidcraft-cli")
    design = sys.argv[1] if len(sys.argv) > 1 else None
    part = sys.argv[2] if len(sys.argv) > 2 else None
    extra = json.loads(sys.argv[3]) if len(sys.argv) > 3 else {}
    pkg = os.path.abspath(PKG)
    allow = [os.path.dirname(pkg)] + ([os.path.dirname(os.path.abspath(design))] if design else [])
    if os.environ.get("MOULD_TEST_OUT"):
        allow.append(os.environ["MOULD_TEST_OUT"])
    h = MCP(cli, design, allow=allow)
    if not design:
        # A 300 mm planter bowl: 180 tall, 15 mm floor, wall 20 tapering out, with a 10 mm top fillet.
        h.ex("sketch.create", plane="XZ", name="Profile")
        pts = [[0, 0], [140, 0], [160, -180], [140, -180], [122, -15], [0, -15]]  # sketch Y on XZ is world -Z
        for a, b in zip(pts, pts[1:] + pts[:1]):
            h.ex("sketch.line", points=[a, b])
        h.ex("sketch.finish")
        h.ex("solid.revolve", axis="Z", profiles="all", name="Bowl")
        part = h.tool("measure", names_only=True)["bodies"][0]["name"]
    ins = h.ex("plugin.inspect", path=pkg)
    check(ins.get("within_host_limits", True), f"inspect: within host limits ({ins.get('host_limits')})")
    entry = extra.pop("entry", "round-mould")
    runs = extra.pop("runs", 1)
    select = extra.pop("select", None) or [{"type": "body", "name": part}]
    expect_error = extra.pop("expect_error", None)
    h.ex("select.set", items=select)
    print("selection:", json.dumps(h.ex("selection.get", detail=False))[:200])
    before = h.tool("measure", summary=True)
    part_v = next(b["volume_mm3"] for b in before["bodies"] if b["name"] == part)
    params = {"printer": "K1 Max", "wall": 20, "back_plate": 20, "flange_width": 25, "flange_thickness": 16, "bolt": "M6", "dowel_diameter": 6}
    params.update(extra)
    if expect_error:
        try:
            h.ex("plugin.run", path=pkg, approval=ins["approval"], entry=entry, params=params)
            check(False, f"the run fails with '{expect_error}'")
        except RuntimeError as e:
            check(expect_error in str(e), f"the run fails with '{expect_error}': {str(e)[:400]}")
        after = h.tool("measure", names_only=True)
        check([b["name"] for b in after["bodies"]] == [b["name"] for b in before["bodies"]], "the design is untouched")
        print("FAILED" if check.failed else "ALL PASSED")
        sys.exit(1 if check.failed else 0)
    reps = []
    for i in range(runs):
        if i:
            print("bodies now:", [b["name"] for b in h.tool("measure", names_only=True)["bodies"]])
            h.ex("select.set", items=select)
        t = time.time()
        res = h.ex("plugin.run", path=pkg, approval=ins["approval"], entry=entry, params=params)
        dt = time.time() - t
        r1 = res.get("result", res)
        print(f"run {i + 1}:", json.dumps({k: v for k, v in r1.items() if k not in ("jacket", "plate_rings", "plate_centre", "piece_kinds", "sizes")}))
        check(dt < 30, f"run {i + 1} took {dt:.1f} s (host deadline 30 s)")
        reps.append(r1)
    if entry == "box-mould":
        return check_box(h, part, reps[-1], params)
    if entry == "silicone-mould":
        return check_silicone(h, part, reps[-1])
    check(reps[-1].get("next") is None, "the last run says nothing is left to build")
    rep = reps[-1]
    rep = dict(rep, jacket=[p for r in reps for p in r["jacket"]],
               plate_rings=[[p for r in reps for ring in r["plate_rings"][i:i+1] for p in ring] for i in range(max(len(r["plate_rings"]) for r in reps))],
               plate_centre=[p for r in reps for p in r["plate_centre"]],
               pieces=sum(r["pieces"] for r in reps))
    pieces = rep["jacket"] + [p for r in rep["plate_rings"] for p in r] + rep["plate_centre"]
    check(len(pieces) == rep["pieces"], f"{len(pieces)} pieces named")
    after = h.tool("measure", summary=True)
    bodies = {b["name"]: b for b in after["bodies"]}
    extra_bodies = sorted(set(bodies) - set(pieces) - {part})
    check(not extra_bodies, f"only the part and the pieces remain (extra: {extra_bodies})")
    pb = bodies[part]["bbox"]; print("part bbox", pb)
    check(all(bodies[p]["closed"] for p in pieces), "every piece is a closed solid")
    check(all(k["print"] for k in rep["piece_kinds"]), "every piece kind fits the printer: " +
          "; ".join(f"{k['kind']} x{k['count']}: {k['print']}" for k in rep["piece_kinds"]))
    inter = h.ex("inspect.interference", bodies=pieces + [part])
    clashes = [c for c in inter.get("interferences", inter.get("pairs", [])) if c.get("volume_mm3", 1) > 1.0]
    check(not clashes, f"no piece overlaps another or the part ({len(clashes)} clashes)")
    if clashes:
        print(json.dumps(clashes[:4]))
        print("interference keys:", list(inter.keys()))
    total = sum(bodies[p]["volume_mm3"] for p in pieces)
    print(f"mould volume {total/1e6:.2f} L, part {part_v/1e6:.2f} L")
    for _ in range(runs):
        h.tool("undo")
    back = h.tool("measure", summary=True)
    check([b["name"] for b in back["bodies"]] == [part], f"{runs} undo step(s) remove the whole mould")
    for _ in range(runs):
        h.tool("redo")
    pin = extra.get("core_pin_diameter", 0)
    if pin:
        import math
        a = math.radians(extra.get("core_pin_angle", 0)); z = pb["min"][2] + extra.get("core_pin_height", 0)
        cx = (pb["min"][0] + pb["max"][0]) / 2; cy = (pb["min"][1] + pb["max"][1]) / 2
        r0 = rep["layout"]["jacket_split"][0]["radius_mm"] - 30
        h.ex("solid.cylinder", diameter=pin - 2, height=40, base=[cx + r0 * math.cos(a), cy + r0 * math.sin(a), z],
             axis=[math.cos(a), math.sin(a), 0], body_name="Probe")
        probe = [b for b in h.tool("measure", names_only=True)["bodies"] if b["name"].startswith("Probe")][0]["name"]
        hit = h.ex("inspect.interference", bodies=[probe] + pieces)["interferences"]
        owners = sorted({c["a"] if c["b"] == probe else c["b"] for c in hit if probe in (c["a"], c["b"]) and c["volume_mm3"] > 1})
        check(len(owners) == 1 and owners[0].startswith("Mould jacket"), f"core pin: the side hole is filled by one jacket piece {owners}")
        h.ex("solid.remove", bodies=[probe])
    finish(h)


def finish(h):
    out = os.environ.get("MOULD_TEST_OUT")
    if out:
        h.tool("save", path=os.path.join(out, "mould-test.sc"))
        for view in ("iso", "bottom"):
            h.tool("screenshot", view=view, grid=False, axes=False, width=1400, height=1000, path=os.path.join(out, f"mould-{view}.png"))
    print("FAILED" if check.failed else "ALL PASSED")
    sys.exit(1 if check.failed else 0)


def check_silicone(h, part, rep):
    pieces = [rep["plug"]] + rep["sleeve"] + [rep["base"]]
    after = h.tool("measure", summary=True)
    bodies = {b["name"]: b for b in after["bodies"]}
    check(sorted(set(bodies) - {part}) == sorted(pieces), f"plug, two sleeve halves and base plate: {sorted(set(bodies) - {part})}")
    check(all(bodies[p]["closed"] for p in pieces), "every piece is a closed solid")
    check(abs(bodies[rep["plug"]]["volume_mm3"] - bodies[part]["volume_mm3"]) < 1.0, "the plug is the part")
    inter = h.ex("inspect.interference", bodies=pieces)
    clashes = [c for c in inter["interferences"] if c.get("volume_mm3", 1) > 0.01]
    check(not clashes, f"no piece overlaps another ({clashes[:3]})")
    for s in rep["sizes"]:
        check(all(a <= 300.01 for a in s["size_mm"]), f"{s['piece']} {s['size_mm']} fits the printer")
    print("silicone:", rep["silicone"], "rods:", rep["rods"], "warnings:", rep["warnings"])
    h.tool("undo")
    check([b["name"] for b in h.tool("measure", names_only=True)["bodies"]] == [part], "one undo step removes the whole mould")
    h.tool("redo")
    finish(h)


def check_box(h, part, rep, params):
    pieces = rep["walls"] + rep["back"]
    check(len(pieces) == rep["pieces"] and len(pieces) >= 5, f"{len(pieces)} pieces named")
    after = h.tool("measure", summary=True)
    bodies = {b["name"]: b for b in after["bodies"]}
    extra_bodies = sorted(set(bodies) - set(pieces) - {part})
    check(not extra_bodies, f"only the part and the pieces remain (extra: {extra_bodies})")
    check(all(bodies[p]["closed"] for p in pieces), "every piece is a closed solid")
    inter = h.ex("inspect.interference", bodies=pieces + [part])
    clashes = [c for c in inter["interferences"] if c.get("volume_mm3", 1) > 1.0]
    check(not clashes, f"no piece overlaps another or the part ({len(clashes)} clashes)")
    bed = sorted([300, 300, 300])
    for s in rep["sizes"]:
        check(all(a <= b + 0.01 for a, b in zip(sorted(s["size_mm"]), bed)), f"{s['piece']} {s['size_mm']} fits the printer")
    # The mould and part fill the outer box: nothing of the cavity is left open but the open face.
    total = sum(bodies[p]["volume_mm3"] for p in pieces)
    print(f"mould volume {total/1e6:.2f} L; warnings: {rep['warnings']}")
    h.tool("undo")
    check([b["name"] for b in h.tool("measure", names_only=True)["bodies"]] == [part], "one undo step removes the whole mould")
    h.tool("redo")
    finish(h)


if __name__ == "__main__":
    main()
