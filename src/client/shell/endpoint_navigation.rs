use super::*;

impl ClientShellState {
    pub(super) fn active_endpoint_workspace_at(&self, point: (u16, u16)) -> Option<String> {
        self.hits
            .workspaces
            .iter()
            .find(|hit| {
                hit.endpoint_id == self.active_endpoint_id && super::contains(hit.rect, point)
            })
            .map(|hit| hit.workspace_id.clone())
    }

    pub(super) fn endpoint_workspace_is_draggable(&self, press: &ClientWorkspacePress) -> bool {
        press.endpoint_id == self.active_endpoint_id
            && self
                .snapshot
                .as_deref()
                .and_then(|snapshot| {
                    snapshot
                        .workspaces
                        .iter()
                        .find(|workspace| workspace.workspace_id == press.workspace_id)
                })
                .is_some_and(|workspace| {
                    !workspace
                        .worktree
                        .as_ref()
                        .is_some_and(|worktree| worktree.is_linked_worktree)
                })
    }

    pub(super) fn finish_endpoint_workspace_press(
        &mut self,
        press: ClientWorkspacePress,
        outcome: &mut ClientShellInput,
    ) {
        self.focus_or_activate(
            press.endpoint_id,
            ClientEndpointFocusTarget::Workspace(press.workspace_id),
            outcome,
        );
    }

    pub(super) fn handle_endpoint_machine_click(
        &mut self,
        point: (u16, u16),
        outcome: &mut ClientShellInput,
    ) -> bool {
        let Some(hit) = self
            .hits
            .machines
            .iter()
            .find(|hit| super::contains(hit.rect, point))
        else {
            return false;
        };
        let endpoint_id = hit.endpoint_id.clone();
        let collapse_toggle = super::contains(hit.collapse_toggle, point);
        if collapse_toggle || endpoint_id == self.active_endpoint_id {
            if !self.collapsed_endpoints.remove(&endpoint_id) {
                self.collapsed_endpoints.insert(endpoint_id.clone());
            }
            outcome.repaint = true;
            if !collapse_toggle && endpoint_id.is_local() {
                self.activate_endpoint(endpoint_id, outcome);
            }
        } else if endpoint_id.is_local() || self.endpoint_is_online(&endpoint_id) {
            outcome.actions.push(ClientShellAction::ActivateEndpoint {
                endpoint_id,
                target: None,
            });
        } else {
            let label = self.endpoint_label(&endpoint_id).to_owned();
            self.receive_endpoint_unavailable(format!("{label} is not ready"));
            outcome.repaint = true;
        }
        true
    }

    pub(super) fn handle_endpoint_agent_click(
        &mut self,
        point: (u16, u16),
        outcome: &mut ClientShellInput,
    ) -> bool {
        let Some((endpoint_id, identity)) = self
            .hits
            .endpoint_agents
            .iter()
            .find(|(rect, _, _)| super::contains(*rect, point))
            .map(|(_, endpoint_id, identity)| (endpoint_id.clone(), identity.clone()))
        else {
            return false;
        };
        self.focus_agent_identity(endpoint_id, identity, outcome);
        true
    }

