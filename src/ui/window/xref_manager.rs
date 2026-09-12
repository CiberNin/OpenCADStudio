//! Reference Manager palette — display-only list of external references.
//!
//! Task 7 build: a read-only table (Reference / Status / Size / Type / Date /
//! Saved Path) fed by [`collect_entries`], a List/Tree toggle over the nested
//! closure, a details pane for the selected entry, and a toolbar whose
//! mutating affordances are present but disabled. Mutations (attach result
//! handling, path edits, unload/reload, missing-on-open prompt) arrive in
//! Tasks 8–9 — this file performs no document mutation.

use crate::app::Message;
use crate::io::xref::collect_entries;
use crate::io::xref_model::{normalize_lexical, RefKind, RefStatus, RefType, ReferenceEntry};
use crate::ui::ROW_H;
use acadrust::CadDocument;
use iced::widget::{button, column, container, mouse_area, row, scrollable, text, tooltip};
use iced::Padding;
use iced::{Background, Border, Element, Fill, Length, Theme};
use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

/// `true` on web builds, where the filesystem and native file pickers do not
/// exist. Mutating affordances stay compiled but render disabled; the
/// read-only list renders everywhere.
const IS_WASM: bool = cfg!(target_arch = "wasm32");

/// Font size for table cells (mirrors `layers.rs`).
const FONT_SZ: f32 = ROW_H * 0.42; // ≈11 px at ROW_H=26
/// Fixed table height so the details pane below keeps stable space.
const TABLE_H: f32 = 240.0;
/// Indent per tree depth level.
const INDENT_W: f32 = 16.0;

/// One visible row in [`XrefManagerPanel::display_rows`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DisplayRow {
    /// Index into [`XrefManagerPanel::entries`].
    pub index: usize,
    /// Tree depth (0 in list mode and for roots).
    pub depth: u32,
    /// True for entries enumerated from a nested file (never rendered).
    pub is_nested: bool,
}

/// Display-only state for the Reference Manager palette.
///
/// `unloaded_keys` is the session unloaded set — empty for now; Task 8 wires
/// it to unload/reload without restructuring this state.
#[derive(Default)]
pub struct XrefManagerPanel {
    /// Cached [`collect_entries`] output for the active drawing.
    pub entries: Vec<ReferenceEntry>,
    /// Block-record / definition handles treated as unloaded (Task 8).
    pub unloaded_keys: HashSet<u64>,
    /// Multi-selected entry indices — consumed by batch path ops in Task 8.
    pub selected: HashSet<usize>,
    /// Anchor entry driving the details pane (last row clicked).
    pub anchor: Option<usize>,
    /// List (`false`) vs tree (`true`) presentation.
    pub tree: bool,
    /// Parent keys expanded in tree mode. Empty by default = collapsed.
    pub expanded: HashSet<u64>,
    /// Parent key → child entry indices, rebuilt on every refresh.
    pub children: HashMap<u64, Vec<usize>>,
    /// Entry indices enumerated from nested files (not the host drawing).
    pub nested: HashSet<usize>,
    /// Document tab id that produced `entries` (stale check).
    pub source_tab_id: Option<u64>,
}

