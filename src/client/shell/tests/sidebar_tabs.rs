use super::*;

fn tab(number: usize, workspace_id: &str, focused: bool) -> ClientShellTab {
    ClientShellTab {
        tab_id: format!("tab_{number}"),
        workspace_id: workspace_id.into(),
        number,
        label: number.to_string(),
        custom_label: false,
        zoomed: false,
        focused,
        agent_status: AgentStatus::Idle,
    }
}

fn agent(pane_id: &str) -> crate::protocol::ClientShellAgent {
    crate::protocol::ClientShellAgent {
        pane_id: pane_id.into(),
        workspace_id: "ws_1".into(),
        tab_id: "tab_1".into(),
        name: Some(pane_id.into()),
        display_agent: None,
        agent: Some("codex".into()),
        title: None,
        terminal_title: None,
        terminal_title_stripped: None,
        agent_status: AgentStatus::Idle,
        state_change_seq: 0,
        state_labels: Vec::new(),
        tokens: Vec::new(),
        focused: false,
    }
}

fn updated_surface(revision: u64) -> PaneSurfaceFrame {
    let mut next = surface();
    next.projection_revision = revision;
    next
}

#[test]
fn sidebar_tabs_render_between_spaces_and_agents() {
    let mut projected = snapshot();
    projected.tabs.push(tab(2, "ws_1", false));
    projected.tabs.push(tab(3, "ws_1", false));
    let mut state = ClientShellState::new(vertical_tabs_config());
    state.set_snapshot(Box::new(projected));
    state.set_pane_surface(surface());
    let frame = state.compose(106, 20).expect("sidebar tabs");

    let layout = state.layout(106, 20);
    assert!(layout.sidebar_tabs);
    assert_eq!(layout.tab_bar.height, 0, "the desktop tab strip is removed");
    assert_eq!(layout.pane_surface.y, 0, "panes keep the freed row");

    assert!(state.hits.tabs_vertical);
    let spaces = state.hits.workspaces[0].rect;
    let first_tab = state.hits.tabs[0].0;
    let last_tab = state.hits.tabs[2].0;
    assert!(spaces.bottom() <= first_tab.y, "tabs sit below spaces");
    assert_eq!(state.hits.tabs.len(), 3);
    assert!(
        state.hits.agent_body.y > last_tab.bottom(),
        "agents sit below the tab list"
    );

    // The upper area splits in half; the agent panel keeps the original split.
    // Tab count zero gives the two-section split every other assertion started from.
    let unsplit = crate::ui::expanded_sidebar_tabs_sections(
        layout.sidebar,
        state.sidebar_section_split,
        0,
        default_order(),
    );
    let (original_spaces, original_agents) = (unsplit.primary, unsplit.agents);
    let splits = crate::ui::expanded_sidebar_tabs_sections(
        layout.sidebar,
        state.sidebar_section_split,
        3,
        default_order(),
    );
    assert_eq!(splits.primary.height, 5);
    assert_eq!(splits.tabs.expect("tab section").height, 5);
    assert_eq!(splits.tabs.expect("tab section").y, splits.primary.bottom());
    assert_eq!(splits.agents, original_agents);
    assert_eq!(state.hits.new_workspace.y, splits.primary.bottom() - 1);
    assert_eq!(state.hits.tabs_body.y, splits.primary.bottom() + 2);
    assert_eq!(state.hits.tabs_body.bottom(), original_spaces.bottom());
    assert_eq!(
        state.hits.agent_body,
        Rect::new(
            original_agents.x,
            original_agents.y + 3,
            original_agents.width,
            original_agents.height - 3,
        ),
        "the agent panel body keeps its original rectangle"
    );

    let text = frame
        .cells
        .chunks(frame.width as usize)
        .map(|row| {
            row.iter()
                .map(|cell| cell.symbol.as_str())
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n");
    assert!(text.contains(" spaces"));
    assert!(text.contains(" tabs"));
    assert!(text.contains(" agents"));
    assert!(
        !state.hits.new_tab.is_empty(),
        "the header keeps its + control"
    );
    assert_eq!(
        state.hits.new_tab.y,
        first_tab.y - 1,
        "on the tabs header row"
    );
}

#[test]
fn sidebar_tabs_split_keeps_the_agent_panel_hits() {
    let mut projected = snapshot();
    for index in 1..=12 {
        projected.agents.push(agent(&format!("agent-pane-{index}")));
    }
    // With a single hidden tab there is no tab chrome at all, so the sidebar
    // shows the original two-section split.
    let mut config = vertical_tabs_config();
    config.hide_tab_bar_when_single_tab = true;
    let mut state = ClientShellState::new(config);
    state.set_snapshot(Box::new(projected));
    state.set_pane_surface(surface());
    state.compose(106, 20).expect("no tab section");
    assert!(!state.hits.tabs_vertical);
    let agents = state.hits.agents.clone();
    let agent_body = state.hits.agent_body;
    let spaces_footer = state.hits.new_workspace.y;
    assert!(agent_body.height > 0);

    let mut update = (**state.snapshot.as_ref().expect("snapshot")).clone();
    update.revision = 2;
    for number in 2..=4 {
        update.tabs.push(tab(number, "ws_1", false));
    }
    state.set_snapshot(Box::new(update));
    state.set_pane_surface(updated_surface(2));
    state.compose(106, 20).expect("sidebar tab section");

    assert!(state.hits.tabs_vertical);
    assert_eq!(
        state.hits.agents, agents,
        "agent rows do not move or shrink"
    );
    assert_eq!(state.hits.agent_body, agent_body);
    assert!(
        state.hits.new_workspace.y < spaces_footer,
        "the split comes out of the spaces area"
    );
    assert_eq!(
        state.hits.tabs_body.bottom(),
        spaces_footer + 1,
        "tabs end where the spaces area used to"
    );
}

#[test]
fn sidebar_tabs_fall_back_below_the_upper_split_threshold() {
    let mut state = ClientShellState::new(vertical_tabs_config());
    state.set_snapshot(Box::new(snapshot()));
    state.set_pane_surface(surface());

    // 13 rows: the upper area is 7 rows and splits into 4 + 3.
    state.compose(106, 13).expect("split upper area");
    let layout = state.layout(106, 13);
    assert!(layout.sidebar_tabs);
    let splits = crate::ui::expanded_sidebar_tabs_sections(
        layout.sidebar,
        state.sidebar_section_split,
        1,
        default_order(),
    );
    assert_eq!(splits.primary.height, 4);
    assert_eq!(splits.tabs.expect("tab section").height, 3);

    // 12 rows: the upper area is 6 rows, too small to split.
    state.set_pane_surface(surface());
    state.compose(106, 12).expect("unsplittable upper area");
    let layout = state.layout(106, 12);
    assert!(!layout.sidebar_tabs);
    assert_eq!(layout.tab_bar.height, 1, "the strip returns");
    assert!(!state.hits.tabs_vertical);
    assert_eq!(state.hits.tabs[0].0.y, layout.tab_bar.y);
    let original_spaces = crate::ui::expanded_sidebar_tabs_sections(
        layout.sidebar,
        state.sidebar_section_split,
        0,
        default_order(),
    )
    .primary;
    assert_eq!(
        state.hits.new_workspace.y,
        original_spaces.bottom() - 1,
        "the spaces section keeps its original rows"
    );
}

#[test]
fn sidebar_tab_scrollbar_drag_and_click_after_scroll_use_visible_rows() {
    let mut projected = snapshot();
    for number in 2..=9 {
        projected.tabs.push(tab(number, "ws_1", false));
    }
    let mut state = ClientShellState::new(vertical_tabs_config());
    state.set_snapshot(Box::new(projected));
    state.set_pane_surface(surface());
    state.compose(106, 20).expect("overflowing tab list");
    assert!(state.hits.tabs_max_scroll > 0);
    assert!(state.hits.tabs_scrollbar.width > 0);

    let track = state.hits.tabs_scrollbar;
    let metrics = state.hits.tabs_scroll_metrics.expect("tab metrics");
    let thumb = crate::ui::scrollbar_thumb(metrics, track).expect("tab scrollbar thumb");
    let dragged =
        state.handle_raw_events(vec![RawInputEvent::Mouse(crossterm::event::MouseEvent {
            kind: MouseEventKind::Drag(MouseButton::Left),
            column: track.x,
            row: track.bottom().saturating_sub(1),
            modifiers: KeyModifiers::empty(),
        })]);
    assert!(!dragged.repaint, "a drag without a grab does nothing");
    state.handle_raw_events(vec![RawInputEvent::Mouse(MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: track.x,
        row: thumb.top,
        modifiers: KeyModifiers::empty(),
    })]);
    let dragged = state.handle_raw_events(vec![RawInputEvent::Mouse(MouseEvent {
        kind: MouseEventKind::Drag(MouseButton::Left),
        column: track.x,
        row: track.bottom().saturating_sub(1),
        modifiers: KeyModifiers::empty(),
    })]);
    assert!(dragged.repaint);
    assert_eq!(state.tab_scroll, state.hits.tabs_max_scroll);

    let frame = state.compose(106, 20).expect("scrolled tab list");
    let visible = state.hits.tabs.clone();
    let (row, tab_id) = visible.first().expect("visible tab row").clone();
    let outcome = state.handle_raw_events(vec![RawInputEvent::Mouse(MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: row.x + 1,
        row: row.y,
        modifiers: KeyModifiers::empty(),
    })]);
    assert!(outcome.actions.is_empty(), "press waits for release");
    let clicked = state.handle_raw_events(vec![RawInputEvent::Mouse(MouseEvent {
        kind: MouseEventKind::Up(MouseButton::Left),
        column: row.x + 1,
        row: row.y,
        modifiers: KeyModifiers::empty(),
    })]);
    assert!(
        matches!(
            &clicked.actions[..],
            [ClientShellAction::Endpoint { request, .. }]
                if matches!(&request.method, crate::api::schema::Method::TabFocus(target) if target.tab_id == tab_id)
        ),
        "the row under the pointer focuses its own tab after scrolling"
    );
    let _ = frame;
}