    pub(super) fn handle_endpoint_navigation(
        &mut self,
        action: crate::input::KeybindAction,
        outcome: &mut ClientShellInput,
    ) -> bool {
        use crate::input::KeybindAction;
        if matches!(
            action,
            KeybindAction::PreviousAgent | KeybindAction::NextAgent | KeybindAction::FocusAgent(_)
        ) {
            self.reconcile_agent_selection();
            let agents = super::aggregate_navigation::online_agent_targets(
                &self.endpoints,
                &self.active_endpoint_id,
                self.config.agent_panel_sort,
            );
            if agents.is_empty() {
                return true;
            }
            let next = match action {
                KeybindAction::FocusAgent(index) => {
                    if index >= agents.len() {
                        return true;
                    }
                    index
                }
                KeybindAction::PreviousAgent | KeybindAction::NextAgent => {
                    // Walk from the newest intended selection first. The server
                    // may still report the previous tab as active for a moment;
                    // anchoring on that flag would make repeated presses bounce.
                    let current = self
                        .agent_selection
                        .as_ref()
                        .and_then(|selection| {
                            agents.iter().position(|target| {
                                target.endpoint_id == selection.endpoint_id
                                    && target.identity == selection.identity
                            })
                        })
                        .or_else(|| {
                            let focused = self
                                .snapshot
                                .as_deref()
                                .and_then(|snapshot| snapshot.focused_pane_id.as_deref());
                            agents
                                .iter()
                                .position(|target| {
                                    target.endpoint_id == self.active_endpoint_id
                                        && Some(target.identity.pane_id()) == focused
                                        && target.active
                                })
                                .or_else(|| {
                                    agents.iter().position(|target| {
                                        target.endpoint_id == self.active_endpoint_id
                                            && Some(target.identity.pane_id()) == focused
                                    })
                                })
                        });
                    match (current, action) {
                        (Some(index), KeybindAction::PreviousAgent) => {
                            (index + agents.len() - 1) % agents.len()
                        }
                        (Some(index), KeybindAction::NextAgent) => (index + 1) % agents.len(),
                        (None, KeybindAction::PreviousAgent) => agents.len() - 1,
                        _ => 0,
                    }
                }
                _ => unreachable!("endpoint agent navigation"),
            };
            let endpoint_id = agents[next].endpoint_id.clone();
            let identity = agents[next].identity.clone();
            if self.focus_agent_identity(endpoint_id.clone(), identity.clone(), outcome) {
                if endpoint_id == self.active_endpoint_id {
                    let before = self.agent_scroll;
                    self.reveal_endpoint_agent(
                        &endpoint_id,
                        &identity,
                        self.hits.agent_body.height,
                    );
                    if self.agent_scroll != before {
                        outcome.repaint = true;
                    }
                } else {
                    self.pending_agent_reveal = Some((endpoint_id, identity));
                    outcome.repaint = true;
                }
            }
            return true;
        }
        if !self.multi_endpoint_active() {
            return false;
        }
        if matches!(
            action,
            KeybindAction::PreviousWorkspace | KeybindAction::NextWorkspace
        ) {
            let workspaces = self
                .endpoints
                .iter()
                .filter(|endpoint| endpoint.status == ClientEndpointStatus::Online)
                .flat_map(|endpoint| {
                    endpoint
                        .snapshot
                        .as_deref()
                        .map_or_else(Vec::new, |snapshot| {
                            render::workspace_entries(snapshot, &HashSet::new())
                                .into_iter()
                                .filter_map(|entry| {
                                    snapshot.workspaces.get(entry.index).map(|workspace| {
                                        (
                                            endpoint.endpoint_id.clone(),
                                            workspace.workspace_id.clone(),
                                        )
                                    })
                                })
                                .collect()
                        })
                })
                .collect::<Vec<_>>();
            if workspaces.is_empty() {
                return true;
            }
            let focused = self
                .snapshot
                .as_deref()
                .and_then(|snapshot| snapshot.focused_workspace_id.as_deref());
            let current = workspaces.iter().position(|(endpoint_id, workspace_id)| {
                endpoint_id == &self.active_endpoint_id && Some(workspace_id.as_str()) == focused
            });
            let next = match (current, action) {
                (Some(index), KeybindAction::PreviousWorkspace) => {
                    (index + workspaces.len() - 1) % workspaces.len()
                }
                (Some(index), KeybindAction::NextWorkspace) => (index + 1) % workspaces.len(),
                (None, KeybindAction::PreviousWorkspace) => workspaces.len() - 1,
                (None, KeybindAction::NextWorkspace) => 0,
                _ => unreachable!("endpoint workspace navigation"),
            };
            let (endpoint_id, workspace_id) = workspaces[next].clone();
            self.focus_or_activate(
                endpoint_id,
                ClientEndpointFocusTarget::Workspace(workspace_id),
                outcome,
            );
            return true;
        }
        false
    }

    pub(super) fn activate_endpoint(
        &mut self,
        endpoint_id: ClientEndpointId,
        outcome: &mut ClientShellInput,
    ) -> bool {
        self.pending_workspace_highlight = None;
        self.pending_agent_reveal = None;
        self.clear_pending_opencode_focus();
        self.clear_agent_selection();
        let online = self.endpoint_is_online(&endpoint_id);
        if !online && !endpoint_id.is_local() {
            let label = self.endpoint_label(&endpoint_id).to_owned();
            self.receive_endpoint_unavailable(format!("{label} is not ready"));
            outcome.repaint = true;
            return false;
        }
        if (endpoint_id.is_local() && (self.multi_endpoint_active() || !online))
            || endpoint_id != self.active_endpoint_id
        {
            outcome.actions.push(ClientShellAction::ActivateEndpoint {
                endpoint_id,
                target: None,
            });
        }
        true
    }

