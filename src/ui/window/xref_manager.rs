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
/// Minimum table content width: the six columns stay readable and the list
/// sidescrolls in narrower docks instead of squeezing.
const TABLE_MIN_W: f32 = 480.0;
/// Indent per tree depth level.
const INDENT_W: f32 = 16.0;
/// Longest edge of a preview image, in pixels.
const PREVIEW_MAX: u32 = 256;

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

/// Display-row index reserved for the host-drawing pseudo-row (list first
/// row, tree root). It addresses no entry: row rendering special-cases it
/// and selection ignores it.
pub const HOST_ROW: usize = usize::MAX;

/// Palette state for the Reference Manager.
///
/// Session state (unloaded set, stat cache) lives per-tab on `DocumentTab`
/// and is passed into [`refresh`](XrefManagerPanel::refresh); this panel
/// only caches display rows, expansion, selection, and text inputs.
#[derive(Default)]
pub struct XrefManagerPanel {
    /// Cached [`collect_entries_with_prev`] output for the active drawing.
    pub entries: Vec<ReferenceEntry>,
    /// Display name of the host drawing (first list row / tree root).
    pub host_name: String,
    /// Absolute path of the host drawing, if saved (host row Saved Path).
    pub host_path: String,
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
    /// Open dropdown menus (one at a time; dismissed together).
    pub attach_open: bool,
    pub refresh_open: bool,
    pub path_open: bool,
    /// Decoded preview images keyed by `(entry key, resolved path)` — rebuilt
    /// on every refresh for the anchor entry only, so the per-frame `view`
    /// stays pure and file I/O never happens during rendering.
    pub previews: HashMap<(u64, String), iced::widget::image::Handle>,
    /// Details (`false`) vs Preview (`true`) lower pane.
    pub show_preview: bool,
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
        host_name: &str,
        host_path: &str,
    ) -> Vec<(u64, SystemTime)> {
        self.host_name = host_name.to_string();
        self.host_path = host_path.to_string();
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
        // Previews decode here (not in `view`, which must stay pure): the
        // anchor entry only, and only under single selection — the spec shows
        // a preview solely for one selected reference.
        self.previews.clear();
        if self.selected.len() <= 1 {
            if let Some(a) = self.anchor.and_then(|i| self.entries.get(i)) {
                if let (Some(found), Some(img)) =
                    (a.found_at.clone(), reference_preview(a))
                {
                    let (w, h) = (img.width(), img.height());
                    self.previews.insert(
                        (a.key, found),
                        iced::widget::image::Handle::from_rgba(w, h, img.into_raw()),
                    );
                }
            }
        }
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
        if index == HOST_ROW || index >= self.entries.len() {
            return;
        }
        if self.tree {
            // Tree view selects a single file reference at a time.
            if self.selected.contains(&index) {
                return;
            }
            self.selected.clear();
            self.selected.insert(index);
            self.anchor = Some(index);
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
        // The host drawing leads in both modes (list first row, tree root).
        let host = DisplayRow {
            index: HOST_ROW,
            depth: 0,
            is_nested: false,
        };
        if !self.tree {
            let mut rows = vec![host];
            rows.extend(self.entries.iter().enumerate().map(|(index, _)| {
                DisplayRow {
                    index,
                    depth: 0,
                    is_nested: self.nested.contains(&index),
                }
            }));
            return rows;
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
        // The host drawing leads: first list row, tree root. In tree mode
        // everything below renders one level deeper as its descendants.
        if self.tree {
            for row in rows.iter_mut() {
                row.depth += 1;
            }
        }
        rows.insert(
            0,
            DisplayRow {
                index: HOST_ROW,
                depth: 0,
                is_nested: false,
            },
        );
        rows
    }

    /// Render the palette as a docked side panel (EXTERNALREFERENCES).
    ///
    /// `missing` is the active tab's open-time NotFound count — a neutral
    /// notice renders while non-zero. Mirrors the block palette's dock
    /// chrome (title, pin, close) and sizing conventions.
    pub fn view<'a>(
        &'a self,
        width: f32,
        auto_collapse: bool,
        missing: usize,
        doc: &'a CadDocument,
    ) -> Element<'a, Message> {
        use crate::ui::dock::{DockMsg, PanelId};
        // ── Dock chrome (title, pin, close) — matches the block palette ──
        let pin_icon = if auto_collapse {
            crate::ui::icons::themed_primary_weak_text(crate::ui::icons::PIN, 12.0)
        } else {
            crate::ui::icons::themed_secondary(crate::ui::icons::PIN, 12.0)
        };
        let pin = button(pin_icon)
            .on_press(Message::Dock(DockMsg::AutoCollapseToggle(
                PanelId::ExternalReferences,
            )))
            .style(move |theme: &Theme, status| {
                let mut style = button::subtle(theme, status);
                if auto_collapse {
                    let palette = theme.palette();
                    style.background = Some(Background::Color(palette.primary.weak.color));
                    style.text_color = palette.primary.weak.text;
                    style.border.color = palette.primary.base.color;
                    style.border.width = 1.0;
                }
                style
            })
            .padding([3, 5]);
        let pin = tooltip(pin, text("Auto").size(10), tooltip::Position::Bottom).gap(4);
        let close = button(crate::ui::icons::themed_secondary(crate::ui::icons::CLOSE, 12.0))
            .on_press(Message::Dock(DockMsg::Close(PanelId::ExternalReferences)))
            .style(button::subtle)
            .padding([3, 5]);
        let close = tooltip(close, text("Close").size(10), tooltip::Position::Bottom).gap(4);
        let title_bar = mouse_area(
            container(
                row![
                    text(crate::t!("External References")).size(12),
                    iced::widget::Space::new().width(Fill),
                    pin,
                    close,
                ]
                .spacing(3)
                .align_y(iced::Center),
            )
            .style(|theme: &Theme| container::Style {
                background: Some(Background::Color(theme.palette().background.weak.color)),
                ..Default::default()
            })
            .width(Fill)
            .padding([3, 6]),
        )
        .on_press(Message::Dock(DockMsg::DockGrab(PanelId::ExternalReferences)))
        .interaction(iced::mouse::Interaction::Grab);
        // Table content follows the dock width (minus chrome padding) with a
        // floor so columns stay readable: widening the dock widens the table,
        // narrowing it sidescrolls instead of squeezing.
        let table_w = (width - 16.0).max(TABLE_MIN_W);
        // ── Toolbar: split-button groups per spec ───────────────────────
        // Attach ▾ (default DWG), Refresh ▾ (default Refresh), Change Path ▾.
        // The main button runs the default; the triangle opens the rest in an
        // overlay menu. Formats without an attach command in this build
        // (DWF/DGN/point clouds/coordination models) are omitted, not dead.
        let web_tip = crate::t!("File attach is not available on web — the reference list below is read-only.").into_owned();
        let attach = if IS_WASM {
            toolbar_tip(crate::t!("Attach").into_owned(), web_tip.clone())
        } else {
            split_button(
                toolbar_btn(crate::t!("Attach").into_owned(), Some(Message::XAttachPick)),
                self.attach_open,
                Message::XrefManagerAttachMenu,
                vec![
                    menu_item(
                        crate::t!("Image").into_owned(),
                        Some(Message::ImagePick),
                        None,
                    ),
                    menu_item(
                        crate::t!("PDF").into_owned(),
                        Some(Message::PdfAttachPick),
                        None,
                    ),
                ],
            )
        };
        let refresh = if IS_WASM {
            toolbar_tip(crate::t!("Refresh").into_owned(), web_tip.clone())
        } else {
            split_button(
                toolbar_btn(
                    crate::t!("Refresh").into_owned(),
                    Some(Message::XrefManagerRefresh),
                ),
                self.refresh_open,
                Message::XrefManagerRefreshMenu,
                vec![menu_item(
                    crate::t!("Reload All References").into_owned(),
                    Some(Message::XrefManagerReloadAll),
                    None,
                )],
            )
        };
        // List / Tree are two buttons; the active mode renders inert.
        let list_btn = if self.tree {
            toolbar_btn(crate::t!("List").into_owned(), Some(Message::XrefManagerToggleTree))
        } else {
            toolbar_btn(crate::t!("List").into_owned(), None)
        };
        let tree_btn = if self.tree {
            toolbar_btn(crate::t!("Tree").into_owned(), None)
        } else {
            toolbar_btn(crate::t!("Tree").into_owned(), Some(Message::XrefManagerToggleTree))
        };
        let mode_row: Element<'static, Message> =
            row![list_btn, tree_btn].spacing(2).into();
        let mode_btn: Element<'_, Message> = tooltip(
            mode_row,
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
        // Change Path group: greyed until a reference whose path can change
        // (a direct, non-nested row) is selected. Each option carries its own
        // gate so non-executable choices render greyed with the reason.
        let has_direct = self
            .entries
            .iter()
            .enumerate()
            .any(|(i, _)| self.selected.contains(&i) && !self.nested.contains(&i));
        let single_direct_anchor = self.anchor.is_some_and(|a| {
            self.selected.len() == 1
                && self.entries.get(a).is_some()
                && !self.nested.contains(&a)
        });
        let host_saved = !self.host_path.is_empty();
        let find_ready = !self.find_input.is_empty() && !self.replace_input.is_empty();
        let web_readonly: Option<String> = IS_WASM.then(|| {
            crate::t!("Reference changes are not available on web — the reference list is read-only.").into_owned()
        });
        let path_group_gate: Option<String> = web_readonly.clone().or_else(|| {
            if has_direct {
                None
            } else if self.selected.is_empty() {
                Some(crate::t!("Select a reference first.").into_owned())
            } else {
                Some(
                    crate::t!("Nested references are read-only — edit them in their host drawing.")
                        .into_owned(),
                )
            }
        });
        let change_path = match path_group_gate {
            Some(reason) => toolbar_tip(crate::t!("Change Path").into_owned(), reason),
            None => {
                let relative_gate = if host_saved {
                    None
                } else {
                    Some(
                        crate::t!("XREF  Save the drawing first to resolve relative XREF paths.")
                            .into_owned(),
                    )
                };
                let new_path_gate = if single_direct_anchor {
                    None
                } else {
                    Some(crate::t!("Select a single reference first.").into_owned())
                };
                let find_gate = if find_ready {
                    None
                } else {
                    Some(crate::t!("Type the path prefix to find first.").into_owned())
                };
                split_button(
                    toolbar_btn(
                        crate::t!("Change Path").into_owned(),
                        Some(Message::XrefManagerPathMenu),
                    ),
                    self.path_open,
                    Message::XrefManagerPathMenu,
                    vec![
                        menu_item(
                            crate::t!("Make Absolute").into_owned(),
                            Some(Message::XrefManagerOp(XrefPaletteOp::Pathtype(
                                Pathtype::Full,
                            ))),
                            None,
                        ),
                        menu_item(
                            crate::t!("Make Relative").into_owned(),
                            Some(Message::XrefManagerOp(XrefPaletteOp::Pathtype(
                                Pathtype::Relative,
                            ))),
                            relative_gate,
                        ),
                        menu_item(
                            crate::t!("Remove Path").into_owned(),
                            Some(Message::XrefManagerOp(XrefPaletteOp::Pathtype(
                                Pathtype::None,
                            ))),
                            None,
                        ),
                        menu_item(
                            crate::t!("Select New Path").into_owned(),
                            Some(Message::XrefPathPick),
                            new_path_gate,
                        ),
                        menu_item(
                            crate::t!("Find and Replace").into_owned(),
                            Some(Message::XrefManagerFindReplaceApply),
                            find_gate,
                        ),
                    ],
                )
            }
        };
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
        .width(Fill)
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
        .width(Fill)
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
            .width(Length::Fixed(table_w))
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
        .width(Fill);

        // ── Reference rows ────────────────────────────────────────────────
        let mut rows_col = column![].spacing(0);
        for display in self.display_rows() {
            if display.index == HOST_ROW {
                rows_col = rows_col.push(host_row(
                    display,
                    &self.host_name,
                    &self.host_path,
                    table_w,
                ));
                continue;
            }
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
                table_w,
            ));
        }
        // ── Reference table: header + rows scroll together, horizontally
        // (fixed content width overflows narrow docks) and vertically.
        let table_rows: Element<'_, Message> = if self.entries.is_empty() {
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
            scrollable(column![col_header, rows_col])
                .direction(iced::widget::scrollable::Direction::Both {
                    vertical: iced::widget::scrollable::Scrollbar::new(),
                    horizontal: iced::widget::scrollable::Scrollbar::new(),
                })
                .height(Length::Fixed(TABLE_H))
                .into()
        };

        // ── Missing-on-open notice + details pane ─────────────────────────
        let notice: Option<Element<'_, Message>> = if missing > 0 {
            Some(
                container(text(crate::tf!(
                    "{} reference(s) not found — open the reference manager with EXTERNALREFERENCES.",
                    missing
                )))
                .padding([6, 8])
                .width(Fill)
                .into(),
            )
        } else {
            None
        };
        // ── Details / Preview lower pane ────────────────────────────────
        // Toggle buttons mirror the spec's Details/Preview switch. Preview is
        // a placeholder in v1 (no thumbnail pipeline): a grey field, or
        // "Preview not available" — never a broken image control.
        let details_tab = toolbar_btn(
            crate::t!("Details").into_owned(),
            if self.show_preview {
                Some(Message::XrefManagerTogglePreview)
            } else {
                None
            },
        );
        let preview_tab = toolbar_btn(
            crate::t!("Preview").into_owned(),
            if self.show_preview {
                None
            } else {
                Some(Message::XrefManagerTogglePreview)
            },
        );
        let pane_tabs = row![details_tab, preview_tab]
            .spacing(4)
            .align_y(iced::Center);
        let lower: Element<'_, Message> = if self.show_preview {
            // Single selection → the decoded image, or "Preview not
            // available" (DXF/PDF/unresolvable). Anything else → the spec's
            // solid grey field.
            let single = self.anchor.and_then(|i| {
                (self.selected.len() <= 1)
                    .then(|| self.entries.get(i))
                    .flatten()
            });
            let preview_body: Element<'_, Message> = match single {
                Some(e) => {
                    let cached = e.found_at.as_deref().and_then(|found| {
                        self.previews.get(&(e.key, found.to_string()))
                    });
                    match cached {
                        Some(handle) => container(
                            iced::widget::image(handle.clone())
                                .width(Length::Fixed(220.0)),
                        )
                        .center_x(Fill)
                        .center_y(Fill)
                        .width(Fill)
                        .height(Length::Fixed(120.0))
                        .into(),
                        None => container(
                            column![
                                text(e.name.as_str()).size(11),
                                text(crate::t!("Preview not available")).size(11).style(muted_style),
                            ]
                            .spacing(4)
                            .align_x(iced::Center),
                        )
                        .center_x(Fill)
                        .center_y(Fill)
                        .width(Fill)
                        .height(Length::Fixed(120.0))
                        .into(),
                    }
                }
                None => container(text("—").size(11).style(muted_style))
                    .center_x(Fill)
                    .center_y(Fill)
                    .width(Fill)
                    .height(Length::Fixed(120.0))
                    .style(|theme: &Theme| container::Style {
                        background: Some(Background::Color(
                            theme.palette().background.weak.color,
                        )),
                        ..Default::default()
                    })
                    .into(),
            };
            container(column![pane_tabs, preview_body].spacing(2))
                .padding([6, 8])
                .width(Fill)
                .into()
        } else {
            let details =
                details_pane(self.anchor.and_then(|i| self.entries.get(i)), doc);
            container(column![pane_tabs, details].spacing(2))
                .padding([6, 8])
                .width(Fill)
                .into()
        };

        let mut content = column![title_bar, toolbar, edit_row, table_rows];
        if let Some(notice) = notice {
            content = content.push(notice);
        }
        content = content.push(lower);
        container(content.spacing(0))
            .style(|theme: &Theme| container::Style {
                background: Some(Background::Color(
                    theme.palette().background.base.color,
                )),
                ..Default::default()
            })
            .width(Fill)
            .height(Fill)
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
        // Unreadable file: the industry palette term is Unresolved.
        RefStatus::Failed => crate::t!("Unresolved"),
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
        // Raster images display their file format (spec: type column shows
        // the image format); unknown extensions fall back to Image.
        RefKind::Image => image_format(&entry.saved_path),
        RefKind::Pdf => crate::t!("PDF"),
    }
}

