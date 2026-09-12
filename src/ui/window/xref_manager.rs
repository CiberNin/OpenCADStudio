//! Reference Manager palette — list of external references with operations.
//!
//! Task 8b build: the toolbar's mutating affordances are wired to the same
//! engine fns the CLI uses, operating on the multi-selection with per-item
//! report lines. Session state (unloaded set, stat cache) lives per-tab on
//! `DocumentTab`; this panel only caches display rows plus text inputs.

use crate::app::Message;
use crate::io::xref::collect_entries_with_prev;
use crate::io::xref_model::{normalize_lexical, Pathtype, RefKind, RefStatus, RefType, ReferenceEntry};
use crate::ui::ROW_H;
use acadrust::CadDocument;
use iced::widget::{button, column, container, mouse_area, row, scrollable, text, text_input, tooltip};
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

/// Palette state for the Reference Manager.
///
/// Session state (unloaded set, stat cache) lives per-tab on `DocumentTab`
/// and is passed into [`refresh`](XrefManagerPanel::refresh); this panel
/// only caches display rows, expansion, selection, and text inputs.
#[derive(Default)]
pub struct XrefManagerPanel {
    /// Cached [`collect_entries_with_prev`] output for the active drawing.
    pub entries: Vec<ReferenceEntry>,
    /// Multi-selected entry indices — consumed by batch path ops.
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
    /// `edit_revision` that produced `entries` — the palette auto-rescans
    /// when the tab's revision moves (reuses the undo-snapshot counter, so
    /// CLI and palette mutations both trip it).
    pub source_edit_revision: u64,
    /// Draft for the details-pane "new path" edit.
    pub path_input: String,
    /// Drafts for the Find & Replace row.
    pub find_input: String,
    pub replace_input: String,
}

/// Selection-gated palette operation the toolbar offers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum XrefPaletteOp {
    Detach,
    Unload,
    Reload,
    Bind,
    Overlay,
    Attach,
    Pathtype(Pathtype),
}

