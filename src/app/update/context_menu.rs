//! Viewport right-click context menu: gathering the menu's inputs from app
//! state, running a picked row, and keyboard navigation while it is open.
//!
//! The rows themselves are built by `ui::popup::context_menu` (a pure model);
//! this module is the glue between that model and the app's messages.

use crate::app::{ArrowKey, ContextMenuNav, Message, OpenCADStudio};
use crate::ui::popup::context_menu::{
    build_context_menu, ContextMenu, GripMenuContext, MenuAction, MenuContext, SubmenuId,
};
use iced::Task;

impl OpenCADStudio {
    /// True while the viewport context menu is open on the active tab.
    pub(in crate::app) fn context_menu_open(&self) -> bool {
        self.tabs
            .get(self.active_tab)
            .is_some_and(|tab| tab.scene.selection.borrow().context_menu.is_some())
    }

    /// Everything the menu builder needs, read from the active tab. Called by
    /// the view (to render) and by the keyboard handler (to resolve picks), so
    /// both always see the same rows.
    pub(in crate::app) fn context_menu_context(&self) -> MenuContext {
        let tab = &self.tabs[self.active_tab];
        if let Some(grip) = tab.active_grip.as_ref() {
            return MenuContext::Grip(GripMenuContext {
                mode: grip.mode,
                copy_on: tab.grip_copy,
                moved: grip.last_world != grip.origin_world,
            });
        }
        if let Some(cmd) = tab.active_cmd.as_ref() {
            // Same guard as typed MTP/M2P and SnapOverrideMtp: the step must
            // take a point pick (possibly alongside keywords), not an entity.
            let has_point_step = (!cmd.input_kind().wants_text()
                || cmd.point_step_accepts_keywords())
                && !cmd.needs_entity_pick();
            return MenuContext::Command {
                options: cmd.options(),
                has_point_step,
                recent_inputs: self.command_line.recent_inputs.clone(),
            };
        }
        MenuContext::Idle {
            has_selection: !tab.scene.selected.is_empty(),
            selected_constraint: tab.scene.selected_constraint,
            isolation_active: tab.scene.is_isolation_active(),
            recent_cmds: self
                .command_line
                .recent_commands
                .iter()
                .rev()
                .take(crate::ui::popup::context_menu::RECENT_LIMIT)
                .cloned()
                .collect(),
            clipboard_nonempty: !self.clipboard.is_empty(),
            undo_label: tab.history.undo_stack.last().map(|s| s.label().to_string()),
            redo_label: tab.history.redo_stack.last().map(|s| s.label().to_string()),
            props_open: self.show_properties,
        }
    }

    /// The menu as currently shown (rows + expanded submenu).
    pub(in crate::app) fn current_context_menu(&self) -> ContextMenu {
        let open = self.tabs[self.active_tab]
            .scene
            .selection
            .borrow()
            .context_menu_ui
            .open_submenu;
        build_context_menu(&self.context_menu_context(), open)
    }

    /// Close the menu and give the keyboard back to the command line.
    pub(in crate::app) fn close_context_menu(&mut self) -> Task<Message> {
        let i = self.active_tab;
        let mut sel = self.tabs[i].scene.selection.borrow_mut();
        if sel.context_menu.is_none() {
            return Task::none();
        }
        sel.close_context_menu();
        drop(sel);
        self.focus_cmd_input()
    }

    pub(in crate::app) fn on_context_menu_submenu_toggle(&mut self, id: SubmenuId) -> Task<Message> {
        let i = self.active_tab;
        let mut sel = self.tabs[i].scene.selection.borrow_mut();
        let ui = &mut sel.context_menu_ui;
        ui.open_submenu = if ui.open_submenu == Some(id) { None } else { Some(id) };
        drop(sel);
        // Keep the keyboard highlight on the header that was toggled.
        let menu = self.current_context_menu();
        let header = menu
            .selectable()
            .iter()
            .position(|s| s.header == Some(id));
        let mut sel = self.tabs[i].scene.selection.borrow_mut();
        if sel.context_menu_ui.highlighted.is_some() {
            sel.context_menu_ui.highlighted = header;
        }
        Task::none()
    }

