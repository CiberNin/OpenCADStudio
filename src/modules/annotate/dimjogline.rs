// DIMJOGLINE — add or remove a jog on a linear/aligned dimension.

use acadrust::entities::Dimension;
use acadrust::{EntityType, Handle};
use glam::{DVec3, Vec3};

use crate::command::{CadCommand, CmdOption, CmdResult, InputKind};
use crate::modules::{IconKind, ModuleEvent, ToolDef};
use crate::scene::model::wire_model::WireModel;
use crate::t;

pub const ICON: IconKind = IconKind::Svg(include_bytes!("../../../assets/icons/dim_jog.svg"));

pub fn tool() -> ToolDef {
    ToolDef {
        id: "DIMJOGLINE",
        label: "Jog Line",
        icon: ICON,
        event: ModuleEvent::Command("DIMJOGLINE".to_string()),
    }
}

enum Step {
    PickDim,
    PickJogPos { handle: Handle },
}

pub struct DimJogLineCommand {
    step: Step,
    remove: bool,
    picked_entity: Option<EntityType>,
    dimension: Option<Dimension>,
}

impl DimJogLineCommand {
    pub fn new() -> Self {
        Self {
            step: Step::PickDim,
            remove: false,
            picked_entity: None,
            dimension: None,
        }
    }

    fn result(handle: Handle, point: Option<DVec3>) -> CmdResult {
        use acadrust::entities::XLine;
        let mut marker = XLine::default();
        marker.common.layer = match point {
            Some(point) => format!(
                "__DIMJOG__{}|{:.15},{:.15},{:.15}",
                handle.value(), point.x, point.y, point.z
            ),
            None => format!("__DIMJOG_REMOVE__{}", handle.value()),
        };
        CmdResult::ReplaceEntity(handle, vec![EntityType::XLine(marker)])
    }

    fn default_position(dimension: &Dimension) -> Option<DVec3> {
        let (first, second, definition, axis) = match dimension {
            Dimension::Linear(value) => (
                value.first_point,
                value.second_point,
                value.definition_point,
                DVec3::new(value.rotation.cos(), value.rotation.sin(), 0.0),
            ),
            Dimension::Aligned(value) => {
                let first = DVec3::new(value.first_point.x, value.first_point.y, value.first_point.z);
                let second = DVec3::new(value.second_point.x, value.second_point.y, value.second_point.z);
                (
                    value.first_point,
                    value.second_point,
                    value.definition_point,
                    (second - first).try_normalize().unwrap_or(DVec3::X),
                )
            }
            _ => return None,
        };
        let first = DVec3::new(first.x, first.y, first.z);
        let second = DVec3::new(second.x, second.y, second.z);
        let definition = DVec3::new(definition.x, definition.y, definition.z);
        let axis = axis.try_normalize().unwrap_or(DVec3::X);
        let perpendicular = DVec3::new(-axis.y, axis.x, 0.0);
        let offset = definition.dot(perpendicular);
        let first_on_line = first + perpendicular * (offset - first.dot(perpendicular));
        let second_on_line = second + perpendicular * (offset - second.dot(perpendicular));
        let midpoint = (first_on_line + second_on_line) * 0.5;

        let text = dimension.base().text_middle_point;
        let text = DVec3::new(text.x, text.y, text.z);
        let line = second_on_line - first_on_line;
        let length_squared = line.length_squared();
        if length_squared <= 1.0e-18 {
            return Some(midpoint);
        }
        let parameter = (text - first_on_line).dot(line) / length_squared;
        if text.is_finite() && (0.0..=1.0).contains(&parameter) {
            Some((first_on_line + text) * 0.5)
        } else {
            Some(midpoint)
        }
    }
}

