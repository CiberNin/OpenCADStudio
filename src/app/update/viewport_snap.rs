//! Paper-space snapping THROUGH layout viewports (PR1).
//!
//! In a paper layout with no active viewport, `Scene::hit_test_wires` returns
//! only the paper sheet's own entities — viewport contents are deliberately not
//! interactive, and that must stay true for *selection*. Snapping, however,
//! should see the model geometry a viewport displays, so a paper-space LINE /
//! DIMENSION can pick a real model endpoint and land on the paper pixel where
//! it is drawn.
//!
//! This module adds that as a SEPARATE query. It never changes
//! `hit_test_wires`. For each displayed content viewport under the cursor
//! (top-most first) it runs the EXISTING snap engine against that viewport's
//! resident model wires, using the viewport's own camera and screen rectangle
//! (`Scene::viewport_edit_frame_for` — the same adapter MSPACE editing uses),
//! then maps the accepted model point back onto the sheet with
//! [`ViewportFrame::model_to_paper`].
//!
//! Performance: no projected geometry is rebuilt per pointer event. The wire
//! set comes from `model_wires_for_viewport_arc`, which is the renderer's own
//! resident, camera-independent `Arc`, and the cursor-local broad phase reuses
//! the shared interaction index built over it.

use iced::Point;

use acadrust::types::Handle;

use crate::app::OpenCADStudio;
use crate::scene::viewport_ref::{AcceptedSnap, ViewportFrame};
use crate::snap::{SnapResult, SnapType};

/// Bound on the retained accepted-snap history. A command's point steps are a
/// handful at most; the cap only stops a long polyline from growing forever.
const MAX_ACCEPTED_SNAPS: usize = 64;

impl OpenCADStudio {
    /// Snaps accepted by the active command's point steps, oldest first, in
    /// step order. Consumed by the dimension commands in PR2/PR3.
    #[allow(dead_code)]
    pub(crate) fn accepted_snaps(&self) -> &[AcceptedSnap] {
        &self.accepted_snaps
    }

    /// The most recently accepted snap, if any.
    #[allow(dead_code)]
    pub(crate) fn last_accepted_snap(&self) -> Option<&AcceptedSnap> {
        self.accepted_snaps.last()
    }

    /// `Scene::infer_dimension_sources`, suppressed when the dimension being
    /// created measures through a layout viewport.
    ///
    /// Inference works by looking for geometry near the dimension's own
    /// definition points, in whatever space it is handed. A paper-space
    /// dimension measured through a viewport has PAPER definition points that
    /// happen to sit over projected model geometry, so unguarded inference
    /// would either associate it with unrelated paper-sheet geometry at those
    /// coordinates, or match model geometry using paper coordinates. Neither
    /// is correct, and a wrong association is worse than none. Returning an
    /// empty source list makes `attach_dimension_association` a no-op.
    ///
    /// The classification is
    /// [`OpenCADStudio::dimension_measure_space`] — the *same* one that
    /// decides whether to compensate the measurement, so the two can never
    /// disagree. In particular it ignores the trailing dimension-line
    /// placement click, which routinely lands inside a viewport rectangle for
    /// an ordinary paper-space dimension and must not suppress its
    /// association.
    pub(crate) fn infer_dimension_sources_guarded(
        &self,
        tab: usize,
        dimension: Handle,
    ) -> Vec<Option<Handle>> {
        if !self.dimension_association_allowed(tab) {
            return Vec::new();
        }
        self.tabs[tab].scene.infer_dimension_sources(dimension)
    }

    /// The single gate on creating a paper-space association for the
    /// dimension the active command is committing.
    ///
    /// `false` exactly when the dimension measures through (or partly through)
    /// a layout viewport, whatever supplied its sources — inference,
    /// an explicit object pick, or an explicit source list. Everything in the
    /// commit path that used to decide this for itself now asks here, so the
    /// measurement rule and the association rule cannot drift apart.
    pub(crate) fn dimension_association_allowed(&self, tab: usize) -> bool {
        matches!(
            self.dimension_measure_space(tab),
            crate::app::dim_viewport::DimensionMeasureSpace::Direct
        )
    }

    /// Forget the accepted snaps of a finished / abandoned command.
    pub(crate) fn clear_accepted_snaps(&mut self) {
        self.accepted_snaps.clear();
    }