impl XrefManagerPanel {
    /// Rebuild `entries` from `doc` via [`collect_entries`] and recompute the
    /// parent → children linkage for tree mode. Selection/anchor survive by
    /// `(key, saved_path)` identity; dead indices are dropped.
    pub fn refresh(&mut self, doc: &CadDocument, base_dir: &Path) {
        let old = std::mem::take(&mut self.entries);
        let sel_ids: HashSet<(u64, String)> = self
            .selected
            .iter()
            .filter_map(|&i| old.get(i))
            .map(|e| (e.key, e.saved_path.clone()))
            .collect();
        let anchor_id: Option<(u64, String)> = self
            .anchor
            .and_then(|i| old.get(i))
            .map(|e| (e.key, e.saved_path.clone()));

        let entries = collect_entries(doc, base_dir, &self.unloaded_keys);
        // Direct references, keyed by (handle, saved path) exactly as
        // `collect_entries` emits them. The saved path disambiguates a nested
        // entry whose foreign handle happens to collide with a host handle.
        let direct_ids = direct_identities(doc);
        let mut nested = HashSet::new();
        for (i, e) in entries.iter().enumerate() {
            if !direct_ids.contains(&(e.key, e.saved_path.clone())) {
                nested.insert(i);
            }
        }
        // Parent linkage: each loaded drawing root's file is opened read-only
        // and its direct reference paths are matched (by lexical identity)
        // against nested entries. First parent wins; unmatchable nested
        // entries stay unclaimed and render as roots so nothing vanishes.
        let mut children: HashMap<u64, Vec<usize>> = HashMap::new();
        let mut claimed: HashSet<usize> = HashSet::new();
        let direct_order: Vec<usize> = entries
            .iter()
            .enumerate()
            .filter(|(i, _)| !nested.contains(i))
            .map(|(i, _)| i)
            .collect();
        for &pi in &direct_order {
            let parent = &entries[pi];
            if parent.kind != RefKind::DwgXref
                || parent.status != RefStatus::Loaded
                || parent.found_at.is_none()
            {
                continue;
            }
            let found = parent.found_at.as_deref().unwrap_or("");
            let child_paths: HashSet<String> = match crate::io::load_file(Path::new(found)) {
                Ok(nested_doc) => nested_doc
                    .block_records
                    .iter()
                    .filter(|br| br.flags.is_xref || br.flags.is_xref_overlay)
                    .map(|br| normalize_lexical(&br.xref_path))
                    .collect(),
                Err(_) => continue,
            };
            for (ci, child) in entries.iter().enumerate() {
                if !nested.contains(&ci) || claimed.contains(&ci) {
                    continue;
                }
                if child_paths.contains(&normalize_lexical(&child.saved_path)) {
                    children.entry(parent.key).or_default().push(ci);
                    claimed.insert(ci);
                }
            }
        }

        let live_keys: HashSet<u64> = entries.iter().map(|e| e.key).collect();
        self.entries = entries;
        self.nested = nested;
        self.children = children;
        self.selected = self
            .entries
            .iter()
            .enumerate()
            .filter(|(_, e)| sel_ids.contains(&(e.key, e.saved_path.clone())))
            .map(|(i, _)| i)
            .collect();
        self.anchor = anchor_id.and_then(|(k, p)| {
            self.entries
                .iter()
                .position(|e| e.key == k && e.saved_path == p)
        });
        self.expanded.retain(|k| live_keys.contains(k));
    }

    /// Click toggles membership in the multi-selection set; selecting sets the
    /// details anchor, deselecting the anchor falls back to a survivor.
    pub fn toggle_select(&mut self, index: usize) {
        if index >= self.entries.len() {
            return;
        }
        if !self.selected.remove(&index) {
            self.selected.insert(index);
            self.anchor = Some(index);
        } else if self.anchor == Some(index) {
            self.anchor = self.selected.iter().copied().max();
        }
    }

    /// Flip list/tree presentation.
    pub fn toggle_tree(&mut self) {
        self.tree = !self.tree;
    }

    /// Expand/collapse one tree parent (no-op for unknown keys).
    pub fn toggle_expand(&mut self, parent_key: u64) {
        if !self.expanded.remove(&parent_key) && self.children.contains_key(&parent_key) {
            self.expanded.insert(parent_key);
        }
    }

    /// Flat render order for the current mode. Tree mode emits roots
    /// (direct entries plus unclaimed nested ones) with expanded children
    /// inline, skipping repeats by normalized saved path so a cyclic closure
    /// can never recurse on screen.
    pub fn display_rows(&self) -> Vec<DisplayRow> {
        if !self.tree {
            return self
                .entries
                .iter()
                .enumerate()
                .map(|(index, _)| DisplayRow {
                    index,
                    depth: 0,
                    is_nested: self.nested.contains(&index),
                })
                .collect();
        }
        let mut rows = Vec::new();
        let mut seen: HashSet<String> = HashSet::new();
        // Roots first, in collect order; children follow an expanded parent.
        for (index, entry) in self.entries.iter().enumerate() {
            let dominated = self
                .children
                .values()
                .any(|kids| kids.contains(&index));
            if self.nested.contains(&index) && dominated {
                continue;
            }
            if !claim_path(&mut seen, &entry.saved_path) {
                continue;
            }
            rows.push(DisplayRow {
                index,
                depth: 0,
                is_nested: self.nested.contains(&index),
            });
            if self.expanded.contains(&entry.key) {
                if let Some(kids) = self.children.get(&entry.key) {
                    for &ci in kids {
                        let Some(child) = self.entries.get(ci) else {
                            continue;
                        };
                        if !claim_path(&mut seen, &child.saved_path) {
                            continue;
                        }
                        rows.push(DisplayRow {
                            index: ci,
                            depth: 1,
                            is_nested: true,
                        });
                    }
                }
            }
        }
        rows
    }