/// Uppercase file extension of an image path (`plan.BMP` → `BMP`).
/// Extensionless or overlong suffixes fall back to `Image`.
fn image_format(saved_path: &str) -> std::borrow::Cow<'static, str> {
    let file = saved_path.rsplit(['/', '\\']).next().unwrap_or("");
    let ext = file
        .rsplit('.')
        .next()
        .filter(|_| file.contains('.'))
        .unwrap_or("");
    if ext.is_empty() || ext.len() > 5 {
        crate::t!("Image")
    } else {
        std::borrow::Cow::Owned(ext.to_ascii_uppercase())
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

/// Split button: a default action plus a triangle that opens the rest in an
/// overlay menu (spec toolbar: Attach ▾, Refresh ▾, Change Path ▾).
///
/// `main` is the already-gated default-action element; `toggle` flips this
/// menu (closing the others is handled at the message site); `items` are the
/// menu rows, each full-width. Dismissal (Escape / outside click) funnels to
/// [`Message::XrefManagerDismissMenus`].
fn split_button(
    main: Element<'static, Message>,
    menu_open: bool,
    toggle: Message,
    items: Vec<Element<'static, Message>>,
) -> Element<'static, Message> {
    let caret = button(
        container(crate::ui::icons::themed_arrow_down(9.0)).align_y(iced::Center),
    )
    .on_press(toggle)
    .style(button::subtle)
    .padding([4, 6]);
    let head = row![main, caret].spacing(0).align_y(iced::Center);
    let popup: Element<'static, Message> = container(
        column(items.into_iter().map(|item| {
            container(item).width(Fill).into()
        }))
        .spacing(2)
        .padding(4),
    )
    .style(|theme: &Theme| container::Style {
        background: Some(Background::Color(
            theme.palette().background.base.color,
        )),
        border: Border {
            color: theme.palette().background.neutral.color,
            width: 1.0,
            radius: 3.0.into(),
        },
        ..Default::default()
    })
    .width(Length::Fixed(220.0))
    .into();
    iced_aw::DropDown::new(head, popup, menu_open)
        .alignment(iced_aw::drop_down::Alignment::Bottom)
        .offset(2.0)
        .on_dismiss(Message::XrefManagerDismissMenus)
        .into()
}

