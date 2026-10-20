use super::*;

use crate::client::shell::agent_sidebar::{AgentIdentity, AgentRow, OPENCODE_TABS_TOKEN};
use crate::protocol::ClientShellAgent;

const OWNER: &str = "0123456789abcdef0123456789abcdef";

fn opencode_agent(tokens: Vec<(String, String)>, focused: bool) -> ClientShellAgent {
    ClientShellAgent {
        pane_id: "pane_1".into(),
        workspace_id: "ws_1".into(),
        tab_id: "tab_1".into(),
        name: Some("opencode".into()),
        display_agent: Some("OpenCode".into()),
        agent: Some("opencode".into()),
        title: None,
        terminal_title: None,
        terminal_title_stripped: None,
        agent_status: AgentStatus::Idle,
        state_change_seq: 0,
        state_labels: Vec::new(),
        tokens,
        focused,
    }
}

fn snapshot_with_agent(tokens: Vec<(String, String)>) -> ClientShellSnapshot {
    let mut projected = snapshot();
    projected.focused_pane_id = Some("pane_1".into());
    projected.panes[0].focused = true;
    projected.agents.push(opencode_agent(tokens, true));
    projected
}

fn tabs_token(tabs: &str) -> String {
    format!(r#"{{"v":1,"owner":"{OWNER}","tabs":{tabs}}}"#)
}

fn tab(session: &str, title: &str, active: bool, busy: bool, attention: bool) -> String {
    format!(
        r#"{{"sessionID":"{session}","title":"{title}","active":{active},"busy":{busy},"attention":{attention},"unread":false}}"#
    )
}

fn rows(projected: &ClientShellSnapshot) -> Vec<AgentRow> {
    rows_with(
        projected,
        &ClientShellConfig::from_config(&Config::default()),
    )
}

fn rows_with(projected: &ClientShellSnapshot, config: &ClientShellConfig) -> Vec<AgentRow> {
    crate::client::shell::agent_sidebar::agent_rows(projected, config, None)
}

fn scaled_snapshot(panes: usize, tabs_per_pane: usize) -> ClientShellSnapshot {
    let mut projected = snapshot();
    projected.agents.clear();
    projected.panes.clear();
    projected.tabs.clear();
    let tabs = (0..tabs_per_pane)
        .map(|tab_index| {
            tab(
                &format!("ses_{tab_index}"),
                &format!("tab {tab_index}"),
                tab_index == 0,
                false,
                false,
            )
        })
        .collect::<Vec<_>>()
        .join(",");
    for index in 0..panes {
        let pane_id = format!("pane_{index}");
        let tab_id = format!("tab_{index}");
        projected.tabs.push(ClientShellTab {
            tab_id: tab_id.clone(),
            workspace_id: "ws_1".into(),
            number: index,
            label: index.to_string(),
            custom_label: false,
            zoomed: false,
            focused: index == 0,
            agent_status: AgentStatus::Idle,
        });
        projected.panes.push(ClientShellPane {
            pane_id: pane_id.clone(),
            workspace_id: "ws_1".into(),
            tab_id,
            label: None,
            cwd: None,
            foreground_cwd: None,
            focused: index == 0,
            right_click_passthrough: false,
        });
        projected.agents.push(ClientShellAgent {
            pane_id,
            workspace_id: "ws_1".into(),
            tab_id: format!("tab_{index}"),
            name: Some("opencode".into()),
            display_agent: Some("OpenCode".into()),
            agent: Some("opencode".into()),
            title: None,
            terminal_title: None,
            terminal_title_stripped: None,
            agent_status: AgentStatus::Idle,
            state_change_seq: 0,
            state_labels: Vec::new(),
            tokens: vec![(OPENCODE_TABS_TOKEN.into(), tabs_token(&format!("[{tabs}]")))],
            focused: index == 0,
        });
    }
    projected
}

#[test]
fn expired_snapshot_falls_back_to_one_pane_row() {
    let shell = snapshot_with_agent(vec![(
        OPENCODE_TABS_TOKEN.to_string(),
        format!(
            "{{\"v\":1,\"owner\":\"{}\",\"expiresAtMs\":1,\"tabs\":[{}]}}",
            OWNER,
            tab("ses_expired", "expired", true, false, false)
        ),
    )]);

    let rows = crate::client::shell::agent_sidebar::agent_rows(
        &shell,
        &ClientShellConfig::from_config(&Config::default()),
        None,
    );
    assert_eq!(rows.len(), 1, "expired snapshot must not expand");
    assert_eq!(
        rows[0].identity,
        crate::client::shell::agent_sidebar::AgentIdentity::Pane {
            pane_id: "pane_1".to_string()
        }
    );
}

#[test]
fn opencode_tab_expansion_parses_each_pane_token_once_per_pass() {
    const PANES: usize = 24;
    const TABS: usize = 3;
    let projected = scaled_snapshot(PANES, TABS);
    let config = ClientShellConfig::from_config(&Config::default());

    crate::client::shell::agent_sidebar::parse_probe::reset();
    let rows = crate::client::shell::agent_sidebar::agent_rows(&projected, &config, None);
    assert_eq!(rows.len(), PANES * TABS, "rows scale with total tab count");
    assert_eq!(
        crate::client::shell::agent_sidebar::parse_probe::count(),
        PANES,
        "one parse per pane per expansion pass, not one per tab"
    );

    // A second pass repeats the parse exactly once per pane and yields the same
    // row identities, so a widened render loop stays deterministic.
    let identities = |rows: &[crate::client::shell::agent_sidebar::AgentRow]| {
        rows.iter()
            .map(|row| row.identity.clone())
            .collect::<Vec<_>>()
    };
    crate::client::shell::agent_sidebar::parse_probe::reset();
    let again = crate::client::shell::agent_sidebar::agent_rows(&projected, &config, None);
    assert_eq!(identities(&rows), identities(&again));
    assert_eq!(
        crate::client::shell::agent_sidebar::parse_probe::count(),
        PANES
    );
}

#[test]
fn malformed_tab_token_counts_one_parse_and_falls_back_once() {
    let projected = scaled_snapshot(4, 2);
    let mut malformed = projected.clone();
    malformed.agents[1].tokens = vec![(OPENCODE_TABS_TOKEN.into(), "{".into())];
    let config = ClientShellConfig::from_config(&Config::default());

    crate::client::shell::agent_sidebar::parse_probe::reset();
    let rows = crate::client::shell::agent_sidebar::agent_rows(&malformed, &config, None);
    assert_eq!(rows.len(), 3 * 2 + 1, "broken pane keeps a single row");
    assert_eq!(
        crate::client::shell::agent_sidebar::parse_probe::count(),
        4,
        "a malformed token is still parsed once, then falls back"
    );
}

#[test]
fn opencode_tabs_expand_to_one_row_per_root_tab() {
    let token = tabs_token(&format!(
        "[{},{}]",
        tab("ses_a", "attention tab", false, false, true),
        tab("ses_b", "busy tab", true, true, false)
    ));
    let projected = snapshot_with_agent(vec![(OPENCODE_TABS_TOKEN.into(), token)]);
    let rows = rows(&projected);
    assert_eq!(rows.len(), 2, "one pane must expand to two tab rows");
    assert_eq!(
        rows[0].identity,
        AgentIdentity::OpenCodeTab {
            pane_id: "pane_1".into(),
            owner: OWNER.into(),
            session_id: "ses_a".into(),
        }
    );
    assert_eq!(rows[0].status, AgentStatus::Blocked);
    assert_eq!(rows[1].status, AgentStatus::Working);
    assert!(!rows[0].focused, "inactive tab must not be highlighted");
    assert!(
        rows[1].focused,
        "active tab on focused owner pane is highlighted"
    );
}

#[test]
fn inactive_owner_pane_does_not_expand_focus_highlight() {
    let token = tabs_token(&format!(
        "[{},{}]",
        tab("ses_a", "one", true, false, false),
        tab("ses_b", "two", false, false, false)
    ));
    let mut projected = snapshot_with_agent(vec![(OPENCODE_TABS_TOKEN.into(), token)]);
    projected.agents[0].focused = false;
    let rows = rows(&projected);
    assert_eq!(rows.len(), 2);
    assert!(rows.iter().all(|row| !row.focused));
    assert!(rows.iter().all(|row| row.status == AgentStatus::Idle));
}

#[test]
fn malformed_empty_or_absent_tabs_fall_back_to_one_pane_row() {
    let cases = [
        Vec::new(),
        vec![(OPENCODE_TABS_TOKEN.to_string(), "{not json".to_string())],
        vec![(OPENCODE_TABS_TOKEN.to_string(), tabs_token("[]"))],
    ];
    for tokens in cases {
        let rows = rows(&snapshot_with_agent(tokens));
        assert_eq!(rows.len(), 1);
        assert_eq!(
            rows[0].identity,
            AgentIdentity::Pane {
                pane_id: "pane_1".into()
            }
        );
    }
}

#[test]
fn tab_without_a_title_does_not_keep_the_parent_session_title() {
    let (config, _) = radar_shaped_sidebar();
    let token = tabs_token(&format!(
        "[{},{}]",
        tab("ses_a", "", true, true, false),
        tab("ses_b", "root-b", false, false, true)
    ));
    let mut agent = opencode_agent(
        vec![
            (OPENCODE_TABS_TOKEN.into(), token),
            ("title_working".into(), "⡿ OC | parent session".into()),
        ],
        true,
    );
    agent.terminal_title_stripped = Some("OC | parent session".into());
    let mut projected = snapshot();
    projected.focused_pane_id = Some("pane_1".into());
    projected.panes[0].focused = true;
    projected.agents.push(agent);

    let rows = rows_with(&projected, &config);
    assert_eq!(rows.len(), 2);
    for row in &rows {
        assert!(
            custom_tokens(row)
                .iter()
                .all(|(text, _)| !text.contains("parent session")),
            "an untitled tab must drop the title token, not reuse the pane's"
        );
    }
}

#[test]
fn opencode_tab_click_focuses_owner_then_reports_tab_focus() {
    let token = tabs_token(&format!(
        "[{},{}]",
        tab("ses_a", "attention tab", false, false, true),
        tab("ses_b", "busy tab", true, true, false)
    ));
    let projected = snapshot_with_agent(vec![(OPENCODE_TABS_TOKEN.into(), token)]);
    let mut state = ClientShellState::new(ClientShellConfig::from_config(&Config::default()));
    state.set_snapshot(Box::new(projected));
    state.compose(106, SIDEBAR_TEST_ROWS).unwrap();
    assert_eq!(state.hits.agents.len(), 2);

    let first = state.hits.agents[0].0;
    let outcome = state.handle_raw_events(vec![RawInputEvent::Mouse(MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: first.x + 1,
        row: first.y,
        modifiers: KeyModifiers::empty(),
    })]);
    let [ClientShellAction::Endpoint { request: focus, .. }, ClientShellAction::Endpoint {
        request: report, ..
    }] = &outcome.actions[..]
    else {
        panic!("tab click must focus the pane then report the tab");
    };
    assert!(matches!(
        &focus.method,
        crate::api::schema::Method::PaneFocus(target) if target.pane_id == "pane_1"
    ));
    let crate::api::schema::Method::PaneReportMetadata(params) = &report.method else {
        panic!("expected pane.report_metadata");
    };
    assert_eq!(params.pane_id, "pane_1");
    assert_eq!(params.source, crate::opencode_tabs::FOCUS_SOURCE);
    assert_eq!(params.ttl_ms, Some(crate::opencode_tabs::FOCUS_TTL_MS));
    let raw = params
        .tokens
        .get(crate::opencode_tabs::FOCUS_TOKEN)
        .expect("focus token")
        .as_deref()
        .expect("focus token value");
    let value: serde_json::Value = serde_json::from_str(raw).expect("focus JSON");
    assert_eq!(value["v"], 1);
    assert_eq!(value["owner"], OWNER);
    assert_eq!(value["sessionID"], "ses_a");
    assert_eq!(value["requestID"].as_str().unwrap().len(), 32);
}

