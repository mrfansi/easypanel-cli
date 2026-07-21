//! What a user can DO, and how those actions are grouped.
//!
//! A third `impl App`, deliberately in its own file — `app` holds the state and
//! the selectors, `keys` maps a keypress to an action, and this is the CATALOGUE
//! those two reach for: the context menu, its submenus, and the command palette.
//!
//! Keeping it here is what makes "one door per thing" checkable: every action a
//! service has is defined once, in this file, and both the menu and the palette
//! are built from the same definitions rather than from parallel lists.

use std::sync::mpsc::Sender;

use ratatui::layout::Rect;
use ratatui::widgets::ListState;
use serde_json::Value;

use crate::output::field;

use super::app::{App, Screen, TABS, TAB_SCREENS};
use super::table::Line2;
use super::worker::{Req, View};
use ratatui::crossterm::event::KeyCode;

/// A menu item's action: the function run when the item is chosen.
///
/// A closure with NO captures (its parameters baked in as literals, e.g.
/// `View::Env`) becomes an `fn` automatically, so no Box is needed. The menu holds
/// its action directly — rather than simulating a key — so that one group key
/// (e.g. `e` → Env menu) doesn't trigger itself, and the menu becomes a SINGLE
/// definition of the action.
pub(super) type MenuRun = fn(&mut App, &Sender<Req>);

/// One menu item: the displayed label + the action when chosen.
pub(super) struct MenuItem {
    pub(super) label: String,
    pub(super) run: MenuRun,
}

impl MenuItem {
    pub(super) fn new(label: impl Into<String>, run: MenuRun) -> Self {
        Self {
            label: label.into(),
            run,
        }
    }
}

/// A context/action menu: a popup list of items; choosing one runs its `run`,
/// which may in turn open a submenu (items marked ▸). Opened by right click or a
/// group key on the keyboard.
pub(super) struct Menu {
    pub(super) items: Vec<MenuItem>,
    pub(super) state: ListState,
    /// The parent menu when this is a submenu (e.g. Env opened from the service
    /// menu). `←` returns here; None = the top menu, `←` closes.
    pub(super) parent: Option<Box<Menu>>,
    /// The cursor position when the menu opened (its top-left corner, before being
    /// clamped to the screen).
    pub(super) col: u16,
    pub(super) row: u16,
    /// The menu box as actually drawn (after clamping to the screen), filled in
    /// during render — used to map a click/hover to an item.
    pub(super) rect: Rect,
}

impl Menu {
    /// The item index under (col,row), or None if outside the item area. Item i is
    /// drawn on row `rect.y + 1 + i` (the first & last rows are the border).
    pub(super) fn item_at(&self, col: u16, row: u16) -> Option<usize> {
        let r = self.rect;
        let inside = col >= r.x
            && col < r.x.saturating_add(r.width)
            && row > r.y
            && row < r.y.saturating_add(r.height).saturating_sub(1);
        if !inside {
            return None;
        }
        let i = (row - r.y - 1) as usize;
        (i < self.items.len()).then_some(i)
    }
}

/// A command palette (global search) entry's action.
pub(super) enum PaletteAction {
    /// Jump to a service (switch to Services, highlight its row).
    Service { project: String, service: String },
    /// Run a contextual action on the CURRENTLY selected row (the same action
    /// function from the menu/leaf). The row is already highlighted, so the action
    /// hits the right one.
    Run(MenuRun),
    /// Switch to a tab.
    Tab(Screen),
}

pub(super) struct PaletteItem {
    /// What is DRAWN. Kept short: an action's row says "Deploy", not
    /// "Deploy  ·  project/service" repeated down thirty rows.
    pub(super) label: String,
    /// What is SEARCHED. Carries the context the label no longer repeats, so
    /// "deploy api" still finds the deploy action on api.
    pub(super) search: String,
    pub(super) action: PaletteAction,
}

/// The command palette: a global search for quick navigation to a service/tab
/// without browsing menus. Type to filter, ↑↓ select, Enter jump, Esc close.
pub(super) struct Palette {
    /// What the action entries apply to, shown ONCE in the title. It used to be
    /// repeated on every row, which turned thirty actions into thirty copies of
    /// the same service name.
    pub(super) context: Option<String>,
    pub(super) query: String,
    pub(super) items: Vec<PaletteItem>,
    pub(super) state: ListState,
    /// The box as drawn (for clicks, filled in during render).
    pub(super) rect: Rect,
}

