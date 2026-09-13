//! Ribbon tools for persistent geometric and dimensional constraints.
//! Commands collect their input here; the scene stores and re-solves the
//! resulting constraints through the geometry kernel.

mod coincident;
mod equal_distance;
mod point_on_entity;
mod tools;
mod value;
pub use coincident::{coincident_tool, CoincidentConstraintCommand};
pub use equal_distance::{equal_distance_tool, EqualDistanceConstraintCommand};
pub use point_on_entity::{
    center_point_tool, midpoint_tool, point_on_curve_tool, PointOnEntityConstraintCommand,
};
pub use tools::{
    colinear, concentric, equal, fixed, horizontal, normal, parallel, perpendicular, symmetric,
    tangent, vertical,
};
pub use value::{angle_tool, distance_tool, AngleConstraintCommand, DistanceConstraintCommand};

use crate::modules::{CadModule, IconKind, ModuleEvent, RibbonGroup, RibbonItem, ToolDef};

pub struct ParametricModule;

impl CadModule for ParametricModule {
    fn id(&self) -> &'static str {
        "parametric"
    }

    fn title(&self) -> &'static str {
        "Parametric"
    }

    fn ribbon_groups(&self) -> &[RibbonGroup] {
        static GROUPS: std::sync::OnceLock<Vec<RibbonGroup>> = std::sync::OnceLock::new();
        GROUPS.get_or_init(|| {
            let extra_constraints = [
                normal::tool(),
                center_point_tool::tool(),
                midpoint_tool::tool(),
                point_on_curve_tool::tool(),
                equal_distance_tool::tool(),
            ];
            vec![
                RibbonGroup {
                    title: "Geometry",
                    tools: vec![
                        coincident_tool::tool().into(),
                        parallel::tool().into(),
                        tangent::tool().into(),
                        colinear::tool().into(),
                        perpendicular::tool().into(),
                        RibbonItem::Dropdown {
                            id: "PARAMETRIC_MORE",
                            icon: extra_constraints[0].icon,
                            items: extra_constraints
                                .iter()
                                .map(|tool| (tool.id, tool.label, tool.icon))
                                .collect(),
                            default: "NRCONSTRAINT",
                        },
                        concentric::tool().into(),
                        horizontal::tool().into(),
                        symmetric::tool().into(),
                        fixed::tool().into(),
                        vertical::tool().into(),
                        equal::tool().into(),
                    ],
                },
                RibbonGroup {
                    title: "Dimension",
                    tools: vec![
                        RibbonItem::LargeTool(distance_tool::tool()),
                        RibbonItem::LargeTool(angle_tool::tool()),
                    ],
                },
                RibbonGroup {
                    title: "Manage",
                    tools: vec![RibbonItem::LargeTool(ToolDef {
                        id: "PARAMETERS",
                        label: "Parameters",
                        icon: IconKind::Glyph("ƒ"),
                        event: ModuleEvent::Command("PARAMETERS".to_string()),
                    })],
                },
            ]
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item_id(item: &RibbonItem) -> &'static str {
        match item {
            RibbonItem::Tool(tool) | RibbonItem::LargeTool(tool) => tool.id,
            RibbonItem::Dropdown { id, .. } | RibbonItem::LargeDropdown { id, .. } => id,
            _ => panic!("unexpected composite ribbon item"),
        }
    }

    #[test]
    fn ribbon_uses_geometry_dimension_and_manage_panels() {
        let groups = ParametricModule.ribbon_groups();

        assert_eq!(
            groups.iter().map(|group| group.title).collect::<Vec<_>>(),
            ["Geometry", "Dimension", "Manage"]
        );
        assert_eq!(
            groups[0].tools.iter().map(item_id).collect::<Vec<_>>(),
            [
                "CCONSTRAINT",
                "PCONSTRAINT",
                "TCONSTRAINT",
                "LCONSTRAINT",
                "QCONSTRAINT",
                "PARAMETRIC_MORE",
                "NCONSTRAINT",
                "HCONSTRAINT",
                "SYCONSTRAINT",
                "FXCONSTRAINT",
                "VCONSTRAINT",
                "ECONSTRAINT",
            ]
        );
        assert_eq!(
            groups[1].tools.iter().map(item_id).collect::<Vec<_>>(),
            ["DCONSTRAINT", "ACONSTRAINT"]
        );
        assert_eq!(item_id(&groups[2].tools[0]), "PARAMETERS");

        let RibbonItem::Dropdown { items, .. } = &groups[0].tools[5] else {
            panic!("additional geometric constraints must stay in a dropdown");
        };
        assert_eq!(
            items.iter().map(|(id, _, _)| *id).collect::<Vec<_>>(),
            [
                "NRCONSTRAINT",
                "CPCONSTRAINT",
                "MPCONSTRAINT",
                "OCCONSTRAINT",
                "EDCONSTRAINT",
            ]
        );
    }
}
