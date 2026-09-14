//! Additional annotation tools reached from the Draw tab's Annotation panel title.

use super::draw_panel::Tool;

pub(super) const TOOLS: &[Tool] = &[
    Tool {
        command: "DDEDIT",
        label: "Edit Text",
        icon: include_bytes!("../../../assets/icons/ddedit.svg"),
        options: &[],
    },
    Tool {
        command: "TOLERANCE",
        label: "Geometric Tolerance",
        icon: include_bytes!("../../../assets/icons/tolerance.svg"),
        options: &[],
    },
    Tool {
        command: "DIMALIGNED",
        label: "Aligned Dimension",
        icon: include_bytes!("../../../assets/icons/dim_aligned.svg"),
        options: &[],
    },
    Tool {
        command: "DIMARC",
        label: "Arc Length Dimension",
        icon: include_bytes!("../../../assets/icons/dim_angular.svg"),
        options: &[],
    },
    Tool {
        command: "DIMJOGGED",
        label: "Jogged Radius Dimension",
        icon: include_bytes!("../../../assets/icons/dim_jog.svg"),
        options: &[],
    },
    Tool {
        command: "DIMDIAMETER",
        label: "Diameter Dimension",
        icon: include_bytes!("../../../assets/icons/dim_diameter.svg"),
        options: &[],
    },
    Tool {
        command: "DIMORDINATE",
        label: "Ordinate Dimension",
        icon: include_bytes!("../../../assets/icons/dim_ordinate.svg"),
        options: &[],
    },
    Tool {
        command: "QDIM",
        label: "Quick Dimension",
        icon: include_bytes!("../../../assets/icons/qdim.svg"),
        options: &[],
    },
    Tool {
        command: "DIMCONTINUE",
        label: "Continue Dimension",
        icon: include_bytes!("../../../assets/icons/dim_continue.svg"),
        options: &[],
    },
    Tool {
        command: "DIMBASELINE",
        label: "Baseline Dimension",
        icon: include_bytes!("../../../assets/icons/dim_baseline.svg"),
        options: &[],
    },
    Tool {
        command: "DIMBREAK",
        label: "Dimension Break",
        icon: include_bytes!("../../../assets/icons/dim_break.svg"),
        options: &[],
    },
    Tool {
        command: "DIMEDIT",
        label: "Edit Dimension",
        icon: include_bytes!("../../../assets/icons/dim_edit.svg"),
        options: &[],
    },
    Tool {
        command: "DIMTEDIT",
        label: "Edit Dimension Text",
        icon: include_bytes!("../../../assets/icons/dim_tedit.svg"),
        options: &[],
    },
    Tool {
        command: "DIMSPACE",
        label: "Adjust Dimension Space",
        icon: include_bytes!("../../../assets/icons/dim_space.svg"),
        options: &[],
    },
    Tool {
        command: "DIMJOGLINE",
        label: "Add or Remove Jog Line",
        icon: include_bytes!("../../../assets/icons/dim_jog.svg"),
        options: &[],
    },
    Tool {
        command: "MLEADERADD",
        label: "Add Leader",
        icon: include_bytes!("../../../assets/icons/mleader_add.svg"),
        options: &[],
    },
    Tool {
        command: "MLEADERREMOVE",
        label: "Remove Leader",
        icon: include_bytes!("../../../assets/icons/mleader_remove.svg"),
        options: &[],
    },
    Tool {
        command: "MLEADERALIGN",
        label: "Align Leaders",
        icon: include_bytes!("../../../assets/icons/mleader_align.svg"),
        options: &[],
    },
    Tool {
        command: "MLEADERCOLLECT",
        label: "Collect Leaders",
        icon: include_bytes!("../../../assets/icons/mleader_collect.svg"),
        options: &[],
    },
    Tool {
        command: "WIPEOUT",
        label: "Wipeout",
        icon: include_bytes!("../../../assets/icons/wipeout.svg"),
        options: &[],
    },
    Tool {
        command: "TABLE",
        label: "Table and Data",
        icon: include_bytes!("../../../assets/icons/table.svg"),
        options: &[
            ("TABLE", "Table"),
            ("DATALINK", "Link Data"),
            ("DATAEXTRACTION", "Extract Data"),
        ],
    },
    Tool {
        command: "CENTERMARK",
        label: "Center Mark and Line",
        icon: include_bytes!("../../../assets/icons/line.svg"),
        options: &[
            ("CENTERMARK", "Center Mark"),
            ("CENTERLINE", "Center Line"),
            ("CENTERREASSOCIATE", "Reassociate Center Object"),
            ("CENTERDISASSOCIATE", "Disassociate Center Object"),
            ("CENTERRESET", "Reset Center Object"),
        ],
    },
    Tool {
        command: "REVCLOUD",
        label: "Revision Cloud",
        icon: include_bytes!("../../../assets/icons/revcloud.svg"),
        options: &[
            ("REVCLOUD_RECTANGULAR", "Rectangular"),
            ("REVCLOUD_POLYGONAL", "Polygonal"),
            ("REVCLOUD_FREEHAND", "Freehand"),
        ],
    },
    Tool {
        command: "OBJECTSCALE",
        label: "Annotation Scales",
        icon: include_bytes!("../../../assets/icons/add_scale.svg"),
        options: &[
            ("OBJECTSCALE ADD", "Add Current Scale"),
            ("OBJECTSCALE DELETE", "Delete Current Scale"),
            ("OBJECTSCALE", "Add or Delete Scales"),
            ("SCALELISTEDIT", "Edit Scale List"),
            ("ANNORESET", "Sync Scale Positions"),
        ],
    },
];