#[test]
fn focused_tab_reveal_keeps_manual_scroll_between_renders() {
    let mut projected = snapshot();
    for number in 2..=9 {
        projected.tabs.push(tab(number, "ws_1", false));
    }
    let mut state = ClientShellState::new(vertical_tabs_config());
    state.set_snapshot(Box::new(projected));
    state.set_pane_surface(surface());
    state.compose(106, 20).expect("revealed first tab");
    assert_eq!(state.tab_scroll, 0);
    let max_scroll = state.hits.tabs_max_scroll;

    // A manual scroll survives renders that do not change the tab list.
    state.tab_scroll = max_scroll;
    state.compose(106, 20).expect("manual scroll");
    assert_eq!(state.tab_scroll, max_scroll);
    state.compose(106, 20).expect("same state");
    assert_eq!(state.tab_scroll, max_scroll);

    // Focusing a tab above the viewport reveals exactly that tab.
    let mut update = (**state.snapshot.as_ref().expect("snapshot")).clone();
    update.revision = 2;
    update.focused_tab_id = Some("tab_2".into());
    for entry in &mut update.tabs {
        entry.focused = entry.tab_id == "tab_2";
    }
    state.set_snapshot(Box::new(update));
    state.set_pane_surface(updated_surface(2));
    state.compose(106, 20).expect("revealed focus change");
    assert!(state.hits.tabs.iter().any(|(_, tab_id)| tab_id == "tab_2"));
    assert!(state.tab_scroll < max_scroll);
}