/// The live client learns which methods its local server accepts from the
/// client-shell handshake. `EndpointServerWelcome::compatible` fills that list
/// from `supported_client_shell_method_names()`, the same allow-list the server
/// uses to admit inbound endpoint requests. A method absent from it is dropped
/// by `push_endpoint_method` before it leaves the client and rejected by
/// `decode_endpoint_request` if it ever arrives, so the two halves cannot drift.
#[test]
fn internal_tab_click_reports_focus_with_the_advertised_client_shell_methods() {
    let token = tabs_token(&format!(
        "[{},{}]",
        tab("ses_a", "attention tab", true, false, false),
        tab("ses_b", "busy tab", false, true, false)
    ));
    let projected = snapshot_with_agent(vec![(OPENCODE_TABS_TOKEN.into(), token)]);
    let mut state = ClientShellState::new(ClientShellConfig::from_config(&Config::default()));
    state.set_snapshot(Box::new(projected));
    state.set_endpoint_methods(Some(
        crate::server::client_commands::supported_client_shell_method_names()
            .iter()
            .map(|method| (*method).to_owned())
            .collect(),
    ));
    state.compose(106, SIDEBAR_TEST_ROWS).unwrap();
    assert_eq!(state.hits.agents.len(), 2, "two expanded tab rows");

    let second = state.hits.agents[1].0;
    let outcome = state.handle_raw_events(vec![RawInputEvent::Mouse(MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: second.x + 1,
        row: second.y,
        modifiers: KeyModifiers::empty(),
    })]);
    let methods = outcome
        .actions
        .iter()
        .map(|action| match action {
            ClientShellAction::Endpoint { request, .. } => Some(&request.method),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert!(
        methods
            .iter()
            .any(|method| matches!(method, Some(crate::api::schema::Method::PaneFocus(_)))),
        "a tab click must focus the pane: {methods:?}"
    );
    assert!(
        methods.iter().any(|method| matches!(
            method,
            Some(crate::api::schema::Method::PaneReportMetadata(_))
        )),
        "a tab click must report the clicked tab with the methods the handshake \
         advertises, so the local client-shell allow-list must carry \
         pane.report_metadata: {methods:?}"
    );
}

#[test]
fn pending_activation_focus_revalidates_membership_before_reporting() {
    let token = tabs_token(&format!("[{}]", tab("ses_a", "one", true, false, false)));
    let projected = snapshot_with_agent(vec![(OPENCODE_TABS_TOKEN.into(), token)]);
    let mut state = ClientShellState::new(ClientShellConfig::from_config(&Config::default()));
    state.set_snapshot(Box::new(projected));
    assert!(state.activate_endpoint_projection(&ClientEndpointId::Local));
    assert_eq!(
        state.active_endpoint_id,
        ClientEndpointId::Local,
        "activation must keep the local endpoint active"
    );
    assert!(
        state.endpoint_snapshot(&ClientEndpointId::Local).is_some(),
        "activation must install a snapshot for revalidation"
    );

    state.pending_opencode_focus = Some((
        ClientEndpointId::Local,
        "pane_1".into(),
        OWNER.into(),
        "ses_missing".into(),
    ));
    let mut stale = ClientShellInput::default();
    assert!(!state.send_pending_opencode_focus(&mut stale));
    assert!(stale.actions.is_empty());
    assert!(state.pending_opencode_focus.is_none());

    state.pending_opencode_focus = Some((
        ClientEndpointId::Local,
        "pane_1".into(),
        OWNER.into(),
        "ses_a".into(),
    ));
    let mut fresh = ClientShellInput::default();
    assert!(state.send_pending_opencode_focus(&mut fresh));
    let [ClientShellAction::Endpoint { request, .. }] = fresh.actions.as_slice() else {
        panic!("fresh pending focus reports one metadata token");
    };
    let crate::api::schema::Method::PaneReportMetadata(params) = &request.method else {
        panic!("expected pane.report_metadata");
    };
    assert_eq!(params.pane_id, "pane_1");
    assert!(params
        .tokens
        .get(crate::opencode_tabs::FOCUS_TOKEN)
        .is_some_and(Option::is_some));
}

#[test]
fn keyboard_agent_navigation_uses_expanded_tab_rows() {
    let token = tabs_token(&format!(
        "[{},{}]",
        tab("ses_a", "one", true, false, false),
        tab("ses_b", "two", false, false, false)
    ));
    let projected = snapshot_with_agent(vec![(OPENCODE_TABS_TOKEN.into(), token)]);
    let mut state = ClientShellState::new(ClientShellConfig::from_config(&Config::default()));
    state.set_snapshot(Box::new(projected));
    assert!(state.activate_endpoint_projection(&ClientEndpointId::Local));
    state.compose(106, SIDEBAR_TEST_ROWS).unwrap();

    let mut outcome = ClientShellInput::default();
    assert!(state.handle_endpoint_navigation(crate::input::KeybindAction::NextAgent, &mut outcome,));
    let [ClientShellAction::Endpoint { request: focus, .. }, ClientShellAction::Endpoint {
        request: report, ..
    }] = outcome.actions.as_slice()
    else {
        panic!("tab keyboard navigation focuses the pane then reports the tab");
    };
    assert!(matches!(
        &focus.method,
        crate::api::schema::Method::PaneFocus(target) if target.pane_id == "pane_1"
    ));
    assert!(matches!(
        &report.method,
        crate::api::schema::Method::PaneReportMetadata(_)
    ));
}

fn three_tab_snapshot(active: &str) -> ClientShellSnapshot {
    let token = tabs_token(&format!(
        "[{},{},{}]",
        tab("ses_a", "one", active == "ses_a", false, false),
        tab("ses_b", "two", active == "ses_b", false, false),
        tab("ses_c", "three", active == "ses_c", false, false)
    ));
    snapshot_with_agent(vec![(OPENCODE_TABS_TOKEN.into(), token)])
}

fn reported_session(outcome: &ClientShellInput) -> Option<String> {
    outcome.actions.iter().find_map(|action| {
        let ClientShellAction::Endpoint { request, .. } = action else {
            return None;
        };
        let crate::api::schema::Method::PaneReportMetadata(params) = &request.method else {
            return None;
        };
        let raw = params
            .tokens
            .get(crate::opencode_tabs::FOCUS_TOKEN)?
            .as_deref()?;
        let value: serde_json::Value = serde_json::from_str(raw).ok()?;
        value["sessionID"].as_str().map(str::to_owned)
    })
}

fn apply_snapshot(state: &mut ClientShellState, mut projected: ClientShellSnapshot) {
    projected.revision = projected.revision.saturating_add(1);
    state.set_snapshot(Box::new(projected));
    assert!(state.activate_endpoint_projection(&ClientEndpointId::Local));
}

#[test]
fn rapid_agent_navigation_walks_from_intent_not_stale_active() {
    let mut state = ClientShellState::new(ClientShellConfig::from_config(&Config::default()));
    state.set_snapshot(Box::new(three_tab_snapshot("ses_a")));
    assert!(state.activate_endpoint_projection(&ClientEndpointId::Local));
    state.compose(106, SIDEBAR_TEST_ROWS).unwrap();

    let mut first = ClientShellInput::default();
    assert!(state.handle_endpoint_navigation(crate::input::KeybindAction::NextAgent, &mut first,));
    assert_eq!(reported_session(&first).as_deref(), Some("ses_b"));

    let mut second = ClientShellInput::default();
    assert!(state.handle_endpoint_navigation(crate::input::KeybindAction::NextAgent, &mut second,));
    assert_eq!(
        reported_session(&second).as_deref(),
        Some("ses_c"),
        "the second press must walk from the intended tab, not the stale active one"
    );

    let mut back = ClientShellInput::default();
    assert!(
        state.handle_endpoint_navigation(crate::input::KeybindAction::PreviousAgent, &mut back,)
    );
    assert_eq!(
        reported_session(&back).as_deref(),
        Some("ses_b"),
        "PreviousAgent must also walk from the intended tab"
    );
}

#[test]
fn late_stale_active_snapshot_does_not_reset_the_navigation_cursor() {
    let mut state = ClientShellState::new(ClientShellConfig::from_config(&Config::default()));
    state.set_snapshot(Box::new(three_tab_snapshot("ses_a")));
    assert!(state.activate_endpoint_projection(&ClientEndpointId::Local));
    state.compose(106, SIDEBAR_TEST_ROWS).unwrap();

    let mut first = ClientShellInput::default();
    assert!(state.handle_endpoint_navigation(crate::input::KeybindAction::NextAgent, &mut first,));
    assert_eq!(reported_session(&first).as_deref(), Some("ses_b"));

    // The server has not processed the focus request yet and still reports the
    // old tab as active.
    apply_snapshot(&mut state, three_tab_snapshot("ses_a"));
    state.compose(106, SIDEBAR_TEST_ROWS).unwrap();
    let selected = state
        .agent_selection
        .as_ref()
        .map(|selection| selection.identity.clone());
    assert!(matches!(
        selected,
        Some(AgentIdentity::OpenCodeTab { ref session_id, .. }) if session_id == "ses_b"
    ));

    let mut second = ClientShellInput::default();
    assert!(state.handle_endpoint_navigation(crate::input::KeybindAction::NextAgent, &mut second,));
    assert_eq!(
        reported_session(&second).as_deref(),
        Some("ses_c"),
        "a stale active flag must not hijack the cursor"
    );
}

#[test]
fn agent_selection_clears_on_ack_timeout_and_removed_tab() {
    let mut state = ClientShellState::new(ClientShellConfig::from_config(&Config::default()));
    state.set_snapshot(Box::new(three_tab_snapshot("ses_a")));
    assert!(state.activate_endpoint_projection(&ClientEndpointId::Local));
    state.compose(106, SIDEBAR_TEST_ROWS).unwrap();
    let mut outcome = ClientShellInput::default();
    assert!(state.handle_endpoint_navigation(crate::input::KeybindAction::NextAgent, &mut outcome,));

    // Acknowledgement: the server now reports the intended tab as active.
    apply_snapshot(&mut state, three_tab_snapshot("ses_b"));
    assert!(
        state.agent_selection.is_none(),
        "an acknowledged intent must retire"
    );

    // Timeout without acknowledgement.
    state.set_agent_selection(
        ClientEndpointId::Local,
        AgentIdentity::OpenCodeTab {
            pane_id: "pane_1".into(),
            owner: OWNER.into(),
            session_id: "ses_c".into(),
        },
    );
    if let Some(selection) = state.agent_selection.as_mut() {
        selection.deadline = std::time::Instant::now()
            .checked_sub(std::time::Duration::from_millis(1))
            .unwrap_or_else(std::time::Instant::now);
    }
    state.reconcile_agent_selection();
    assert!(state.agent_selection.is_none(), "an expired intent clears");

    // Membership lost: the selected tab disappears from the token.
    state.set_agent_selection(
        ClientEndpointId::Local,
        AgentIdentity::OpenCodeTab {
            pane_id: "pane_1".into(),
            owner: OWNER.into(),
            session_id: "ses_c".into(),
        },
    );
    let token = tabs_token(&format!("[{}]", tab("ses_a", "one", true, false, false)));
    let mut removed = snapshot_with_agent(vec![(OPENCODE_TABS_TOKEN.into(), token)]);
    removed.revision = removed.revision.saturating_add(1);
    state.set_snapshot(Box::new(removed));
    assert!(state.activate_endpoint_projection(&ClientEndpointId::Local));
    assert!(
        state.agent_selection.is_none(),
        "a removed tab must not leave a stuck highlight"
    );
}

#[test]
fn a_snapshot_at_the_intent_revision_cannot_acknowledge_the_focus() {
    let mut state = ClientShellState::new(ClientShellConfig::from_config(&Config::default()));
    state.set_snapshot(Box::new(three_tab_snapshot("ses_a")));
    assert!(state.activate_endpoint_projection(&ClientEndpointId::Local));
    state.compose(106, SIDEBAR_TEST_ROWS).unwrap();
    let mut outcome = ClientShellInput::default();
    assert!(state.handle_endpoint_navigation(crate::input::KeybindAction::NextAgent, &mut outcome,));

    // A frame already buffered before the focus request describes the future
    // active tab but carries the base revision. It must not retire the intent.
    let stale = three_tab_snapshot("ses_b");
    state.set_snapshot(Box::new(stale));
    assert!(
        state.agent_selection.is_some(),
        "an older snapshot cannot acknowledge the newest intent"
    );

    apply_snapshot(&mut state, three_tab_snapshot("ses_b"));
    assert!(
        state.agent_selection.is_none(),
        "a newer snapshot retires the acknowledged intent"
    );
}

#[test]
fn an_active_tab_on_an_unfocused_owner_does_not_acknowledge_the_intent() {
    let mut state = ClientShellState::new(ClientShellConfig::from_config(&Config::default()));
    state.set_snapshot(Box::new(three_tab_snapshot("ses_a")));
    assert!(state.activate_endpoint_projection(&ClientEndpointId::Local));
    let mut outcome = ClientShellInput::default();
    assert!(state.handle_endpoint_navigation(crate::input::KeybindAction::NextAgent, &mut outcome,));

    let mut unfocused = three_tab_snapshot("ses_b");
    unfocused.agents[0].focused = false;
    unfocused.panes[0].focused = false;
    unfocused.focused_pane_id = None;
    apply_snapshot(&mut state, unfocused);
    assert!(
        state.agent_selection.is_some(),
        "an active tab is not an acknowledgement while its owning pane is unfocused"
    );
}

#[test]
fn a_boot_change_clears_the_pending_intent_without_waiting_for_the_ttl() {
    let mut state = ClientShellState::new(ClientShellConfig::from_config(&Config::default()));
    state.set_snapshot(Box::new(three_tab_snapshot("ses_a")));
    assert!(state.activate_endpoint_projection(&ClientEndpointId::Local));
    let mut outcome = ClientShellInput::default();
    assert!(state.handle_endpoint_navigation(crate::input::KeybindAction::NextAgent, &mut outcome,));
    assert!(state.agent_selection.is_some());

    let mut restarted = three_tab_snapshot("ses_b");
    restarted.boot_id = "boot-2".into();
    state.set_snapshot(Box::new(restarted));
    assert!(state.activate_endpoint_projection(&ClientEndpointId::Local));
    assert!(
        state.agent_selection.is_none(),
        "a new endpoint boot invalidates the old intent"
    );
}

#[test]
fn a_pending_tab_selection_clears_the_previous_ordinary_row_highlight() {
    let token = tabs_token(&format!(
        "[{},{}]",
        tab("ses_a", "root-a", true, false, false),
        tab("ses_b", "root-b", false, false, false)
    ));
    let mut projected = snapshot_with_agent(vec![(OPENCODE_TABS_TOKEN.into(), token)]);
    projected.agents[0].focused = false;
    projected.panes[0].focused = false;
    projected.focused_pane_id = Some("pane_2".into());
    let mut second_pane = projected.panes[0].clone();
    second_pane.pane_id = "pane_2".into();
    second_pane.focused = true;
    projected.panes.push(second_pane);
    let mut ordinary = opencode_agent(Vec::new(), true);
    ordinary.pane_id = "pane_2".into();
    ordinary.name = Some("codex".into());
    ordinary.display_agent = None;
    ordinary.agent = Some("codex".into());
    projected.agents.push(ordinary);

    let mut state = ClientShellState::new(ClientShellConfig::from_config(&Config::default()));
    state.set_snapshot(Box::new(projected));
    assert!(state.activate_endpoint_projection(&ClientEndpointId::Local));
    state.compose(106, SIDEBAR_TEST_ROWS).unwrap();
    let tab_hit = state
        .hits
        .agents
        .iter()
        .find(|(_, identity)| {
            matches!(
                identity,
                AgentIdentity::OpenCodeTab { session_id, .. } if session_id == "ses_b"
            )
        })
        .map(|(rect, _)| *rect)
        .expect("tab row hit");
    let ordinary_hit = state
        .hits
        .agents
        .iter()
        .find(|(_, identity)| {
            matches!(identity, AgentIdentity::Pane { pane_id } if pane_id == "pane_2")
        })
        .map(|(rect, _)| *rect)
        .expect("ordinary row hit");

    let outcome = state.handle_raw_events(vec![RawInputEvent::Mouse(MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: tab_hit.x + 1,
        row: tab_hit.y,
        modifiers: KeyModifiers::empty(),
    })]);
    assert!(outcome.repaint);
    let frame = state.compose(106, SIDEBAR_TEST_ROWS).unwrap();
    let buffer = frame.to_ratatui_buffer().expect("frame should reconstruct");
    let selected_bg = state.config.palette.active_row_bg;
    assert_eq!(buffer[(tab_hit.x, tab_hit.y)].bg, selected_bg);
    assert_ne!(
        buffer[(ordinary_hit.x, ordinary_hit.y)].bg,
        selected_bg,
        "the old focused ordinary row must not stay highlighted"
    );
}