    /// Record the snap a point step just accepted. `snap` is the displayed snap
    /// result (already in the current space's coordinates) or `None` for a free
    /// / typed point; `committed` is the point the command actually received
    /// after ortho / polar / axis-lock / dynamic-input resolution.
    pub(crate) fn record_accepted_snap(
        &mut self,
        tab: usize,
        snap: Option<SnapResult>,
        frame: Option<ViewportFrame>,
        committed: glam::DVec3,
    ) -> bool {
        let mut accepted = match snap {
            // A viewport snap keeps its frame only when the hit really came
            // through it, so a paper-sheet snap never claims a model point.
            Some(hit) => {
                let frame = hit.viewport.and(frame);
                AcceptedSnap::from_snap(&hit, frame).with_paper_point(committed)
            }
            None => AcceptedSnap::free(committed),
        };
        if self.tabs[tab]
            .active_cmd
            .as_ref()
            .is_some_and(|cmd| cmd.measures_through_viewports())
        {
            if self.tabs[tab]
                .active_cmd
                .as_ref()
                .is_some_and(|cmd| cmd.dimension_placement_pending())
            {
                return true;
            }
            if !self.dimension_acquisition_allowed(tab, accepted.viewport) {
                return false;
            }
        }
        self.resolve_source_block_path(tab, &mut accepted);
        self.push_accepted_snap(accepted);
        true
    }

    /// Replace a snap source that is a block INSTANCE with the entity inside
    /// it that actually owns the snapped feature, plus the INSERT path.
    ///
    /// The snap engine can only report what the wire carries, and the
    /// renderer stamps every wire of an expanded block with the *top-level
    /// INSERT's* handle — nested inserts included. `SnapSourceRef` promises
    /// the innermost entity in `source.handle` and the instance chain in
    /// `block_path`, so the descent happens here, once, at accept time (not
    /// per pointer move).
    ///
    /// When the descent cannot identify the inner entity the source stays the
    /// INSERT with an empty path. See
    /// [`crate::scene::Scene::resolve_block_snap_source`] for exactly when
    /// that happens; the resulting association is coarser (it tracks the whole
    /// block instance) but never wrong.
    fn resolve_source_block_path(&self, tab: usize, accepted: &mut AcceptedSnap) {
        let Some(source) = accepted.source.as_mut() else {
            return;
        };
        if !source.block_path.is_empty() {
            return;
        }
        if let Some((entity, path)) = self.tabs[tab]
            .scene
            .resolve_block_snap_source(source.source.handle, accepted.model_point)
        {
            source.source =
                crate::command::DimensionAssociationSource::inferred(entity);
            source.block_path = path;
        }
    }