    /// Run a picked row. The menu closes first so the action sees the same
    /// state a typed keyword / shortcut would (no menu open, command line
    /// focused), then the action is routed through the existing message for
    /// that behaviour.
    pub(in crate::app) fn on_context_menu_pick(&mut self, action: MenuAction) -> Task<Message> {
        if let MenuAction::ToggleSubmenu(id) = action {
            return self.on_context_menu_submenu_toggle(id);
        }
        let i = self.active_tab;
        {
            let mut sel = self.tabs[i].scene.selection.borrow_mut();
            sel.close_context_menu();
            // A pick counts as "another interaction" for the Enter-first
            // right-click cycle: the next right-click acts as Enter again.
            sel.right_click_entered = false;
        }
        let focus = self.focus_cmd_input();
        let task = match action {
            MenuAction::Enter => self.update(Message::CommandFinalize),
            MenuAction::Cancel => self.update(Message::CommandEscape),
            MenuAction::Option(kw) => self.update(Message::CommandOptionPick(kw)),
            MenuAction::FeedInput(token) => {
                if self.tabs[i].active_cmd.is_some() {
                    self.feed_active_cmd(&token)
                } else {
                    self.run_command_line(&token)
                }
            }
            MenuAction::Command(cmd) => self.update(Message::Command(cmd)),
            MenuAction::Mtp => self.update(Message::SnapOverrideMtp),
            MenuAction::DeleteSelected => self.update(Message::DeleteSelected),
            MenuAction::SelectSimilar => self.update(Message::SelectSimilar),
            MenuAction::InvertSelection => self.update(Message::InvertSelection),
            MenuAction::DeselectAll => {
                self.tabs[i].scene.deselect_all();
                self.refresh_properties();
                Task::none()
            }
            MenuAction::QuickSelect => self.update(Message::QSelectOpen),
            MenuAction::Properties => self.update(Message::ToggleProperties),
            MenuAction::Options => self.update(Message::OptionsOpen),
            MenuAction::Undo => self.update(Message::Undo),
            MenuAction::Redo => self.update(Message::Redo),
            MenuAction::DrawOrderPickRef(above) => self.update(Message::DrawOrderPickRef(above)),
            MenuAction::ConstraintDelete(id) => self.update(Message::PropConstraintDelete(id)),
            MenuAction::Grip(cmd) => self.on_grip_context_pick(cmd),
            MenuAction::ToggleSubmenu(_) => Task::none(),
        };
        Task::batch(vec![focus, task])
    }

    /// Keyboard navigation while the menu is open.
    pub(in crate::app) fn on_context_menu_navigate(&mut self, nav: ContextMenuNav) -> Task<Message> {
        if !self.context_menu_open() {
            return Task::none();
        }
        let i = self.active_tab;
        let menu = self.current_context_menu();
        let rows = menu.selectable();
        if rows.is_empty() {
            return Task::none();
        }
        let current = self.tabs[i].scene.selection.borrow().context_menu_ui.highlighted;
        let next_enabled = |from: usize, step: isize| -> usize {
            let n = rows.len() as isize;
            let mut idx = from as isize;
            for _ in 0..n {
                idx = (idx + step).rem_euclid(n);
                if rows[idx as usize].enabled {
                    return idx as usize;
                }
            }
            from
        };
        match nav {
            ContextMenuNav::Down => {
                let idx = match current {
                    // First press lands on the default row itself.
                    None => menu.default_index(),
                    Some(cur) => next_enabled(cur, 1),
                };
                self.tabs[i].scene.selection.borrow_mut().context_menu_ui.highlighted = Some(idx);
                Task::none()
            }
            ContextMenuNav::Up => {
                let idx = match current {
                    None => menu.default_index(),
                    Some(cur) => next_enabled(cur, -1),
                };
                self.tabs[i].scene.selection.borrow_mut().context_menu_ui.highlighted = Some(idx);
                Task::none()
            }
            ContextMenuNav::Enter => {
                let idx = current.unwrap_or_else(|| menu.default_index());
                let row = &rows[idx];
                if !row.enabled {
                    return Task::none();
                }
                self.on_context_menu_pick(row.action.clone())
            }
            ContextMenuNav::Mnemonic(ch) => match menu.find_mnemonic(ch, current) {
                Some(idx) => {
                    // A letter shared by several rows (PLINE's CEnter / CLose)
                    // cycles the highlight; a unique one picks at once.
                    let unique = menu.find_mnemonic(ch, Some(idx)) == Some(idx);
                    if unique {
                        self.on_context_menu_pick(rows[idx].action.clone())
                    } else {
                        self.tabs[i].scene.selection.borrow_mut().context_menu_ui.highlighted =
                            Some(idx);
                        Task::none()
                    }
                }
                None => Task::none(),
            },
        }
    }