#[test]
fn clicking_a_tab_moves_the_highlight_before_snapshot_ack() {
    let mut state = ClientShellState::new(ClientShellConfig::from_config(&Config::default()));
    state.set_snapshot(Box::new(three_tab_snapshot("ses_a")));
    assert!(state.activate_endpoint_projection(&ClientEndpointId::Local));
    state.compose(106, SIDEBAR_TEST_ROWS).unwrap();
    assert_eq!(state.hits.agents.len(), 3);

    let second = state.hits.agents[1].0;
    let outcome = state.handle_raw_events(vec![RawInputEvent::Mouse(MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: second.x + 1,
        row: second.y,
        modifiers: KeyModifiers::empty(),
    })]);
    assert_eq!(reported_session(&outcome).as_deref(), Some("ses_b"));

    let frame = state.compose(106, SIDEBAR_TEST_ROWS).unwrap();
    let buffer = frame.to_ratatui_buffer().expect("frame should reconstruct");
    let selected_bg = state.config.palette.active_row_bg;
    let row_bg = |rect: Rect| buffer[(rect.x, rect.y)].bg;
    assert_eq!(
        row_bg(state.hits.agents[1].0),
        selected_bg,
        "clicked tab must highlight immediately"
    );
    assert_ne!(
        row_bg(state.hits.agents[0].0),
        selected_bg,
        "the previously active tab must stop being highlighted"
    );
}