impl Palette {
    /// The indices of items matching the query. The query is split into words and
    /// ALL words must appear (not necessarily in order), so "deploy pay" matches
    /// "Deploy …/pay".
    pub(super) fn matches(&self) -> Vec<usize> {
        let q = self.query.to_lowercase();
        let terms: Vec<&str> = q.split_whitespace().collect();
        self.items
            .iter()
            .enumerate()
            .filter(|(_, it)| {
                let l = it.search.to_lowercase();
                terms.iter().all(|t| l.contains(t))
            })
            .map(|(i, _)| i)
            .collect()
    }
}

impl App {
    /// The action menu for the highlighted row on the active screen (opened by
    /// right click or Enter). Empty = no row selected / a screen with no row actions.
    ///
    /// On Projects this is a nested menu: related actions are grouped
    /// (Env/Networking/Build/…) so they don't scatter into 25 loose keys.
    /// Bulk entries, shown ONLY while something is marked.
    ///
    /// They sit at the top of whatever menu opens, and every label carries the
    /// count. A bulk action must never be reachable by the same words as a
    /// single-service one: "Restart" acting on 12 services because of marks made
    /// several screens ago is precisely the silent action this UI refuses.
    fn bulk_items(&self) -> Vec<MenuItem> {
        let n = self.marked.len();
        if n == 0 {
            return vec![];
        }
        let mut items = vec![];
        // Comparing needs exactly two — "diff" of three services is not a thing.
        // It sits at the very top because it is a read, not a mutation: the one
        // safe entry among a menu of actions that change things.
        if n == 2 {
            items.push(MenuItem::new("Compare the 2 marked services", |a, r| {
                a.diff_marked(r);
            }));
        }
        items.extend([
            MenuItem::new(format!("Deploy {n} marked services"), |a, _| {
                a.open_bulk_confirm("deploy", false)
            }),
            MenuItem::new(format!("Force rebuild {n} marked services"), |a, _| {
                a.open_bulk_confirm("deploy", true)
            }),
            MenuItem::new(format!("Restart {n} marked services"), |a, _| {
                a.open_bulk_confirm("restart", false)
            }),
            MenuItem::new(format!("Stop {n} marked services"), |a, _| {
                a.open_bulk_confirm("stop", false)
            }),
            MenuItem::new(format!("Start {n} marked services"), |a, _| {
                a.open_bulk_confirm("start", false)
            }),
            MenuItem::new(format!("Clear the {n} marks"), |a, _| {
                a.marked.clear();
                a.status = "Marks cleared".into();
            }),
        ]);
        items
    }

    pub(super) fn context_items(&self) -> Vec<MenuItem> {
        let bulk = self.bulk_items();
        if !bulk.is_empty() && self.screen == Screen::Projects {
            // The single-service actions stay reachable underneath: marking some
            // rows must not lock you out of acting on the one under the cursor.
            let mut items = bulk;
            items.extend(self.single_context_items());
            return items;
        }
        self.single_context_items()
    }

    fn single_context_items(&self) -> Vec<MenuItem> {
        match self.screen {
            Screen::Projects if self.selected_row().is_some() => self.service_menu(),
            // A project header row: the actions that apply to the PROJECT. It used
            // to open nothing at all, which made migrating a project unreachable
            // from the row a user would naturally try it on.
            Screen::Projects if self.selected_project().is_some() => vec![
                MenuItem::new("Migrate WHOLE project to another server", |a, _| {
                    a.open_migrate_form(true)
                }),
                MenuItem::new("Compare WHOLE project with another host", |a, _| {
                    a.open_diff_project_across_form()
                }),
                MenuItem::new("New service", |a, r| a.on_key(KeyCode::Char('n'), r)),
                MenuItem::new("New project", |a, r| a.on_key(KeyCode::Char('N'), r)),
                MenuItem::new("Destroy project", |a, r| a.on_key(KeyCode::Char('X'), r)),
            ],
            Screen::Domains if self.domains_state.selected().is_some() => vec![
                MenuItem::new("Edit", |a, r| a.on_key(KeyCode::Char('e'), r)),
                MenuItem::new("Set primary", |a, r| a.on_key(KeyCode::Char('P'), r)),
                MenuItem::new("Delete", |a, r| a.on_key(KeyCode::Char('x'), r)),
            ],
            Screen::Actions if self.selected_action_id().is_some() => {
                vec![MenuItem::new("View detail", |a, r| {
                    a.on_key(KeyCode::Enter, r)
                })]
            }
            _ => vec![],
        }
    }