#[test]
fn workspace_switch_rebinds_the_sidebar_tab_list() {
    let mut projected = snapshot();
    for number in 2..=6 {
        projected.tabs.push(tab(number, "ws_1", false));
    }
    let mut second = projected.workspaces[0].clone();
    second.workspace_id = "ws_2".into();
    second.number = 2;
    second.label = "other".into();
    second.focused = false;
    second.active_tab_id = "tab_9".into();
    projected.workspaces.push(second);
    for number in 6..=9 {
        projected.tabs.push(tab(number, "ws_2", number == 9));
    }
    projected.tabs[0].focused = false;
    let mut state = ClientShellState::new(vertical_tabs_config());
    state.set_snapshot(Box::new(projected));
    state.set_pane_surface(surface());
    state.compose(106, 20).expect("first workspace tabs");
    assert_eq!(state.hits.tabs.len(), 3, "only the visible rows are listed");
    assert!(state.hits.tabs.iter().all(|(_, tab_id)| tab_id != "tab_9"));
    assert!(state.hits.tabs_max_scroll > 0);
    state.tab_scroll = 2;

    let mut update = (**state.snapshot.as_ref().expect("snapshot")).clone();
    update.revision = 2;
    update.focused_workspace_id = Some("ws_2".into());
    update.focused_tab_id = Some("tab_9".into());
    for workspace in &mut update.workspaces {
        workspace.focused = workspace.workspace_id == "ws_2";
    }
    state.set_snapshot(Box::new(update));
    state.set_pane_surface(updated_surface(2));
    state.compose(106, 20).expect("second workspace tabs");

    assert_eq!(
        state.tab_scroll, 1,
        "the ws_1 offset is not carried over; the new active tab is revealed"
    );
    assert_eq!(state.hits.tabs.len(), 3);
    assert!(state
        .hits
        .tabs
        .iter()
        .all(|(_, tab_id)| tab_id.starts_with("tab_") && tab_id != "tab_1"));
    assert!(
        state.hits.tabs.iter().any(|(_, tab_id)| tab_id == "tab_9"),
        "the selected workspace's active tab is revealed"
    );
}

#[test]
fn tab_close_clamps_the_sidebar_tab_scroll() {
    let mut projected = snapshot();
    for number in 2..=9 {
        projected.tabs.push(tab(number, "ws_1", false));
    }
    let mut state = ClientShellState::new(vertical_tabs_config());
    state.set_snapshot(Box::new(projected));
    state.set_pane_surface(surface());
    state.compose(106, 20).expect("overflowing tab list");
    state.tab_scroll = state.hits.tabs_max_scroll;
    state.compose(106, 20).expect("scrolled tab list");
    assert!(state.tab_scroll > 0);

    let mut update = (**state.snapshot.as_ref().expect("snapshot")).clone();
    update.revision = 2;
    update.tabs.truncate(3);
    state.set_snapshot(Box::new(update));
    state.set_pane_surface(updated_surface(2));
    state.compose(106, 20).expect("closed tabs");
    assert_eq!(state.tab_scroll, 0);
    assert_eq!(state.hits.tabs.len(), 3);
    assert!(state.hits.tabs_scrollbar.is_empty());
}

#[test]
fn sidebar_tabs_fall_back_to_the_strip_when_unusable() {
    // Short terminal: the sidebar cannot show spaces, tabs, and agents together.
    let mut state = ClientShellState::new(vertical_tabs_config());
    state.set_snapshot(Box::new(snapshot()));
    state.set_pane_surface(surface());
    state.compose(106, 11).expect("short terminal");
    let layout = state.layout(106, 11);
    assert!(!layout.sidebar_tabs);
    assert_eq!(layout.tab_bar.height, 1, "the strip returns");
    assert_eq!(layout.pane_surface.y, 1);
    assert!(!state.hits.tabs_vertical);
    assert_eq!(
        state.hits.tabs[0].0.y, layout.tab_bar.y,
        "horizontal tab row"
    );

    // Hiding the sidebar restores the strip on a tall terminal.
    use_horizontal_tab_strip(&mut state);
    state.set_pane_surface(surface());
    state.compose(106, 20).expect("hidden sidebar");
    let layout = state.layout(106, 20);
    assert!(!layout.sidebar_tabs);
    assert_eq!(layout.sidebar.width, 0);
    assert_eq!(layout.tab_bar.height, 1);
    assert_eq!(state.hits.tabs[0].0.y, 0);

    // Mobile keeps its own header and tab switcher.
    state.set_pane_surface(surface());
    state.compose(44, 20).expect("mobile header");
    assert!(state.hits.tabs.is_empty());
    assert!(state.hits.tab_scroll_left.is_empty());
}