    /// Route keyboard-derived messages to the open menu. Returns `Some(task)`
    /// when the message was consumed; `None` lets it fall through to the
    /// normal handlers (after closing the menu when the key is not a menu key,
    /// so typing a command name or a coordinate just works — the menu gets
    /// out of the way exactly as it does in AutoCAD).
    pub(in crate::app) fn intercept_context_menu_key(&mut self, msg: &Message) -> Option<Task<Message>> {
        match msg {
            Message::ArrowKeyPressed { direction, .. }
            | Message::CommandLineArrowProbe { direction, .. } => match direction {
                ArrowKey::Up => Some(self.on_context_menu_navigate(ContextMenuNav::Up)),
                ArrowKey::Down => Some(self.on_context_menu_navigate(ContextMenuNav::Down)),
                ArrowKey::Left | ArrowKey::Right => {
                    // On a submenu header, Right expands and Left collapses.
                    let i = self.active_tab;
                    let ui = self.tabs[i].scene.selection.borrow().context_menu_ui.clone();
                    let menu = self.current_context_menu();
                    let rows = menu.selectable();
                    let header = ui.highlighted.and_then(|c| rows.get(c)).and_then(|r| r.header);
                    let want_open = matches!(direction, ArrowKey::Right);
                    match header {
                        Some(id) if (ui.open_submenu == Some(id)) != want_open => {
                            Some(self.on_context_menu_submenu_toggle(id))
                        }
                        _ => Some(Task::none()),
                    }
                }
            },
            Message::DynTabNext => Some(self.on_context_menu_navigate(ContextMenuNav::Down)),
            Message::CommandFinalize | Message::CommandSpace => {
                if self.command_line.input.trim().is_empty() {
                    Some(self.on_context_menu_navigate(ContextMenuNav::Enter))
                } else {
                    // Typed text pending: Enter runs it, the menu just closes.
                    let close = self.close_context_menu();
                    let run = self.update(Message::CommandFinalize);
                    Some(Task::batch(vec![close, run]))
                }
            }
            Message::CommandSubmit => {
                if self.command_line.input.trim().is_empty() {
                    Some(self.on_context_menu_navigate(ContextMenuNav::Enter))
                } else {
                    let _ = self.close_context_menu();
                    None
                }
            }
            Message::CommandAppendChar(s) => self.context_menu_typed(s.chars().next()),
            Message::CommandInput(buf) => {
                // The focused command-line field delivers the whole buffer;
                // only a single appended character is a candidate mnemonic.
                let appended = buf
                    .strip_prefix(self.command_line.input.as_str())
                    .filter(|rest| rest.chars().count() == 1)
                    .and_then(|rest| rest.chars().next());
                match appended {
                    Some(ch) => self.context_menu_typed(Some(ch)),
                    None => {
                        let _ = self.close_context_menu();
                        None
                    }
                }
            }
            _ => None,
        }
    }

