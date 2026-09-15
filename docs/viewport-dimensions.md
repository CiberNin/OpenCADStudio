# Dimensions through paper-space viewports

Paper-space point commands can snap to displayed model geometry through a planar,
orthographic layout viewport. The acquired result retains the original model
coordinate, its paper coordinate, the viewport frame, and feature identity.
Ordinary selection continues to operate on the active drawing space.

`DIMLINEAR`, `DIMALIGNED`, `DIMANGULAR`, `DIMRADIUS`, and `DIMDIAMETER` use this
acquisition context. The supported object-pick modes also query viewport geometry.
Snapping and object picking share viewport draw order, visibility, and clipping
rules. Object picking uses the resident interaction index and applies block
instance transforms. Nonuniformly transformed circles and circular arcs are
declined as radial object picks because their displayed geometry is elliptical.

## Input and measurement rules

- Free and typed sheet points measure paper units, including points inside a
  viewport rectangle.
- Measuring points must all belong to the same space and, when applicable, the
  same viewport. An incompatible pick leaves the command at its current step.
- Dimension-line and text placement do not change the measurement's space.
- Paper geometry takes priority over viewport geometry at an object pick.
- The final dimension style is applied before computing measurement scaling.
  Length dimensions keep paper-space definition points and a negative `DIMLFAC`
  override containing the effective user factor multiplied by `1 / viewport scale`.
  Angular measurements do not use this length factor.
- `DIMASSOC=0` uses the same compensated value before exploding the annotation.

## Persistent measurement data

`OCS_VIEWPORT_MEASUREMENT` XData stores three values: version `1` (integer16),
the positive user factor (real), and viewport compensation (real). This keeps the
user's unit conversion separate from viewport scale. The ordinary `DIMLFAC`
override remains sufficient for displaying the saved measurement in other CAD
readers. The data survives DXF/DWG saves and undo/redo.

With `DIMASSOC=2`, acquired features attach a dimension to its viewport, each
block instance, and the source entity. Intersections retain both source chains.
Source edits and viewport changes update the definition points and recompute
compensation while preserving explicit user-positioned sheet text and dimension
style overrides. A successful update invalidates the saved picture block so the
new geometry and number are rendered and exported. Undo restores both the
reference data and the previous picture.

Properties reports whether the dimension is associated, partially associated,
unresolved, broken, or nonassociative. A broken or unsupported required reference
preserves the entire dimension's last valid appearance and its original records.
Creating another dimension does not announce geometry changes to its sources.

## Imported references and support limits

The resolver distinguishes the application's vertex/parameter convention from
imported edge references. Imported start/end and midpoint modes use their edge
marker and parameter together. The original regression drawing's first-edge
start/end pair now reproduces its `0.128` measurement. An imported polyline
endpoint beyond the first edge requires a usable stored point to validate the
marker; zero/sentinel data cannot establish that identity. Ambiguous imports
remain unresolved. Imported viewport scale is updated only when existing OCS
metadata or `ACAD_DIMASSOC_CALC_DIMLFAC` separates compensation from user scaling.

Unsupported cases retain their saved data and appearance:

- Perspective/oblique viewport projections and malformed/missing reference chains.
- Ambiguous intersections within one batched block; they are not attached to a
  guessed pair of children. Intersections between separate identified chains work.
- Array inserts without an addressable instance index, nesting beyond eight levels,
  and radial measurements of nonuniformly transformed circular geometry.
- Imported quadrant references whose moved geometry no longer validates the stored
  point; newly acquired circle/arc quadrants keep their source-local angle.
- Tangency with no verified solution, two dependent perpendicular/tangent references
  requiring a coupled solve, unsupported snap modes, and imported measurement data that does not identify the old viewport compensation.

The osnap codes and reference fields follow the
[Autodesk DIMASSOC reference](https://help.autodesk.com/cloudhelp/2024/ENU/AutoCAD-DXF/files/GUID-C0B96256-A911-4B4D-85E6-EB4AF2C91E27.htm).
The restricted imported-marker interpretation above is validated by fixtures;
it is not a claim that every exporter uses an interchangeable marker scheme.

## Validation

`src/app/viewport_dimension_tests.rs` exercises snap-engine acquisition, command
commits for all five commands, object picks, input retries, active positive and
negative style factors, exploded dimensions, undo/redo, DXF/DWG persistence,
clipping, overlapping viewports, pan, scale, and twist. These are headless tests;
they do not establish visual acceptance in the desktop UI.

Association tests also exercise regeneration, nearest and quadrant parameters,
angular arc/polyline object picks, spline tangency,
perpendicularity under nonuniform block scale, nested paths, intersection source
edits, preserved overrides, erased sources, copy detachment, saved picture
invalidation, and history. The checked-in DXF fixture is exercised directly.
The original private drawing can be checked separately:

```powershell
$env:OPENCAD_VIEWPORT_REGRESSION_DXF = 'C:\path\to\115448.dxf'
cargo test --locked --offline --lib viewport_dimension_original -- --ignored --nocapture
```

Do not treat passing headless checks as completed desktop visual acceptance.