    /// The service action menu (top-level): common actions + a submenu per group.
    /// A ▸ item opens its submenu. Actions whose key is repurposed into a menu
    /// opener (Env `e`, Networking `o`, Build `u`, Storage `m`, Lifecycle `d`,
    /// Shell `t`, Danger `x`) call the method directly; the rest go through
    /// `on_key` on their still-live leaf key — there's no second action path that
    /// could drift.
    pub(super) fn service_menu(&self) -> Vec<MenuItem> {
        vec![
            MenuItem::new("Logs", |a, r| a.open_view(View::Logs, r)),
            MenuItem::new("Env ▸", |a, _| {
                let m = a.env_menu();
                a.open_menu(m);
            }),
            MenuItem::new("Networking ▸", |a, _| {
                let m = a.net_menu();
                a.open_menu(m);
            }),
            MenuItem::new("Build & source ▸", |a, _| {
                let m = a.build_menu();
                a.open_menu(m);
            }),
            MenuItem::new("Storage ▸", |a, _| {
                let m = a.store_menu();
                a.open_menu(m);
            }),
            MenuItem::new("Lifecycle ▸", |a, _| {
                let m = a.life_menu();
                a.open_menu(m);
            }),
            MenuItem::new("Shell ▸", |a, _| {
                let m = a.shell_menu();
                a.open_menu(m);
            }),
            MenuItem::new("Clone", |a, r| a.on_key(KeyCode::Char('c'), r)),
            MenuItem::new("Migrate to another server", |a, _| {
                a.open_migrate_form(false)
            }),
            MenuItem::new("Migrate WHOLE project", |a, _| a.open_migrate_form(true)),
            MenuItem::new("Compare with another host", |a, _| {
                a.open_diff_across_form()
            }),
            MenuItem::new("Compare WHOLE project with another host", |a, _| {
                a.open_diff_project_across_form()
            }),
            MenuItem::new("Danger ▸", |a, _| {
                let m = a.danger_menu();
                a.open_menu(m);
            }),
        ]
    }

    fn is_selected_type(&self, kinds: &[&str]) -> bool {
        self.selected_row()
            .map(|(_, _, t)| kinds.contains(&t.as_str()))
            .unwrap_or(false)
    }

    pub(super) fn env_menu(&self) -> Vec<MenuItem> {
        // ONE door. This used to be three — "View env", "Edit env (partial)" and
        // "Replace entire env" — for what is one screen and one operation.
        //
        // The labels were also wrong: saving sends the whole `env` string, so
        // BOTH "edit" and "replace" replaced everything. The only difference was
        // whether $EDITOR opened pre-filled or blank, which is a thing you do
        // inside your editor, not a separate feature.
        let mut v = vec![MenuItem::new("Env", |a, r| a.open_view(View::Env, r))];
        // The project's SHARED env lives behind the same door: it is the same
        // idea one level up, and it reaches these containers too — verified live,
        // a project variable shows up inside the container next to the service's
        // own. A separate key for it would be a second door into env.
        if let Some(p) = self.selected_project() {
            let n = self.deployable_in(&p).len();
            v.push(MenuItem::new(
                format!("Project env ({p} — shared by {n} service(s))"),
                |a, _| a.start_project_env_edit(),
            ));
        }
        // The .env file is only for app services (see the `.` handler).
        if self.is_selected_type(&["app"]) {
            v.push(MenuItem::new("Toggle .env file", |a, r| {
                a.on_key(KeyCode::Char('.'), r)
            }));
        }
        v
    }

    pub(super) fn net_menu(&self) -> Vec<MenuItem> {
        // Redirects and basic auth are web-only. Offering them on a redis service
        // and refusing one keystroke later — in a status line that then fades —
        // is a door painted on a wall. Worse for redirects: it OPENED, showed
        // "No redirects" and a footer saying `n add`, on a service type that can
        // never have one. Same type list the handlers check.
        let stype = self.selected_row().map(|(_, _, t)| t).unwrap_or_default();
        let mut v = Vec::new();
        // A domain in front of a database is not a thing EasyPanel will make:
        // createDomain answers "Wrong service type".
        if crate::lifecycle::has_domains(&stype) {
            v.push(MenuItem::new("Domain", |a, r| a.open_service_domains(r)));
        }
        // Ports are app/box only — every other type answers "Invalid service
        // type", so the entry could only ever open an error.
        if crate::lifecycle::has_mounts_and_ports(&stype) {
            // The viewer adds and deletes, so "Add X" is not a separate door.
            v.push(MenuItem::new("Ports", |a, r| {
                a.on_key(KeyCode::Char('p'), r)
            }));
        }
        if crate::lifecycle::is_web(&stype) {
            v.push(MenuItem::new("Redirects", |a, r| {
                a.on_key(KeyCode::Char('f'), r)
            }));
            v.push(MenuItem::new("Basic auth", |a, r| {
                a.on_key(KeyCode::Char('H'), r)
            }));
        }
        v
    }

