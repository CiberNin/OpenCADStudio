//! Viewport-aware dimension measurement (PR2).
//!
//! A dimension created in a paper-space layout while measuring geometry seen
//! through a content viewport must *place* itself in paper coordinates but
//! *report* the model measurement. Because a viewport frame is a similarity
//! (uniform scale plus in-plane twist), the model measurement of any linear,
//! aligned or radial span is exactly the raw paper measurement times the
//! viewport compensation `1 / scale` -- projecting the model span onto the
//! rotated paper axis and compensating is the same number. So no geometry is
//! rewritten: the compensation is persisted once, as a negative DIMLFAC
//! override on the dimension (see
//! [`MeasurementScale::viewport_dimlfac_override`]), and applied once, by the
//! linear formatter.
//!
//! Angular dimensions are never compensated -- a similarity preserves angles.
//!
//! ## Where the snaps come from
//! The per-point [`AcceptedSnap`] list is PR1's
//! ([`OpenCADStudio::accepted_snaps`]): the click handler records the real
//! snap for every interactive pick, and `apply_step_input` records a free
//! snap for typed / dynamic-input / headless points. There is exactly one
//! source of truth. Two things are layered on top here:
//!
//! * the explicit object-pick path ([`Scene::dimension_pick_through_viewport`])
//!   is not a point step at all, so it records its own snaps
//!   ([`OpenCADStudio::record_dimension_viewport_pick`]);
//! * a *typed* coordinate carries no snap, so PR1 records it as a free point
//!   with no viewport. [`OpenCADStudio::measure_snap`] re-attaches the
//!   content viewport under such a point, which is what makes
//!   `DIMLINEAR` + typed coordinates inside a 1:10 viewport measure the model.
//!   It is deliberately limited to snaps that are free *and* source-less, so a
//!   genuine paper-sheet snap that happens to sit over a viewport is never
//!   reinterpreted.

use acadrust::entities::Dimension;
use acadrust::types::Handle;
use acadrust::xdata::XDataValue;
use acadrust::EntityType;
use glam::DVec3;