#[test]
fn radar_row_click_highlights_the_pending_tab_before_the_ack_snapshot() {
    let (config, _) = radar_shaped_sidebar();
    let mut state = ClientShellState::new(config);
    let token = tabs_token(&format!(
        "[{},{}]",
        tab("ses_a", "root-a", true, false, false),
        tab("ses_b", "root-b", false, false, false)
    ));
    let mut agent = opencode_agent(
        vec![
            (OPENCODE_TABS_TOKEN.into(), token),
            ("split_mark".into(), "└".into()),
            ("logo".into(), "◆".into()),
            ("title_idle".into(), "parent session".into()),
        ],
        true,
    );
    agent.terminal_title_stripped = Some("parent session".into());
    let mut projected = snapshot();
    projected.focused_pane_id = Some("pane_1".into());
    projected.panes[0].focused = true;
    projected.agents.push(agent);
    state.set_snapshot(Box::new(projected));
    assert!(state.activate_endpoint_projection(&ClientEndpointId::Local));
    state.compose(106, SIDEBAR_TEST_ROWS).unwrap();
    assert_eq!(state.hits.agents.len(), 2);

    let clicked = state.hits.agents[1].0;
    let outcome = state.handle_raw_events(vec![RawInputEvent::Mouse(MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: clicked.x + 1,
        row: clicked.y,
        modifiers: KeyModifiers::empty(),
    })]);
    assert!(
        outcome.repaint,
        "the click must schedule an immediate repaint"
    );
    assert!(
        !crate::client::shell::agent_sidebar::identity_is_active(
            state.snapshot.as_deref().expect("snapshot"),
            &state.agent_selection.as_ref().expect("intent").identity,
        ),
        "the authoritative snapshot still names the previous tab"
    );

    let frame = state.compose(106, SIDEBAR_TEST_ROWS).unwrap();
    let buffer = frame.to_ratatui_buffer().expect("frame should reconstruct");
    let selected_bg = state.config.palette.active_row_bg;
    assert_eq!(
        buffer[(clicked.x, clicked.y)].bg,
        selected_bg,
        "Radar row must highlight the pending tab in the click's own frame"
    );
    assert_ne!(
        buffer[(state.hits.agents[0].0.x, state.hits.agents[0].0.y)].bg,
        selected_bg,
        "the previous tab must stop being highlighted before the ack"
    );
}

/// Sidebar rows shaped like the installed Radar plugin: one custom token per
/// state, each painted its own colour, and only the member matching the pane's
/// own session ever carries a value. Returns the parsed config and each custom
/// token's configured style, so a row can be checked against the exact name it
/// should have resolved.
fn radar_shaped_sidebar() -> (
    ClientShellConfig,
    Vec<(String, crate::config::SidebarTokenStyle)>,
) {
    let config: Config = toml::from_str(
        r##"
[ui.sidebar.agents]
rows = [
  [
    "$split_mark",
    { token = "$logo", fg = "#8899aa" },
    { token = "$logo_working", fg = "#00ff99" },
    { token = "$logo_stale", fg = "#556677" },
    { token = "$title_working", fg = "#ff0055", bold = true },
    { token = "$title_blocked", fg = "#ffaa00" },
    { token = "$title_idle", fg = "#8899aa" },
  ],
  ["$gap"],
]
row_gap = 0
"##,
    )
    .expect("radar-shaped sidebar tokens");
    let mut styles = Vec::new();
    for row in &config.ui.sidebar.agents.rows {
        for token in row {
            let crate::config::AgentSidebarToken::Styled { token, style, .. } = token else {
                continue;
            };
            if let crate::config::AgentSidebarToken::Custom(name) = token.as_ref() {
                styles.push((name.clone(), *style));
            }
        }
    }
    (ClientShellConfig::from_config(&config), styles)
}