impl CadCommand for DimJogLineCommand {
    fn name(&self) -> &'static str {
        "DIMJOGLINE"
    }

    fn prompt(&self) -> String {
        match self.step {
            Step::PickDim if self.remove => {
                t!("DIMJOGLINE  Select dimension to remove jog:").into_owned()
            }
            Step::PickDim => {
                t!("DIMJOGLINE  Select dimension to add jog or [Remove]:").into_owned()
            }
            Step::PickJogPos { .. } => {
                t!("DIMJOGLINE  Specify jog location (or press Enter):").into_owned()
            }
        }
    }

    fn options(&self) -> Vec<CmdOption> {
        if matches!(self.step, Step::PickDim) && !self.remove {
            vec![CmdOption::new("Remove", "REMOVE")]
        } else {
            Vec::new()
        }
    }

    fn input_kind(&self) -> InputKind {
        InputKind::Point
    }

    fn point_step_accepts_keywords(&self) -> bool {
        matches!(self.step, Step::PickDim) && !self.remove
    }

    fn needs_entity_pick(&self) -> bool {
        matches!(self.step, Step::PickDim)
    }

    fn inject_before_entity_pick(&self) -> bool {
        matches!(self.step, Step::PickDim)
    }

    fn inject_picked_entity(&mut self, entity: EntityType) {
        self.picked_entity = Some(entity);
    }

    fn on_entity_pick(&mut self, handle: Handle, _point: DVec3) -> CmdResult {
        let Some(EntityType::Dimension(dimension @ (Dimension::Linear(_) | Dimension::Aligned(_)))) =
            self.picked_entity.take()
        else {
            return CmdResult::ReportError(
                t!("DIMJOGLINE: select a linear or aligned dimension.").into_owned(),
            );
        };
        if handle.is_null() {
            return CmdResult::NeedPoint;
        }
        if self.remove {
            return Self::result(handle, None);
        }
        self.dimension = Some(dimension);
        self.step = Step::PickJogPos { handle };
        CmdResult::NeedPoint
    }

    fn on_text_input(&mut self, text: &str) -> Option<CmdResult> {
        if matches!(self.step, Step::PickDim)
            && matches!(text.trim().to_ascii_uppercase().as_str(), "R" | "REMOVE")
        {
            self.remove = true;
            Some(CmdResult::NeedPoint)
        } else {
            None
        }
    }

    fn on_point(&mut self, point: DVec3) -> CmdResult {
        match self.step {
            Step::PickJogPos { handle } => Self::result(handle, Some(point)),
            Step::PickDim => CmdResult::NeedPoint,
        }
    }

    fn on_enter(&mut self) -> CmdResult {
        match (&self.step, &self.dimension) {
            (Step::PickJogPos { handle }, Some(dimension)) => Self::default_position(dimension)
                .map(|point| Self::result(*handle, Some(point)))
                .unwrap_or(CmdResult::Cancel),
            _ => CmdResult::Cancel,
        }
    }

    fn on_mouse_move(&mut self, point: DVec3) -> Option<WireModel> {
        if !matches!(self.step, Step::PickJogPos { .. }) {
            return None;
        }
        let axis = self
            .dimension
            .as_ref()
            .and_then(|dimension| match dimension {
                Dimension::Linear(value) => {
                    Some(Vec3::new(value.rotation.cos() as f32, value.rotation.sin() as f32, 0.0))
                }
                Dimension::Aligned(value) => Some(Vec3::new(
                    (value.second_point.x - value.first_point.x) as f32,
                    (value.second_point.y - value.first_point.y) as f32,
                    0.0,
                )),
                _ => None,
            })
            .and_then(Vec3::try_normalize)
            .unwrap_or(Vec3::X);
        let perpendicular = Vec3::new(-axis.y, axis.x, 0.0);
        let center = point.as_vec3();
        let size = 0.3;
        let points = vec![
            center - axis * size,
            center - axis * size * 0.25 + perpendicular * size,
            center + axis * size * 0.25 - perpendicular * size,
            center + axis * size,
        ];
        let mut preview = WireModel::default();
        preview.name = "dimjog_preview".into();
        preview.points = points.into_iter().map(|point| point.to_array()).collect();
        preview.color = WireModel::CYAN;
        preview.line_weight_px = 1.2;
        Some(preview)
    }
}

inventory::submit!(crate::command::CommandRegistration { names: &["DIMJOGLINE"] });
