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
pub use value::{
    angle_tool, dimensional_tools, distance_tool, AngleConstraintCommand,
    DistanceConstraintCommand, DistanceMode,
};

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
            let command = |id: &'static str, label: &'static str, icon: &'static [u8]| ToolDef {
                id, label, icon: IconKind::Svg(icon), event: ModuleEvent::Command(id.to_string()),
            };
            vec![
                RibbonGroup {
                    title: "Geometric",
                    tools: vec![
                        RibbonItem::LargeTool(command(
                            "AUTOCONSTRAIN", "Auto Constrain",
                            include_bytes!("../../../assets/icons/constrain/auto.svg"),
                        )),
                        RibbonItem::ToolGrid { columns: vec![
                            vec![coincident_tool::tool(), parallel::tool(), tangent::tool()],
                            vec![
                                colinear::tool(), perpendicular::tool(),
                                command("SMOOTHCONSTRAINT", "Smooth", include_bytes!("../../../assets/icons/constrain/smooth.svg")),
                            ],
                            vec![concentric::tool(), horizontal::tool(), symmetric::tool()],
                            vec![fixed::tool(), vertical::tool(), equal::tool()],
                        ] },
                        RibbonItem::LabeledDropdown {
                            id: "GCVISIBILITY", label: "Show/Hide",
                            icon: IconKind::Svg(include_bytes!("../../../assets/icons/constrain/show.svg")),
                            items: vec![
                                ("GCSHOW", "Show", IconKind::Svg(include_bytes!("../../../assets/icons/constrain/show.svg"))),
                                ("GCHIDE", "Hide", IconKind::Svg(include_bytes!("../../../assets/icons/constrain/hide_all.svg"))),
                                ("GCRESET", "Reset", IconKind::Svg(include_bytes!("../../../assets/icons/constrain/show.svg"))),
                            ], default: "GCSHOW",
                        },
                        RibbonItem::LabeledTool(command(
                            "GCSHOWALL", "Show All", include_bytes!("../../../assets/icons/constrain/show_all.svg"),
                        )),
                        RibbonItem::LabeledTool(command(
                            "GCHIDEALL", "Hide All", include_bytes!("../../../assets/icons/constrain/hide_all.svg"),
                        )),
                    ],
                },
                RibbonGroup {
                    title: "Dimensional",
                    tools: vec![
                        RibbonItem::LargeDropdown {
                            id: "DC_LINEAR_MENU", label: "Linear", icon: dimensional_tools::linear().icon,
                            items: [dimensional_tools::linear(), dimensional_tools::horizontal(), dimensional_tools::vertical()]
                                .iter().map(|tool| (tool.id, tool.label, tool.icon)).collect(),
                            default: "DCLINEAR",
                        },
                        RibbonItem::LargeTool(dimensional_tools::aligned()),
                        RibbonItem::ToolGrid { columns: vec![
                            vec![dimensional_tools::angular(), dimensional_tools::diameter()],
                            vec![dimensional_tools::radius(), dimensional_tools::convert()],
                        ] },
                        RibbonItem::LabeledDropdown {
                            id: "DCVISIBILITY", label: "Show/Hide",
                            icon: IconKind::Svg(include_bytes!("../../../assets/icons/constrain/show.svg")),
                            items: vec![
                                ("DCSHOW", "Show", IconKind::Svg(include_bytes!("../../../assets/icons/constrain/show.svg"))),
                                ("DCHIDE", "Hide", IconKind::Svg(include_bytes!("../../../assets/icons/constrain/hide_all.svg"))),
                            ], default: "DCSHOW",
                        },
                        RibbonItem::LabeledTool(command(
                            "DCSHOWALL", "Show All", include_bytes!("../../../assets/icons/constrain/show_all.svg"),
                        )),
                        RibbonItem::LabeledTool(command(
                            "DCHIDEALL", "Hide All", include_bytes!("../../../assets/icons/constrain/hide_all.svg"),
                        )),
                    ],
                },
                RibbonGroup {
                    title: "Manage",
                    tools: vec![
                        RibbonItem::LargeTool(command(
                            "DELCONSTRAINT", "Delete Constraints", include_bytes!("../../../assets/icons/constrain/delete.svg"),
                        )),
                        RibbonItem::LargeTool(command(
                            "PARAMETERS", "Parameters Manager", include_bytes!("../../../assets/icons/constrain/parameters.svg"),
                        )),
                    ],
                },
            ]
        })
    }
}