#[test]
fn sidebar_tabs_keep_agents_and_the_status_strip() {
    let mut projected = snapshot();
    projected.tab_bar_right = vec![crate::protocol::ClientShellTabStatusSegment {
        text: "ZOOM".into(),
        accent: true,
    }];
    projected.agents.push(agent("agent-pane-1"));
    projected.agents.push(agent("agent-pane-2"));
    let mut state = ClientShellState::new(vertical_tabs_config());
    state.set_snapshot(Box::new(projected));
    state.set_pane_surface(surface());
    let frame = state
        .compose(106, 20)
        .expect("status strip with sidebar tabs");

    let layout = state.layout(106, 20);
    assert!(layout.sidebar_tabs);
    assert_eq!(layout.tab_bar.height, 1, "status segments keep their strip");
    let top = frame.cells[..frame.width as usize]
        .iter()
        .map(|cell| cell.symbol.as_str())
        .collect::<String>();
    assert!(top.contains("ZOOM"));
    assert!(state.hits.tabs.iter().all(|(rect, _)| rect.y > 0));

    let agent_row = state.hits.agents[1].0;
    assert!(agent_row.y > state.hits.tabs.last().expect("tab rows").0.y);
    let outcome = state.handle_raw_events(vec![RawInputEvent::Mouse(MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: agent_row.x + 2,
        row: agent_row.y,
        modifiers: KeyModifiers::empty(),
    })]);
    assert!(matches!(
        &outcome.actions[..],
        [ClientShellAction::Endpoint { request, .. }]
            if matches!(&request.method, crate::api::schema::Method::PaneFocus(target) if target.pane_id == "agent-pane-2")
    ));
}

#[test]
fn sidebar_scroll_areas_stay_independent() {
    let mut projected = snapshot();
    for number in 2..=6 {
        projected.tabs.push(tab(number, "ws_1", false));
    }
    let template = projected.workspaces[0].clone();
    projected.workspaces = (1..=12)
        .map(|number| ClientShellWorkspace {
            workspace_id: format!("ws_{number}"),
            number,
            label: format!("space-{number}"),
            branch: None,
            focused: number == 1,
            ..template.clone()
        })
        .collect();
    for index in 1..=10 {
        projected.agents.push(agent(&format!("agent-pane-{index}")));
    }
    let mut state = ClientShellState::new(vertical_tabs_config());
    state.set_snapshot(Box::new(projected));
    state.set_pane_surface(surface());
    state.compose(106, 20).expect("three scrolling lists");
    assert!(state.hits.workspace_max_scroll > 0);
    assert!(state.hits.agent_max_scroll > 0);
    assert!(state.hits.tabs_max_scroll > 0);

    let wheel = |state: &mut ClientShellState, rect: Rect| {
        state.handle_raw_events(vec![RawInputEvent::Mouse(MouseEvent {
            kind: MouseEventKind::ScrollDown,
            column: rect.x,
            row: rect.y,
            modifiers: KeyModifiers::empty(),
        })])
    };
    let spaces = state.hits.workspace_body;
    let agents = state.hits.agent_body;
    let tabs = state.hits.tabs_body;

    wheel(&mut state, spaces);
    assert_eq!(
        (state.workspace_scroll, state.agent_scroll, state.tab_scroll),
        (1, 0, 0)
    );
    wheel(&mut state, agents);
    assert_eq!(
        (state.workspace_scroll, state.agent_scroll, state.tab_scroll),
        (1, 1, 0)
    );
    wheel(&mut state, tabs);
    assert_eq!(
        (state.workspace_scroll, state.agent_scroll, state.tab_scroll),
        (1, 1, 1)
    );
    wheel(&mut state, spaces);
    wheel(&mut state, agents);
    assert_eq!(
        (state.workspace_scroll, state.agent_scroll, state.tab_scroll),
        (2, 2, 1)
    );
}

#[test]
fn tab_hit_metrics_reset_when_the_vertical_list_hides_and_returns() {
    let mut projected = snapshot();
    for number in 2..=9 {
        projected.tabs.push(tab(number, "ws_1", false));
    }
    let mut state = ClientShellState::new(vertical_tabs_config());
    state.set_snapshot(Box::new(projected));
    state.set_pane_surface(surface());
    state.compose(106, 20).expect("vertical tab list");
    state.tab_scroll = 2;
    state.compose(106, 20).expect("scrolled tab list");
    assert!(state.hits.tabs_vertical);
    assert!(state.hits.tabs_max_scroll > 0);
    assert!(!state.hits.tabs_body.is_empty());
    assert!(state.hits.tabs_scroll_metrics.is_some());

    // A short terminal hides the vertical list: no stale metrics survive.
    state.set_pane_surface(surface());
    state.compose(106, 12).expect("fallback strip");
    assert!(!state.hits.tabs_vertical);
    assert!(state.hits.tabs_body.is_empty());
    assert!(state.hits.tabs_scrollbar.is_empty());
    assert_eq!(state.hits.tabs_max_scroll, 0);
    assert!(state.hits.tabs_scroll_metrics.is_none());
    assert_eq!(state.hits.tabs[0].0.y, 0, "the strip owns the tab rows");

    // Back to a tall terminal: the list returns, clamped and revealed.
    state.set_pane_surface(surface());
    state.compose(106, 20).expect("vertical tab list again");
    assert!(state.hits.tabs_vertical);
    assert!(state.hits.tabs_max_scroll > 0);
    assert!(state.tab_scroll <= state.hits.tabs_max_scroll);
    assert!(
        state.hits.tabs.iter().any(|(_, tab_id)| tab_id == "tab_1"),
        "the focused tab is revealed again"
    );
}