    /// Render the palette as the full content of its modal dialog.
    pub fn view_window(&self, sizing: crate::ui::modal::ModalSizing) -> Element<'_, Message> {
        // ── Toolbar ───────────────────────────────────────────────────────
        let can_attach = !IS_WASM;
        let attach = if can_attach {
            toolbar_btn("Attach", Some(Message::XAttachPick)) // XREF-Task8: locale
        } else {
            toolbar_tip(
                "Attach", // XREF-Task8: locale
                "File attach is not available on web — the reference list below is read-only.", // XREF-Task8: locale
            )
        };
        let image_btn = toolbar_tip(
            "Image…", // XREF-Task8: locale
            "images/PDF attach — Task 8", // XREF-Task8: locale
        );
        let pdf_btn = toolbar_tip(
            "PDF…", // XREF-Task8: locale
            "images/PDF attach — Task 8", // XREF-Task8: locale
        );
        let refresh = toolbar_btn("Refresh", Some(Message::XrefManagerRefresh)); // XREF-Task8: locale
        let mode_label = if self.tree { "List" } else { "Tree" }; // XREF-Task8: locale
        let mode_btn = toolbar_btn(mode_label, Some(Message::XrefManagerToggleTree));
        let mode_btn: Element<'_, Message> = tooltip(
            mode_btn,
            text("Toggle list/tree").size(11), // XREF-Task8: locale
            tooltip::Position::Bottom,
        )
        .into();
        // Change Path submenu items exist but stay disabled until Task 8.
        let change_path = row![
            text("Change Path:").size(11), // XREF-Task8: locale
            toolbar_tip("Full", "path operations — Task 8"), // XREF-Task8: locale
            toolbar_tip("Relative", "path operations — Task 8"), // XREF-Task8: locale
            toolbar_tip("File name", "path operations — Task 8"), // XREF-Task8: locale
        ]
        .spacing(2)
        .align_y(iced::Center);
        // No docs hook exists yet — tooltip only, no docs files (Task 7).
        let help = toolbar_tip(
            "Help", // XREF-Task8: locale
            "Reference Manager — path operations ship with a later task.", // XREF-Task8: locale
        );
        let toolbar = container(
            row![attach, image_btn, pdf_btn, refresh, mode_btn, change_path, help]
                .spacing(4)
                .align_y(iced::Center),
        )
        .style(|theme: &Theme| container::Style {
            background: Some(Background::Color(theme.palette().background.weak.color)),
            ..Default::default()
        })
        .width(sizing.width)
        .padding([4, 8]);

        // ── Column header ─────────────────────────────────────────────────
        let col_header = container(
            row![
                text("Reference").size(10).style(muted_style).width(Length::FillPortion(3)), // XREF-Task8: locale
                text("Status").size(10).style(muted_style).width(Length::Fixed(88.0)), // XREF-Task8: locale
                text("Size").size(10).style(muted_style).width(Length::Fixed(76.0)), // XREF-Task8: locale
                text("Type").size(10).style(muted_style).width(Length::Fixed(76.0)), // XREF-Task8: locale
                text("Date").size(10).style(muted_style).width(Length::Fixed(96.0)), // XREF-Task8: locale
                text("Saved Path").size(10).style(muted_style).width(Length::FillPortion(4)), // XREF-Task8: locale
            ]
            .spacing(4)
            .width(sizing.width)
            .align_y(iced::Center),
        )
        .style(|theme: &Theme| {
            let palette = theme.palette();
            container::Style {
                background: Some(Background::Color(palette.background.weak.color)),
                border: Border {
                    color: palette.background.neutral.color,
                    width: 1.0,
                    radius: 0.0.into(),
                },
                ..Default::default()
            }
        })
        .padding([4, 8])
        .width(sizing.width);

        // ── Reference rows ────────────────────────────────────────────────
        let mut rows_col = column![].spacing(0);
        for display in self.display_rows() {
            let Some(entry) = self.entries.get(display.index) else {
                continue;
            };
            let is_sel = self.selected.contains(&display.index);
            let kids = self.children.get(&entry.key);
            let has_children = kids.is_some_and(|k| !k.is_empty());
            let is_expanded = has_children && self.expanded.contains(&entry.key);
            rows_col = rows_col.push(xref_row(
                display,
                entry,
                is_sel,
                self.tree && has_children,
                is_expanded,
            ));
        }
        let body: Element<'_, Message> = if self.entries.is_empty() {
            container(
                text("No external references in this drawing") // XREF-Task8: locale
                    .size(12)
                    .color(iced::Color {
                        r: 0.55,
                        g: 0.55,
                        b: 0.55,
                        a: 1.0,
                    }),
            )
            .center_x(Fill)
            .center_y(Fill)
            .width(Fill)
            .height(Length::Fixed(TABLE_H))
            .into()
        } else {
            scrollable(rows_col).height(Length::Fixed(TABLE_H)).into()
        };

        // ── Details pane ──────────────────────────────────────────────────
        let details = details_pane(self.anchor.and_then(|i| self.entries.get(i)));

        container(column![toolbar, col_header, body, details].spacing(0))
            .style(|theme: &Theme| container::Style {
                background: Some(Background::Color(
                    theme.palette().background.base.color,
                )),
                ..Default::default()
            })
            .width(sizing.width)
            .height(sizing.height)
            .into()
    }
}

