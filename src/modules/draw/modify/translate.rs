// Move tool — ribbon definition + interactive command.
//
// Command:  MOVE (M)
//   Requires at least one entity selected before starting.
//   Step 1: pick base point
//   Step 2: pick destination → translates all selected entities by (dest - base)

use acadrust::Handle;
use glam::DVec3;

use crate::command::{CadCommand, CmdResult, EntityTransform};
use crate::modules::{IconKind, ModuleEvent, ToolDef};
use crate::scene::model::wire_model::WireModel;

// ── Ribbon definition ──────────────────────────────────────────────────────

pub fn tool() -> ToolDef {
    ToolDef {
        id: "MOVE",
        label: "Move",
        icon: IconKind::Svg(include_bytes!("../../../../assets/icons/move.svg")),
        event: ModuleEvent::Command("MOVE".to_string()),
    }
}

// ── Command implementation ─────────────────────────────────────────────────

enum Step {
    Base,
    Target(DVec3),
}

pub struct MoveCommand {
    handles: Vec<Handle>,
    wire_models: Vec<WireModel>,
    step: Step,
}

impl MoveCommand {
    pub fn new(handles: Vec<Handle>, wire_models: Vec<WireModel>) -> Self {
        Self {
            handles,
            wire_models,
            step: Step::Base,
        }
    }
}

impl CadCommand for MoveCommand {
    fn name(&self) -> &'static str {
        "MOVE"
    }

    fn prompt(&self) -> String {
        match &self.step {
            Step::Base => crate::tr!(
                "command-move", "base",
                count = (self.handles.len() as i64),
            ),
            Step::Target(base) => crate::tr!(
                "command-move", "target",
                x = format!("{:.3}", base.x),
                y = format!("{:.3}", base.y),
            ),
        }
    }

    fn on_point(&mut self, pt: DVec3) -> CmdResult {
        match &self.step {
            Step::Base => {
                self.step = Step::Target(pt);
                CmdResult::NeedPoint
            }
            Step::Target(base) => {
                let delta = pt - *base;
                CmdResult::TransformSelected(
                    self.handles.clone(),
                    EntityTransform::Translate(delta),
                )
            }
        }
    }

    fn on_enter(&mut self) -> CmdResult {
        CmdResult::Cancel
    }
    fn on_escape(&mut self) -> CmdResult {
        CmdResult::Cancel
    }

    fn on_preview_wires(&mut self, pt: DVec3) -> Vec<WireModel> {
        let Step::Target(base) = &self.step else {
            return vec![];
        };
        let delta = pt - *base;
        // Translated ghost of each selected object + rubber-band line.
        let mut out: Vec<WireModel> = self
            .wire_models
            .iter()
            .map(|w| w.translated(delta.as_vec3()))
            .collect();
        out.push(WireModel::solid(
            "rubber_band".into(),
            vec![
                [base.x as f32, base.y as f32, base.z as f32],
                [pt.x as f32, pt.y as f32, pt.z as f32],
            ],
            WireModel::CYAN,
            false,
        ));
        out
    } 
}