    pub(super) fn focus_or_activate(
        &mut self,
        endpoint_id: ClientEndpointId,
        target: ClientEndpointFocusTarget,
        outcome: &mut ClientShellInput,
    ) -> bool {
        #[cfg(windows)]
        if !self.notification_target_is_current(&endpoint_id, &target) {
            return false;
        }
        if self.pending_workspace_highlight.take().is_some() {
            outcome.repaint = true;
        }
        self.pending_agent_reveal = None;
        self.clear_pending_opencode_focus();
        self.clear_agent_selection();
        let online = self.endpoint_is_online(&endpoint_id);
        if !online && !endpoint_id.is_local() {
            let label = self.endpoint_label(&endpoint_id).to_owned();
            self.receive_endpoint_unavailable(format!("{label} is not ready"));
            outcome.repaint = true;
            return false;
        }
        // Local can still be displayed while a remote activation is pending.
        // Route explicit selections through the runtime so they can cancel that handoff.
        if endpoint_id == self.active_endpoint_id
            && !(endpoint_id.is_local() && (self.multi_endpoint_active() || !online))
        {
            let method = match target {
                ClientEndpointFocusTarget::Workspace(workspace_id) => {
                    crate::api::schema::Method::WorkspaceFocus(
                        crate::api::schema::WorkspaceTarget { workspace_id },
                    )
                }
                ClientEndpointFocusTarget::Tab(tab_id) => {
                    crate::api::schema::Method::TabFocus(crate::api::schema::TabTarget { tab_id })
                }
                ClientEndpointFocusTarget::Pane(pane_id) => {
                    crate::api::schema::Method::PaneFocus(crate::api::schema::PaneTarget {
                        pane_id,
                    })
                }
                #[cfg(windows)]
                ClientEndpointFocusTarget::Notification { pane_id, .. } => {
                    crate::api::schema::Method::PaneFocus(crate::api::schema::PaneTarget {
                        pane_id,
                    })
                }
            };
            self.push_endpoint_method(method, outcome);
        } else {
            outcome.actions.push(ClientShellAction::ActivateEndpoint {
                endpoint_id,
                target: Some(target),
            });
        }
        true
    }

    pub(super) fn set_agent_selection(
        &mut self,
        endpoint_id: ClientEndpointId,
        identity: super::agent_sidebar::AgentIdentity,
    ) {
        let base_snapshot = self
            .endpoint_snapshot(&endpoint_id)
            .map(|snapshot| (snapshot.boot_id.clone(), snapshot.revision));
        self.agent_selection = Some(super::agent_sidebar::AgentSelection {
            endpoint_id,
            identity,
            deadline: std::time::Instant::now()
                + std::time::Duration::from_millis(crate::opencode_tabs::FOCUS_TTL_MS),
            base_snapshot,
        });
    }

    pub(super) fn clear_agent_selection(&mut self) {
        self.agent_selection = None;
    }

    /// Drops an intent that timed out or no longer names a live tab, and retires
    /// it once the authoritative snapshot acknowledges the same tab.
    pub(super) fn reconcile_agent_selection(&mut self) {
        let Some(selection) = self.agent_selection.as_ref() else {
            return;
        };
        if std::time::Instant::now() >= selection.deadline {
            self.agent_selection = None;
            return;
        }
        let endpoint_id = selection.endpoint_id.clone();
        let identity = selection.identity.clone();
        if endpoint_id != self.active_endpoint_id {
            let pending_same_endpoint = self
                .pending_opencode_focus
                .as_ref()
                .is_some_and(|(pending_endpoint, ..)| pending_endpoint == &endpoint_id);
            if !pending_same_endpoint {
                self.agent_selection = None;
            }
            return;
        }
        let Some(snapshot) = self.endpoint_snapshot(&endpoint_id) else {
            self.agent_selection = None;
            return;
        };
        if selection
            .base_snapshot
            .as_ref()
            .is_some_and(|(base_boot_id, _)| snapshot.boot_id != *base_boot_id)
        {
            self.agent_selection = None;
            return;
        }
        if !super::agent_sidebar::identity_is_current(snapshot, &identity) {
            self.agent_selection = None;
            return;
        }
        if super::agent_sidebar::identity_is_active(snapshot, &identity) {
            // Only a snapshot newer than the one the intent was formed against
            // can acknowledge the focus request. An already-buffered older
            // frame may still name the previous active tab.
            let acknowledged =
                selection
                    .base_snapshot
                    .as_ref()
                    .is_none_or(|(base_boot_id, base_revision)| {
                        snapshot.boot_id == *base_boot_id && snapshot.revision > *base_revision
                    });
            if acknowledged {
                self.agent_selection = None;
            }
        }
    }