    pub(super) fn build_menu(&self) -> Vec<MenuItem> {
        // Source & build (and therefore auto deploy) belong to app services: a
        // database runs an image EasyPanel picks, and the handler refuses with
        // "only for app services". Offering it anyway makes the menu a list of
        // things that might work.
        let stype = self.selected_row().map(|(_, _, t)| t).unwrap_or_default();
        let mut v = Vec::new();
        if self.is_selected_type(&["app"]) {
            v.push(MenuItem::new("Source & build", |a, r| {
                a.open_view(View::Source, r)
            }));
            v.push(MenuItem::new("Auto deploy on/off", |a, r| {
                a.on_key(KeyCode::Char('A'), r)
            }));
            // Replicas live in the `deploy` block, beside the start command and
            // zero-downtime — one door for the three of them.
            v.push(MenuItem::new("Replicas & deploy", |a, r| {
                a.open_deploy_form(r)
            }));
        }
        // A compose service has no updateResources route: its limits belong in the
        // compose file, and the entry could only ever draw a 404.
        if crate::lifecycle::has_resource_limits(&stype) {
            v.push(MenuItem::new("Resource limit", |a, r| {
                a.on_key(KeyCode::Char('L'), r)
            }));
        }
        // Config File (Advanced) = updateAdvanced. The databases have it (that's
        // where a MySQL replication config lives) and so does `box`, which used to
        // be left out even though the panel offers it there.
        if crate::lifecycle::has_config_file(&stype) {
            v.push(MenuItem::new("Config file (Advanced)", |a, _| {
                a.start_config_edit()
            }));
        }
        v
    }

    pub(super) fn store_menu(&self) -> Vec<MenuItem> {
        let stype = self.selected_row().map(|(_, _, t)| t).unwrap_or_default();
        let mut v = Vec::new();
        if crate::lifecycle::has_mounts_and_ports(&stype) {
            v.push(MenuItem::new("Mounts", |a, r| a.open_view(View::Mounts, r)));
        }
        // Backups belong to databases. listDatabaseBackups answers [] for an app
        // rather than an error, so the entry used to open an empty box on every
        // service in the panel and explain nothing.
        if crate::backup::can_back_up(&stype) {
            v.push(MenuItem::new("Backup now", |a, r| a.backup_now(r)));
            v.push(MenuItem::new("Restore from a backup", |a, r| {
                a.open_restore(r)
            }));
            v.push(MenuItem::new("Restore from another server", |a, _| {
                a.open_restore_from()
            }));
            v.push(MenuItem::new("Backup schedules", |a, r| {
                a.open_view(View::Backups, r)
            }));
        }
        v
    }

    /// Only the actions this service type actually HAS.
    ///
    /// A database has no deploy — it is pulled, not built — and offering one used
    /// to send a request that could only come back 404. The panel's own verbs
    /// differ per type; the menu now follows them rather than assuming every
    /// service works like an app.
    pub(super) fn life_menu(&self) -> Vec<MenuItem> {
        let stype = self.selected_row().map(|(_, _, t)| t).unwrap_or_default();
        let has = |action: &str| crate::lifecycle::ops(&stype, action).is_some();
        let mut v = Vec::new();
        if has("deploy") {
            v.push(MenuItem::new("Deploy", |a, _| a.ask_action("deploy")));
            // Its own entry rather than a change to Deploy: skipping the cache can
            // turn a seconds-long deploy into minutes, so it should be a choice the
            // user made, not a surprise.
            v.push(MenuItem::new("Force rebuild (no cache)", |a, _| {
                a.ask_action("deploy-force")
            }));
        }
        // A database's restart is a stop and a start, so the label says so: the
        // service is briefly OFF, which "Restart" alone rather understates.
        let restart = if crate::lifecycle::is_database(&stype) {
            "Restart (stop, then start)"
        } else {
            "Restart"
        };
        v.push(MenuItem::new(restart, |a, _| a.ask_action("restart")));
        v.push(MenuItem::new("Stop", |a, _| a.ask_action("stop")));
        v.push(MenuItem::new("Start", |a, _| a.ask_action("start")));
        v
    }