    /// A printable key while the menu is open: a mnemonic picks its row;
    /// anything else closes the menu and is typed into the command line.
    fn context_menu_typed(&mut self, ch: Option<char>) -> Option<Task<Message>> {
        let ch = ch?;
        let menu = self.current_context_menu();
        if ch.is_ascii_alphanumeric() && menu.find_mnemonic(ch, None).is_some() {
            return Some(self.on_context_menu_navigate(ContextMenuNav::Mnemonic(ch)));
        }
        let _ = self.close_context_menu();
        None
    }

    /// Grip-mode shortcut menu actions (Stage 7).
    pub(in crate::app) fn on_grip_context_pick(&mut self, cmd: crate::ui::popup::context_menu::GripMenuCmd) -> Task<Message> {
        let _ = cmd;
        Task::none()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::settings::RightClickMode;
    use crate::command::{CadCommand, StepInput};
    use crate::ui::popup::context_menu::MenuAction;
    use glam::DVec3;
    use iced::Point;

    /// `update` recursion (right-click → CommandFinalize → CommandSubmit →
    /// apply) is deeper than the default test-thread stack; production
    /// threads are sized for it (see `.cargo/config.toml`), so tests get an
    /// equivalent stack here.
    fn with_stack(f: impl FnOnce() + Send + 'static) {
        std::thread::Builder::new()
            .stack_size(32 * 1024 * 1024)
            .spawn(f)
            .expect("spawn test thread")
            .join()
            .unwrap_or_else(|e| std::panic::resume_unwind(e));
    }

    fn app() -> OpenCADStudio {
        let mut app = OpenCADStudio::new_for_test();
        app.automation_op(r#"{"op":"new"}"#);
        app.tabs[0].scene.selection.borrow_mut().vp_size = (800.0, 600.0);
        let _ = app.update(Message::ViewportMove(Point::new(200.0, 150.0)));
        app
    }

    fn start(app: &mut OpenCADStudio, cmd: &str, points: &[(f64, f64)]) {
        let _ = app.dispatch_command(cmd);
        for &(x, y) in points {
            let _ = app.feed_command(StepInput::Point(DVec3::new(x, y, 0.0)));
        }
    }

    fn right_click(app: &mut OpenCADStudio) {
        let _ = app.update(Message::ViewportRightPress);
        let _ = app.update(Message::ViewportRightRelease);
    }

    fn menu_open(app: &OpenCADStudio) -> bool {
        app.tabs[0].scene.selection.borrow().context_menu.is_some()
    }

    fn active(app: &OpenCADStudio) -> Option<&'static str> {
        app.tabs[0].active_cmd.as_ref().map(|c| c.name())
    }

    fn menu_actions(app: &OpenCADStudio) -> Vec<MenuAction> {
        app.current_context_menu()
            .selectable()
            .into_iter()
            .map(|s| s.action)
            .collect()
    }

    #[test]
    fn shortcut_menu_mode_single_rmb_opens_menu_during_command() {
        with_stack(|| {
            let mut app = app();
            assert_eq!(app.right_click_mode, RightClickMode::ShortcutMenu);
            start(&mut app, "LINE", &[(0.0, 0.0), (10.0, 0.0)]);
            right_click(&mut app);
            assert!(menu_open(&app));
            assert_eq!(active(&app), Some("LINE"));
            let acts = menu_actions(&app);
            assert_eq!(acts[0], MenuAction::Enter);
            assert_eq!(acts[1], MenuAction::Cancel);
            assert!(acts.contains(&MenuAction::Option("C".into())));
            assert!(acts.contains(&MenuAction::Option("U".into())));
            assert!(acts.contains(&MenuAction::Mtp));
        });
    }

    #[test]
    fn enter_first_mode_first_rmb_is_enter_second_opens_menu() {
        with_stack(|| {
            let mut app = app();
            app.right_click_mode = RightClickMode::EnterFirst;
            start(&mut app, "LINE", &[(0.0, 0.0), (10.0, 0.0)]);
            right_click(&mut app);
            // Enter ends LINE.
            assert!(!menu_open(&app));
            assert_eq!(active(&app), None);
            // Idle: the next right-click opens the menu.
            right_click(&mut app);
            assert!(menu_open(&app));
            let acts = menu_actions(&app);
            assert_eq!(acts[0], MenuAction::Command("LINE".into()), "Repeat LINE first");
        });
    }

    #[test]
    fn time_sensitive_quick_click_is_enter_and_hold_opens_menu() {
        with_stack(|| {
            let mut app = app();
            app.right_click_mode = RightClickMode::TimeSensitive;
            app.right_click_hold_ms = 250;
            start(&mut app, "PLINE", &[(0.0, 0.0), (10.0, 0.0), (10.0, 10.0)]);
            // Held: opens the menu, PLINE keeps running.
            let _ = app.update(Message::ViewportRightPress);
            {
                let mut sel = app.tabs[0].scene.selection.borrow_mut();
                sel.right_press_time = iced::time::Instant::now()
                    .checked_sub(std::time::Duration::from_millis(400));
            }
            let _ = app.update(Message::ViewportRightRelease);
            assert!(menu_open(&app));
            assert_eq!(active(&app), Some("PLINE"));
            // Dismiss, then a quick click finishes the polyline.
            let _ = app.update(Message::CommandEscape);
            assert!(!menu_open(&app));
            assert_eq!(active(&app), Some("PLINE"));
            right_click(&mut app);
            assert!(!menu_open(&app));
            assert_eq!(active(&app), None);
        });
    }

    #[test]
    fn pending_text_runs_as_enter_in_all_modes() {
        with_stack(|| {
            for mode in RightClickMode::ALL {
                let mut app = app();
                app.right_click_mode = mode;
                start(&mut app, "LINE", &[(0.0, 0.0), (10.0, 0.0), (10.0, 10.0)]);
                app.command_line.input = "C".into();
                right_click(&mut app);
                assert!(!menu_open(&app), "{mode:?}");
                assert_eq!(active(&app), None, "{mode:?}: Close should end LINE");
                let lines = app.tabs[0]
                    .scene
                    .document
                    .entities()
                    .filter(|e| matches!(e, acadrust::EntityType::Line(_)))
                    .count();
                assert_eq!(lines, 3, "{mode:?}");
            }
        });
    }

    #[test]
    fn context_menu_pick_option_feeds_keyword() {
        with_stack(|| {
            let mut app = app();
            start(&mut app, "PLINE", &[(0.0, 0.0), (10.0, 0.0), (10.0, 10.0)]);
            right_click(&mut app);
            assert!(menu_open(&app));
            let _ = app.update(Message::ContextMenuPick(MenuAction::Option("C".into())));
            assert!(!menu_open(&app));
            assert_eq!(active(&app), None);
            let closed = app.tabs[0].scene.document.entities().any(|e| {
                matches!(e, acadrust::EntityType::LwPolyline(p) if p.is_closed && p.vertices.len() == 3)
            });
            assert!(closed, "PLINE should have been closed by the menu pick");
        });
    }

    #[test]
    fn context_menu_pick_closes_menu_and_resets_enter_cycle() {
        with_stack(|| {
            let mut app = app();
            app.right_click_mode = RightClickMode::EnterFirst;
            start(&mut app, "LINE", &[(0.0, 0.0), (10.0, 0.0)]);
            right_click(&mut app); // Enter → LINE ends
            right_click(&mut app); // idle → menu
            assert!(menu_open(&app));
            let _ = app.update(Message::ContextMenuPick(MenuAction::Command("LINE".into())));
            assert!(!menu_open(&app));
            assert_eq!(active(&app), Some("LINE"));
            assert!(!app.tabs[0].scene.selection.borrow().right_click_entered);
        });
    }

    #[test]
    fn menu_enter_with_no_highlight_picks_default() {
        with_stack(|| {
            let mut app = app();
            start(&mut app, "PLINE", &[(0.0, 0.0), (10.0, 0.0)]);
            right_click(&mut app);
            let _ = app.update(Message::CommandFinalize);
            assert!(!menu_open(&app));
            assert_eq!(active(&app), None, "default row (Done) finishes PLINE");
        });
    }

    #[test]
    fn menu_arrow_down_then_enter_picks_cancel() {
        with_stack(|| {
            let mut app = app();
            start(&mut app, "LINE", &[(0.0, 0.0), (10.0, 0.0)]);
            right_click(&mut app);
            let _ = app.update(Message::ContextMenuNavigate(ContextMenuNav::Down)); // Enter row
            assert_eq!(
                app.tabs[0].scene.selection.borrow().context_menu_ui.highlighted,
                Some(0)
            );
            let _ = app.update(Message::ContextMenuNavigate(ContextMenuNav::Down)); // Cancel
            let _ = app.update(Message::ContextMenuNavigate(ContextMenuNav::Enter));
            assert!(!menu_open(&app));
            assert_eq!(active(&app), None);
        });
    }

    #[test]
    fn menu_mnemonic_c_closes_pline() {
        with_stack(|| {
            let mut app = app();
            start(&mut app, "PLINE", &[(0.0, 0.0), (10.0, 0.0), (10.0, 10.0)]);
            right_click(&mut app);
            let _ = app.update(Message::CommandAppendChar("c".into()));
            assert!(!menu_open(&app));
            assert_eq!(active(&app), None);
            let closed = app.tabs[0].scene.document.entities().any(
                |e| matches!(e, acadrust::EntityType::LwPolyline(p) if p.is_closed),
            );
            assert!(closed);
        });
    }

    #[test]
    fn menu_unmatched_char_closes_menu_and_types() {
        with_stack(|| {
            let mut app = app();
            app.dyn_input = false;
            start(&mut app, "LINE", &[(0.0, 0.0)]);
            right_click(&mut app);
            assert!(menu_open(&app));
            let _ = app.update(Message::CommandAppendChar("7".into()));
            assert!(!menu_open(&app));
            assert_eq!(active(&app), Some("LINE"));
            assert!(app.command_line.input.contains('7'), "{:?}", app.command_line.input);
        });
    }

    #[test]
    fn recent_input_pick_refeeds_token() {
        with_stack(|| {
            let mut app = app();
            start(&mut app, "LINE", &[(0.0, 0.0)]);
            let _ = app.feed_active_cmd("10,10");
            assert_eq!(app.command_line.recent_inputs, vec!["10,10".to_string()]);
            let _ = app.feed_active_cmd("20,0");
            right_click(&mut app);
            let _ = app.update(Message::ContextMenuSubmenuToggle(SubmenuId::RecentInput));
            let acts = menu_actions(&app);
            assert!(acts.contains(&MenuAction::FeedInput("10,10".into())));
            let _ = app.update(Message::ContextMenuPick(MenuAction::FeedInput("10,10".into())));
            assert!(!menu_open(&app));
            let lines = app.tabs[0]
                .scene
                .document
                .entities()
                .filter(|e| matches!(e, acadrust::EntityType::Line(_)))
                .count();
            assert_eq!(lines, 3);
        });
    }

    #[test]
    fn idle_selection_menu_erase_deletes_selection() {
        with_stack(|| {
            let mut app = app();
            start(&mut app, "LINE", &[(0.0, 0.0), (10.0, 0.0)]);
            let _ = app.update(Message::CommandEscape);
            let handle = app.tabs[0]
                .scene
                .document
                .entities()
                .find_map(|e| match e {
                    acadrust::EntityType::Line(l) => Some(l.common.handle),
                    _ => None,
                })
                .expect("a line");
            app.tabs[0].scene.select_entities(&[handle]);
            right_click(&mut app);
            let acts = menu_actions(&app);
            assert!(acts.contains(&MenuAction::DeleteSelected));
            assert!(acts.contains(&MenuAction::Command("ROTATE".into())));
            let _ = app.update(Message::ContextMenuPick(MenuAction::DeleteSelected));
            assert!(!menu_open(&app));
            let lines = app.tabs[0]
                .scene
                .document
                .entities()
                .filter(|e| matches!(e, acadrust::EntityType::Line(_)))
                .count();
            assert_eq!(lines, 0);
        });
    }
}