impl XrefManagerPanel {
    /// Rebuild `entries` from `doc` via [`collect_entries_with_prev`] and
    /// recompute the parent → children linkage for tree mode. Selection/anchor
    /// survive by `(key, saved_path)` identity; dead indices are dropped.
    ///
    /// `unloaded`/`prev` are the active tab's session sets, so CLI and
    /// palette agree. Returns fresh `(key, mtime)` baselines for every
    /// `Loaded` entry — the caller writes them into the tab's stat cache,
    /// which makes `Stale` detectable from the second refresh on.
    /// Wasm-safe by construction: the only I/O is `collect_entries`' path
    /// stat plus `load_file` for nested enumeration, both already wasm-safe.
    pub fn refresh(
        &mut self,
        doc: &CadDocument,
        base_dir: &Path,
        unloaded: &HashSet<crate::io::xref_model::UnloadKey>,
        prev: &HashMap<u64, SystemTime>,
    ) -> Vec<(u64, SystemTime)> {
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

        let entries = collect_entries_with_prev(doc, base_dir, unloaded, prev);
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
        self.entries
            .iter()
            .filter(|e| e.status == RefStatus::Loaded)
            .filter_map(|e| e.modified.map(|m| (e.key, m)))
            .collect()
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

    /// True when the selection includes at least one nested row. Nested rows
    /// stay read-only — every mutating button disables with a reason then.
    pub fn selection_has_nested(&self) -> bool {
        self.selected.iter().any(|i| self.nested.contains(i))
    }

    /// Selected entry indices that are directly actionable (nested rows
    /// excluded — they have no host definition to mutate).
    pub fn actionable_selection(&self) -> Vec<usize> {
        let mut out: Vec<usize> = self
            .selected
            .iter()
            .copied()
            .filter(|i| *i < self.entries.len() && !self.nested.contains(i))
            .collect();
        out.sort_unstable();
        out
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
        fn append_tree(
            panel: &XrefManagerPanel,
            index: usize,
            depth: u32,
            seen: &mut HashSet<(String, String)>,
            rows: &mut Vec<DisplayRow>,
        ) {
            let Some(entry) = panel.entries.get(index) else { return; };
            if !claim_path(seen, &entry.saved_path, &entry.name) {
                return;
            }
            rows.push(DisplayRow {
                index,
                depth,
                is_nested: panel.nested.contains(&index),
            });
            if !panel.expanded.contains(&entry.key) {
                return;
            }
            if let Some(children) = panel.children.get(&entry.key) {
                for &child in children {
                    append_tree(panel, child, depth + 1, seen, rows);
                }
            }
        }

        let mut rows = Vec::new();
        let mut seen: HashSet<(String, String)> = HashSet::new();
        // Roots first, then recursively expanded descendants.
        for (index, _) in self.entries.iter().enumerate() {
            let dominated = self
                .children
                .values()
                .any(|kids| kids.contains(&index));
            if self.nested.contains(&index) && dominated {
                continue;
            }
            append_tree(self, index, 0, &mut seen, &mut rows);
        }
        rows
    }

    /// Render the palette as the full content of its modal dialog.
    ///
    /// `missing` is the active tab's open-time NotFound count — a neutral
    /// notice renders while non-zero (never an auto-open modal).
    pub fn view_window(
        &self,
        sizing: crate::ui::modal::ModalSizing,
        missing: usize,
    ) -> Element<'_, Message> {
        // ── Toolbar ───────────────────────────────────────────────────────
        let can_attach = !IS_WASM;
        let attach = if can_attach {
            toolbar_btn(crate::t!("Attach").into_owned(), Some(Message::XAttachPick))
        } else {
            toolbar_tip(
                crate::t!("Attach").into_owned(),
                crate::t!("File attach is not available on web — the reference list below is read-only.").into_owned(),
            )
        };
        let refresh = toolbar_btn(
            crate::t!("Refresh").into_owned(),
            Some(Message::XrefManagerRefresh),
        );
        let mode_label = if self.tree {
            crate::t!("List").into_owned()
        } else {
            crate::t!("Tree").into_owned()
        };
        let mode_btn = toolbar_btn(mode_label, Some(Message::XrefManagerToggleTree));
        let mode_btn: Element<'_, Message> = tooltip(
            mode_btn,
            text(crate::t!("Toggle list/tree")).size(11),
            tooltip::Position::Bottom,
        )
        .into();
        // Selection-gated ops: disabled with a reason when nothing actionable
        // is selected (empty selection, or nested rows which stay read-only).
        let gate: Option<String> = if IS_WASM {
            Some(crate::t!("Reference changes are not available on web — the reference list is read-only.").into_owned())
        } else if self.selected.is_empty() {
            Some(crate::t!("Select a reference first.").into_owned())
        } else if self.selection_has_nested() {
            Some(
                crate::t!("Nested references are read-only — edit them in their host drawing.")
                    .into_owned(),
            )
        } else {
            None
        };
        let op_btn = |label: String, op: XrefPaletteOp, gate: &Option<String>| match gate {
            Some(reason) => toolbar_tip(label, reason.clone()),
            None => toolbar_btn(label, Some(Message::XrefManagerOp(op))),
        };
        let detach = op_btn(
            crate::t!("Detach").into_owned(),
            XrefPaletteOp::Detach,
            &gate,
        );
        let unload = op_btn(
            crate::t!("Unload").into_owned(),
            XrefPaletteOp::Unload,
            &gate,
        );
        let reload = op_btn(
            crate::t!("Reload").into_owned(),
            XrefPaletteOp::Reload,
            &gate,
        );
        let bind = op_btn(
            crate::t!("Bind").into_owned(),
            XrefPaletteOp::Bind,
            &gate,
        );
        let overlay = op_btn(
            crate::t!("Overlay").into_owned(),
            XrefPaletteOp::Overlay,
            &gate,
        );
        // Type control, both directions: Attach ↔ Overlay (same engine fn the
        // CLI Overlay arm uses; images/PDFs report the drawing-only error).
        let attach_type = op_btn(
            crate::t!("Attach").into_owned(),
            XrefPaletteOp::Attach,
            &gate,
        );
        let change_path = row![
            text(crate::t!("Change Path:")).size(11),
            op_btn(
                crate::t!("Full").into_owned(),
                XrefPaletteOp::Pathtype(Pathtype::Full),
                &gate,
            ),
            op_btn(
                crate::t!("Relative").into_owned(),
                XrefPaletteOp::Pathtype(Pathtype::Relative),
                &gate,
            ),
            op_btn(
                crate::t!("File name").into_owned(),
                XrefPaletteOp::Pathtype(Pathtype::None),
                &gate,
            ),
        ]
        .spacing(2)
        .align_y(iced::Center);
        let help = toolbar_tip(
            crate::t!("Help").into_owned(),
            crate::t!("Reference Manager — select rows, then Detach, Unload, Reload, Bind, Overlay, or a Change Path mode.").into_owned(),
        );
        let toolbar = container(
            row![
                attach,
                refresh,
                mode_btn,
                detach,
                unload,
                reload,
                bind,
                overlay,
                attach_type,
                change_path,
                help
            ]
            .spacing(4)
            .align_y(iced::Center),
        )
        .style(|theme: &Theme| container::Style {
            background: Some(Background::Color(theme.palette().background.weak.color)),
            ..Default::default()
        })
        .width(sizing.width)
        .padding([4, 8]);