    pub(super) fn shell_menu(&self) -> Vec<MenuItem> {
        let mut v = vec![MenuItem::new("Terminal", |a, _| a.start_terminal())];
        if self.is_selected_type(&["mysql", "mariadb", "postgres", "mongo", "redis"]) {
            v.push(MenuItem::new("DB shell (auto login)", |a, _| {
                a.start_db_shell()
            }));
            v.push(MenuItem::new("Credentials (view & copy)", |a, _| {
                a.start_credentials()
            }));
        }
        v
    }

    pub(super) fn danger_menu(&self) -> Vec<MenuItem> {
        vec![
            MenuItem::new("Delete service", |a, _| a.ask_action("destroy")),
            MenuItem::new("Delete project", |a, r| a.on_key(KeyCode::Char('X'), r)),
        ]
    }

    /// Open a menu that was triggered by the keyboard. Anchored to the SELECTED row
    /// (not the top-left corner) so it appears in that row's context, like a right
    /// click. Right-click menus use the cursor position (see on_right_click).
    /// Open a row menu, but say so instead of doing nothing when no service is
    /// selected. Without this the menu opens over a project header and every item
    /// inside it silently fails — the action surface looks broken rather than
    /// unavailable.
    pub(super) fn open_service_menu(&mut self, items: Vec<MenuItem>) {
        let Some((_, _, stype)) = self.selected_row() else {
            self.status = "Select a service first".into();
            return;
        };
        // A group with nothing in it for THIS type must say so. Gating entries by
        // what a type supports can empty a whole group — Storage on a redis, once
        // mounts and backups were both correctly withdrawn — and a group key that
        // silently does nothing is the same dead end the gating set out to fix,
        // only quieter.
        if items.is_empty() {
            self.status = format!("Nothing here for a {stype} service");
            return;
        }
        self.open_menu(items);
    }

    pub(super) fn open_menu(&mut self, items: Vec<MenuItem>) {
        if items.is_empty() {
            return;
        }
        let mut state = ListState::default();
        state.select(Some(0));
        let a = self.table_area;
        // The selected row on screen = area.y + border(1) + header(1) + (index - offset).
        // From the ACTIVE table (Projects/Domains/Actions), so the menu appears on
        // the right row on any screen.
        let (sel, off) = self
            .active_table()
            .map(|t| (t.selected().unwrap_or(0), t.offset()))
            .unwrap_or((0, 0));
        let rel = sel.saturating_sub(off) as u16;
        let row = a.y.saturating_add(2).saturating_add(rel);
        self.menu = Some(Menu {
            items,
            state,
            parent: None,
            // +2: skip the border + service indent, so the menu's left edge doesn't
            // cover (or get bled through by) the first column's text.
            col: a.x.saturating_add(2),
            row,
            rect: Rect::default(),
        });
    }

    /// Open the command palette (global search): every service on this server + the
    /// tabs as entries. A keyboard-friendly alternative to the context menu for
    /// quick navigation.
    /// ALL leaf actions for the selected service, flattened from the submenu
    /// builders — so the palette has the menu's full set of actions (not just
    /// lifecycle).
    fn service_leaf_actions(&self) -> Vec<MenuItem> {
        let mut v = vec![MenuItem::new("Logs", |a, r| a.open_view(View::Logs, r))];
        v.extend(self.env_menu());
        v.extend(self.net_menu());
        v.extend(self.build_menu());
        v.extend(self.store_menu());
        v.extend(self.life_menu());
        v.extend(self.shell_menu());
        v.push(MenuItem::new("Clone", |a, r| {
            a.on_key(KeyCode::Char('c'), r)
        }));
        v.push(MenuItem::new("Migrate to another server", |a, _| {
            a.open_migrate_form(false)
        }));
        v.push(MenuItem::new("Compare with another host", |a, _| {
            a.open_diff_across_form()
        }));
        v.push(MenuItem::new(
            "Compare WHOLE project with another host",
            |a, _| a.open_diff_project_across_form(),
        ));
        v.push(MenuItem::new("Migrate WHOLE project", |a, _| {
            a.open_migrate_form(true)
        }));
        v.extend(self.danger_menu());
        v
    }