#[test]
fn tab_hit_metrics_clear_when_mouse_capture_is_disabled() {
    let mut projected = snapshot();
    for number in 2..=9 {
        projected.tabs.push(tab(number, "ws_1", false));
    }
    let mut state = ClientShellState::new(vertical_tabs_config());
    state.set_snapshot(Box::new(projected));
    state.set_pane_surface(surface());
    state.compose(106, 20).expect("vertical tab list");
    assert!(state.hits.tabs_max_scroll > 0);

    state.config.mouse_capture = false;
    state.set_pane_surface(surface());
    state.compose(106, 20).expect("mouse capture disabled");
    assert!(!state.hits.tabs_vertical);
    assert!(state.hits.tabs_body.is_empty());
    assert!(state.hits.tabs_scrollbar.is_empty());
    assert_eq!(state.hits.tabs_max_scroll, 0);
    assert!(state.hits.tabs_scroll_metrics.is_none());
    assert!(state.hits.new_tab.is_empty());
}

#[test]
fn next_tab_keybind_still_switches_tabs_with_the_sidebar_tab_list() {
    let mut projected = snapshot();
    projected.tabs.push(tab(2, "ws_1", false));
    let mut state = ClientShellState::new(vertical_tabs_config());
    state.set_snapshot(Box::new(projected));
    state.set_pane_surface(surface());
    state.compose(106, 20).expect("sidebar tabs");
    assert!(state.hits.tabs_vertical);

    let mut outcome = ClientShellInput::default();
    state.record_binding(
        crate::input::KeybindMatch::Action(crate::input::KeybindAction::NextTab),
        &mut outcome,
    );
    assert!(matches!(
        &outcome.actions[..],
        [ClientShellAction::Endpoint { request, .. }]
            if matches!(&request.method, crate::api::schema::Method::TabFocus(target) if target.tab_id == "tab_2")
    ));
}

/// Restores an environment variable when the test ends.
struct EnvVarRestore(&'static str);

impl EnvVarRestore {
    fn set(name: &'static str, value: &str) -> Self {
        std::env::set_var(name, value);
        Self(name)
    }
}

impl Drop for EnvVarRestore {
    fn drop(&mut self) {
        std::env::remove_var(self.0);
    }
}

fn ordered_state(order: [crate::config::SidebarSectionConfig; 3]) -> ClientShellState {
    let mut projected = snapshot();
    projected.tabs.push(tab(2, "ws_1", false));
    projected.tabs.push(tab(3, "ws_1", false));
    projected.agents.push(agent("agent-pane-1"));
    let mut config = vertical_tabs_config();
    config.sidebar_section_order = order;
    let mut state = ClientShellState::new(config);
    state.set_snapshot(Box::new(projected));
    state.set_pane_surface(surface());
    state
}

/// Press on the section divider and drag it `rows` rows down.
/// Returns (split before, split after, row offset, sidebar height, row above kind).
fn drag_sidebar_divider(
    order: [crate::config::SidebarSectionConfig; 3],
    rows: u16,
) -> (f32, f32, u16, u16, crate::ui::SidebarSectionAbove) {
    let mut state = ordered_state(order);
    state.compose(106, 20).expect("ordered sidebar");
    let before = state.sidebar_section_split;
    let divider = state.hits.sidebar_section_divider;
    assert!(!divider.is_empty(), "{order:?}: the divider row is visible");
    let column = divider.x + 1;
    let row = divider.y;
    state.handle_raw_events(vec![RawInputEvent::Mouse(MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column,
        row,
        modifiers: KeyModifiers::empty(),
    })]);
    let dragged = state.handle_raw_events(vec![RawInputEvent::Mouse(MouseEvent {
        kind: MouseEventKind::Drag(MouseButton::Left),
        column,
        row: row + rows,
        modifiers: KeyModifiers::empty(),
    })]);
    assert!(dragged.repaint, "{order:?}: the drag resizes the sections");
    let sheet = state.hits.sidebar_divider;
    let above = row + rows - sheet.y;
    (
        before,
        state.sidebar_section_split,
        above,
        sheet.height,
        state.hits.sidebar_section_above,
    )
}

#[test]
fn default_settings_keep_the_horizontal_strip_and_the_spaces_then_agents_sidebar() {
    let mut projected = snapshot();
    projected.tabs.push(tab(2, "ws_1", false));
    projected.agents.push(agent("agent-pane-1"));
    let mut state = ClientShellState::new(ClientShellConfig::from_config(&Config::default()));
    assert_eq!(
        state.config.tabs_orientation,
        crate::config::TabsOrientationConfig::Horizontal
    );
    assert_eq!(state.config.sidebar_section_order, default_order());
    state.set_snapshot(Box::new(projected));
    state.set_pane_surface(surface());
    state.compose(106, 20).expect("default strip");

    let layout = state.layout(106, 20);
    assert!(!layout.sidebar_tabs, "the default tab list is the strip");
    assert_eq!(layout.tab_bar.height, 1);
    assert!(!state.hits.tabs_vertical);
    assert!(state.hits.tabs.iter().all(|(rect, _)| rect.y == 0));
    assert!(
        state.hits.agent_body.y > state.hits.workspaces[0].rect.y,
        "agents stay below the spaces list"
    );
    assert_eq!(
        state.hits.sidebar_section_above,
        crate::ui::SidebarSectionAbove::Group
    );
    let sections = crate::ui::expanded_sidebar_tabs_sections(
        layout.sidebar,
        state.sidebar_section_split,
        0,
        default_order(),
    );
    assert_eq!(state.hits.sidebar_section_divider, sections.divider);
    assert_eq!(state.hits.agent_body.y, sections.agents.y + 3);
    assert_eq!(state.hits.new_workspace.y, sections.primary.bottom() - 1);
}