    /// Focuses an agent row, including an individual OpenCode root tab.
    ///
    /// The identity is revalidated against the endpoint's current snapshot so a
    /// stale row fails closed. OpenCode tab rows additionally report a short
    /// lived focus token after the pane focus request.
    pub(super) fn focus_agent_identity(
        &mut self,
        endpoint_id: ClientEndpointId,
        identity: super::agent_sidebar::AgentIdentity,
        outcome: &mut ClientShellInput,
    ) -> bool {
        let current = self
            .endpoint_snapshot(&endpoint_id)
            .is_some_and(|snapshot| super::agent_sidebar::identity_is_current(snapshot, &identity));
        if !current {
            return true;
        }
        let direct = endpoint_id == self.active_endpoint_id
            && !(endpoint_id.is_local()
                && (self.multi_endpoint_active() || !self.endpoint_is_online(&endpoint_id)));
        // Focus targets the real pane; the tab owner stays in the focus token.
        let pane_id = identity.pane_id().to_owned();
        if !self.focus_or_activate(
            endpoint_id.clone(),
            ClientEndpointFocusTarget::Pane(pane_id),
            outcome,
        ) {
            return false;
        }
        if identity.opencode_tab().is_some() {
            self.set_agent_selection(endpoint_id.clone(), identity.clone());
            outcome.repaint = true;
        }
        if let Some((tab_owner, session_id)) = identity.opencode_tab() {
            if direct {
                self.report_opencode_focus(&identity, outcome);
            } else {
                self.pending_opencode_focus = Some((
                    endpoint_id,
                    identity.pane_id().to_owned(),
                    tab_owner.to_owned(),
                    session_id.to_owned(),
                ));
                self.pending_opencode_focus_deadline = Some(
                    std::time::Instant::now()
                        + std::time::Duration::from_millis(crate::opencode_tabs::FOCUS_TTL_MS),
                );
            }
        }
        true
    }

    pub(super) fn endpoint_snapshot(
        &self,
        endpoint_id: &ClientEndpointId,
    ) -> Option<&ClientShellSnapshot> {
        self.endpoints
            .iter()
            .find(|endpoint| &endpoint.endpoint_id == endpoint_id)
            .and_then(|endpoint| endpoint.snapshot.as_deref())
    }

    fn report_opencode_focus(
        &mut self,
        identity: &super::agent_sidebar::AgentIdentity,
        outcome: &mut ClientShellInput,
    ) {
        let super::agent_sidebar::AgentIdentity::OpenCodeTab {
            pane_id,
            owner,
            session_id,
        } = identity
        else {
            return;
        };
        let request_id = fresh_focus_request_id();
        let value = serde_json::json!({
            "v": 1,
            "owner": owner,
            "sessionID": session_id,
            "requestID": request_id,
        })
        .to_string();
        let mut tokens = HashMap::new();
        tokens.insert(crate::opencode_tabs::FOCUS_TOKEN.to_owned(), Some(value));
        let params = crate::api::schema::PaneReportMetadataParams {
            pane_id: pane_id.clone(),
            source: crate::opencode_tabs::FOCUS_SOURCE.to_owned(),
            agent: None,
            applies_to_source: None,
            title: None,
            display_agent: None,
            state_labels: HashMap::new(),
            tokens,
            clear_title: false,
            clear_display_agent: false,
            clear_state_labels: false,
            seq: None,
            ttl_ms: Some(crate::opencode_tabs::FOCUS_TTL_MS),
        };
        self.push_endpoint_method(
            crate::api::schema::Method::PaneReportMetadata(params),
            outcome,
        );
    }

    /// Queues the shell state that `focus_agent_identity` stores while an
    /// endpoint activates. Test-only seam for activation tests.
    #[cfg(test)]
    pub(crate) fn queue_pending_opencode_focus_for_test(
        &mut self,
        endpoint_id: ClientEndpointId,
        pane_id: &str,
        owner: &str,
        session_id: &str,
    ) {
        self.pending_opencode_focus = Some((
            endpoint_id,
            pane_id.to_owned(),
            owner.to_owned(),
            session_id.to_owned(),
        ));
        self.pending_opencode_focus_deadline = Some(
            std::time::Instant::now()
                + std::time::Duration::from_millis(crate::opencode_tabs::FOCUS_TTL_MS),
        );
    }