fn custom_tokens(row: &AgentRow) -> Vec<(String, crate::config::SidebarTokenStyle)> {
    row.rows
        .iter()
        .flatten()
        .filter_map(|token| match &token.kind {
            crate::ui::ResolvedTokenKind::Custom(value) => Some((value.clone(), token.style)),
            _ => None,
        })
        .collect()
}

fn custom_style(row: &AgentRow, text: &str) -> Option<crate::config::SidebarTokenStyle> {
    custom_tokens(row)
        .into_iter()
        .find(|(value, _)| value == text)
        .map(|(_, style)| style)
}

fn grouped_radar_sidebar() -> ClientShellConfig {
    let config: Config = toml::from_str(
        r##"
[ui.sidebar.agents]
rows = [
  [
    { token = "$group_parent", dim = true },
    { token = "$group", fg = "#aa88ff" },
    { token = "$group_stale" },
  ],
  ["$split_mark"],
  [{ token = "$logo", fg = "#8899aa" }, { token = "$logo_working", fg = "#00ff99" }],
  [
    { token = "$title_working", fg = "#ff0055", bold = true },
    { token = "$title_blocked", fg = "#ffaa00" },
    { token = "$title_idle", fg = "#8899aa" },
  ],
  ["$gap"],
]
row_gap = 0
"##,
    )
    .expect("grouped radar-shaped sidebar tokens");
    ClientShellConfig::from_config(&config)
}

fn grouped_agent(tokens: Vec<(String, String)>) -> ClientShellAgent {
    let mut agent = opencode_agent(tokens, true);
    agent.terminal_title_stripped = Some("parent session".into());
    agent
}

fn radar_group_tokens(token: String) -> Vec<(String, String)> {
    vec![
        (OPENCODE_TABS_TOKEN.into(), token),
        ("group_parent".into(), "  ".into()),
        ("group".into(), "client-shell".into()),
        ("split_mark".into(), "└".into()),
        ("logo".into(), "◆".into()),
        ("title_idle".into(), "parent session".into()),
        ("gap".into(), " ".into()),
    ]
}

fn names_workspace(row: &AgentRow, label: &str) -> bool {
    custom_tokens(row).iter().any(|(value, _)| value == label)
}

#[test]
fn repeated_workspace_header_collapses_into_one_header() {
    let token = tabs_token(&format!(
        "[{},{},{}]",
        tab("ses_a", "root-a", true, false, false),
        tab("ses_b", "root-b", false, true, false),
        tab("ses_c", "root-c", false, false, false)
    ));
    let mut projected = snapshot();
    projected.focused_pane_id = Some("pane_1".into());
    projected.panes[0].focused = true;
    projected
        .agents
        .push(grouped_agent(radar_group_tokens(token)));

    let rows = rows_with(&projected, &grouped_radar_sidebar());
    assert_eq!(rows.len(), 3, "tabs stay separate rows");
    assert_eq!(rows[0].header_rows, 1);
    assert!(
        names_workspace(&rows[0], "client-shell"),
        "the first tab keeps the one workspace header"
    );
    assert!(
        rows[1..]
            .iter()
            .all(|row| !names_workspace(row, "client-shell")),
        "later tabs must not repeat the workspace header"
    );
    assert!(
        rows.iter().all(|row| custom_tokens(row)
            .iter()
            .all(|(text, _)| !text.contains('└'))),
        "tab rows stay flat; the branch corner is not repeated"
    );
    // Tab identity and state survive the fold.
    assert!(custom_style(&rows[1], "root-b").is_some());
    assert!(custom_style(&rows[2], "root-c").is_some());
    assert_ne!(
        custom_style(&rows[0], "◆"),
        custom_style(&rows[1], "◆"),
        "each tab's glyph follows its own state"
    );

    let rendered = rows
        .iter()
        .flat_map(|row| rendered_row_lines(&grouped_radar_sidebar(), row))
        .collect::<Vec<_>>();
    assert_eq!(
        rendered
            .iter()
            .map(|line| line.matches('└').count())
            .sum::<usize>(),
        0,
        "tab rows render without a tree connector: {rendered:?}"
    );
}

#[test]
fn same_workspace_label_on_another_workspace_keeps_its_own_header() {
    let token = tabs_token(&format!(
        "[{},{}]",
        tab("ses_a", "root-a", true, false, false),
        tab("ses_b", "root-b", false, false, false)
    ));
    let mut projected = snapshot();
    projected.workspaces.push(ClientShellWorkspace {
        workspace_id: "ws_2".into(),
        label: "client-shell".into(),
        ..projected.workspaces[0].clone()
    });
    projected.tabs.push(ClientShellTab {
        tab_id: "tab_2".into(),
        workspace_id: "ws_2".into(),
        label: "2".into(),
        ..projected.tabs[0].clone()
    });
    projected.panes.push(ClientShellPane {
        pane_id: "pane_2".into(),
        workspace_id: "ws_2".into(),
        tab_id: "tab_2".into(),
        ..projected.panes[0].clone()
    });
    let mut second = grouped_agent(radar_group_tokens(token.clone()));
    second.pane_id = "pane_2".into();
    second.workspace_id = "ws_2".into();
    second.tab_id = "tab_2".into();
    projected
        .agents
        .push(grouped_agent(radar_group_tokens(token)));
    projected.agents.push(second);

    let rows = rows_with(&projected, &grouped_radar_sidebar());
    let groups = rows
        .iter()
        .map(|row| row.group_key.clone())
        .collect::<std::collections::HashSet<_>>();
    assert_eq!(
        groups.len(),
        2,
        "workspace identity, not label, groups rows"
    );
    let headers = rows
        .iter()
        .filter(|row| names_workspace(row, "client-shell"))
        .count();
    assert_eq!(headers, 2, "one header per real workspace");
    assert_eq!(rows.len(), 4, "two tabs per workspace, no extra rows");
}

#[test]
fn grouped_rows_keep_one_hitbox_per_open_tab() {
    let token = tabs_token(&format!(
        "[{},{},{}]",
        tab("ses_a", "root-a", true, false, false),
        tab("ses_b", "root-b", false, false, false),
        tab("ses_c", "root-c", false, false, false)
    ));
    let mut projected = snapshot();
    projected.focused_pane_id = Some("pane_1".into());
    projected.panes[0].focused = true;
    projected
        .agents
        .push(grouped_agent(radar_group_tokens(token)));
    let mut state = ClientShellState::new(grouped_radar_sidebar());
    state.set_snapshot(Box::new(projected));
    assert!(state.activate_endpoint_projection(&ClientEndpointId::Local));
    state.compose(106, SIDEBAR_TEST_ROWS).unwrap();

    let identities = state
        .hits
        .agents
        .iter()
        .map(|(_, identity)| identity)
        .collect::<Vec<_>>();
    assert_eq!(identities.len(), 3, "one hitbox per open tab");
    assert!(identities.iter().all(|identity| matches!(
        identity,
        AgentIdentity::OpenCodeTab { pane_id, .. } if pane_id == "pane_1"
    )));
    let unique = identities
        .iter()
        .map(|identity| identity.opencode_tab().map(|(_, session)| session))
        .collect::<std::collections::HashSet<_>>();
    assert_eq!(unique.len(), 3, "hitboxes map to distinct tabs");
}

#[test]
fn grouped_tab_rows_click_each_open_tab_after_fold() {
    let token = tabs_token(&format!(
        "[{},{},{}]",
        tab("ses_a", "root-a", true, false, false),
        tab("ses_b", "root-b", false, false, false),
        tab("ses_c", "root-c", false, false, false)
    ));
    let mut projected = snapshot();
    projected.focused_pane_id = Some("pane_1".into());
    projected.panes[0].focused = true;
    projected
        .agents
        .push(grouped_agent(radar_group_tokens(token)));
    let mut state = ClientShellState::new(grouped_radar_sidebar());
    state.set_snapshot(Box::new(projected));
    assert!(state.activate_endpoint_projection(&ClientEndpointId::Local));
    state.compose(106, SIDEBAR_TEST_ROWS).unwrap();

    let hits = state.hits.agents.clone();
    assert_eq!(hits.len(), 3);
    for (index, (rect, _)) in hits.iter().enumerate() {
        let outcome = state.handle_raw_events(vec![RawInputEvent::Mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: rect.x + 2,
            row: rect.y + 1,
            modifiers: KeyModifiers::empty(),
        })]);
        let [ClientShellAction::Endpoint { request: focus, .. }, ClientShellAction::Endpoint {
            request: report, ..
        }] = &outcome.actions[..]
        else {
            panic!("click on folded row {index} must focus the pane then report the tab");
        };
        assert!(matches!(
            &focus.method,
            crate::api::schema::Method::PaneFocus(target) if target.pane_id == "pane_1"
        ));
        let crate::api::schema::Method::PaneReportMetadata(params) = &report.method else {
            panic!("expected pane.report_metadata");
        };
        let raw = params
            .tokens
            .get(crate::opencode_tabs::FOCUS_TOKEN)
            .expect("focus token")
            .as_deref()
            .expect("focus token value");
        let value: serde_json::Value = serde_json::from_str(raw).expect("focus JSON");
        assert_eq!(
            value["sessionID"],
            ["ses_a", "ses_b", "ses_c"][index],
            "click target keeps its own tab identity"
        );
    }
}