#[test]
fn sidebar_section_order_places_the_sections_and_keeps_the_named_heights() {
    for order in crate::config::SIDEBAR_SECTION_ORDERS {
        let mut state = ordered_state(order);
        state.compose(106, 20).expect("ordered sidebar");

        let layout = state.layout(106, 20);
        assert!(layout.sidebar_tabs, "{order:?}: vertical tabs stay enabled");
        let unsplit = crate::ui::expanded_sidebar_tabs_sections(
            layout.sidebar,
            state.sidebar_section_split,
            0,
            order,
        );
        let sections = crate::ui::expanded_sidebar_tabs_sections(
            layout.sidebar,
            state.sidebar_section_split,
            3,
            order,
        );
        let tabs = sections.tabs.expect("tab section");
        assert_eq!(
            sections.agents.height, unsplit.agents.height,
            "{order:?}: the agent panel keeps its named allocation"
        );
        if order[2] == crate::config::SidebarSectionConfig::Agents {
            assert_eq!(
                sections.agents, unsplit.agents,
                "{order:?}: an agent panel below the whole group keeps its rect"
            );
        }
        assert_eq!(
            sections.primary.height + tabs.height,
            unsplit.primary.height,
            "{order:?}: the tab list splits only the spaces allocation"
        );

        let rect_of = |section: crate::config::SidebarSectionConfig| match section {
            crate::config::SidebarSectionConfig::Spaces => sections.primary,
            crate::config::SidebarSectionConfig::Tabs => tabs,
            crate::config::SidebarSectionConfig::Agents => sections.agents,
        };
        let mut expected_y = layout.sidebar.y;
        for section in order {
            let rect = rect_of(section);
            assert_eq!(
                rect.y, expected_y,
                "{order:?}: {section:?} follows the order"
            );
            assert!(rect.height > 0, "{order:?}: {section:?} keeps rows");
            expected_y = rect.bottom();
        }
        assert_eq!(
            expected_y,
            layout.sidebar.bottom(),
            "{order:?}: the sections fill the sidebar"
        );

        // Render and hit testing agree with the shared geometry.
        assert_eq!(state.hits.tabs_body.y, tabs.y + 2, "{order:?}");
        assert_eq!(state.hits.tabs_body.bottom(), tabs.bottom(), "{order:?}");
        assert_eq!(state.hits.agent_body.y, sections.agents.y + 3, "{order:?}");
        assert_eq!(
            state.hits.new_workspace.y,
            sections.primary.bottom() - 1,
            "{order:?}: the spaces footer keeps its rows"
        );
        assert_eq!(
            state.hits.sidebar_section_divider, sections.divider,
            "{order:?}"
        );
        assert_eq!(
            state.hits.sidebar_section_above, sections.divider_above,
            "{order:?}"
        );

        // The section header row belongs to the block above the agent panel.
        assert_eq!(
            state.hits.new_tab.y,
            tabs.y + 1,
            "{order:?}: the `+` control stays on the tabs header"
        );
    }
}

#[test]
fn sidebar_section_divider_drag_follows_the_visible_boundary() {
    use crate::config::SidebarSectionConfig::{Agents, Spaces, Tabs};
    use crate::ui::SidebarSectionAbove;

    // Stock order: the agent panel sits below the group, so dragging down grows
    // the whole spaces/tabs group.
    let (before, after, above, height, kind) = drag_sidebar_divider(default_order(), 4);
    assert_eq!(kind, SidebarSectionAbove::Group);
    assert!((before - 0.5).abs() < 1e-6, "the split starts centered");
    let expected = (f32::from(above) / f32::from(height)).clamp(0.1, 0.9);
    assert!(
        (after - expected).abs() < 1e-6,
        "dragging down grows the group: {after} vs {expected}"
    );
    assert!(after > before);

    // Agents first: the group fills the rows below the divider, so dragging
    // down shrinks it.
    let (before, after, above, height, kind) = drag_sidebar_divider([Agents, Spaces, Tabs], 4);
    assert_eq!(kind, SidebarSectionAbove::Agents);
    let expected = (f32::from(height - above) / f32::from(height)).clamp(0.1, 0.9);
    assert!(
        (after - expected).abs() < 1e-6,
        "with agents on top the group is below the divider: {after} vs {expected}"
    );
    assert!(after < before);

    // A half boundary: the tab list sits above the divider, so the group grows
    // twice as fast as the pointer moves.
    let (before, after, above, height, kind) = drag_sidebar_divider([Tabs, Agents, Spaces], 4);
    assert_eq!(kind, SidebarSectionAbove::GroupHalf);
    let expected = ((2.0 * f32::from(above)) / f32::from(height)).clamp(0.1, 0.9);
    assert!(
        (after - expected).abs() < 1e-6,
        "a half boundary scales the group share: {after} vs {expected}"
    );
    assert!(after > before);
}