        // ── Find & Replace / path-edit row ────────────────────────────────
        let find_apply = if IS_WASM {
            toolbar_tip(
                crate::t!("Apply").into_owned(),
                crate::t!("Reference changes are not available on web — the reference list is read-only.").into_owned(),
            )
        } else if self.find_input.is_empty() {
            toolbar_tip(
                crate::t!("Apply").into_owned(),
                crate::t!("Type the path prefix to find first.").into_owned(),
            )
        } else {
            toolbar_btn(
                crate::t!("Apply").into_owned(),
                Some(Message::XrefManagerFindReplaceApply),
            )
        };
        // Path edit applies to the anchor entry; nested anchors stay disabled.
        let anchor_nested = self
            .anchor
            .is_some_and(|a| self.nested.contains(&a));
        let path_apply = match self.anchor.and_then(|i| self.entries.get(i)) {
            Some(_) if IS_WASM => toolbar_tip(
                crate::t!("Set Path").into_owned(),
                crate::t!("Reference changes are not available on web — the reference list is read-only.").into_owned(),
            ),
            Some(_) if anchor_nested => toolbar_tip(
                crate::t!("Set Path").into_owned(),
                crate::t!("Nested references are read-only — edit them in their host drawing.")
                    .into_owned(),
            ),
            Some(_) if !self.path_input.is_empty() => toolbar_btn(
                crate::t!("Set Path").into_owned(),
                Some(Message::XrefManagerPathApply),
            ),
            Some(_) => toolbar_tip(
                crate::t!("Set Path").into_owned(),
                crate::t!("Type a new path first.").into_owned(),
            ),
            None => toolbar_tip(
                crate::t!("Set Path").into_owned(),
                crate::t!("Select a reference first.").into_owned(),
            ),
        };
        let edit_row = container(
            row![
                text(crate::t!("Find:")).size(11),
                text_input(
                    crate::t!("old path prefix").as_ref(),
                    &self.find_input
                )
                .on_input(Message::XrefManagerFindInput)
                .size(11)
                .padding(4)
                .width(Length::Fixed(150.0)),
                text(crate::t!("Replace:")).size(11),
                text_input(
                    crate::t!("new path prefix").as_ref(),
                    &self.replace_input
                )
                .on_input(Message::XrefManagerReplaceInput)
                .size(11)
                .padding(4)
                .width(Length::Fixed(150.0)),
                find_apply,
                text(crate::t!("Path:")).size(11),
                text_input(crate::t!("new path for selection").as_ref(), &self.path_input)
                    .on_input(Message::XrefManagerPathInput)
                    .size(11)
                    .padding(4)
                    .width(Length::Fill),
                path_apply,
            ]
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
                text(crate::t!("Reference")).size(10).style(muted_style).width(Length::FillPortion(3)),
                text(crate::t!("Status")).size(10).style(muted_style).width(Length::Fixed(88.0)),
                text(crate::t!("Size")).size(10).style(muted_style).width(Length::Fixed(76.0)),
                text(crate::t!("Type")).size(10).style(muted_style).width(Length::Fixed(76.0)),
                text(crate::t!("Date")).size(10).style(muted_style).width(Length::Fixed(96.0)),
                text(crate::t!("Saved Path")).size(10).style(muted_style).width(Length::FillPortion(4)),
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
                text(crate::t!("XREF  No external references in this drawing."))
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

        // ── Missing-on-open notice + details pane ─────────────────────────
        let notice: Option<Element<'_, Message>> = if missing > 0 {
            Some(
                container(text(crate::tf!(
                    "{} reference(s) not found — open the reference manager with XREFMAN.",
                    missing
                )))
                .padding([6, 8])
                .width(Fill)
                .into(),
            )
        } else {
            None
        };
        let details = details_pane(self.anchor.and_then(|i| self.entries.get(i)));

        let mut content = column![toolbar, edit_row, col_header, body];
        if let Some(notice) = notice {
            content = content.push(notice);
        }
        content = content.push(details);
        container(content.spacing(0))
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