#[test]
fn radar_shaped_tokens_give_each_tab_its_own_title_and_state() {
    let token = tabs_token(&format!(
        "[{},{},{}]",
        tab("ses_a", "root-a", true, true, false),
        tab("ses_b", "root-b", false, false, true),
        tab("ses_c", "root-c", false, false, false)
    ));
    let mut agent = opencode_agent(
        vec![
            (OPENCODE_TABS_TOKEN.into(), token),
            ("split_mark".into(), "└".into()),
            ("logo".into(), "◆".into()),
            ("title_idle".into(), "parent session".into()),
        ],
        true,
    );
    agent.terminal_title_stripped = Some("parent session".into());
    let mut projected = snapshot();
    projected.focused_pane_id = Some("pane_1".into());
    projected.panes[0].focused = true;
    projected.agents.push(agent);

    let (config, styles) = radar_shaped_sidebar();
    let styled = |name: &str| {
        styles
            .iter()
            .find(|(token, _)| token == name)
            .map(|(_, style)| *style)
            .unwrap_or_else(|| panic!("`{name}` is not a configured styled token"))
    };
    let rows = rows_with(&projected, &config);
    assert_eq!(rows.len(), 3, "tabs replace the pane row, never join it");

    // Each row carries its own title, under the name painted for its state.
    assert_eq!(
        custom_style(&rows[0], "root-a"),
        Some(styled("title_working"))
    );
    assert_eq!(
        custom_style(&rows[1], "root-b"),
        Some(styled("title_blocked"))
    );
    assert_eq!(custom_style(&rows[2], "root-c"), Some(styled("title_idle")));
    assert!(
        rows.iter()
            .all(|row| custom_style(row, "parent session").is_none()),
        "no row may keep the pane's own session title"
    );
    // The pane's glyph is carried over; only the state name it hangs under moves.
    assert_eq!(custom_style(&rows[0], "◆"), Some(styled("logo_working")));
    assert_eq!(custom_style(&rows[1], "◆"), Some(styled("logo")));
    assert_eq!(custom_style(&rows[2], "◆"), Some(styled("logo")));
    assert!(
        rows.iter().all(|row| custom_style(row, "└").is_none()),
        "flat tab rows must not repeat the branch corner"
    );
}

fn rendered_row_lines(config: &ClientShellConfig, row: &AgentRow) -> Vec<String> {
    let width = 72;
    let height = row.rows.len().max(1) as u16;
    let mut buffer = Buffer::empty(Rect::new(0, 0, width, height));
    let rect = Rect::new(0, 0, width, height);
    crate::client::shell::agent_sidebar::render_agent_row(&mut buffer, rect, row, config);
    (0..height)
        .map(|y| {
            (0..width)
                .map(|x| buffer[(x, y)].symbol().to_string())
                .collect()
        })
        .collect()
}

#[test]
fn opencode_tab_rows_start_at_the_ordinary_agent_column() {
    let config = grouped_radar_sidebar();
    let token = tabs_token(&format!(
        "[{},{}]",
        tab("ses_a", "root-a", true, false, false),
        tab("ses_b", "root-b", false, false, true)
    ));
    let mut projected = snapshot();
    projected.focused_pane_id = Some("pane_1".into());
    projected.panes[0].focused = true;
    projected
        .agents
        .push(grouped_agent(radar_group_tokens(token)));

    projected.panes.push(ClientShellPane {
        pane_id: "pane_2".into(),
        focused: false,
        ..projected.panes[0].clone()
    });
    let mut ordinary = opencode_agent(
        vec![
            ("group_parent".into(), "  ".into()),
            ("group".into(), "client-shell".into()),
            ("logo".into(), "◆".into()),
            ("title_idle".into(), "ordinary session".into()),
        ],
        false,
    );
    ordinary.pane_id = "pane_2".into();
    ordinary.agent = Some("codex".into());
    ordinary.display_agent = Some("Codex".into());
    ordinary.name = Some("codex".into());
    projected.agents.push(ordinary);

    let rows = rows_with(&projected, &config);
    let ordinary_row = rows
        .iter()
        .find(|row| matches!(&row.identity, AgentIdentity::Pane { pane_id } if pane_id == "pane_2"))
        .expect("ordinary row");
    let tab_rows = rows
        .iter()
        .filter(|row| matches!(row.identity, AgentIdentity::OpenCodeTab { .. }))
        .collect::<Vec<_>>();
    assert_eq!(tab_rows.len(), 2, "one rendered row per open tab");

    let leading = |lines: Vec<String>, text: &str| {
        lines
            .into_iter()
            .find(|line| line.contains(text))
            .map(|line| {
                line.chars()
                    .take_while(|character| *character == ' ')
                    .count()
            })
            .unwrap_or_else(|| panic!("rendered row contains {text:?}"))
    };
    let ordinary_column = leading(
        rendered_row_lines(&config, ordinary_row),
        "ordinary session",
    );
    for (row, title) in tab_rows.iter().zip(["root-a", "root-b"]) {
        assert_eq!(
            leading(rendered_row_lines(&config, row), title),
            ordinary_column,
            "`{title}` starts where an ordinary pane row starts, including later tabs"
        );
    }
}

#[test]
fn radar_live_prefix_renders_without_spinner_on_nonworking_tabs() {
    let token = tabs_token(&format!(
        "[{},{},{}]",
        tab("ses_a", "root-a", true, true, false),
        tab("ses_b", "root-b", false, false, true),
        tab("ses_c", "root-c", false, false, false)
    ));
    let (config, _) = radar_shaped_sidebar();
    let frame = |title: &str| {
        let mut agent = opencode_agent(
            vec![
                (OPENCODE_TABS_TOKEN.into(), token.clone()),
                ("split_mark".into(), "└".into()),
                ("logo".into(), "◆".into()),
                ("title_working".into(), title.into()),
                ("gap".into(), "\u{200b}".into()),
            ],
            true,
        );
        agent.terminal_title_stripped = Some("OC | parent session".into());
        let mut projected = snapshot();
        projected.focused_pane_id = Some("pane_1".into());
        projected.panes[0].focused = true;
        projected.agents.push(agent);
        rows_with(&projected, &config)
    };

    let lines = |text: &str| {
        frame(text)
            .iter()
            .map(|row| rendered_row_lines(&config, row).join("\n"))
            .collect::<Vec<_>>()
    };
    let indent = |row: &str, title: &str| {
        row.lines()
            .find(|line| line.contains(title))
            .map(|line| {
                line.chars()
                    .take_while(|character| *character == ' ')
                    .count()
            })
            .expect("rendered row contains the tab title")
    };

    let first = lines("⡿ OC | parent session");
    assert!(first[0].contains("⡿ OC | root-a"));
    assert!(first[1].contains("OC | root-b"));
    assert!(first[2].contains("OC | root-c"));
    assert_eq!(
        first[0].lines().count(),
        1,
        "top-level and later tab rows are one line, like ordinary agent rows: {first:?}"
    );
    assert_eq!(first[1].lines().count(), 1);
    assert!(
        first[0]
            .lines()
            .chain(first[1].lines())
            .all(|line| !line.trim().is_empty()),
        "no blank spacer between tabs: {first:?}"
    );
    assert!(
        first
            .iter()
            .all(|row| !row.contains('└') && !row.contains('─')),
        "no tree corner or separator on any tab row: {first:?}"
    );
    assert!(
        first[0].contains("OC | root-a"),
        "the vendor label stays verbatim: {first:?}"
    );
    assert_eq!(
        indent(&first[0], "root-a"),
        1,
        "without a workspace header, tab content keeps the ordinary first-row column: {first:?}"
    );
    assert_eq!(
        indent(&first[0], "root-a"),
        indent(&first[1], "root-b"),
        "tab rows share the same indent: {first:?}"
    );
    assert_eq!(indent(&first[1], "root-b"), indent(&first[2], "root-c"));
    for line in &first[1..] {
        assert!(
            !line.contains('⡿') && !line.contains('⢿') && !line.contains('⠋'),
            "nonworking tab rows must not inherit the parent spinner: {line:?}"
        );
    }

    let second = lines("⢿ OC | parent session");
    assert!(second[0].contains("⢿ OC | root-a"));
    assert!(!second[0].contains('⡿'));
    for line in &second[1..] {
        assert!(
            !line.contains('⡿') && !line.contains('⢿') && !line.contains('⠋'),
            "the next frame's spinner stays on the working tab: {line:?}"
        );
    }
}