#[test]
fn horizontal_orientation_keeps_the_strip_and_orders_spaces_and_agents_only() {
    use crate::config::SidebarSectionConfig::{Agents, Spaces, Tabs};

    let mut config = ClientShellConfig::from_config(&Config::default());
    config.sidebar_section_order = [Agents, Spaces, Tabs];
    let mut projected = snapshot();
    projected.tabs.push(tab(2, "ws_1", false));
    projected.agents.push(agent("agent-pane-1"));
    let mut state = ClientShellState::new(config);
    state.set_snapshot(Box::new(projected));
    state.set_pane_surface(surface());
    state.compose(106, 20).expect("reordered strip sidebar");

    let layout = state.layout(106, 20);
    assert!(!layout.sidebar_tabs);
    assert_eq!(layout.tab_bar.height, 1);
    assert!(!state.hits.tabs_vertical);
    assert!(
        state.hits.tabs.iter().all(|(rect, _)| rect.y == 0),
        "the strip owns the tab rows"
    );

    let sections = crate::ui::expanded_sidebar_tabs_sections(
        layout.sidebar,
        state.sidebar_section_split,
        0,
        state.config.sidebar_section_order,
    );
    assert!(
        sections.tabs.is_none(),
        "no tab section without vertical tabs"
    );
    assert!(
        state.hits.agent_body.y < state.hits.workspaces[0].rect.y,
        "agents move above the spaces list"
    );
    assert_eq!(state.hits.agent_body.y, sections.agents.y + 3);
    assert_eq!(
        state.hits.new_workspace.y,
        sections.primary.bottom() - 1,
        "the spaces footer follows its block"
    );
    assert_eq!(state.hits.sidebar_section_divider, sections.divider);
    assert_eq!(
        state.hits.sidebar_section_divider.y, sections.primary.y,
        "the drag row sits above the spaces list"
    );
    assert_eq!(
        state.hits.sidebar_section_above,
        crate::ui::SidebarSectionAbove::Agents
    );

    // Tabs first is the same as spaces first while the strip owns the tabs.
    let mut config = ClientShellConfig::from_config(&Config::default());
    config.sidebar_section_order = [Tabs, Spaces, Agents];
    let mut state = ClientShellState::new(config);
    state.set_snapshot(Box::new(snapshot()));
    state.set_pane_surface(surface());
    state.compose(106, 20).expect("tabs-first strip sidebar");
    assert!(!state.layout(106, 20).sidebar_tabs);
    assert_eq!(
        state.hits.sidebar_section_above,
        crate::ui::SidebarSectionAbove::Group
    );
}