/// `(key, saved_path)` identities for every reference the host drawing holds
/// directly — mirrors the three [`collect_entries`] scans so nested entries
/// (foreign handles) classify correctly even on handle collisions.
fn direct_identities(doc: &CadDocument) -> HashSet<(u64, String)> {
    use acadrust::entities::UnderlayType;
    use acadrust::objects::ObjectType;
    use acadrust::types::Handle;
    use acadrust::EntityType;
    use rustc_hash::FxHashSet;

    let mut ids: HashSet<(u64, String)> = HashSet::new();
    for br in doc.block_records.iter() {
        if br.flags.is_xref || br.flags.is_xref_overlay {
            ids.insert((br.handle.value(), br.xref_path.clone()));
        }
    }
    let mut referenced_images: FxHashSet<Handle> = FxHashSet::default();
    for e in doc.entities() {
        if let EntityType::RasterImage(img) = e {
            if let Some(h) = img.definition_handle {
                referenced_images.insert(h);
            }
        }
    }
    for (handle, obj) in doc.objects.iter() {
        match obj {
            ObjectType::ImageDefinition(def) => {
                if referenced_images.contains(handle) {
                    ids.insert((handle.value(), def.file_name.clone()));
                }
            }
            ObjectType::UnderlayDefinition(def) => {
                if def.underlay_type == UnderlayType::Pdf {
                    ids.insert((handle.value(), def.file_path.clone()));
                }
            }
            _ => {}
        }
    }
    ids
}

/// Claim a normalized saved path for display; empty paths always pass, repeats
/// are skipped (cycle guard).
fn claim_path(seen: &mut HashSet<String>, saved_path: &str) -> bool {
    if saved_path.is_empty() {
        return true;
    }
    seen.insert(normalize_lexical(saved_path))
}

// ── Text helpers ──────────────────────────────────────────────────────────

fn status_text(status: RefStatus) -> &'static str {
    match status {
        RefStatus::Loaded => "Loaded", // XREF-Task8: locale
        RefStatus::Unloaded => "Unloaded", // XREF-Task8: locale
        RefStatus::NotFound => "Not found", // XREF-Task8: locale
        RefStatus::Failed => "Failed", // XREF-Task8: locale
        RefStatus::Stale => "Stale", // XREF-Task8: locale
        RefStatus::Orphaned => "Orphaned", // XREF-Task8: locale
    }
}