    /// Contextual actions for the selected row on the active screen, as
    /// (label, function). Services → the full list with an id suffix; other screens
    /// → their context-menu actions prefixed with the screen name. Empty when no
    /// row is selected.
    /// What the palette's action entries apply to, for the title.
    fn palette_context_label(&self) -> Option<String> {
        if self.screen == Screen::Projects {
            let (project, service, _) = self.selected_row()?;
            return Some(format!("{project}/{service}"));
        }
        (!self.context_items().is_empty()).then(|| TABS[self.screen.index()].to_string())
    }

    fn palette_context_actions(&self) -> Vec<(String, String, MenuRun)> {
        if self.screen == Screen::Projects {
            let Some((project, service, _)) = self.selected_row() else {
                return vec![];
            };
            let id = format!("{project}/{service}");
            self.service_leaf_actions()
                .into_iter()
                .map(|it| {
                    let search = format!("{} {id}", it.label);
                    (it.label, search, it.run)
                })
                .collect()
        } else {
            let scr = TABS[self.screen.index()];
            self.context_items()
                .into_iter()
                .map(|it| {
                    let search = format!("{scr} {}", it.label);
                    (it.label, search, it.run)
                })
                .collect()
        }
    }

    pub(super) fn open_palette(&mut self) {
        let mut items = Vec::new();
        // Actions are CONTEXTUAL: only for the row currently selected on the active
        // screen (preventing hundreds of entries / bloat). ALL of that row's actions
        // are flattened — a service gets the full list (env/networking/build/mount/
        // lifecycle/shell/clone/delete), other screens get their context-menu
        // actions. With no row selected, the palette is pure navigation.
        for (label, search, run) in self.palette_context_actions() {
            items.push(PaletteItem {
                label,
                search,
                action: PaletteAction::Run(run),
            });
        }
        // Navigation: switch tabs.
        for s in TAB_SCREENS {
            let label = format!("⇥  Tab: {}", TABS[s.index()]);
            items.push(PaletteItem {
                search: label.clone(),
                label,
                action: PaletteAction::Tab(s),
            });
        }
        // Navigation: jump to any service.
        let mut svcs: Vec<&Value> = self.all_services.iter().collect();
        svcs.sort_by_key(|s| (field(s, "/projectName"), field(s, "/name")));
        for s in svcs {
            let (project, service, t) = (
                field(s, "/projectName"),
                field(s, "/name"),
                field(s, "/type"),
            );
            let label = format!("Open  {project}/{service}  ·  {t}");
            items.push(PaletteItem {
                search: label.clone(),
                label,
                action: PaletteAction::Service { project, service },
            });
        }
        let mut state = ListState::default();
        state.select(Some(0));
        self.palette = Some(Palette {
            context: self.palette_context_label(),
            query: String::new(),
            items,
            state,
            rect: Rect::default(),
        });
    }

    /// Run the selected palette entry (from the FILTERED list), then close.
    pub(super) fn palette_run(&mut self, req: &Sender<Req>) {
        let Some(pal) = self.palette.take() else {
            return;
        };
        let matches = pal.matches();
        let Some(&item_idx) = pal.state.selected().and_then(|i| matches.get(i)) else {
            // Enter on a query that matches nothing used to close the palette in
            // silence — indistinguishable from having run something.
            self.status = format!("Nothing matches '{}'", pal.query);
            return;
        };
        match &pal.items[item_idx].action {
            PaletteAction::Tab(s) => self.goto(*s, req),
            PaletteAction::Service { project, service } => {
                let (p, s) = (project.clone(), service.clone());
                self.jump_to_service(&p, &s, req);
            }
            PaletteAction::Run(run) => run(self, req),
        }
    }

    /// Switch to Services and highlight this service (quick navigation from the palette).
    fn jump_to_service(&mut self, project: &str, service: &str, req: &Sender<Req>) {
        self.goto(Screen::Projects, req);
        self.filter.clear();
        self.filter_input = false;
        let idx = self.visible_rows().iter().position(|r| {
            matches!(r, Line2::Service(s)
                if field(s, "/projectName") == project && field(s, "/name") == service)
        });
        match idx {
            Some(i) => {
                self.services_table.select(Some(i));
                self.status = format!("→ {project}/{service}");
            }
            None => self.status = format!("{project}/{service} isn't in the current list"),
        }
    }
}