#[test]
fn settings_choices_persist_and_reload_the_tab_layout() {
    let _guard = crate::config::test_config_env_lock().lock().unwrap();
    let dir = std::env::temp_dir().join(format!(
        "herdr-shell-tab-settings-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("config.toml");
    let _env = EnvVarRestore::set(crate::config::CONFIG_PATH_ENV_VAR, &path.to_string_lossy());

    let mut projected = snapshot();
    for number in 2..=6 {
        projected.tabs.push(tab(number, "ws_1", false));
    }
    let mut state = ClientShellState::new(ClientShellConfig::from_config(&Config::default()));
    state.set_snapshot(Box::new(projected));
    state.set_pane_surface(surface());
    state.compose(106, 20).expect("default strip");
    assert!(!state.layout(106, 20).sidebar_tabs);

    // The overlay writes the vertical choice through the config file and
    // rebinds the layout on the reload.
    let mut outcome = ClientShellInput::default();
    state.open_settings_overlay();
    state.select_settings_section(ClientSettingsSection::Tabs, &mut outcome);
    let settings = state.compose(106, 30).expect("tabs settings section");
    let settings_text = settings
        .cells
        .iter()
        .map(|cell| cell.symbol.as_str())
        .collect::<String>();
    assert!(
        settings_text.contains("horizontal") && settings_text.contains("vertical"),
        "the tab list choices render"
    );
    assert!(
        settings_text.contains(" tabs ") && settings_text.contains(" sidebar "),
        "both new settings sections fit in the section row"
    );
    state.select_settings_choice(1);
    state.apply_settings_choice(&mut outcome);
    let written = std::fs::read_to_string(&path).expect("config written");
    let parsed: Config = toml::from_str(&written).unwrap();
    assert_eq!(
        parsed.ui.tabs_orientation,
        crate::config::TabsOrientationConfig::Vertical
    );
    assert_eq!(state.config.tabs_orientation, parsed.ui.tabs_orientation);
    state.set_pane_surface(surface());
    state.compose(106, 20).expect("vertical tab list");
    assert!(state.layout(106, 20).sidebar_tabs);
    assert!(state.hits.tabs_vertical);

    // The sidebar order choice persists and drives the stack.
    state.open_settings_overlay();
    state.select_settings_section(ClientSettingsSection::Sidebar, &mut outcome);
    state.select_settings_choice(4);
    state.apply_settings_choice(&mut outcome);
    let agents_first = [
        crate::config::SidebarSectionConfig::Agents,
        crate::config::SidebarSectionConfig::Spaces,
        crate::config::SidebarSectionConfig::Tabs,
    ];
    assert_eq!(state.config.sidebar_section_order, agents_first);
    let written = std::fs::read_to_string(&path).expect("config written");
    let parsed: Config = toml::from_str(&written).unwrap();
    assert_eq!(parsed.ui.sidebar_order(), Some(agents_first));
    state.set_pane_surface(surface());
    state.compose(106, 20).expect("reordered sidebar");
    assert!(
        state.hits.agent_body.y < state.hits.workspaces[0].rect.y,
        "the reloaded order moves the agent panel to the top"
    );

    // Switching the orientation back resets a manual tab scroll and reveals
    // the focused tab again.
    std::fs::write(
        &path,
        "[ui]\ntabs_orientation = \"horizontal\"\nsidebar_section_order = [\"agents\", \"spaces\", \"tabs\"]\n",
    )
    .unwrap();
    state.tab_scroll = 2;
    state.reveal_focused_tab = false;
    state.last_tab_viewport = None;
    state.reload_client_config();
    assert_eq!(
        state.config.tabs_orientation,
        crate::config::TabsOrientationConfig::Horizontal
    );
    assert_eq!(state.config.sidebar_section_order, agents_first);
    assert_eq!(state.tab_scroll, 0);
    assert!(state.reveal_focused_tab);
    assert!(state.last_tab_viewport.is_none());
    state.set_pane_surface(surface());
    state.compose(106, 20).expect("strip again");
    assert!(!state.layout(106, 20).sidebar_tabs);

    // An invalid order keeps the previous [ui] settings and reports a problem.
    std::fs::write(
        &path,
        "[ui]\ntabs_orientation = \"horizontal\"\nsidebar_section_order = [\"tabs\", \"tabs\", \"agents\"]\n",
    )
    .unwrap();
    state.reload_client_config();
    assert_eq!(state.config.sidebar_section_order, agents_first);
    assert!(
        state.local_config_diagnostic.is_some(),
        "an invalid order keeps the previous settings and warns"
    );
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn sidebar_tabs_follow_the_configured_single_tab_hidden_rule() {
    let mut config = vertical_tabs_config();
    config.hide_tab_bar_when_single_tab = true;
    let mut state = ClientShellState::new(config);
    state.set_snapshot(Box::new(snapshot()));
    state.set_pane_surface(surface());
    state.compose(106, 20).expect("single hidden tab");
    let layout = state.layout(106, 20);
    assert!(!layout.sidebar_tabs);
    assert_eq!(layout.tab_bar.height, 0);
    assert!(state.hits.tabs.is_empty());

    let mut update = (**state.snapshot.as_ref().expect("snapshot")).clone();
    update.revision = 2;
    update.tabs.push(tab(2, "ws_1", false));
    state.set_snapshot(Box::new(update));
    state.set_pane_surface(updated_surface(2));
    state.compose(106, 20).expect("two tabs");
    let layout = state.layout(106, 20);
    assert!(layout.sidebar_tabs);
    assert_eq!(state.hits.tabs.len(), 2);
    assert!(state.hits.tabs_vertical);
}

#[test]
fn sidebar_order_choices_stay_visible_and_mouse_reachable() {
    use crate::config::SIDEBAR_SECTION_ORDERS;

    let mut state = ClientShellState::new(vertical_tabs_config());
    state.set_snapshot(Box::new(snapshot()));
    state.set_pane_surface(surface());
    let mut outcome = ClientShellInput::default();
    state.open_settings_overlay();
    state.select_settings_section(ClientSettingsSection::Sidebar, &mut outcome);

    // Every permutation renders at the default terminal size.
    let frame = state.compose(106, 30).expect("settings popup");
    assert_eq!(
        state.hits.settings_choices.len(),
        SIDEBAR_SECTION_ORDERS.len(),
        "all six orders are reachable"
    );
    assert_eq!(
        state
            .hits
            .settings_choices
            .iter()
            .map(|(_, index)| *index)
            .collect::<Vec<_>>(),
        (0..SIDEBAR_SECTION_ORDERS.len()).collect::<Vec<_>>()
    );
    let text = frame
        .cells
        .iter()
        .map(|cell| cell.symbol.as_str())
        .collect::<String>();
    for label in [
        "spaces · tabs · agents",
        "spaces · agents · tabs",
        "tabs · spaces · agents",
        "tabs · agents · spaces",
        "agents · spaces · tabs",
        "agents · tabs · spaces",
    ] {
        assert!(text.contains(label), "missing choice {label}");
    }

    // The default 20-row terminal still fits all six rows.
    let frame = state.compose(106, 20).expect("default settings popup");
    assert_eq!(
        state.hits.settings_choices.len(),
        SIDEBAR_SECTION_ORDERS.len()
    );
    assert!(frame_text(&frame).contains("agents · tabs · spaces"));

    // The last row is clickable and only selects: this section still applies on
    // the apply button.
    let (last, last_index) = *state.hits.settings_choices.last().expect("last choice");
    assert_eq!(last_index, SIDEBAR_SECTION_ORDERS.len() - 1);
    let clicked = state.handle_raw_events(vec![RawInputEvent::Mouse(MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: last.x + 2,
        row: last.y,
        modifiers: KeyModifiers::empty(),
    })]);
    assert!(clicked.repaint);
    assert!(
        matches!(
            state.overlay.as_ref(),
            Some(ClientShellOverlay::Settings(settings)) if settings.selected == last_index
        ),
        "clicking the last row selects it"
    );
    assert_eq!(
        state.config.sidebar_section_order,
        crate::config::SidebarSectionConfig::DEFAULT_ORDER,
        "a click only selects; the apply button still writes the choice"
    );

    // A short terminal windows the list so the selected row stays visible.
    fn frame_text(frame: &crate::client::frame_output::ComposedFrame) -> String {
        frame
            .cells
            .iter()
            .map(|cell| cell.symbol.as_str())
            .collect::<String>()
    }
    let frame = state.compose(106, 16).expect("short settings popup");
    let text = frame_text(&frame);
    assert!(
        text.contains("agents · tabs · spaces"),
        "the selected order stays visible on a short terminal"
    );
    let visible = state
        .hits
        .settings_choices
        .iter()
        .map(|(_, index)| *index)
        .collect::<Vec<_>>();
    assert!(visible.contains(&last_index), "{visible:?}");
    assert!(!visible.contains(&0), "{visible:?}");

    state.select_settings_choice(0);
    let frame = state.compose(106, 16).expect("short settings popup");
    let text = frame_text(&frame);
    assert!(text.contains("spaces · tabs · agents"));
    assert_eq!(
        state.hits.settings_choices.first().map(|(_, index)| *index),
        Some(0)
    );
    assert!(!state
        .hits
        .settings_choices
        .iter()
        .any(|(_, index)| *index == last_index));
}
