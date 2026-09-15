//! Viewport-aware dimension measurement (PR2).
//!
//! A dimension created in a paper-space layout while measuring geometry seen
//! through a content viewport must *place* itself in paper coordinates but
//! *report* the model measurement. Because a viewport frame is a similarity
//! (uniform scale plus in-plane twist), the model measurement of any linear,
//! aligned or radial span is exactly the raw paper measurement times the
//! viewport compensation `1 / scale` — projecting the model span onto the
//! rotated paper axis and compensating is the same number. So no geometry is
//! rewritten: the compensation is persisted once, as a negative DIMLFAC
//! override on the dimension (see
//! [`MeasurementScale::viewport_dimlfac_override`]), and applied once, by the
//! linear formatter.
//!
//! Angular dimensions are never compensated — a similarity preserves angles.
//!
//! ## PR1 seam
//! [`OpenCADStudio::record_dimension_accepted_snap`] is the one function PR1
//! replaces. PR1 records a real [`AcceptedSnap`] per accepted point (with the
//! snapped source and the frame in effect at snap time); this branch derives
//! the same information from the cursor's paper position and the content
//! viewport under it. Everything downstream consumes `AcceptedSnap` only.

use acadrust::entities::Dimension;
use acadrust::xdata::XDataValue;
use acadrust::EntityType;
use glam::DVec3;

use crate::entities::dim_override;
use crate::scene::viewport_ref::{AcceptedSnap, MeasurementScale, ViewportFrame};

use super::OpenCADStudio;

/// How a pending dimension should be measured.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum DimensionMeasureSpace {
    /// Plain measurement in the space the definition points live in.
    Direct,
    /// Every measuring point came through one viewport; compensate by its
    /// scale.
    Viewport(ViewportFrame),
    /// The points came from different viewports, or from a viewport and the
    /// bare sheet. Falls back to `Direct` with a warning — never a silently
    /// wrong number.
    Mixed,
}

impl OpenCADStudio {
    // ───────────────────────── PR1 SEAM: BEGIN ──────────────────────────

    /// Record the snap accepted for one collected point of the active
    /// command.
    ///
    /// PR1 replaces the body with the real snap result (which knows the
    /// snapped entity, the block path and the frame at snap time). Until then
    /// the viewport is resolved from the paper position of the accepted point,
    /// which is correct for every point that lands inside a content viewport
    /// and yields a free snap everywhere else.
    pub(crate) fn record_dimension_accepted_snap(&mut self, i: usize, point: DVec3) {
        self.sync_dimension_snap_owner(i);
        let snap = match self.tabs[i].scene.viewport_frame_at_paper_point(point) {
            Some(frame) => AcceptedSnap {
                paper_point: point,
                model_point: frame.paper_to_model(point),
                viewport: Some(frame.viewport),
                frame: Some(frame),
                source: None,
            },
            None => AcceptedSnap::free(point),
        };
        self.dim_accepted_snaps.push(snap);
    }

    // ────────────────────────── PR1 SEAM: END ───────────────────────────

    /// Record an accepted snap produced by the explicit object-pick path (a
    /// pick resolved through a content viewport). Independent of PR1.
    pub(crate) fn record_dimension_viewport_pick(
        &mut self,
        i: usize,
        frame: ViewportFrame,
        paper_point: DVec3,
        model_point: DVec3,
    ) {
        self.sync_dimension_snap_owner(i);
        // An object pick supplies both extension origins at once; record it
        // twice so the "every measuring point came through one viewport"
        // test sees a complete set.
        for _ in 0..2 {
            self.dim_accepted_snaps.push(AcceptedSnap {
                paper_point,
                model_point,
                viewport: Some(frame.viewport),
                frame: Some(frame),
                source: None,
            });
        }
    }

    /// The accepted snap for the `idx`-th collected point of the active
    /// command, if one was recorded. The contracted accessor PR1 and PR3 read;
    /// PR2 itself classifies the whole set at commit time.
    #[allow(dead_code)]
    pub(crate) fn accepted_snap_for_point(&self, idx: usize) -> Option<&AcceptedSnap> {
        self.dim_accepted_snaps.get(idx)
    }

    /// Drop recorded snaps when the active command changes, so one command
    /// never sees another's points.
    fn sync_dimension_snap_owner(&mut self, i: usize) {
        let name = self.tabs[i]
            .active_cmd
            .as_ref()
            .map(|command| command.name().to_string());
        if self.dim_accepted_snaps_owner != name {
            self.dim_accepted_snaps.clear();
            self.dim_accepted_snaps_owner = name;
        }
    }

    pub(crate) fn clear_dimension_accepted_snaps(&mut self) {
        self.dim_accepted_snaps.clear();
        self.dim_accepted_snaps_owner = None;
    }