    let mut ids: HashSet<(u64, String)> = HashSet::new();
    for br in doc.block_records.iter() {
        if br.flags.is_xref || br.flags.is_xref_overlay {
            ids.insert((br.handle.value(), br.xref_path.clone()));
        }
    }
    for (handle, obj) in doc.objects.iter() {
        match obj {
            ObjectType::ImageDefinition(def) => {
                ids.insert((handle.value(), def.file_name.clone()));
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

/// Claim a `(normalized saved path, entry name)` pair for display; empty
/// paths always pass, exact repeats are skipped (cycle guard). Two entries
/// sharing one file under different names both render.
fn claim_path(seen: &mut HashSet<(String, String)>, saved_path: &str, name: &str) -> bool {
    if saved_path.is_empty() {
        return true;
    }
    seen.insert((normalize_lexical(saved_path), name.to_string()))
}

// ── Text helpers ──────────────────────────────────────────────────────────

fn status_text(status: RefStatus) -> std::borrow::Cow<'static, str> {
    match status {
        RefStatus::Loaded => crate::t!("Loaded"),
        RefStatus::Unloaded => crate::t!("Unloaded"),
        RefStatus::NotFound => crate::t!("Not found"),
        RefStatus::Failed => crate::t!("Failed"),
        RefStatus::Stale => crate::t!("Stale"),
        RefStatus::Orphaned => crate::t!("Orphaned"),
        RefStatus::Unreferenced => crate::t!("Unreferenced"),
    }
}

fn type_text(entry: &ReferenceEntry) -> std::borrow::Cow<'static, str> {
    match entry.kind {
        RefKind::DwgXref => match entry.ref_type {
            RefType::Attach => crate::t!("Attach"),
            RefType::Overlay => crate::t!("Overlay"),
        },
        RefKind::Image => crate::t!("Image"),
        RefKind::Pdf => crate::t!("PDF"),
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

fn toolbar_btn(label: String, msg: Option<Message>) -> Element<'static, Message> {
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

/// Disabled toolbar affordance with an explanatory tooltip (gated
/// operations, nested-selection blocks, or web-gated mutations).
fn toolbar_tip(label: String, tip: String) -> Element<'static, Message> {
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
        name.push_str(&crate::t!(" (nested — not rendered)"));
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
        None => text(crate::t!("Select a reference to inspect its details"))
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
                detail_row(crate::t!("Reference"), e.name.as_str()),
                detail_row(crate::t!("Status"), status_text(e.status)),
                detail_row(crate::t!("Size"), size),
                detail_row(crate::t!("Type"), type_text(e)),
                detail_row(crate::t!("Date"), date),
                detail_row(crate::t!("Found At"), found),
                detail_row(crate::t!("Saved Path"), saved),
            ]
            .spacing(1)
            .into()
        }
    };
    container(
        column![
            text(crate::t!("Details")).size(10).style(muted_style),
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

fn detail_row<'a>(
    label: impl Into<std::borrow::Cow<'a, str>>,
    value: impl Into<std::borrow::Cow<'a, str>>,
) -> Element<'a, Message> {
    let label: std::borrow::Cow<'a, str> = label.into();
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
        // Same file under two names renders twice; only exact
        // (path, name) repeats collapse.
        assert_eq!(panel.display_rows().len(), 3);
        panel.entries.push(entry(4, "B", "b.dwg"));
        assert_eq!(panel.display_rows().len(), 3);
        // Mark B2 nested under A: A claims (a.dwg, A), B2-as-child claims
        // (b.dwg, B2); the B root still renders as (b.dwg, B).
        panel.nested.insert(2);
        panel.children.insert(1, vec![2]);
        panel.expanded.insert(1);
        let rows = panel.display_rows();
        assert_eq!(rows.len(), 3); // A, B2-as-child-of-A, B
        assert!(rows.iter().any(|r| r.index == 2 && r.is_nested));
        // A second parent claiming the same child shows nothing twice.
        panel.entries.push(entry(5, "C", "c.dwg"));
        panel.children.insert(5, vec![2]);
        panel.expanded.insert(5);
        let count = panel
            .display_rows()
            .iter()
            .filter(|r| r.index == 2)
            .count();
        assert_eq!(count, 1);
    }

    #[test]
    fn actionable_selection_skips_nested() {
        let mut panel = XrefManagerPanel::default();
        panel.entries = vec![entry(1, "A", "a.dwg"), entry(2, "B", "b.dwg")];
        panel.nested.insert(1);
        panel.toggle_select(0);
        panel.toggle_select(1);
        assert!(panel.selection_has_nested());
        // Only the direct row is actionable; the nested row stays read-only.
        assert_eq!(panel.actionable_selection(), vec![0]);
        panel.toggle_select(1);
        assert!(!panel.selection_has_nested());
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
