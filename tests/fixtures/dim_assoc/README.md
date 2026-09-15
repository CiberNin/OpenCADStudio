# Viewport association fixture

`viewport_associations.dxf` is a synthetic paper-space fixture for line, circle,
arc, polyline, spline, block-instance, and intersection references through one
1:2 viewport. `src/app/viewport_dimension_tests.rs` loads it and exercises actual
association refresh and an edit to the second intersection source.

Each dimension carries `DIMLFAC=-2` and `ACAD_DIMASSOC_CALC_DIMLFAC=-2`. These are
necessary for its paper definition points and saved model measurements to agree;
the original fixture omitted both factors. The spline tangent also uses the
actual touch point `(90, 283.125)` at parameter `0.75`, measured from `(0, 300)`.
The original endpoint `(120, 300)` was not tangent. Tests verify the cross product
with the spline derivative and preservation after an unsupported reference edit.

The larger original `115448.dxf` is private and is not included. An opt-in test
uses `OPENCAD_VIEWPORT_REGRESSION_DXF` to read a local copy without modifying it.