    /// Classify how the pending dimension should be measured from the snaps
    /// recorded for it.
    fn dimension_measure_space(&self, i: usize) -> DimensionMeasureSpace {
        let scene = &self.tabs[i].scene;
        // Only a dimension being created *on the sheet* can measure through a
        // viewport. Inside MSPACE the command already works in model space.
        if scene.current_layout == "Model" || scene.active_viewport.is_some() {
            return DimensionMeasureSpace::Direct;
        }
        // The final collected point of every dimension command in scope is the
        // dimension-line / text placement click, which says nothing about what
        // is being measured — it routinely lands on bare sheet even for a
        // viewport measurement, and inside a viewport rectangle even for a
        // plain paper one. Only the measuring points are classified.
        let measuring = match self.dim_accepted_snaps.len() {
            0 | 1 => self.dim_accepted_snaps.as_slice(),
            n => &self.dim_accepted_snaps[..n - 1],
        };
        if measuring.is_empty() {
            return DimensionMeasureSpace::Direct;
        }
        let mut frame: Option<ViewportFrame> = None;
        let mut any_free = false;
        for snap in measuring {
            match snap.frame {
                Some(f) => match frame {
                    None => frame = Some(f),
                    Some(existing) if existing.viewport == f.viewport => {}
                    Some(_) => return DimensionMeasureSpace::Mixed,
                },
                None => any_free = true,
            }
        }
        match (frame, any_free) {
            (None, _) => DimensionMeasureSpace::Direct,
            (Some(f), false) => DimensionMeasureSpace::Viewport(f),
            // One origin through a viewport and the other on the sheet: the
            // two points are not in a common measurable space.
            (Some(_), true) => DimensionMeasureSpace::Mixed,
        }
    }

    /// Apply viewport compensation to a dimension about to be committed.
    ///
    /// Returns `true` when the dimension measures through a viewport, which
    /// tells the caller not to infer a paper-space association from the
    /// projected definition points (PR3 creates the real viewport
    /// associations).
    pub(crate) fn apply_viewport_dimension_measurement(
        &mut self,
        i: usize,
        entity: &mut EntityType,
    ) -> bool {
        let EntityType::Dimension(dimension) = entity else {
            return false;
        };
        let is_angular = matches!(
            dimension,
            Dimension::Angular2Ln(_) | Dimension::Angular3Pt(_)
        );
        let frame = match self.dimension_measure_space(i) {
            DimensionMeasureSpace::Direct => return false,
            DimensionMeasureSpace::Mixed => {
                self.command_line.push_warning(
                    crate::t!(
                        "Mixed-space measurement is not supported: the extension origins come from different viewports, or from a viewport and the sheet. Measuring in paper space instead (no viewport scale compensation)."
                    )
                    .as_ref(),
                );
                return false;
            }
            DimensionMeasureSpace::Viewport(frame) => frame,
        };

        // A similarity preserves angles, so an angular dimension reads the
        // model angle already. It still measured through a viewport, so the
        // caller must skip association inference.
        if is_angular {
            return true;
        }

        let compensation = frame.paper_to_model_length_factor();
        let style_dimlfac = self.pending_dimension_style_dimlfac(i, entity);
        if let Some(value) =
            MeasurementScale::viewport_dimlfac_override(style_dimlfac, compensation)
        {
            dim_override::set_on_entity(
                entity,
                dim_override::DIMLFAC,
                Some(XDataValue::Real(value)),
            );
        }
        true
    }

    /// Explicit "select object" pick for a dimension command, resolved through
    /// a paper-space content viewport.
    ///
    /// Paper-space hit testing only sees sheet entities, so a click on model
    /// geometry displayed inside a viewport finds nothing. This maps the
    /// cursor into model space through the viewport frame, hit-tests the
    /// viewport's resident model wires (descending into block instances and
    /// baking the instance transform), then hands the command a copy of the
    /// picked entity projected onto the sheet. The commands therefore keep
    /// working entirely in paper coordinates, and the measurement is
    /// compensated at commit time like any other viewport measurement.
    ///
    /// Returns `None` when the click is not inside a content viewport or hits
    /// nothing, so the caller falls back to the ordinary paper-space pick.
    pub(crate) fn try_dimension_viewport_entity_pick(
        &mut self,
        i: usize,
        paper: DVec3,
        aperture_paper: f64,
    ) -> Option<crate::command::CmdResult> {
        let pick = self.tabs[i]
            .scene
            .dimension_pick_through_viewport(paper, aperture_paper)?;
        self.record_dimension_viewport_pick(i, pick.frame, pick.paper_point, pick.model_point);
        let command = self.tabs[i].active_cmd.as_mut()?;
        command.inject_picked_entity(pick.paper_entity);
        Some(command.on_entity_pick(pick.entity_handle, pick.paper_point))
    }

    /// The DIMLFAC the pending dimension would inherit *without* a viewport
    /// override: its own existing override if it has one, otherwise its named
    /// dimension style's value.
    fn pending_dimension_style_dimlfac(&self, i: usize, entity: &EntityType) -> f64 {
        let EntityType::Dimension(dimension) = entity else {
            return 1.0;
        };
        if let Some(value) = dim_override::real(
            &dimension.base().common.extended_data,
            dim_override::DIMLFAC,
        ) {
            return value;
        }
        let name = dimension.base().style_name.as_str();
        self.tabs[i]
            .scene
            .document
            .dim_styles
            .iter()
            .find(|style| {
                style.name.eq_ignore_ascii_case(name)
                    || (name.trim().is_empty() && style.name.eq_ignore_ascii_case("Standard"))
            })
            .map(|style| style.dimlfac)
            .unwrap_or(1.0)
    }
}