fn type_text(entry: &ReferenceEntry) -> &'static str {
    match entry.kind {
        RefKind::DwgXref => match entry.ref_type {
            RefType::Attach => "Attach", // XREF-Task8: locale
            RefType::Overlay => "Overlay", // XREF-Task8: locale
        },
        RefKind::Image => "Image", // XREF-Task8: locale
        RefKind::Pdf => "PDF", // XREF-Task8: locale
    }
}

fn format_size(size_bytes: Option<u64>) -> String {
    match size_bytes {
        None => "—".to_string(),
        Some(b) if b < 1024 => format!("{b} B"),
        Some(b) if b < 1024 * 1024 => format!("{:.1} KB", b as f64 / 1024.0),
        Some(b) if b < 1024 * 1024 * 1024 => format!("{:.1} MB", b as f64 / 1_048_576.0),
        Some(b) => format!("{:.1} GB", b as f64 / 1_073_741_824.0),
    }
}

/// Calendar date (`YYYY-MM-DD`) for a file modification time. Hand-rolled
/// civil-from-days so no date crate joins the dependency tree.
fn format_date(modified: Option<SystemTime>) -> String {
    let Some(t) = modified else {
        return "—".to_string();
    };
    let secs = t
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let z = secs.div_euclid(86_400) + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy as i64 - (153 * mp as u64 + 2) as i64 / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as i64;
    format!("{:04}-{:02}-{:02}", if m <= 2 { y + 1 } else { y }, m, d)
}

// ── Widget helpers (layers.rs conventions) ────────────────────────────────

fn muted_style(theme: &Theme) -> iced::widget::text::Style {
    iced::widget::text::Style {
        color: Some(theme.palette().background.base.text.scale_alpha(0.68)),
    }
}

fn row_button_style(selected: bool, index: usize) -> impl Fn(&Theme, button::Status) -> button::Style {
    move |theme: &Theme, status: button::Status| {
        let palette = theme.palette();
        let highlighted = matches!(status, button::Status::Hovered);
        let pair = if highlighted {
            palette.background.strong
        } else if selected {
            palette.primary.weak
        } else if index % 2 == 0 {
            palette.background.base
        } else {
            palette.background.weak
        };
        button::Style {
            background: highlighted.then_some(Background::Color(pair.color)),
            text_color: pair.text,
            ..Default::default()
        }
    }
}

fn toolbar_btn<'a>(label: &'static str, msg: Option<Message>) -> Element<'a, Message> {
    let mut b = button(text(label).size(11))
        .style(|theme: &Theme, status| {
            let palette = theme.palette();
            let pair = match status {
                button::Status::Hovered | button::Status::Pressed => {
                    palette.background.strong
                }
                _ => palette.background.weak,
            };
            button::Style {
                background: Some(Background::Color(pair.color)),
                border: Border {
                    radius: 3.0.into(),
                    color: palette.background.neutral.color,
                    width: 1.0,
                },
                text_color: pair.text,
                ..Default::default()
            }
        })
        .padding([4, 10]);
    if let Some(m) = msg {
        b = b.on_press(m);
    }
    b.into()
}

/// Disabled toolbar affordance with an explanatory tooltip (Task 8 work or
/// web-gated mutations).
fn toolbar_tip<'a>(label: &'static str, tip: &'static str) -> Element<'a, Message> {
    let b = button(text(label).size(11).style(|theme: &Theme| {
        iced::widget::text::Style {
            color: Some(
                theme
                    .palette()
                    .background
                    .base
                    .text
                    .scale_alpha(0.42),
            ),
        }
    }))
    .style(|theme: &Theme, _| {
        let palette = theme.palette();
        button::Style {
            background: Some(Background::Color(palette.background.weak.color)),
            border: Border {
                radius: 3.0.into(),
                color: palette.background.neutral.color,
                width: 1.0,
            },
            text_color: palette.background.weak.text.scale_alpha(0.42),
            ..Default::default()
        }
    })
    .padding([4, 10]);
    tooltip(b, text(tip).size(11), tooltip::Position::Bottom).into()
}