inventory::submit!(crate::command::CommandRegistration {
    names: &[
        "AUTOCONSTRAIN", "SMOOTHCONSTRAINT", "GCSHOW", "GCHIDE", "GCRESET",
        "GCSHOWALL", "GCHIDEALL", "DCSHOW", "DCHIDE", "DCSHOWALL", "DCHIDEALL",
        "DCCONVERT", "DELCONSTRAINT",
    ]
});

#[cfg(test)]
mod tests {
    use super::*;

    fn item_id(item: &RibbonItem) -> &'static str {
        match item {
            RibbonItem::Tool(tool) | RibbonItem::LabeledTool(tool) | RibbonItem::LargeTool(tool) => tool.id,
            RibbonItem::Dropdown { id, .. } | RibbonItem::LabeledDropdown { id, .. }
            | RibbonItem::LargeDropdown { id, .. } => id,
            RibbonItem::ToolGrid { .. } => "GRID",
            _ => panic!("unexpected composite ribbon item"),
        }
    }

    #[test]
    fn ribbon_uses_geometry_dimension_and_manage_panels() {
        let groups = ParametricModule.ribbon_groups();

        assert_eq!(
            groups.iter().map(|group| group.title).collect::<Vec<_>>(),
            ["Geometric", "Dimensional", "Manage"]
        );
        assert_eq!(
            groups[0].tools.iter().map(item_id).collect::<Vec<_>>(),
            ["AUTOCONSTRAIN", "GRID", "GCVISIBILITY", "GCSHOWALL", "GCHIDEALL"]
        );
        assert_eq!(
            groups[1].tools.iter().map(item_id).collect::<Vec<_>>(),
            ["DC_LINEAR_MENU", "DCALIGNED", "GRID", "DCVISIBILITY", "DCSHOWALL", "DCHIDEALL"]
        );
        assert_eq!(groups[2].tools.iter().map(item_id).collect::<Vec<_>>(), ["DELCONSTRAINT", "PARAMETERS"]);

        let RibbonItem::LargeDropdown { items, default, .. } = &groups[1].tools[0] else {
            panic!("linear dimensional constraint must be a split dropdown");
        };
        assert_eq!(*default, "DCLINEAR");
        assert_eq!(
            items.iter().map(|(id, _, _)| *id).collect::<Vec<_>>(),
            ["DCLINEAR", "DCHORIZONTAL", "DCVERTICAL"]
        );
        let RibbonItem::ToolGrid { columns } = &groups[0].tools[1] else { panic!("geometry grid") };
        assert_eq!(columns.iter().flatten().map(|tool| tool.id).collect::<Vec<_>>(), [
            "CCONSTRAINT", "PCONSTRAINT", "TCONSTRAINT", "LCONSTRAINT", "QCONSTRAINT", "SMOOTHCONSTRAINT",
            "NCONSTRAINT", "HCONSTRAINT", "SYCONSTRAINT", "FXCONSTRAINT", "VCONSTRAINT", "ECONSTRAINT",
        ]);
        let RibbonItem::ToolGrid { columns } = &groups[1].tools[2] else { panic!("dimension grid") };
        assert_eq!(columns.iter().flatten().map(|tool| tool.id).collect::<Vec<_>>(), [
            "DCANGULAR", "DCDIAMETER", "DCRADIUS", "DCCONVERT",
        ]);
    }
}
