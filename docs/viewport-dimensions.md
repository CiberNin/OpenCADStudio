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

The dependent association change uses this metadata when viewport scale changes.
This creation change alone creates nonassociative viewport dimensions and leaves
the drawing's `DIMASSOC` setting unchanged. Direct model/paper associations retain
their existing behavior.

## Validation

`src/app/viewport_dimension_tests.rs` exercises snap-engine acquisition, command
commits for all five commands, object picks, input retries, active positive and
negative style factors, exploded dimensions, undo/redo, DXF/DWG persistence,
clipping, overlapping viewports, pan, scale, and twist. These are headless tests;
they do not establish visual acceptance in the desktop UI.