use crate::command::DimensionAssociationSource;
use crate::entities::dim_override;
use crate::scene::viewport_ref::{AcceptedSnap, MeasurementScale, SnapSourceRef, ViewportFrame};

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
    /// Record the accepted snaps produced by the explicit object-pick path (a
    /// dimension "select object" pick resolved through a content viewport).
    ///
    /// An object pick is not a point step, so PR1's click handler never sees
    /// it. One pick supplies *both* extension origins, so it is recorded
    /// twice: that keeps the list index-parallel with the definition points
    /// the command derives from the entity, and lets the "every measuring
    /// point came through one viewport" classification see a complete set.
    pub(crate) fn record_dimension_viewport_pick(
        &mut self,
        frame: ViewportFrame,
        paper_point: DVec3,
        model_point: DVec3,
        entity: Handle,
        block_path: Vec<Handle>,
    ) {
        for _ in 0..2 {
            self.push_accepted_snap(AcceptedSnap {
                paper_point,
                model_point,
                viewport: Some(frame.viewport),
                frame: Some(frame),
                source: Some(SnapSourceRef {
                    source: DimensionAssociationSource::inferred(entity),
                    block_path: block_path.clone(),
                    // An object pick acquires the whole curve, not one
                    // feature; the per-slot feature point is derived from the
                    // committed dimension's own definition points.
                    snap_type: crate::snap::SnapType::Nearest,
                }),
            });
        }
    }

    /// The accepted snap for the `idx`-th collected point of the active
    /// command, with the typed-coordinate viewport fallback applied.
    ///
    /// Reads PR1's list -- there is no second store.
    #[allow(dead_code)]
    pub(crate) fn accepted_snap_for_point(&self, i: usize, idx: usize) -> Option<AcceptedSnap> {
        self.accepted_snaps().get(idx).map(|snap| self.measure_snap(i, snap))
    }

    /// One accepted snap as *measurement* sees it.
    ///
    /// Identical to the recorded snap except that a point which carries no
    /// snap at all (a typed coordinate, a dynamic-input point, a headless
    /// script point) and lands inside a content viewport is re-attached to
    /// that viewport, so typing `100,50` inside a 1:10 viewport measures the
    /// model just like clicking there would. A snap that landed on real
    /// paper-sheet geometry keeps `viewport: None` and is never reinterpreted.
    pub(crate) fn measure_snap(&self, i: usize, snap: &AcceptedSnap) -> AcceptedSnap {
        if snap.frame.is_some() || snap.source.is_some() {
            return snap.clone();
        }
        let scene = &self.tabs[i].scene;
        if scene.current_layout == "Model" || scene.active_viewport.is_some() {
            return snap.clone();
        }
        match scene.viewport_frame_at_paper_point(snap.paper_point) {
            Some(frame) => AcceptedSnap {
                paper_point: snap.paper_point,
                model_point: frame.paper_to_model(snap.paper_point),
                viewport: Some(frame.viewport),
                frame: Some(frame),
                source: None,
            },
            None => snap.clone(),
        }
    }

    /// The accepted snaps that say what the pending dimension *measures*, in
    /// collection order.
    ///
    /// The final collected point of every dimension command in scope is the
    /// dimension-line / text placement click, which says nothing about what is
    /// being measured -- it routinely lands on bare sheet even for a viewport
    /// measurement, and inside a viewport rectangle even for a plain paper
    /// one. It is dropped here, which is also why association inference is
    /// guarded on this set rather than on every recorded snap.
    pub(crate) fn dimension_measuring_snaps(&self, i: usize) -> Vec<AcceptedSnap> {
        let all = self.accepted_snaps();
        let measuring = match all.len() {
            0 | 1 => all,
            n => &all[..n - 1],
        };
        measuring.iter().map(|snap| self.measure_snap(i, snap)).collect()
    }

    /// Classify how the pending dimension should be measured from the snaps
    /// recorded for it.
    pub(crate) fn dimension_measure_space(&self, i: usize) -> DimensionMeasureSpace {
        let scene = &self.tabs[i].scene;
        // Only a dimension being created *on the sheet* can measure through a
        // viewport. Inside MSPACE the command already works in model space.
        if scene.current_layout == "Model" || scene.active_viewport.is_some() {
            return DimensionMeasureSpace::Direct;
        }
        let measuring = self.dimension_measuring_snaps(i);
        let measuring = measuring.as_slice();
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

    /// The slot array [`Scene::attach_viewport_dimension_association`] wants,
    /// built from the snaps PR1 recorded for the command that just committed
    /// `dimension`.
    ///
    /// Slot `k` describes the feature at
    /// `Scene::dimension_association_slot_points()[k]`, so the measuring snaps
    /// are taken in collection order and truncated / padded to that length.
    ///
    /// A snap whose paper point is not (still) the definition point it would
    /// fill has its `model_point` recomputed from that definition point
    /// through the frame. Two cases need it:
    ///
    /// * the explicit object pick, which records one click twice — once per
    ///   extension origin — so neither recorded point is the endpoint the
    ///   command derived from the entity;
    /// * a point the command moved after the snap (a rotated DIMLINEAR
    ///   projects its origins onto the measuring axis).
    ///
    /// The source identity is kept in both cases: it is the entity that was
    /// acquired, and the feature marker is re-derived from the corrected
    /// point.
    pub(crate) fn viewport_dimension_snaps(
        &self,
        i: usize,
        dimension: Handle,
    ) -> Vec<Option<AcceptedSnap>> {
        let slots = self.tabs[i].scene.dimension_association_slot_points(dimension);
        if slots.is_empty() {
            return Vec::new();
        }
        let measuring = self.dimension_measuring_snaps(i);
        slots
            .iter()
            .enumerate()
            .map(|(index, slot)| {
                let snap = measuring.get(index)?.clone();
                let frame = snap.frame?;
                let slot = DVec3::new(slot.x, slot.y, slot.z);
                if snap.paper_point.truncate().distance_squared(slot.truncate()) <= 1e-12 {
                    return Some(snap);
                }
                Some(AcceptedSnap {
                    paper_point: slot,
                    model_point: frame.paper_to_model(slot),
                    ..snap
                })
            })
            .collect()
    }

    /// Record the viewport association for a dimension that has just been
    /// committed while measuring through a layout viewport.
    ///
    /// Announces the whole chain — the viewport, every INSERT on the block
    /// path and the source entity — so the dependency index picks the new
    /// association up immediately.
    pub(crate) fn attach_viewport_dimension_association(&mut self, i: usize, dimension: Handle) {
        let snaps = self.viewport_dimension_snaps(i, dimension);
        if snaps.iter().flatten().all(|snap| snap.source.is_none()) {
            return;
        }
        self.tabs[i]
            .scene
            .attach_viewport_dimension_association(dimension, &snaps);
        let mut changes = vec![(dimension, crate::scene::ChangeKind::Modified)];
        changes.extend(
            self.tabs[i]
                .scene
                .dimension_association_sources(dimension)
                .into_iter()
                .map(|handle| (handle, crate::scene::ChangeKind::Modified)),
        );
        changes.sort_by_key(|(handle, _)| handle.value());
        changes.dedup_by_key(|(handle, _)| handle.value());
        self.tabs[i].scene.bump_entities(&changes);
    }

    /// Apply viewport compensation to a dimension about to be committed.
    ///
    /// Whether the dimension may also take a paper-space association is a
    /// separate question with the same answer source:
    /// [`OpenCADStudio::dimension_association_allowed`].
    pub(crate) fn apply_viewport_dimension_measurement(
        &mut self,
        i: usize,
        entity: &mut EntityType,
    ) {
        let EntityType::Dimension(dimension) = entity else {
            return;
        };
        let is_angular = matches!(
            dimension,
            Dimension::Angular2Ln(_) | Dimension::Angular3Pt(_)
        );
        let frame = match self.dimension_measure_space(i) {
            DimensionMeasureSpace::Direct => return,
            DimensionMeasureSpace::Mixed => {
                self.command_line.push_warning(
                    crate::t!(
                        "Mixed-space measurement is not supported: the extension origins come from different viewports, or from a viewport and the sheet. Measuring in paper space instead (no viewport scale compensation)."
                    )
                    .as_ref(),
                );
                return;
            }
            DimensionMeasureSpace::Viewport(frame) => frame,
        };

        // A similarity preserves angles, so an angular dimension reads the
        // model angle already and needs no override.
        if is_angular {
            return;
        }

        let compensation = frame.paper_to_model_length_factor();
        let style_dimlfac = self.pending_dimension_style_dimlfac(i, entity);
        if let Some(value) =
            MeasurementScale::viewport_dimlfac_override(style_dimlfac, compensation)
        {
            // The negative DIMLFAC convention is resolved from the dimension's
            // owner block, and the commit path (or, with DIMASSOC = 0, the
            // explode path) reads the entity before `add_entity_to_layout`
            // assigns one. Stamping the layout block here — the same handle
            // the add would set — keeps the entity self-describing, so the
            // text is compensated whichever path consumes it.
            let layout_block = self.tabs[i].scene.current_layout_block_handle_pub();
            if layout_block.is_valid() {
                entity.common_mut().owner_handle = layout_block;
            }
            dim_override::set_on_entity(
                entity,
                dim_override::DIMLFAC,
                Some(XDataValue::Real(value)),
            );
        }
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
        self.record_dimension_viewport_pick(
            pick.frame,
            pick.paper_point,
            pick.model_point,
            pick.entity_handle,
            pick.block_path.clone(),
        );
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