#[test]
fn radar_working_title_keeps_the_animated_prefix_on_every_tab() {
    let token = tabs_token(&format!(
        "[{},{},{}]",
        tab("ses_a", "root-a", true, true, false),
        tab("ses_b", "root-b", false, false, true),
        tab("ses_c", "root-c", false, false, false)
    ));
    let (config, styles) = radar_shaped_sidebar();
    let styled = |name: &str| {
        styles
            .iter()
            .find(|(token, _)| token == name)
            .map(|(_, style)| *style)
            .unwrap_or_else(|| panic!("`{name}` is not a configured styled token"))
    };
    let frame = |title: &str, terminal_title: &str| {
        let mut agent = opencode_agent(
            vec![
                (OPENCODE_TABS_TOKEN.into(), token.clone()),
                ("split_mark".into(), "└".into()),
                ("logo".into(), "◆".into()),
                ("title_working".into(), title.into()),
                ("gap".into(), "\u{200b}".into()),
            ],
            true,
        );
        agent.terminal_title_stripped = Some(terminal_title.into());
        let mut projected = snapshot();
        projected.focused_pane_id = Some("pane_1".into());
        projected.panes[0].focused = true;
        projected.agents.push(agent);
        rows_with(&projected, &config)
    };

    let first = frame("⡿ OC | parent session", "OC | parent session");
    assert_eq!(first.len(), 3);
    assert_eq!(
        custom_style(&first[0], "⡿ OC | root-a"),
        Some(styled("title_working")),
        "the working animation prefix and OC label survive the title rewrite"
    );
    assert_eq!(
        custom_style(&first[1], "OC | root-b"),
        Some(styled("title_blocked"))
    );
    assert_eq!(
        custom_style(&first[2], "OC | root-c"),
        Some(styled("title_idle"))
    );
    assert!(
        first[1..]
            .iter()
            .all(|row| custom_style(row, "⡿ OC | root-b").is_none()
                && custom_style(row, "⡿ OC | root-c").is_none()),
        "the parent's working spinner must not leak onto idle or waiting tabs"
    );
    assert!(
        first
            .iter()
            .all(|row| custom_style(row, "OC | parent session").is_none()),
        "no row may keep the pane's own session title"
    );

    // The next frame's spinner replaces the previous one; titles stay distinct.
    let second = frame("⢿ OC | parent session", "OC | parent session");
    assert_eq!(
        custom_style(&second[0], "⢿ OC | root-a"),
        Some(styled("title_working"))
    );
    assert!(
        second
            .iter()
            .all(|row| custom_style(row, "⡿ OC | root-a").is_none()),
        "a later frame must not retain the earlier spinner"
    );
    assert_eq!(
        custom_style(&second[1], "OC | root-b"),
        Some(styled("title_blocked"))
    );
    assert_eq!(
        custom_style(&second[2], "OC | root-c"),
        Some(styled("title_idle"))
    );

    // A plugin may strip or rewrite its own pulse before reporting, so the
    // suffix no longer matches byte for byte. The OC label and marks survive.
    let mismatched = frame("⠋ OC | parent session", "a different title");
    assert_eq!(
        custom_style(&mismatched[0], "⠋ OC | root-a"),
        Some(styled("title_working"))
    );
    assert_eq!(
        custom_style(&mismatched[1], "OC | root-b"),
        Some(styled("title_blocked"))
    );
    assert_eq!(
        custom_style(&mismatched[2], "OC | root-c"),
        Some(styled("title_idle"))
    );

    // A tab label in front of the spinner stays; only the working mark goes.
    let labeled = frame("T1 ⡿ OC | parent session", "OC | parent session");
    assert_eq!(
        custom_style(&labeled[0], "T1 ⡿ OC | root-a"),
        Some(styled("title_working"))
    );
    assert_eq!(
        custom_style(&labeled[1], "T1 OC | root-b"),
        Some(styled("title_blocked"))
    );
    assert_eq!(
        custom_style(&labeled[2], "T1 OC | root-c"),
        Some(styled("title_idle"))
    );
}

/// Two endpoints, one OpenCode pane with two roots. The local endpoint is not
/// alone, so an ordinary local focus is deferred through endpoint activation.
/// A click on an internal root tab must still carry that exact root (owner and
/// session), not just the shared pane.
fn two_endpoint_state_with_root_tabs() -> (ClientShellState, ClientEndpointId) {
    use crate::client::endpoint::{ClientEndpointStatus, ProfileId, SavedSshEndpoint};

    let profile = SavedSshEndpoint {
        id: ProfileId::parse("fedcba9876543210fedcba9876543210").unwrap(),
        label: "Build".into(),
        target: "dev@build.example".into(),
        session: "agents".into(),
        enabled: true,
    };
    let remote = ClientEndpointId::Ssh(profile.id.clone());
    let mut state = ClientShellState::new(ClientShellConfig::from_config(&Config::default()));
    state.set_endpoint_catalog(&[profile]);
    state.set_endpoint_status(&remote, ClientEndpointStatus::Online);

    let token = tabs_token(&format!(
        "[{},{}]",
        tab("ses_a", "root-a", true, false, false),
        tab("ses_b", "root-b", false, false, false)
    ));
    let mut local = snapshot_with_agent(vec![(OPENCODE_TABS_TOKEN.into(), token)]);
    local.boot_id = "local-boot".into();
    state.set_snapshot(Box::new(local));
    let mut remote_snapshot = snapshot();
    remote_snapshot.boot_id = "remote-boot".into();
    remote_snapshot.agents.clear();
    state.set_endpoint_snapshot(&remote, Box::new(remote_snapshot));
    (state, remote)
}

#[test]
fn endpoint_switch_drops_a_local_tab_selection() {
    let (mut state, remote) = two_endpoint_state_with_root_tabs();
    assert!(state.activate_endpoint_projection(&ClientEndpointId::Local));
    let identity = AgentIdentity::OpenCodeTab {
        pane_id: "pane_1".into(),
        owner: OWNER.into(),
        session_id: "ses_b".into(),
    };
    state.set_agent_selection(ClientEndpointId::Local, identity);
    assert!(state.agent_selection.is_some());

    assert!(state.activate_endpoint_projection(&remote));
    assert!(
        state.agent_selection.is_none(),
        "switching endpoints must not leave the old endpoint's tab highlighted"
    );
}

#[test]
fn two_endpoint_click_on_internal_tab_row_defers_exact_tab_focus() {
    let (mut state, _remote) = two_endpoint_state_with_root_tabs();
    state.compose(106, SIDEBAR_TEST_ROWS).unwrap();
    assert!(state.multi_endpoint_active(), "two endpoints are visible");
    let internal = state
        .hits
        .endpoint_agents
        .iter()
        .filter(|(_, endpoint_id, identity)| {
            endpoint_id.is_local() && matches!(identity, AgentIdentity::OpenCodeTab { .. })
        })
        .cloned()
        .collect::<Vec<_>>();
    assert_eq!(
        internal.len(),
        2,
        "each root tab keeps its own endpoint-agent hitbox: {:?}",
        state.hits.endpoint_agents
    );
    let (rect, endpoint_id, _identity) = internal
        .into_iter()
        .find(|(_, _, identity)| {
            matches!(
                identity,
                AgentIdentity::OpenCodeTab { session_id, .. } if session_id == "ses_b"
            )
        })
        .expect("root-b row is clickable");
    assert_eq!(endpoint_id, ClientEndpointId::Local);

    let outcome = state.handle_raw_events(vec![RawInputEvent::Mouse(MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: rect.x + 1,
        row: rect.y,
        modifiers: KeyModifiers::empty(),
    })]);
    assert!(
        matches!(
            outcome.actions.as_slice(),
            [ClientShellAction::ActivateEndpoint {
                endpoint_id: ClientEndpointId::Local,
                target: Some(ClientEndpointFocusTarget::Pane(pane_id)),
            }] if pane_id == "pane_1"
        ),
        "deferred local focus must route through activation, got {:?}",
        outcome.actions
    );
    assert_eq!(
        state.pending_opencode_focus.as_ref(),
        Some(&(
            ClientEndpointId::Local,
            "pane_1".to_string(),
            OWNER.to_string(),
            "ses_b".to_string()
        )),
        "the clicked root must survive the deferral"
    );
}