    /// Append an already-built accepted snap, keeping the retention cap.
    ///
    /// Used by paths that are not point steps (the dimension object pick
    /// resolved through a viewport) and therefore never reach the click
    /// handler.
    pub(crate) fn push_accepted_snap(&mut self, accepted: AcceptedSnap) {
        if self.accepted_snaps.len() >= MAX_ACCEPTED_SNAPS {
            self.accepted_snaps.remove(0);
        }
        self.accepted_snaps.push(accepted);
    }
    /// Snap the paper-space cursor to model geometry seen through a layout
    /// viewport.
    ///
    /// * `i` — tab index.
    /// * `cursor_canvas` — cursor in canvas pixels.
    /// * `canvas` — canvas size in pixels.
    /// * `cursor_paper` — the paper-space point under the cursor.
    ///
    /// Returns the hit expressed in PAPER coordinates (`world` projected onto
    /// the sheet, `screen` in canvas pixels) together with the frame it came
    /// through, so the caller can merge it with the ordinary paper-sheet snap
    /// and later recover the model point.
    ///
    /// `None` in the Model layout, while a viewport is active (MSPACE already
    /// snaps in model space), when snapping is off, or when the cursor is not
    /// over a displayed viewport.
    pub(in crate::app) fn paper_viewport_snap(
        &mut self,
        i: usize,
        cursor_canvas: Point,
        canvas: (f32, f32),
        cursor_paper: glam::DVec3,
    ) -> Option<(SnapResult, ViewportFrame)> {
        if !self.snapper.snap_enabled {
            return None;
        }
        {
            let scene = &self.tabs[i].scene;
            if scene.current_layout == "Model" || scene.active_viewport.is_some() {
                return None;
            }
        }
        if canvas.0 < 1.0 || canvas.1 < 1.0 {
            return None;
        }

        for frame in self.tabs[i]
            .scene
            .viewport_frames_at_paper_point(cursor_paper)
        {
            let handle = frame.viewport;
            let Some((cam, rect)) = self.tabs[i].scene.viewport_edit_frame_for(handle, canvas)
            else {
                continue;
            };
            if rect.width < 1.0 || rect.height < 1.0 {
                continue;
            }
            let bounds = iced::Rectangle {
                x: 0.0,
                y: 0.0,
                width: rect.width,
                height: rect.height,
            };
            let local = Point::new(cursor_canvas.x - rect.x, cursor_canvas.y - rect.y);
            let view_rot = cam.view_proj_rte(bounds);
            let eye = cam.eye();
            // The cursor in MODEL space, through the frame — exactly the point
            // the viewport camera projects back onto `local`.
            let model_cursor = frame.paper_to_model(cursor_paper);

            let wires = self.tabs[i]
                .scene
                .model_wires_for_viewport_arc(handle, bounds.height);
            let candidates = self.tabs[i].scene.interaction_candidates_near(
                wires,
                model_cursor,
                view_rot,
                eye,
                bounds,
                self.snapper.osnap_radius_px,
            );

            // The grid belongs to the sheet. Translate the construction
            // anchor for perpendicular/tangent snaps into this viewport's
            // model coordinates, then restore the shared snapper state.
            let saved_grid = self.snapper.grid_snap_on;
            let saved_from = self.snapper.from_point;
            self.snapper.grid_snap_on = false;
            self.snapper.from_point =
                saved_from.map(|p| frame.paper_to_model(p.as_dvec3()).as_vec3());
            let hit = self.snapper.snap(
                model_cursor,
                local,
                &candidates,
                view_rot,
                eye,
                bounds,
                glam::Vec3::ZERO,
                (glam::Vec3::X, glam::Vec3::Y, glam::Vec3::Z),
                None,
            );
            self.snapper.grid_snap_on = saved_grid;
            self.snapper.from_point = saved_from;

            let Some(hit) = hit.filter(|h| h.snap_type != SnapType::Grid) else {
                continue;
            };
            // The wire set is the whole model, not a clipped copy, so the only
            // way a feature outside the visible part could be offered is via
            // the aperture reaching past the clip. Reject those: a snap must be
            // a real feature the viewport actually displays.
            let paper = frame.model_to_paper(hit.world);
            if !self.tabs[i]
                .scene
                .viewport_displays_paper_point(handle, paper.truncate())
            {
                continue;
            }
            return Some((project_hit_to_paper(hit, &frame, rect, handle), frame));
        }
        None
    }
}

/// Re-express a snap taken in a viewport's model space as a paper-space result:
/// the point projects onto the sheet through `frame`, and the pane-local pixel
/// positions shift by the viewport's screen origin so they share the canvas
/// frame with the ordinary paper snap.
fn project_hit_to_paper(
    hit: SnapResult,
    frame: &ViewportFrame,
    rect: iced::Rectangle,
    viewport: Handle,
) -> SnapResult {
    let to_canvas = |p: Point| Point::new(p.x + rect.x, p.y + rect.y);
    SnapResult {
        world: frame.model_to_paper(hit.world),
        screen: to_canvas(hit.screen),
        snap_type: hit.snap_type,
        // Tangent geometry is model-space and would be consumed as paper-space
        // by TTR/TTT; drop it rather than hand over a wrong circle.
        tangent_obj: None,
        extension_base: hit.extension_base.map(to_canvas),
        extension_base2: hit.extension_base2.map(to_canvas),
        extension_origin: hit.extension_origin.map(|p| frame.model_to_paper(p)),
        extension_dir: hit.extension_dir.map(|d| {
            let mapped = frame.model_to_paper_dir(d.truncate());
            glam::DVec3::new(mapped.x, mapped.y, d.z)
        }),
        viewport: Some(viewport),
        source: hit.source,
        secondary_source: hit.secondary_source,
        model_point: Some(hit.world),
    }
}
