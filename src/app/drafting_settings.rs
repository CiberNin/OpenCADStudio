use super::OpenCADStudio;
use crate::ui::window::drafting_settings::{DraftingSettingsState, DraftingSettingsTab};

impl DraftingSettingsState {
    pub(crate) fn from_app(app: &OpenCADStudio) -> Self {
        Self {
            active_tab: DraftingSettingsTab::SnapAndGrid,
            snap_on: app.snapper.grid_snap(),
            grid_on: app.show_grid,
            isometric: app.isometric_drafting,
            iso_plane: app.iso_plane,
            snap_angle_deg: app.snap_angle_deg,
            polar_on: app.polar_mode,
            ortho_on: app.ortho_mode,
            polar_increment_deg: app.polar_increment_deg,
            osnap_on: app.snapper.snap_enabled,
            otrack_on: app.snapper.otrack_enabled,
            snap_modes: app.snapper.enabled.clone(),
            osnap3d_on: false,
            dyn_input_on: app.dyn_input,
            quick_props_on: app.quick_properties,
            selection_cycling_on: app.selection_cycling,
        }
    }
}

impl OpenCADStudio {
    pub(super) fn drafting_settings_dirty(&self) -> bool {
        match (&self.drafting_settings_state, &self.drafting_settings_saved) {
            (Some(curr), Some(saved)) => curr.is_dirty(saved),
            (Some(_), None) => true,
            _ => false,
        }
    }

    pub(super) fn apply_drafting_settings(&mut self) {
        if let Some(state) = &self.drafting_settings_state {
            self.show_grid = state.grid_on;
            self.snapper.grid_snap_on = state.snap_on;
            self.isometric_drafting = state.isometric;
            self.iso_plane = state.iso_plane;
            self.snap_angle_deg = state.snap_angle_deg;
            self.polar_mode = state.polar_on;
            self.ortho_mode = state.ortho_on;
            self.polar_increment_deg = state.polar_increment_deg;
            self.snapper.snap_enabled = state.osnap_on;
            self.snapper.otrack_enabled = state.otrack_on;
            self.snapper.enabled = state.snap_modes.clone();
            self.dyn_input = state.dyn_input_on;
            self.quick_properties = state.quick_props_on;
            self.selection_cycling = state.selection_cycling_on;
            self.sync_vport_display(self.active_tab);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_drafting_settings_tab_switching_not_dirty() {
        let state1 = DraftingSettingsState {
            active_tab: DraftingSettingsTab::SnapAndGrid,
            snap_on: false,
            grid_on: true,
            isometric: false,
            iso_plane: crate::app::settings::IsoPlane::Left,
            snap_angle_deg: 0.0,
            polar_on: false,
            ortho_on: false,
            polar_increment_deg: 15.0,
            osnap_on: true,
            otrack_on: false,
            snap_modes: rustc_hash::FxHashSet::default(),
            osnap3d_on: false,
            dyn_input_on: false,
            quick_props_on: false,
            selection_cycling_on: false,
        };
        let mut state2 = state1.clone();
        assert!(!state2.is_dirty(&state1));

        // Switching tabs should NOT mark it dirty
        state2.active_tab = DraftingSettingsTab::ObjectSnap;
        assert!(!state2.is_dirty(&state1));

        // Toggling a setting should mark it dirty
        state2.grid_on = false;
        assert!(state2.is_dirty(&state1));
    }

    #[test]
    fn test_drafting_settings_dirty_toggles() {
        let base = DraftingSettingsState {
            active_tab: DraftingSettingsTab::SnapAndGrid,
            snap_on: false,
            grid_on: true,
            isometric: false,
            iso_plane: crate::app::settings::IsoPlane::Left,
            snap_angle_deg: 0.0,
            polar_on: false,
            ortho_on: false,
            polar_increment_deg: 15.0,
            osnap_on: true,
            otrack_on: false,
            snap_modes: rustc_hash::FxHashSet::default(),
            osnap3d_on: false,
            dyn_input_on: false,
            quick_props_on: false,
            selection_cycling_on: false,
        };

        let mut modded = base.clone();
        modded.snap_on = true;
        assert!(modded.is_dirty(&base));

        let mut modded = base.clone();
        modded.isometric = true;
        assert!(modded.is_dirty(&base));

        let mut modded = base.clone();
        modded.iso_plane = crate::app::settings::IsoPlane::Top;
        assert!(modded.is_dirty(&base));

        let mut modded = base.clone();
        modded.snap_angle_deg = 45.0;
        assert!(modded.is_dirty(&base));

        let mut modded = base.clone();
        modded.polar_on = true;
        assert!(modded.is_dirty(&base));

        let mut modded = base.clone();
        modded.ortho_on = true;
        assert!(modded.is_dirty(&base));

        let mut modded = base.clone();
        modded.polar_increment_deg = 30.0;
        assert!(modded.is_dirty(&base));

        let mut modded = base.clone();
        modded.osnap_on = false;
        assert!(modded.is_dirty(&base));

        let mut modded = base.clone();
        modded.otrack_on = true;
        assert!(modded.is_dirty(&base));

        let mut modded = base.clone();
        modded.snap_modes.insert(crate::snap::SnapType::Endpoint);
        assert!(modded.is_dirty(&base));

        let mut modded = base.clone();
        modded.osnap3d_on = true;
        assert!(modded.is_dirty(&base));

        let mut modded = base.clone();
        modded.dyn_input_on = true;
        assert!(modded.is_dirty(&base));

        let mut modded = base.clone();
        modded.quick_props_on = true;
        assert!(modded.is_dirty(&base));

        let mut modded = base.clone();
        modded.selection_cycling_on = true;
        assert!(modded.is_dirty(&base));
    }
}