/// Clicking an OpenCode root row that belongs to the *other* endpoint must
/// defer that endpoint's activation while keeping the exact root for the
/// replay, so the remote activation cannot degrade to a bare pane focus.
#[test]
fn two_endpoint_click_on_remote_tab_row_defers_that_endpoints_tab_focus() {
    let (mut state, remote) = two_endpoint_state_with_root_tabs();
    let token = tabs_token(&format!(
        "[{},{}]",
        tab("ses_a", "remote-a", true, false, false),
        tab("ses_b", "remote-b", false, false, false)
    ));
    let mut remote_snapshot = snapshot_with_agent(vec![(OPENCODE_TABS_TOKEN.into(), token)]);
    remote_snapshot.boot_id = "remote-boot".into();
    state.set_endpoint_snapshot(&remote, Box::new(remote_snapshot));
    state.compose(106, SIDEBAR_TEST_ROWS).unwrap();

    let rect = state
        .hits
        .endpoint_agents
        .iter()
        .find(|(_, endpoint_id, identity)| {
            endpoint_id == &remote
                && matches!(
                    identity,
                    AgentIdentity::OpenCodeTab { session_id, .. } if session_id == "ses_b"
                )
        })
        .map(|(rect, _, _)| *rect)
        .expect("remote root-b row is clickable");

    let outcome = state.handle_raw_events(vec![RawInputEvent::Mouse(MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: rect.x + 1,
        row: rect.y,
        modifiers: KeyModifiers::empty(),
    })]);
    assert!(
        matches!(
            outcome.actions.as_slice(),
            [ClientShellAction::ActivateEndpoint {
                endpoint_id,
                target: Some(ClientEndpointFocusTarget::Pane(pane_id)),
            }] if endpoint_id == &remote && pane_id == "pane_1"
        ),
        "remote root click must activate that endpoint, got {:?}",
        outcome.actions
    );
    assert_eq!(
        state.pending_opencode_focus.as_ref(),
        Some(&(
            remote,
            "pane_1".to_string(),
            OWNER.to_string(),
            "ses_b".to_string()
        )),
        "the remote root must survive the deferral"
    );
}

struct RecordingTransport(std::sync::Arc<std::sync::Mutex<Vec<crate::protocol::ClientMessage>>>);
impl crate::client::endpoint::EndpointTransport for RecordingTransport {
    fn send(&mut self, message: &crate::protocol::ClientMessage) -> std::io::Result<()> {
        self.0.lock().unwrap().push(message.clone());
        Ok(())
    }
}

fn activation_negotiation() -> crate::client::endpoint::EndpointNegotiation {
    crate::client::endpoint::EndpointNegotiation::new(
        vec!["client_shell.surface.set".into()],
        vec![
            crate::protocol::endpoint::SURFACE_INTEREST_CAPABILITY.into(),
            crate::protocol::endpoint::PRESENTATION_EFFECTS_FENCE_CAPABILITY.into(),
        ],
    )
}

/// Full click-to-wire path: click an internal OpenCode row in the composed
/// two-endpoint sidebar, dispatch the scheduled activation, and verify the
/// runtime sends pane focus first and then the exact per-tab focus report.
#[test]
fn click_on_internal_tab_row_reports_exact_focus_after_activation() {
    use crate::client::endpoint::EndpointRegistry;
    use crate::client::endpoint_commands::EndpointCommands;
    use crate::client::shell_runtime::{begin_endpoint_activation, dispatch_client_shell_actions};
    use crate::client::{ClientLoopEvent, ClientState};
    use std::time::Instant;

    let (mut state, remote) = two_endpoint_state_with_root_tabs();
    let local_sent = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let remote_sent = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let mut endpoints = EndpointRegistry::new(
        RecordingTransport(local_sent.clone()),
        1,
        activation_negotiation(),
    );
    endpoints.insert(
        remote,
        RecordingTransport(remote_sent.clone()),
        7,
        activation_negotiation(),
        false,
    );
    assert!(endpoints.set_active(&ClientEndpointId::Local));

    state.compose(106, SIDEBAR_TEST_ROWS).unwrap();
    let rect = state
        .hits
        .endpoint_agents
        .iter()
        .find(|(_, endpoint_id, identity)| {
            endpoint_id.is_local()
                && matches!(
                    identity,
                    AgentIdentity::OpenCodeTab { session_id, .. } if session_id == "ses_b"
                )
        })
        .map(|(rect, _, _)| *rect)
        .expect("root-b row is clickable");

    let outcome = state.handle_raw_events(vec![RawInputEvent::Mouse(MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: rect.x + 1,
        row: rect.y,
        modifiers: KeyModifiers::empty(),
    })]);

    let mut commands = EndpointCommands::default();
    let mut scheduled = None;
    dispatch_client_shell_actions(
        outcome.actions,
        &mut commands,
        &mut endpoints,
        Some(&mut state),
        &mut Vec::new(),
        &mut scheduled,
    )
    .unwrap();
    let Some(ClientLoopEvent::ActivateEndpoint {
        endpoint_id,
        target,
        force,
    }) = scheduled.take()
    else {
        panic!("click must schedule an endpoint activation");
    };
    assert_eq!(endpoint_id, ClientEndpointId::Local);
    assert_eq!(
        target,
        Some(ClientEndpointFocusTarget::Pane("pane_1".into()))
    );

    let mut client_state = ClientState::test_new();
    client_state.shell = Some(state);
    let mut pending = None;
    let mut scheduled_activation = None;
    begin_endpoint_activation(
        &mut client_state,
        &mut endpoints,
        &mut commands,
        &mut pending,
        &mut 60,
        endpoint_id,
        target,
        force,
        Instant::now(),
        &mut scheduled_activation,
    )
    .unwrap();
    assert!(pending.is_none(), "fast path must not start an activation");
    assert!(scheduled_activation.is_none());

    let sent_requests = || {
        local_sent
            .lock()
            .unwrap()
            .iter()
            .filter_map(|message| {
                let crate::protocol::ClientMessage::ClientShellEndpointRequest {
                    boot_id,
                    request,
                } = message
                else {
                    return None;
                };
                let request = serde_json::from_str::<crate::api::schema::Request>(request).ok()?;
                Some((boot_id.clone(), request.id, request.method))
            })
            .collect::<Vec<_>>()
    };
    let after_focus = sent_requests();
    assert_eq!(
        after_focus.len(),
        1,
        "pane focus is the only in-flight request: {:?}",
        after_focus
    );
    assert!(matches!(
        &after_focus[0].2,
        crate::api::schema::Method::PaneFocus(target) if target.pane_id == "pane_1"
    ));

    let (boot_id, request_id, _) = &after_focus[0];
    let response = serde_json::to_vec(&crate::api::schema::SuccessResponse {
        id: request_id.clone(),
        result: crate::api::schema::ResponseResult::Pong {
            version: "test".into(),
            protocol: 1,
            capabilities: None,
        },
    })
    .unwrap();
    assert!(commands
        .receive_chunk(
            &ClientEndpointId::Local,
            1,
            boot_id,
            request_id,
            true,
            response,
        )
        .unwrap()
        .is_some());
    assert!(commands
        .send_next(&ClientEndpointId::Local, &mut endpoints)
        .is_empty());

    let methods = sent_requests()
        .into_iter()
        .map(|(_, _, method)| method)
        .collect::<Vec<_>>();
    assert_eq!(
        methods.len(),
        2,
        "pane focus then tab report: {:?}",
        methods
    );
    let crate::api::schema::Method::PaneReportMetadata(params) = &methods[1] else {
        panic!("expected pane.report_metadata, got {:?}", methods[1]);
    };
    assert_eq!(params.pane_id, "pane_1");
    assert_eq!(params.source, crate::opencode_tabs::FOCUS_SOURCE);
    assert_eq!(params.ttl_ms, Some(crate::opencode_tabs::FOCUS_TTL_MS));
    let raw = params
        .tokens
        .get(crate::opencode_tabs::FOCUS_TOKEN)
        .and_then(Option::as_deref)
        .expect("focus token");
    let value: serde_json::Value = serde_json::from_str(raw).expect("focus JSON");
    assert_eq!(value["v"], 1);
    assert_eq!(value["owner"], OWNER);
    assert_eq!(value["sessionID"], "ses_b");
    let request_id = value["requestID"].as_str().expect("requestID");
    assert_eq!(request_id.len(), 32);
    assert!(request_id.bytes().all(|byte| byte.is_ascii_hexdigit()));

    assert!(
        client_state
            .shell
            .as_ref()
            .unwrap()
            .pending_opencode_focus
            .is_none(),
        "pending focus must be consumed exactly once"
    );
}