/// One dropdown-menu row: a full-width button when the option can execute,
/// otherwise greyed text carrying the reason.
fn menu_item(label: String, msg: Option<Message>, gate: Option<String>) -> Element<'static, Message> {
    match (msg, gate) {
        (Some(m), None) => toolbar_btn(label, Some(m)),
        (_, reason) => toolbar_tip(label, reason.unwrap_or_default()),
    }
}

fn xref_row<'a>(
    display: DisplayRow,
    entry: &'a ReferenceEntry,
    is_selected: bool,
    show_expand: bool,
    is_expanded: bool,
    table_w: f32,
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
    .width(Length::Fixed(table_w))
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

/// The host-drawing pseudo-row: always first, never selectable, never
/// actionable. Mirrors the industry layout (current drawing leads the list
/// and roots the tree) with an `Opened` / `Current` status/type pair.
fn host_row<'a>(display: DisplayRow, host_name: &'a str, host_path: &'a str, table_w: f32) -> Element<'a, Message> {
    let name = if host_name.is_empty() {
        crate::t!("Untitled").into_owned()
    } else {
        host_name.to_string()
    };
    let saved = if host_path.is_empty() { "—" } else { host_path };
    let mut name_cell = row![].spacing(2).align_y(iced::Center);
    if display.depth > 0 {
        name_cell = name_cell.push(
            iced::widget::Space::new().width(Length::Fixed(INDENT_W * display.depth as f32)),
        );
    }
    name_cell = name_cell.push({
        let name_text: Element<'_, Message> = text(name).size(FONT_SZ).into();
        name_text
    });
    let content = row![
        name_cell.width(Length::FillPortion(3)),
        text(crate::t!("Opened")).size(FONT_SZ).width(Length::Fixed(88.0)),
        text("—").size(FONT_SZ).width(Length::Fixed(76.0)),
        text(crate::t!("Current")).size(FONT_SZ).width(Length::Fixed(76.0)),
        text("—").size(FONT_SZ).width(Length::Fixed(96.0)),
        text(saved).size(FONT_SZ).width(Length::FillPortion(4)),
    ]
    .spacing(4)
    .width(Length::Fixed(table_w))
    .align_y(iced::Center);
    container(content)
        .style(|theme: &Theme| {
            let pair = theme.palette().background.base;
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
        .width(Fill)
        .into()
}

fn details_pane<'a>(entry: Option<&'a ReferenceEntry>, doc: &'a CadDocument) -> Element<'a, Message> {
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
            let mut rows = column![
                detail_row(crate::t!("Reference"), e.name.as_str()),
                detail_row(crate::t!("Status"), status_text(e.status)),
                detail_row(crate::t!("Size"), size),
                detail_row(crate::t!("Type"), type_text(e)),
                detail_row(crate::t!("Date"), date),
                detail_row(crate::t!("Found At"), found),
                detail_row(crate::t!("Saved Path"), saved),
            ]
            .spacing(1);
            // Image-specific properties (read-only). The file format exposes
            // pixel dimensions and resolution units; color system / color
            // depth are not stored by the format and are omitted deliberately.
            if e.kind == RefKind::Image {
                if let Some(def) = find_image_def(doc, e.key) {
                    rows = rows
                        .push(detail_row(
                            crate::t!("Pixel Width"),
                            format!("{}", def.size_in_pixels.0),
                        ))
                        .push(detail_row(
                            crate::t!("Pixel Height"),
                            format!("{}", def.size_in_pixels.1),
                        ))
                        .push(detail_row(
                            crate::t!("Resolution Unit"),
                            format!("{:?}", def.resolution_unit),
                        ));
                }
            }
            rows.into()
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

/// Decode one entry's preview image (`None` = placeholder territory).
///
/// Sources mirror what the formats actually carry: DWG files embed a preview
/// (read header-only via `dwg_thumbnailer`, no full parse — DXF has none, so
/// it resolves to no preview); raster images decode through the `image`
/// crate and downscale; PDFs have no rasterizer in this build. Only
/// Loaded/Stale rows resolve — Unloaded/NotFound show the placeholder by
/// design. Web builds skip filesystem decoding entirely.
fn reference_preview(entry: &ReferenceEntry) -> Option<image::RgbaImage> {
    if IS_WASM {
        return None;
    }
    if !matches!(entry.status, RefStatus::Loaded | RefStatus::Stale) {
        return None;
    }
    let found = entry.found_at.as_deref().filter(|s| !s.is_empty())?;
    match entry.kind {
        RefKind::DwgXref => {
            let ext = found
                .rsplit(['/', '\\'])
                .next()
                .and_then(|f| f.rsplit('.').next())
                .unwrap_or("")
                .to_ascii_lowercase();
            if ext != "dwg" {
                return None;
            }
            dwg_thumbnailer::extract(std::path::Path::new(found), PREVIEW_MAX)
        }
        RefKind::Image => {
            let img = image::open(found).ok()?;
            Some(img.thumbnail(PREVIEW_MAX, PREVIEW_MAX).to_rgba8())
        }
        RefKind::Pdf => None,
    }
}

/// Image definition backing a [`RefKind::Image`] entry, looked up by the
/// entry key (definition-object handle) for the Details pane extras.
fn find_image_def(
    doc: &CadDocument,
    key: u64,
) -> Option<&acadrust::objects::ImageDefinition> {
    doc.objects.get(&acadrust::types::Handle::from(key)).and_then(|o| match o {
        acadrust::objects::ObjectType::ImageDefinition(def) => Some(def),
        _ => None,
    })
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
        // Host row leads; same file under two names renders twice; only
        // exact (path, name) repeats collapse.
        let rows = panel.display_rows();
        assert_eq!(rows.len(), 4);
        assert_eq!(rows[0].index, HOST_ROW);
        assert!(rows[1..].iter().all(|r| r.depth >= 1));
        panel.entries.push(entry(4, "B", "b.dwg"));
        assert_eq!(panel.display_rows().len(), 4);
        // Mark B2 nested under A: A claims (a.dwg, A), B2-as-child claims
        // (b.dwg, B2); the B root still renders as (b.dwg, B).
        panel.nested.insert(2);
        panel.children.insert(1, vec![2]);
        panel.expanded.insert(1);
        let rows = panel.display_rows();
        assert_eq!(rows.len(), 4); // host, A, B2-as-child-of-A, B
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
    fn host_row_leads_and_ignores_selection() {
        let mut panel = XrefManagerPanel::default();
        panel.entries = vec![entry(1, "A", "a.dwg")];
        panel.host_name = "host".to_string();
        for tree in [false, true] {
            panel.tree = tree;
            let rows = panel.display_rows();
            assert_eq!(rows[0].index, HOST_ROW);
            assert_eq!(rows[0].depth, 0);
        }
        panel.toggle_select(HOST_ROW); // no-op, never selected
        assert!(panel.selected.is_empty());
        assert_eq!(panel.anchor, None);
    }

    #[test]
    fn tree_selects_single_reference() {
        let mut panel = XrefManagerPanel::default();
        panel.entries = vec![entry(1, "A", "a.dwg"), entry(2, "B", "b.dwg")];
        panel.tree = true;
        panel.toggle_select(0);
        panel.toggle_select(1);
        assert_eq!(panel.selected, HashSet::from([1]));
        assert_eq!(panel.anchor, Some(1));
        panel.tree = false;
        panel.toggle_select(0);
        assert_eq!(panel.selected, HashSet::from([0, 1]));
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
    fn image_format_reads_extension() {
        assert_eq!(image_format("C:/exa/plan.BMP"), "BMP");
        assert_eq!(image_format("a/b/c.png"), "PNG");
        assert_eq!(image_format("noext"), "Image");
        assert_eq!(image_format("toolong.abcdef"), "Image");
        assert_eq!(image_format(""), "Image");
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

    fn preview_doc2(dir: &std::path::Path) -> acadrust::CadDocument {
        // Two referenced images → genuine multi-select.
        use acadrust::objects::{ImageDefinition, ObjectType};
        let mut doc = acadrust::CadDocument::new();
        for (i, name) in ["a.png", "b.png"].iter().enumerate() {
            let img = image::RgbaImage::from_pixel(8, 8, image::Rgba([9, 9, 9, 255]));
            img.save(dir.join(name)).unwrap();
            let path = dir.join(name).to_string_lossy().into_owned();
            let h = doc.allocate_handle();
            let mut def = ImageDefinition::with_dimensions(path.clone(), 8, 8);
            def.handle = h;
            doc.objects.insert(h, ObjectType::ImageDefinition(def));
            let mut ent = acadrust::entities::RasterImage::new(
                &path,
                acadrust::types::Vector3::ZERO,
                8.0 + i as f64,
                8.0,
            );
            ent.definition_handle = Some(h);
            doc.add_entity(acadrust::EntityType::RasterImage(ent)).unwrap();
        }
        doc
    }

    fn preview_doc(dir: &std::path::Path, name: &str) -> acadrust::CadDocument {
        // Referenced 8x8 PNG under `dir`; the entry resolves Loaded.
        use acadrust::objects::{ImageDefinition, ObjectType};
        let img = image::RgbaImage::from_pixel(8, 8, image::Rgba([9, 9, 9, 255]));
        img.save(dir.join(name)).unwrap();
        let img_path = dir.join(name).to_string_lossy().into_owned();
        let mut doc = acadrust::CadDocument::new();
        let h = doc.allocate_handle();
        let mut def = ImageDefinition::with_dimensions(img_path.clone(), 8, 8);
        def.handle = h;
        doc.objects.insert(h, ObjectType::ImageDefinition(def));
        let mut ent = acadrust::entities::RasterImage::new(
            &img_path,
            acadrust::types::Vector3::ZERO,
            8.0,
            8.0,
        );
        ent.definition_handle = Some(h);
        doc.add_entity(acadrust::EntityType::RasterImage(ent)).unwrap();
        doc
    }

    #[test]
    fn preview_decodes_for_single_selection() {
        let dir = std::env::temp_dir().join(format!(
            "ocs_xref_preview_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let doc = preview_doc(&dir, "img.png");
        let mut panel = XrefManagerPanel::default();
        let empty = std::collections::HashSet::new();
        let no_prev = std::collections::HashMap::new();
        panel.refresh(&doc, &dir, &empty, &no_prev, "host", "");
        assert!(panel.previews.is_empty(), "nothing selected → no decode");
        panel.toggle_select(0);
        panel.refresh(&doc, &dir, &empty, &no_prev, "host", "");
        assert_eq!(panel.previews.len(), 1, "single anchor decodes once");
        let (handle, _) = panel.previews.iter().next().unwrap();
        assert!(panel.entries.iter().any(|e| e.key == handle.0));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn preview_skipped_for_multi_selection() {
        // Spec: the preview pane shows an image for a single selected
        // reference only — multi-select decodes nothing (grey field).
        let dir = std::env::temp_dir().join(format!(
            "ocs_xref_preview_multi_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let doc = preview_doc2(&dir);
        let mut panel = XrefManagerPanel::default();
        let empty = std::collections::HashSet::new();
        let no_prev = std::collections::HashMap::new();
        panel.refresh(&doc, &dir, &empty, &no_prev, "host", "");
        // One image row exists; duplicate it as a second row to multi-select.
        panel.entries.push(panel.entries[0].clone());
        panel.toggle_select(0);
        panel.toggle_select(1);
        assert_eq!(panel.selected.len(), 2);
        panel.refresh(&doc, &dir, &empty, &no_prev, "host", "");
        assert!(
            panel.previews.is_empty(),
            "multi-select → grey field, no decode"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn preview_missing_without_embedded_data() {
        // A DWG reference whose file has no embedded preview resolves to no
        // preview (placeholder path), never an error.
        let dir = std::env::temp_dir().join(format!(
            "ocs_xref_preview_dwg_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("plan.dwg"), b"not-a-real-dwg").unwrap();
        let mut doc = acadrust::CadDocument::new();
        let mut br = acadrust::tables::BlockRecord::new("PLAN");
        br.flags.is_xref = true;
        br.xref_path = dir.join("plan.dwg").to_string_lossy().into_owned();
        br.handle = doc.allocate_handle();
        doc.block_records.add(br).unwrap();
        let mut panel = XrefManagerPanel::default();
        let empty = std::collections::HashSet::new();
        let no_prev = std::collections::HashMap::new();
        panel.refresh(&doc, &dir, &empty, &no_prev, "host", "");
        panel.toggle_select(0);
        panel.refresh(&doc, &dir, &empty, &no_prev, "host", "");
        assert!(
            panel.previews.is_empty(),
            "undecodable file → placeholder, no cache entry"
        );
        std::fs::remove_dir_all(&dir).ok();
    }
}