    /// Replays an OpenCode tab focus that was requested while its endpoint was
    /// still activating. Call once the activation has committed and the fresh
    /// snapshot is installed.
    ///
    /// Revalidates the live owner and tab membership against the current
    /// snapshot; a stale request is dropped without a request.
    pub(crate) fn send_pending_opencode_focus(&mut self, outcome: &mut ClientShellInput) -> bool {
        let Some((endpoint_id, pane_id, owner, session_id)) = self
            .pending_opencode_focus
            .as_ref()
            .map(|(endpoint_id, pane_id, owner, session_id)| {
                (
                    endpoint_id.clone(),
                    pane_id.clone(),
                    owner.clone(),
                    session_id.clone(),
                )
            })
        else {
            return false;
        };
        if self
            .pending_opencode_focus_deadline
            .is_some_and(|deadline| std::time::Instant::now() >= deadline)
        {
            self.clear_pending_opencode_focus();
            return false;
        }
        if endpoint_id != self.active_endpoint_id {
            // Activation did not commit for the pending endpoint. Drop the
            // request so a later activation cannot replay stale focus.
            self.clear_pending_opencode_focus();
            return false;
        }
        let identity = super::agent_sidebar::AgentIdentity::OpenCodeTab {
            pane_id,
            owner,
            session_id,
        };
        let current = self
            .endpoint_snapshot(&endpoint_id)
            .is_some_and(|snapshot| super::agent_sidebar::identity_is_current(snapshot, &identity));
        if !current {
            self.clear_pending_opencode_focus();
            return false;
        }
        self.clear_pending_opencode_focus();
        self.report_opencode_focus(&identity, outcome);
        true
    }

    fn clear_pending_opencode_focus(&mut self) {
        self.pending_opencode_focus = None;
        self.pending_opencode_focus_deadline = None;
    }
}

/// Generates a fresh 128-bit lowercase hex identifier for one OpenCode focus
/// request.
///
/// The shell request counter restarts at 1 for every client process, so two
/// clients or a client restart could reuse a request id that is still inside
/// the integration's dedupe window. Hashing wall-clock nanoseconds, the
/// process id, and a process-local sequence matches the existing profile id
/// mechanism and stays unique across clients and restarts.
fn fresh_focus_request_id() -> String {
    use sha2::{Digest as _, Sha256};
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT_FOCUS_REQUEST_ID: AtomicU64 = AtomicU64::new(1);
    let sequence = NEXT_FOCUS_REQUEST_ID.fetch_add(1, Ordering::Relaxed);
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let digest = Sha256::digest(format!("{}:{now}:{sequence}", std::process::id()).as_bytes());
    let mut id = String::with_capacity(crate::opencode_tabs::HEX_ID_CHARS);
    for byte in &digest[..16] {
        use std::fmt::Write as _;
        let _ = write!(id, "{byte:02x}");
    }
    id
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn focus_request_ids_are_fresh_128_bit_hex() {
        let first = fresh_focus_request_id();
        let second = fresh_focus_request_id();
        assert_eq!(first.len(), crate::opencode_tabs::HEX_ID_CHARS);
        assert!(first
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)));
        assert_ne!(first, second);
    }

    #[test]
    fn expired_pending_focus_is_cleared_without_replay() {
        let mut state = ClientShellState::new(ClientShellConfig::from_config(&Config::default()));
        state.pending_opencode_focus = Some((
            ClientEndpointId::Local,
            "pane_1".into(),
            "0123456789abcdef0123456789abcdef".into(),
            "ses_a".into(),
        ));
        state.pending_opencode_focus_deadline = std::time::Instant::now().checked_sub(
            std::time::Duration::from_millis(crate::opencode_tabs::FOCUS_TTL_MS + 1),
        );
        let mut outcome = ClientShellInput::default();
        assert!(!state.send_pending_opencode_focus(&mut outcome));
        assert!(state.pending_opencode_focus.is_none());
        assert!(state.pending_opencode_focus_deadline.is_none());
        assert!(outcome.actions.is_empty());
    }
}
