# Mould Maker

A SolidCraft plugin that builds a bolt-together, 3D-printable rigid mould around a part. Made for
GFRC and concrete casting, where the part is cast with one face open: the face you pour into and
trowel.

Needs SolidCraft 0.41 or later (capability `commands@0.1`). Runs sandboxed: no files, no network,
no clock. Everything it makes is ordinary SolidCraft features, so the saved design opens without
the plugin.

## Two moulds

- **Plugins › Round mould…** for bowls, planters, rings, birdbaths, fountain basins: a jacket
  cut into sectors that pull off sideways, and a back plate with the plug that fills the part's
  cavity, cut into rings and sectors that lift off. Optional core pin for one side hole.
- **Plugins › Box mould…** for parts with flat or gently curved sides: four walls that pull
  straight off sideways and a back that lifts off, split on mitres at the corners.
- **Plugins › Silicone sleeve mould…** for parts a rigid mould can't release (flutes, texture,
  undercuts): a printed plug (the part), a sleeve split in two across the part's narrow sides,
  a silicone base and a casting base. Stand the plug on the silicone base, bolt the sleeve round it and pour silicone to the
  sleeve's rim; then turn it over onto the casting base, take the plug out and cast concrete into the silicone liner.
  The sleeve's inside follows the part's outline grown by the silicone thickness and tapers at no
  more than the part's own draft, with a lip at the open end. Round holes across the part get a
  hole through the sleeve for a rod (0.3 mm clearance); holes along it get a socket through the
  base plate. The sleeve is a shell that follows the silicone (3 mm by default) with hoops round
  it and root blocks under its ears; the base is a 4 mm plate the shape of the sleeve's foot. The
  run reports the silicone needed, the rods, and an estimate of the PLA each piece takes.
  Each base has a lug at each seam that the ear's end bolt goes through, so the stack bolts
  together; the silicone base has a sealing ridge that sits in a groove under the sleeve's foot
  (put a thin bead of plasticine or sealant in the groove); each seam has a pry slot at both
  ends for a flat screwdriver.

Both put flanges on the outside of every joint, never on a casting face, with clearance holes
for M4, M5, M6 or M8 bolts (or none, for clamps) and one dowel hole per joint, and size every
piece to your printer.

## Use

1. Select the part. To choose the open face, click it instead: a flat face of the part. With
   the whole part selected, the open face is its bottom face.
2. **Plugins › Round mould…** or **Box mould…**, check the fields, **Run**.

The part doesn't have to stand on its open face: the plugin turns a copy so the open face faces
down, builds the mould round it, and turns the pieces back. The result is one undo step.

Hover a field's name for what it does.

### Big round moulds: two runs

One plugin run may make at most 256 SolidCraft commands and must finish within 30 s. Round
mould counts its commands before touching the design. If the whole mould doesn't fit in one
run, it builds the jacket; run **Round mould…** again with the same part selected and it builds
the back plate. Each run is its own undo step. (SolidCraft 0.41 shows nothing when a plugin
finishes, so watch for the back plate missing.)

## Fields

| Field | Default | Meaning |
|---|---|---|
| Printer | K1 Max | K1 Max (300 × 300 × 300 mm), or Custom to use the bed fields |
| Bed width / depth / height | 300 mm | Build volume, when Printer is Custom |
| Wall | 20 mm | Mould thickness round the part |
| Back plate | 20 mm | Plate on the side opposite the open face |
| Flange width | 25 mm | How far the flanges stand out; at least 2.5 × the bolt hole |
| Flange thickness | 16 mm | Both flanges of a joint together |
| Bolts | M6 | Clearance holes: M4 4.5, M5 5.5, M6 6.6, M8 9.0 mm; None for clamps |
| Dowel pins | 6 mm | 0 for none |
| Sectors (round) | 0 | 0 = automatic; 2 to 64 forces a count |
| Core pin diameter / angle / height (round) | 0 | One side hole: diameter, direction from +X, centre height from the open face |
| Part body | (selection) | Body name if nothing is selected |

## Limits (v0.3)

- Round mould: round parts only. A square or strongly oval part is refused with a message
  pointing to Box mould. The plug must lift straight out: a cavity that narrows going up is
  reported, not fixed.
- Box mould: every face must let go of the piece that moulds it, pulling straight sideways or
  straight back. The plugin checks this first and names the place if not. Recesses in the open
  face (pad seats, say) can't be cast with an open mould: they come out solid. Walls are not
  split into strips yet: a piece bigger than the printer is refused.
- Silicone sleeve mould: the plug isn't drilled or changed, because parts imported from
  SolidWorks STEP can carry edge gaps SolidCraft 0.41's booleans refuse. The same bug stops
  Round and Box mould on such parts.
- Pieces are solid. Print them with low infill; hollowing is planned.
- No gussets, seal grooves or piece numbers yet.
- The part is read from a coarse mesh (about 1/600 of its size), and round splits keep that much
  margin plus 1.5 mm from the part's faces.
- Sand or coat the casting faces before use: printed layer lines show on the concrete.

## Build from source

    ./build.sh                 # Rust + wasm32-unknown-unknown target + wasm-tools
    BUILD_STD=1 ./build.sh     # without a prebuilt wasm32 std (uses -Zbuild-std)
    cargo test                 # analysis and layout unit tests
    SOLIDCRAFT_CLI=/path/to/solidcraft-cli python3 tests/cli_test.py                  # test bowl
    SOLIDCRAFT_CLI=... python3 tests/cli_test.py design.sc Body '{"core_pin_diameter": 50}'
    SOLIDCRAFT_CLI=... python3 tests/cli_test.py bookend.sc Bookend '{"entry": "box-mould"}'

`cargo run --release --example host_run -- <solidcraft-cli> <design.sc> '<params>'` (params may name an
`entry` and a `select`) runs the same
logic natively against `solidcraft-cli mcp --headless` and times every command.

## SolidCraft 0.41 notes found while building this

- `solid.mirror` makes invalid bodies of the tab shapes ("invalid B-rep record: flat arrays").
- Joins of bodies whose faces coincide exactly with each other's can fail ("Face Loop has zero
  area"); tabs stay 0.05 mm clear of the piece faces they meet.
- `body.rename` inside a plugin run, after an earlier plugin result was adopted, fails with
  "unknown body" (also renaming many pieces at once); regions are created under their final names.
- A taken `body_name` falls back to a session counter name (`Body18`); every body here gets a
  unique name.
- A boolean's cost is mostly per tool body and per hole, not per call, so holes are drilled into
  small tab bodies and each piece gets one join.
- Booleans on a body imported from SolidWorks STEP (edge gaps up to 0.001 mm, accepted on import)
  fail SolidCraft's own result check ("Vertex-Face Gap exceeds XSectTol3d", tolerance 0.00001 mm),
  and `body.repair` says there is nothing to repair.
- `model.mesh` and `model.faces` answer in the body's component coordinates, while
  `inspect.measure` and the selection answer in the design's. A part imported from STEP sits in
  a placed component (stood up from Y-up, say), so the plugin reads the placement from
  `component.list` and works on a placed copy.
- The manifest may be at most 16 KB and its description 240 characters.
- An edit entry that succeeds shows only "finished": its result, warnings and log are not shown,
  so a plugin can't tell the user what to do next.

## Licence

MIT