fn xref_row<'a>(
    display: DisplayRow,
    entry: &'a ReferenceEntry,
    is_selected: bool,
    show_expand: bool,
    is_expanded: bool,
) -> Element<'a, Message> {
    let mut name = entry.name.clone();
    if display.is_nested {
        name.push_str(" (nested — not rendered)"); // XREF-Task8: locale
    }
    let mut name_cell = row![].spacing(2).align_y(iced::Center);
    if display.depth > 0 {
        name_cell = name_cell.push(
            iced::widget::Space::new().width(Length::Fixed(INDENT_W * display.depth as f32)),
        );
    }
    // Tree parents own the only interactive cell besides selection: the
    // expand/collapse arrow. Nested rows never show one.
    if show_expand {
        let key = entry.key;
        let arrow = button(crate::ui::icons::themed_arrow_toggle(is_expanded, 10.0))
            .on_press(Message::XrefManagerToggleExpand(key))
            .style(row_button_style(is_selected, display.index))
            .padding(Padding {
                top: 6.0,
                bottom: 6.0,
                left: 2.0,
                right: 2.0,
            });
        name_cell = name_cell.push(arrow);
    }
    let name_text: Element<'_, Message> = text(name).size(FONT_SZ).into();
    name_cell = name_cell.push(name_text);
    let content = row![
        name_cell.width(Length::FillPortion(3)),
        text(status_text(entry.status)).size(FONT_SZ).width(Length::Fixed(88.0)),
        text(format_size(entry.size_bytes)).size(FONT_SZ).width(Length::Fixed(76.0)),
        text(type_text(entry)).size(FONT_SZ).width(Length::Fixed(76.0)),
        text(format_date(entry.modified)).size(FONT_SZ).width(Length::Fixed(96.0)),
        text(entry.saved_path.clone()).size(FONT_SZ).width(Length::FillPortion(4)),
    ]
    .spacing(4)
    .width(Fill)
    .align_y(iced::Center);

    let index = display.index;
    mouse_area(
        container(content)
            .style(move |theme: &Theme| {
                let palette = theme.palette();
                let pair = if is_selected {
                    palette.primary.weak
                } else if index % 2 == 0 {
                    palette.background.base
                } else {
                    palette.background.weak
                };
                container::Style {
                    background: Some(Background::Color(pair.color)),
                    text_color: Some(pair.text),
                    ..Default::default()
                }
            })
            .padding(Padding {
                top: 0.0,
                bottom: 0.0,
                left: 8.0,
                right: 8.0,
            })
            .height(Length::Fixed(ROW_H))
            .width(Fill),
    )
    .on_press(Message::XrefManagerSelect(index))
    .into()
}

fn details_pane(entry: Option<&ReferenceEntry>) -> Element<'_, Message> {
    let inner: Element<'_, Message> = match entry {
        None => text("Select a reference to inspect its details") // XREF-Task8: locale
            .size(11)
            .style(muted_style)
            .into(),
        Some(e) => {
            let found = e.found_at.as_deref().unwrap_or("—");
            let saved = if e.saved_path.is_empty() {
                "—"
            } else {
                e.saved_path.as_str()
            };
            let size = format_size(e.size_bytes);
            let date = format_date(e.modified);
            column![
                detail_row("Reference", e.name.as_str()), // XREF-Task8: locale
                detail_row("Status", status_text(e.status)), // XREF-Task8: locale
                detail_row("Size", size), // XREF-Task8: locale
                detail_row("Type", type_text(e)), // XREF-Task8: locale
                detail_row("Date", date), // XREF-Task8: locale
                detail_row("Found At", found), // XREF-Task8: locale
                detail_row("Saved Path", saved), // XREF-Task8: locale
            ]
            .spacing(1)
            .into()
        }
    };
    container(
        column![
            text("Details").size(10).style(muted_style), // XREF-Task8: locale
            inner,
        ]
        .spacing(2),
    )
    .style(|theme: &Theme| container::Style {
        background: Some(Background::Color(theme.palette().background.weak.color)),
        border: Border {
            color: theme.palette().background.neutral.color,
            width: 1.0,
            radius: 0.0.into(),
        },
        ..Default::default()
    })
    .padding([6, 8])
    .width(Fill)
    .into()
}

