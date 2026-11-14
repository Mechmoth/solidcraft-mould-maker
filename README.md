# Mould Maker

A SolidCraft plugin that builds a bolt-together, 3D-printable rigid mould around a round part.
Made for GFRC and concrete casting, where the part is cast upside down and its bottom face is the
open face you screed or trowel.

Needs SolidCraft 0.41 or later (capability `commands@0.1`). Runs sandboxed: no files, no network,
no clock. Everything it makes is ordinary SolidCraft features, so the saved design opens without
the plugin.

## What you get

- **Jacket**: the outside wall, cut into sectors that pull off sideways.
- **Plate**: the back plate with the plug that fills the part's cavity, cut into rings and
  sectors that lift off upwards.
- **Flange tabs** on the outside of every joint, never on a casting face, with clearance holes
  for M4, M5, M6 or M8 bolts (2 or more per joint, about 150 mm apart) and one dowel hole per
  joint (dowel diameter + 0.1 mm).
- **Core pin** (optional) for one horizontal side hole, on the jacket piece that pulls straight
  out of the hole.
- **Sizes chosen for your printer**: with Sectors = 0 the plugin picks the layout with the fewest
  pieces that fit the bed, upright, turned on the bed, or on their side.

## Use

1. Model the part standing on its open face: the face you trowel is the lowest face, any cavity
   opens upwards, and the part is roughly round about a vertical axis.
2. Select the part (or type its body name in **Part body**).
3. **Plugins › Printed rigid mould…**, check the fields, **Run**.

The result is one undo step. Pieces are named `Mould jacket`, `Mould jacket (2)`…,
`Mould plate ring 1`…, `Mould plate centre`…. The run returns a report: layout, piece sizes and
how each prints, bolt and dowel counts, and warnings (for example a plug that would lock in, or a
side hole that needs a core pin).

### Big moulds: two runs

One plugin run may make at most 256 SolidCraft commands and must finish within 30 s. The plugin
counts its commands before touching the design. If a mould is too big for one run it stops with
a message saying so; set **Build** to `jacket` and run, then `plate` and run again. Each run is
its own undo step.

## Fields

| Field | Default | Meaning |
|---|---|---|
| Printer bed width / depth / height | 300 / 300 / 300 mm | Build volume |
| Wall | 20 mm | Mould wall outside the part's widest point |
| Back plate | 20 mm | Plate over the part's top |
| Flange width | 25 mm | How far the tabs stand off the mould |
| Flange thickness | 16 mm | Both tabs of a joint together |
| Bolts | M6 | Clearance holes: M4 4.5, M5 5.5, M6 6.6, M8 9.0 mm |
| Dowel pins | 6 mm | 0 for none |
| Build | everything | `jacket` or `plate` for a two-run build |
| Sectors | 0 | 0 = automatic; 2 to 64 forces a count |
| Part body | (selection) | Body name if nothing is selected |
| Core pin diameter / angle / height | 0 | One side hole: diameter, direction from +X, centre height above the bottom face |

## Limits (v0.1)

- Round parts only (bowls, planters, rings, birdbaths, fountain basins). A square or strongly
  oval part is refused with a message: the jacket split radius must fit inside the wall all the
  way round.
- The plug must lift straight up: a cavity that narrows going up is reported as a warning, not
  fixed.
- Pieces are solid. Print them with low infill; hollowing is planned.
- The part is read from a coarse mesh (about 1/600 of its size), and every split keeps that much
  margin plus 1.5 mm from the part's faces.

## Build from source

    ./build.sh                 # Rust + wasm32-unknown-unknown target + wasm-tools
    BUILD_STD=1 ./build.sh     # without a prebuilt wasm32 std (uses -Zbuild-std)
    cargo test                 # analysis and layout unit tests
    SOLIDCRAFT_CLI=/path/to/solidcraft-cli python3 tests/cli_test.py                  # test bowl
    SOLIDCRAFT_CLI=... python3 tests/cli_test.py design.sc Body '{"core_pin_diameter": 50}'

`cargo run --release --example host_run -- <solidcraft-cli> <design.sc> '<params>'` runs the same
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

## Licence

MIT