fn detail_row<'a>(label: &'static str, value: impl Into<std::borrow::Cow<'a, str>>) -> Element<'a, Message> {
    let value: std::borrow::Cow<'a, str> = value.into();
    row![
        text(label).size(11).style(muted_style).width(Length::Fixed(84.0)),
        text(value).size(11),
    ]
    .spacing(6)
    .align_y(iced::Center)
    .into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::io::xref_model::RefKind;

    #[test]
    fn size_formats_without_deps() {
        assert_eq!(format_size(None), "—");
        assert_eq!(format_size(Some(0)), "0 B");
        assert_eq!(format_size(Some(900)), "900 B");
        assert_eq!(format_size(Some(2048)), "2.0 KB");
        assert_eq!(format_size(Some(5 * 1_048_576)), "5.0 MB");
    }

    #[test]
    fn date_formats_known_days() {
        assert_eq!(format_date(None), "—");
        let day = |days: u64| {
            format_date(Some(UNIX_EPOCH + std::time::Duration::from_secs(days * 86_400)))
        };
        assert_eq!(day(0), "1970-01-01");
        assert_eq!(day(10957), "2000-01-01");
        assert_eq!(day(19723), "2024-01-01");
    }

    fn entry(key: u64, name: &str, saved: &str) -> ReferenceEntry {
        let mut e = ReferenceEntry::new(key, name, RefKind::DwgXref);
        e.saved_path = saved.to_string();
        e
    }

    #[test]
    fn select_toggles_anchor_and_set() {
        let mut panel = XrefManagerPanel::default();
        panel.entries = vec![entry(1, "A", "a.dwg"), entry(2, "B", "b.dwg")];
        panel.toggle_select(0);
        panel.toggle_select(1);
        assert!(panel.selected.contains(&0) && panel.selected.contains(&1));
        assert_eq!(panel.anchor, Some(1));
        panel.toggle_select(1);
        assert!(!panel.selected.contains(&1));
        assert_eq!(panel.anchor, Some(0));
        panel.toggle_select(9); // out of range: no-op
        assert_eq!(panel.selected.len(), 1);
    }

    #[test]
    fn tree_skips_repeat_paths() {
        let mut panel = XrefManagerPanel::default();
        panel.entries = vec![
            entry(1, "A", "a.dwg"),
            entry(2, "B", "b.dwg"),
            entry(3, "B2", "b.dwg"),
        ];
        panel.tree = true;
        // Repeats collapse even among roots: B2 shares B's path.
        assert_eq!(panel.display_rows().len(), 2);
        // Mark B2 nested under A: the repeat path now collapses away —
        // A claims a.dwg, B2-as-child claims b.dwg, the B root is a repeat.
        panel.nested.insert(2);
        panel.children.insert(1, vec![2]);
        panel.expanded.insert(1);
        let rows = panel.display_rows();
        assert_eq!(rows.len(), 2); // A, B-as-child-of-A
        assert!(rows.iter().any(|r| r.index == 2 && r.is_nested));
        // A second parent claiming the same path shows nothing twice.
        panel.entries.push(entry(4, "C", "c.dwg"));
        panel.children.insert(4, vec![2]);
        panel.expanded.insert(4);
        let count = panel
            .display_rows()
            .iter()
            .filter(|r| r.index == 2)
            .count();
        assert_eq!(count, 1);
    }

    #[test]
    fn expand_collapses_by_default() {
        let mut panel = XrefManagerPanel::default();
        panel.entries = vec![entry(1, "A", "a.dwg"), entry(2, "B", "b.dwg")];
        panel.nested.insert(1);
        panel.children.insert(1, vec![1]);
        panel.tree = true;
        assert!(!panel.display_rows().iter().any(|r| r.is_nested));
        panel.toggle_expand(1);
        assert!(panel.display_rows().iter().any(|r| r.is_nested));
        panel.toggle_expand(1);
        assert!(!panel.display_rows().iter().any(|r| r.is_nested));
    }
}
