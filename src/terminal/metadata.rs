use std::collections::HashMap;
use std::time::{Duration, Instant};

use crate::detect::AgentState;

use super::{TerminalState, TerminalStateMutation};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentMetadata {
    pub source: String,
    pub agent_label: Option<String>,
    pub applies_to_source: Option<String>,
    pub title: Option<String>,
    pub display_agent: Option<String>,
    pub state_labels: HashMap<String, String>,
    pub reported_at: Instant,
    title_reported_at: Option<Instant>,
    display_agent_reported_at: Option<Instant>,
    state_label_reported_at: HashMap<String, Instant>,
    pub ttl: Option<Duration>,
    expiry_event_pending: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentMetadataReport {
    pub source: String,
    pub agent_label: Option<String>,
    pub applies_to_source: Option<String>,
    pub title: Option<String>,
    pub display_agent: Option<String>,
    pub state_labels: HashMap<String, String>,
    pub clear_title: bool,
    pub clear_display_agent: bool,
    pub clear_state_labels: bool,
    pub ttl: Option<Duration>,
    pub seq: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EffectivePresentation {
    pub title: Option<String>,
    pub display_agent: Option<String>,
    pub state_labels: HashMap<String, String>,
}

impl EffectivePresentation {
    fn empty() -> Self {
        Self {
            title: None,
            display_agent: None,
            state_labels: HashMap::new(),
        }
    }
}

impl TerminalState {
    pub(crate) fn metadata_report_sequence_is_fresh(&self, source: &str, seq: Option<u64>) -> bool {
        crate::metadata_tokens::sequence_is_fresh(&self.metadata_report_sequences, source, seq)
    }

    pub(crate) fn metadata_report_agent(
        source: &str,
        agent_label: Option<&str>,
        applies_to_source: Option<&str>,
    ) -> Option<crate::detect::Agent> {
        agent_label
            .and_then(crate::detect::parse_agent_label)
            .or_else(|| {
                crate::detect::Agent::ALL.iter().copied().find(|agent| {
                    let agent_label = crate::detect::agent_label(*agent);
                    crate::agent_resume::is_official_agent_source(source, agent_label)
                        || applies_to_source.is_some_and(|source| {
                            crate::agent_resume::is_official_agent_source(source, agent_label)
                        })
                })
            })
    }

    pub(crate) fn metadata_report_blocked_by_process_exit(
        &self,
        source: &str,
        agent_label: Option<&str>,
        applies_to_source: Option<&str>,
    ) -> bool {
        let Some(exit) = self.recent_agent_process_exit else {
            return false;
        };
        let exited_agent_label = crate::detect::agent_label(exit.agent);
        agent_label.and_then(crate::detect::parse_agent_label) == Some(exit.agent)
            || crate::agent_resume::is_official_agent_source(source, exited_agent_label)
            || applies_to_source.is_some_and(|source| {
                crate::agent_resume::is_official_agent_source(source, exited_agent_label)
            })
    }

    pub(crate) fn accept_metadata_report(
        &mut self,
        source: &str,
        seq: Option<u64>,
        includes_tokens: bool,
        agent: Option<crate::detect::Agent>,
    ) -> Result<bool, ()> {
        let Some(seq) = seq else {
            return Ok(true);
        };
        if !self.metadata_report_sequence_is_fresh(source, Some(seq)) {
            return Ok(false);
        }
        if includes_tokens
            && !self.metadata_token_sequence_sources.contains(source)
            && self.metadata_token_sequence_sources.len()
                >= crate::metadata_tokens::MAX_SEQUENCE_SOURCES
        {
            return Err(());
        }
        self.metadata_report_sequences
            .insert(source.to_string(), seq);
        if let Some(agent) = agent {
            self.metadata_report_agents
                .insert(source.to_string(), agent);
        }
        if includes_tokens {
            self.metadata_token_sequence_sources
                .insert(source.to_string());
        }
        Ok(true)
    }

    fn metadata_guards_match(
        &self,
        agent_label: Option<&str>,
        applies_to_source: Option<&str>,
    ) -> bool {
        if agent_label.is_some_and(|agent| self.effective_agent_label() != Some(agent)) {
            return false;
        }
        !applies_to_source.is_some_and(|source| {
            self.hook_authority
                .as_ref()
                .is_none_or(|authority| authority.source != source)
        })
    }

    pub fn set_agent_metadata(
        &mut self,
        report: AgentMetadataReport,
    ) -> Option<TerminalStateMutation> {
        if self.metadata_report_blocked_by_process_exit(
            &report.source,
            report.agent_label.as_deref(),
            report.applies_to_source.as_deref(),
        ) {
            return None;
        }
        let report_agent = Self::metadata_report_agent(
            &report.source,
            report.agent_label.as_deref(),
            report.applies_to_source.as_deref(),
        );
        if !matches!(
            self.accept_metadata_report(&report.source, report.seq, false, report_agent),
            Ok(true)
        ) {
            return None;
        }

        let now = Instant::now();
        if self
            .agent_metadata
            .get(&report.source)
            .is_some_and(|metadata| self.agent_metadata_is_expired(metadata, now))
        {
            self.agent_metadata.remove(&report.source);
        }
        let previous_agent_label = self.effective_agent_label().map(str::to_string);
        let previous_known_agent = self.effective_known_agent();
        let previous_state = self.state;
        let previous_presentation = self.effective_presentation_for_state_at(previous_state, now);
        let has_set_fields = report.title.is_some()
            || report.display_agent.is_some()
            || !report.state_labels.is_empty();

        let report_source = report.source.clone();
        let report_has_ttl = report.ttl.is_some();

        if report.clear_title || report.clear_display_agent || report.clear_state_labels {
            let metadata = self
                .agent_metadata
                .entry(report.source.clone())
                .or_insert_with(|| AgentMetadata {
                    source: report.source.clone(),
                    agent_label: report.agent_label.clone(),
                    applies_to_source: report.applies_to_source.clone(),
                    title: None,
                    display_agent: None,
                    state_labels: HashMap::new(),
                    reported_at: now,
                    title_reported_at: None,
                    display_agent_reported_at: None,
                    state_label_reported_at: HashMap::new(),
                    ttl: report.ttl,
                    expiry_event_pending: false,
                });
            if report.clear_title {
                metadata.title = None;
                metadata.title_reported_at = None;
            }
            if report.clear_display_agent {
                metadata.display_agent = None;
                metadata.display_agent_reported_at = None;
            }
            if report.clear_state_labels {
                metadata.state_labels.clear();
                metadata.state_label_reported_at.clear();
            }
            if let Some(agent_label) = report.agent_label {
                metadata.agent_label = Some(agent_label);
            }
            if let Some(applies_to_source) = report.applies_to_source {
                metadata.applies_to_source = Some(applies_to_source);
            }
            if let Some(title) = report.title {
                metadata.title = Some(title);
                metadata.title_reported_at = Some(now);
            }
            if let Some(display_agent) = report.display_agent {
                metadata.display_agent = Some(display_agent);
                metadata.display_agent_reported_at = Some(now);
            }
            for (state, label) in report.state_labels {
                metadata.state_labels.insert(state.clone(), label);
                metadata.state_label_reported_at.insert(state, now);
            }
            if has_set_fields || report.ttl.is_some() {
                metadata.reported_at = now;
                metadata.ttl = report.ttl;
                metadata.expiry_event_pending = false;
            }
        } else {
            let title_reported_at = report.title.as_ref().map(|_| now);
            let display_agent_reported_at = report.display_agent.as_ref().map(|_| now);
            let state_label_reported_at = report
                .state_labels
                .keys()
                .map(|state| (state.clone(), now))
                .collect();
            self.agent_metadata.insert(
                report.source.clone(),
                AgentMetadata {
                    source: report.source,
                    agent_label: report.agent_label,
                    applies_to_source: report.applies_to_source,
                    title: report.title,
                    display_agent: report.display_agent,
                    state_labels: report.state_labels,
                    reported_at: now,
                    title_reported_at,
                    display_agent_reported_at,
                    state_label_reported_at,
                    ttl: report.ttl,
                    expiry_event_pending: false,
                },
            );
        }

        if report_has_ttl
            && self
                .agent_metadata
                .get(&report_source)
                .is_some_and(|metadata| self.agent_metadata_is_visible_ignoring_ttl(metadata))
        {
            if let Some(metadata) = self.agent_metadata.get_mut(&report_source) {
                metadata.expiry_event_pending = true;
            }
        }

        let effective_state_change = self.recompute_effective_state(
            previous_agent_label,
            previous_known_agent,
            previous_state,
            previous_presentation,
            now,
        );

        if effective_state_change.is_some() {
            self.clear_expiry_pending_for_hidden_metadata();
        }

        Some(TerminalStateMutation {
            effective_state_change,
            session_ref_changed: false,
            agent_released: false,
        })
    }
    pub fn effective_title(&self) -> Option<String> {
        self.effective_presentation_for_state_at(self.state, Instant::now())
            .title
    }

    pub fn effective_display_agent(&self) -> Option<String> {
        self.effective_presentation_for_state_at(self.state, Instant::now())
            .display_agent
    }

    pub fn effective_presentation(&self) -> EffectivePresentation {
        self.effective_presentation_for_state_at(self.state, Instant::now())
    }

    pub fn next_agent_metadata_expiry(&self) -> Option<Instant> {
        let now = Instant::now();
        self.agent_metadata
            .values()
            .filter(|metadata| self.agent_metadata_matches_guards(metadata))
            .filter_map(|metadata| self.agent_metadata_expiry(metadata))
            .filter(|deadline| {
                *deadline > now
                    || self.agent_metadata.values().any(|metadata| {
                        metadata.expiry_event_pending
                            && self.agent_metadata_matches_guards(metadata)
                            && self.agent_metadata_expiry(metadata) == Some(*deadline)
                    })
            })
            .min()
    }

    pub fn expire_agent_metadata_at(
        &mut self,
        scheduled_deadline: Instant,
        now: Instant,
    ) -> Option<TerminalStateMutation> {
        let (expired_sources, stale_sources): (Vec<_>, Vec<_>) = self
            .agent_metadata
            .iter()
            .filter_map(|(source, metadata)| {
                let deadline = self.agent_metadata_expiry(metadata)?;
                (deadline <= now).then_some((source.clone(), deadline))
            })
            .partition(|(_, deadline)| *deadline >= scheduled_deadline);
        let expired_sources: Vec<_> = expired_sources
            .into_iter()
            .map(|(source, _)| source)
            .collect();
        let stale_sources: Vec<_> = stale_sources
            .into_iter()
            .map(|(source, _)| source)
            .collect();
        for source in stale_sources {
            self.agent_metadata.remove(&source);
        }
        if expired_sources.is_empty() {
            return None;
        }

        let previous_agent_label = self.effective_agent_label().map(str::to_string);
        let previous_known_agent = self.effective_known_agent();
        let previous_state = self.state;
        let previous_presentation =
            self.effective_presentation_for_state_at_ignoring_ttl(previous_state, now);
        for source in expired_sources {
            if let Some(metadata) = self.agent_metadata.get_mut(&source) {
                metadata.expiry_event_pending = false;
            }
            self.agent_metadata.remove(&source);
        }

        Some(TerminalStateMutation {
            effective_state_change: self.recompute_effective_state(
                previous_agent_label,
                previous_known_agent,
                previous_state,
                previous_presentation,
                now,
            ),
            session_ref_changed: false,
            agent_released: false,
        })
    }

    pub(super) fn effective_presentation_for_state_at(
        &self,
        state: AgentState,
        now: Instant,
    ) -> EffectivePresentation {
        self.effective_presentation_for_state_at_with_ttl(state, now, true)
    }

    fn effective_presentation_for_state_at_ignoring_ttl(
        &self,
        state: AgentState,
        now: Instant,
    ) -> EffectivePresentation {
        self.effective_presentation_for_state_at_with_ttl(state, now, false)
    }

    fn effective_presentation_for_state_at_with_ttl(
        &self,
        _state: AgentState,
        now: Instant,
        enforce_ttl: bool,
    ) -> EffectivePresentation {
        let mut presentation = EffectivePresentation::empty();
        presentation.title = self.newest_metadata_title(now, enforce_ttl);
        presentation.display_agent = self.newest_metadata_display_agent(now, enforce_ttl);
        presentation.state_labels = self.effective_metadata_state_labels(now, enforce_ttl);
        presentation
    }

    fn valid_agent_metadata(
        &self,
        now: Instant,
        enforce_ttl: bool,
    ) -> impl Iterator<Item = &AgentMetadata> {
        self.agent_metadata
            .values()
            .filter(move |metadata| self.agent_metadata_is_valid(metadata, now, enforce_ttl))
    }

    fn newest_metadata_title(&self, now: Instant, enforce_ttl: bool) -> Option<String> {
        self.valid_agent_metadata(now, enforce_ttl)
            .filter(|metadata| metadata.title.is_some())
            .max_by_key(|metadata| metadata.title_reported_at)
            .and_then(|metadata| metadata.title.clone())
    }

    fn newest_metadata_display_agent(&self, now: Instant, enforce_ttl: bool) -> Option<String> {
        self.valid_agent_metadata(now, enforce_ttl)
            .filter(|metadata| metadata.display_agent.is_some())
            .max_by_key(|metadata| metadata.display_agent_reported_at)
            .and_then(|metadata| metadata.display_agent.clone())
    }

    fn effective_metadata_state_labels(
        &self,
        now: Instant,
        enforce_ttl: bool,
    ) -> HashMap<String, String> {
        let mut labels: Vec<_> = self
            .valid_agent_metadata(now, enforce_ttl)
            .flat_map(|metadata| {
                metadata.state_labels.iter().filter_map(|(state, label)| {
                    Some((
                        *metadata.state_label_reported_at.get(state)?,
                        state.clone(),
                        label.clone(),
                    ))
                })
            })
            .collect();
        labels.sort_by_key(|(reported_at, _, _)| *reported_at);
        labels
            .into_iter()
            .map(|(_, state, label)| (state, label))
            .collect()
    }

    fn agent_metadata_is_valid(
        &self,
        metadata: &AgentMetadata,
        now: Instant,
        enforce_ttl: bool,
    ) -> bool {
        if metadata.title.is_none()
            && metadata.display_agent.is_none()
            && metadata.state_labels.is_empty()
        {
            return false;
        }
        if enforce_ttl && self.agent_metadata_is_expired(metadata, now) {
            return false;
        }
        self.agent_metadata_matches_guards(metadata)
    }

    fn agent_metadata_is_visible_ignoring_ttl(&self, metadata: &AgentMetadata) -> bool {
        (metadata.title.is_some()
            || metadata.display_agent.is_some()
            || !metadata.state_labels.is_empty())
            && self.agent_metadata_matches_guards(metadata)
    }

    pub(super) fn clear_expiry_pending_for_hidden_metadata(&mut self) {
        let hidden_sources: Vec<_> = self
            .agent_metadata
            .iter()
            .filter(|(_, metadata)| {
                metadata.expiry_event_pending
                    && !self.agent_metadata_is_visible_ignoring_ttl(metadata)
            })
            .map(|(source, _)| source.clone())
            .collect();
        for source in hidden_sources {
            if let Some(metadata) = self.agent_metadata.get_mut(&source) {
                metadata.expiry_event_pending = false;
            }
        }
    }

    fn agent_metadata_is_expired(&self, metadata: &AgentMetadata, now: Instant) -> bool {
        self.agent_metadata_expiry(metadata)
            .is_some_and(|deadline| now >= deadline)
    }

    fn agent_metadata_expiry(&self, metadata: &AgentMetadata) -> Option<Instant> {
        metadata.ttl.map(|ttl| {
            metadata
                .reported_at
                .checked_add(ttl)
                .unwrap_or(metadata.reported_at)
        })
    }

    fn agent_metadata_matches_guards(&self, metadata: &AgentMetadata) -> bool {
        self.metadata_guards_match(
            metadata.agent_label.as_deref(),
            metadata.applies_to_source.as_deref(),
        )
    }

    /// The live, unexpired OpenCode root tab snapshot stored for this pane.
    ///
    /// Expiry is enforced from the stored `expiresAtMs` so a focus validation
    /// fails closed even before the token TTL sweep removes the value.
    pub(crate) fn opencode_tabs_snapshot(
        &self,
        _now: Instant,
    ) -> Option<crate::opencode_tabs::TabSnapshot> {
        if self.metadata_report_blocked_by_process_exit(
            crate::opencode_tabs::TABS_SOURCE,
            Some(crate::opencode_tabs::OPENCODE_AGENT),
            None,
        ) {
            return None;
        }
        let raw = self
            .metadata_tokens
            .values()
            .get(crate::opencode_tabs::TABS_TOKEN)
            .cloned()?;
        let snapshot = crate::opencode_tabs::parse_snapshot(&raw).ok()?;
        crate::opencode_tabs::snapshot_is_live(&snapshot, crate::opencode_tabs::unix_millis())
            .then_some(snapshot)
    }

    /// Validates an OpenCode tab snapshot report WITHOUT storing it.
    ///
    /// Splitting validation from storage lets the report handler reject before
    /// sequence acceptance, so an ignored (stale) report never mutates tokens.
    pub(crate) fn validate_opencode_tabs_token(
        &self,
        raw: &str,
    ) -> Result<crate::opencode_tabs::TabSnapshot, crate::opencode_tabs::TabError> {
        crate::opencode_tabs::parse_snapshot(raw)
    }

    /// Validates an OpenCode tab focus report WITHOUT storing it.
    ///
    /// The addressed pane must hold a live snapshot for the same owner that
    /// still contains the requested session.
    pub(crate) fn validate_opencode_focus_token(
        &self,
        raw: &str,
        now: Instant,
    ) -> Result<crate::opencode_tabs::FocusRequest, crate::opencode_tabs::TabError> {
        let request = crate::opencode_tabs::parse_focus_request(raw)?;
        // Process evidence outranks a stored snapshot: a pane whose OpenCode
        // process exited is not a live OpenCode pane, even if a snapshot is
        // still present.
        if self.metadata_report_blocked_by_process_exit(
            crate::opencode_tabs::FOCUS_SOURCE,
            Some(crate::opencode_tabs::OPENCODE_AGENT),
            None,
        ) {
            return Err(crate::opencode_tabs::TabError {
                code: "invalid_opencode_focus",
                message: "focus target pane has no live opencode process".into(),
            });
        }
        let Some(snapshot) = self.opencode_tabs_snapshot(now) else {
            return Err(crate::opencode_tabs::TabError {
                code: "invalid_opencode_focus",
                message: "focus target has no live opencode tabs snapshot".into(),
            });
        };
        if snapshot.owner != request.owner
            || !snapshot
                .tabs
                .iter()
                .any(|tab| tab.session_id == request.session_id)
        {
            return Err(crate::opencode_tabs::TabError {
                code: "invalid_opencode_focus",
                message: "focus target is not a live opencode tab".into(),
            });
        }
        Ok(request)
    }

    /// Stores a validated OpenCode tab snapshot with its fixed TTL.
    ///
    /// A non-empty snapshot with a new owner replaces the old owner and clears
    /// any pending focus token. An empty snapshot only clears a matching owner.
    pub(crate) fn commit_opencode_tabs_snapshot(
        &mut self,
        snapshot: crate::opencode_tabs::TabSnapshot,
        now: Instant,
    ) -> bool {
        if self.metadata_report_blocked_by_process_exit(
            crate::opencode_tabs::TABS_SOURCE,
            Some(crate::opencode_tabs::OPENCODE_AGENT),
            None,
        ) {
            return self.clear_opencode_reserved_tokens();
        }
        let previous = self.opencode_tabs_snapshot(now);
        if snapshot.tabs.is_empty() {
            let Some(previous) = previous else {
                return false;
            };
            if previous.owner != snapshot.owner {
                return false;
            }
            let mut changed = self.metadata_tokens.patch(
                clear_reserved_patch(crate::opencode_tabs::TABS_TOKEN),
                None,
                now,
            );
            changed |= self.metadata_tokens.patch(
                clear_reserved_patch(crate::opencode_tabs::FOCUS_TOKEN),
                None,
                now,
            );
            return changed;
        }

        let owner_changed = previous
            .as_ref()
            .is_none_or(|previous| previous.owner != snapshot.owner);
        let mut stored = snapshot;
        stored.expires_at_ms = Some(
            crate::opencode_tabs::unix_millis().saturating_add(crate::opencode_tabs::TABS_TTL_MS),
        );
        let value = crate::opencode_tabs::encode_snapshot(&stored);
        let patch = HashMap::from([(crate::opencode_tabs::TABS_TOKEN.to_string(), Some(value))]);
        let mut changed = self.metadata_tokens.patch(
            patch,
            Some(Duration::from_millis(crate::opencode_tabs::TABS_TTL_MS)),
            now,
        );
        if owner_changed {
            changed |= self.metadata_tokens.patch(
                clear_reserved_patch(crate::opencode_tabs::FOCUS_TOKEN),
                None,
                now,
            );
        }
        changed
    }

    /// Stores a validated OpenCode tab focus request with its fixed TTL.
    pub(crate) fn commit_opencode_focus_request(
        &mut self,
        request: crate::opencode_tabs::FocusRequest,
        now: Instant,
    ) -> bool {
        let mut stored = request;
        stored.expires_at_ms = Some(
            crate::opencode_tabs::unix_millis().saturating_add(crate::opencode_tabs::FOCUS_TTL_MS),
        );
        let value = crate::opencode_tabs::encode_focus_request(&stored);
        let patch = HashMap::from([(crate::opencode_tabs::FOCUS_TOKEN.to_string(), Some(value))]);
        self.metadata_tokens.patch(
            patch,
            Some(Duration::from_millis(crate::opencode_tabs::FOCUS_TTL_MS)),
            now,
        )
    }

    /// Validates then stores an OpenCode tab snapshot (test-facing wrapper).
    #[cfg(test)]
    pub(crate) fn apply_opencode_tabs_token(
        &mut self,
        raw: &str,
        now: Instant,
    ) -> Result<bool, crate::opencode_tabs::TabError> {
        let snapshot = self.validate_opencode_tabs_token(raw)?;
        Ok(self.commit_opencode_tabs_snapshot(snapshot, now))
    }

    /// Validates then stores an OpenCode tab focus request (test-facing wrapper).
    #[cfg(test)]
    pub(crate) fn apply_opencode_focus_token(
        &mut self,
        raw: &str,
        now: Instant,
    ) -> Result<bool, crate::opencode_tabs::TabError> {
        let request = self.validate_opencode_focus_token(raw, now)?;
        Ok(self.commit_opencode_focus_request(request, now))
    }

    /// Clears both reserved OpenCode tokens, returning whether anything changed.
    pub(crate) fn clear_opencode_reserved_tokens(&mut self) -> bool {
        let now = Instant::now();
        let mut changed = self.metadata_tokens.patch(
            clear_reserved_patch(crate::opencode_tabs::TABS_TOKEN),
            None,
            now,
        );
        changed |= self.metadata_tokens.patch(
            clear_reserved_patch(crate::opencode_tabs::FOCUS_TOKEN),
            None,
            now,
        );
        changed
    }
}

fn clear_reserved_patch(key: &str) -> HashMap<String, Option<String>> {
    HashMap::from([(key.to_string(), None)])
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;
    use crate::detect::Agent;
    use crate::terminal::TerminalId;

    fn test_terminal() -> TerminalState {
        TerminalState::new(TerminalId::alloc(), "/tmp".into())
    }

    #[test]
    fn presentation_sequences_remain_unbounded_while_token_sequences_are_bounded() {
        let mut terminal = test_terminal();
        for index in 0..=crate::metadata_tokens::MAX_SEQUENCE_SOURCES {
            assert_eq!(
                terminal.accept_metadata_report(&format!("source-{index}"), Some(1), false, None,),
                Ok(true)
            );
        }
        for index in 0..crate::metadata_tokens::MAX_SEQUENCE_SOURCES {
            assert_eq!(
                terminal.accept_metadata_report(&format!("source-{index}"), Some(2), true, None,),
                Ok(true)
            );
        }
        assert_eq!(
            terminal.accept_metadata_report(
                &format!("source-{}", crate::metadata_tokens::MAX_SEQUENCE_SOURCES),
                Some(2),
                true,
                None,
            ),
            Err(())
        );
    }

    #[test]
    fn custom_metadata_reanchors_sequence_after_process_restart() {
        let mut terminal = test_terminal();
        terminal.set_detected_state(Some(Agent::Pi), AgentState::Idle);
        let report = |seq, ttl| AgentMetadataReport {
            source: "custom:pi-metadata".into(),
            agent_label: Some("pi".into()),
            applies_to_source: None,
            title: Some("Pi task".into()),
            display_agent: None,
            state_labels: HashMap::new(),
            clear_title: false,
            clear_display_agent: false,
            clear_state_labels: false,
            ttl,
            seq: Some(seq),
        };
        assert!(terminal
            .set_agent_metadata(report(100, Some(Duration::ZERO)))
            .is_some());
        let deadline = terminal.next_agent_metadata_expiry().unwrap();
        terminal.expire_agent_metadata_at(deadline, deadline);
        assert!(terminal.agent_metadata.is_empty());
        let exit_at = Instant::now() + Duration::from_millis(1);
        terminal.set_detected_state_with_screen_signals_at(
            Some(Agent::Pi),
            AgentState::Idle,
            false,
            false,
            false,
            true,
            exit_at,
        );
        terminal.set_detected_state_with_screen_signals_at(
            None,
            AgentState::Unknown,
            false,
            false,
            false,
            false,
            exit_at + Duration::from_millis(1),
        );
        assert!(terminal.set_agent_metadata(report(1, None)).is_none());

        terminal.set_detected_state_with_screen_signals_at(
            Some(Agent::Pi),
            AgentState::Idle,
            false,
            false,
            false,
            false,
            exit_at + Duration::from_millis(2),
        );
        assert!(terminal.set_agent_metadata(report(1, None)).is_some());
    }

    #[test]
    fn user_agent_metadata_overrides_presentation_fields_only() {
        let mut terminal = test_terminal();
        terminal.set_hook_authority(
            "herdr:claude".into(),
            "claude".into(),
            AgentState::Working,
            None,
            None,
        );

        let mutation = terminal.set_agent_metadata(AgentMetadataReport {
            source: "user:presentation".into(),
            agent_label: Some("claude".into()),
            applies_to_source: Some("herdr:claude".into()),
            title: Some("Refactor auth".into()),
            display_agent: Some("Claude: auth".into()),
            state_labels: HashMap::from([("working".into(), "deep in the mines".into())]),
            clear_title: false,
            clear_display_agent: false,
            clear_state_labels: false,
            ttl: None,
            seq: None,
        });

        assert_eq!(terminal.state, AgentState::Working);
        assert_eq!(terminal.effective_agent_label(), Some("claude"));
        let presentation = terminal.effective_presentation();
        assert_eq!(presentation.title.as_deref(), Some("Refactor auth"));
        assert_eq!(presentation.display_agent.as_deref(), Some("Claude: auth"));
        assert_eq!(
            presentation.state_labels.get("working").map(String::as_str),
            Some("deep in the mines")
        );
        assert!(mutation.unwrap().effective_state_change.is_some());
    }

    #[test]
    fn metadata_title_takes_pane_border_priority() {
        let mut terminal = test_terminal();
        terminal.set_detected_state(Some(Agent::Claude), AgentState::Idle);
        terminal.set_manual_label("manual".into());
        terminal.set_agent_metadata(AgentMetadataReport {
            source: "user:presentation".into(),
            agent_label: Some("claude".into()),
            applies_to_source: None,
            title: Some("Prompt title".into()),
            display_agent: None,
            state_labels: HashMap::new(),
            clear_title: false,
            clear_display_agent: false,
            clear_state_labels: false,
            ttl: None,
            seq: None,
        });

        assert_eq!(
            terminal.border_label(false).as_deref(),
            Some("Prompt title")
        );
        assert_eq!(terminal.border_label(true).as_deref(), Some("Prompt title"));
    }

    #[test]
    fn metadata_without_sequence_can_update_same_source() {
        let mut terminal = test_terminal();
        terminal.set_hook_authority(
            "herdr:claude".into(),
            "claude".into(),
            AgentState::Working,
            None,
            None,
        );

        for title in ["first", "second"] {
            terminal.set_agent_metadata(AgentMetadataReport {
                source: "user:claude-title".into(),
                agent_label: Some("claude".into()),
                applies_to_source: Some("herdr:claude".into()),
                title: Some(title.into()),
                display_agent: None,
                state_labels: HashMap::new(),
                clear_title: false,
                clear_display_agent: false,
                clear_state_labels: false,
                ttl: None,
                seq: None,
            });
        }

        assert_eq!(terminal.effective_title().as_deref(), Some("second"));
    }

    #[test]
    fn metadata_resolves_newest_value_per_presentation_field() {
        let mut terminal = test_terminal();
        terminal.set_hook_authority(
            "herdr:claude".into(),
            "claude".into(),
            AgentState::Working,
            None,
            None,
        );
        terminal.set_agent_metadata(AgentMetadataReport {
            source: "user:title".into(),
            agent_label: Some("claude".into()),
            applies_to_source: Some("herdr:claude".into()),
            title: Some("Prompt title".into()),
            display_agent: None,
            state_labels: HashMap::new(),
            clear_title: false,
            clear_display_agent: false,
            clear_state_labels: false,
            ttl: None,
            seq: Some(1),
        });
        terminal.set_agent_metadata(AgentMetadataReport {
            source: "user:status".into(),
            agent_label: Some("claude".into()),
            applies_to_source: Some("herdr:claude".into()),
            title: None,
            display_agent: Some("Claude activity".into()),
            state_labels: HashMap::new(),
            clear_title: false,
            clear_display_agent: false,
            clear_state_labels: false,
            ttl: None,
            seq: Some(1),
        });

        let presentation = terminal.effective_presentation();
        assert_eq!(presentation.title.as_deref(), Some("Prompt title"));
        assert_eq!(
            presentation.display_agent.as_deref(),
            Some("Claude activity")
        );
    }

    #[test]
    fn partial_update_does_not_refresh_unchanged_field_precedence() {
        let mut terminal = test_terminal();
        terminal.set_hook_authority(
            "herdr:claude".into(),
            "claude".into(),
            AgentState::Working,
            None,
            None,
        );
        terminal.set_agent_metadata(AgentMetadataReport {
            source: "user:first".into(),
            agent_label: Some("claude".into()),
            applies_to_source: Some("herdr:claude".into()),
            title: None,
            display_agent: Some("First display".into()),
            state_labels: HashMap::new(),
            clear_title: false,
            clear_display_agent: false,
            clear_state_labels: false,
            ttl: None,
            seq: Some(1),
        });
        terminal.set_agent_metadata(AgentMetadataReport {
            source: "user:first".into(),
            agent_label: Some("claude".into()),
            applies_to_source: Some("herdr:claude".into()),
            title: Some("Fresh title".into()),
            display_agent: None,
            state_labels: HashMap::new(),
            clear_title: false,
            clear_display_agent: true,
            clear_state_labels: false,
            ttl: None,
            seq: Some(2),
        });

        let presentation = terminal.effective_presentation();
        assert_eq!(presentation.title.as_deref(), Some("Fresh title"));
        assert_eq!(presentation.display_agent, None);
    }

    #[test]
    fn metadata_clear_plus_set_without_ttl_does_not_keep_old_ttl() {
        let mut terminal = test_terminal();
        terminal.set_hook_authority(
            "herdr:claude".into(),
            "claude".into(),
            AgentState::Working,
            None,
            None,
        );
        terminal.set_agent_metadata(AgentMetadataReport {
            source: "user:status".into(),
            agent_label: Some("claude".into()),
            applies_to_source: Some("herdr:claude".into()),
            title: Some("Old title".into()),
            display_agent: None,
            state_labels: HashMap::new(),
            clear_title: false,
            clear_display_agent: false,
            clear_state_labels: false,
            ttl: Some(Duration::from_millis(1)),
            seq: None,
        });
        let old_deadline = terminal.next_agent_metadata_expiry().unwrap();

        terminal.set_agent_metadata(AgentMetadataReport {
            source: "user:status".into(),
            agent_label: Some("claude".into()),
            applies_to_source: Some("herdr:claude".into()),
            title: Some("Fresh title".into()),
            display_agent: None,
            state_labels: HashMap::new(),
            clear_title: false,
            clear_display_agent: false,
            clear_state_labels: false,
            ttl: None,
            seq: None,
        });

        assert_eq!(terminal.next_agent_metadata_expiry(), None);
        assert_eq!(terminal.effective_title().as_deref(), Some("Fresh title"));
        assert!(terminal
            .expire_agent_metadata_at(
                old_deadline + Duration::from_millis(1),
                old_deadline + Duration::from_millis(1)
            )
            .is_none());
        assert_eq!(terminal.effective_title().as_deref(), Some("Fresh title"));
    }

    #[test]
    fn metadata_clear_only_without_ttl_does_not_extend_old_ttl() {
        let mut terminal = test_terminal();
        terminal.set_hook_authority(
            "herdr:claude".into(),
            "claude".into(),
            AgentState::Working,
            None,
            None,
        );
        terminal.set_agent_metadata(AgentMetadataReport {
            source: "user:status".into(),
            agent_label: Some("claude".into()),
            applies_to_source: Some("herdr:claude".into()),
            title: Some("Prompt title".into()),
            display_agent: Some("Old display".into()),
            state_labels: HashMap::new(),
            clear_title: false,
            clear_display_agent: false,
            clear_state_labels: false,
            // Expiry is forced with the captured deadline below; keep the
            // no-extension assertion independent of wall-clock scheduling.
            ttl: Some(Duration::from_secs(60)),
            seq: None,
        });
        let old_deadline = terminal.next_agent_metadata_expiry().unwrap();

        terminal.set_agent_metadata(AgentMetadataReport {
            source: "user:status".into(),
            agent_label: None,
            applies_to_source: None,
            title: None,
            display_agent: None,
            state_labels: HashMap::new(),
            clear_title: false,
            clear_display_agent: true,
            clear_state_labels: false,
            ttl: None,
            seq: None,
        });

        assert_eq!(terminal.next_agent_metadata_expiry(), Some(old_deadline));
        let presentation = terminal.effective_presentation_for_state_at(
            terminal.state,
            old_deadline - Duration::from_millis(1),
        );
        assert_eq!(presentation.title.as_deref(), Some("Prompt title"));
        assert_eq!(presentation.display_agent, None);

        let mutation = terminal
            .expire_agent_metadata_at(old_deadline, old_deadline)
            .unwrap();
        assert!(mutation.effective_state_change.is_some());
        assert_eq!(terminal.effective_presentation().title, None);
    }

    #[test]
    fn metadata_ttl_expiry_reports_presentation_change() {
        let mut terminal = test_terminal();
        terminal.set_hook_authority(
            "herdr:claude".into(),
            "claude".into(),
            AgentState::Working,
            None,
            None,
        );
        terminal.set_agent_metadata(AgentMetadataReport {
            source: "user:status".into(),
            agent_label: Some("claude".into()),
            applies_to_source: Some("herdr:claude".into()),
            title: Some("Activity".into()),
            display_agent: None,
            state_labels: HashMap::new(),
            clear_title: false,
            clear_display_agent: false,
            clear_state_labels: false,
            ttl: Some(Duration::from_millis(1)),
            seq: None,
        });

        let deadline = terminal.next_agent_metadata_expiry().unwrap();
        let mutation = terminal
            .expire_agent_metadata_at(deadline, deadline)
            .unwrap();
        let change = mutation.effective_state_change.unwrap();

        assert_eq!(change.previous_state, AgentState::Working);
        assert_eq!(change.state, AgentState::Working);
        assert_eq!(
            change.previous_presentation.title.as_deref(),
            Some("Activity")
        );
        assert_eq!(change.presentation.title, None);
        assert_eq!(terminal.effective_title(), None);
    }

    #[test]
    fn stale_guarded_metadata_expiry_does_not_report_visible_change() {
        let mut terminal = test_terminal();
        terminal.set_hook_authority(
            "herdr:codex".into(),
            "codex".into(),
            AgentState::Working,
            None,
            None,
        );
        terminal.set_agent_metadata(AgentMetadataReport {
            source: "user:status".into(),
            agent_label: Some("claude".into()),
            applies_to_source: Some("herdr:claude".into()),
            title: Some("Stale".into()),
            display_agent: None,
            state_labels: HashMap::new(),
            clear_title: false,
            clear_display_agent: false,
            clear_state_labels: false,
            ttl: Some(Duration::from_millis(1)),
            seq: None,
        });
        let deadline = terminal
            .agent_metadata
            .get("user:status")
            .and_then(|metadata| terminal.agent_metadata_expiry(metadata))
            .unwrap();
        assert_eq!(terminal.next_agent_metadata_expiry(), None);
        assert_eq!(terminal.effective_title(), None);

        terminal.set_hook_authority(
            "herdr:claude".into(),
            "claude".into(),
            AgentState::Working,
            None,
            None,
        );
        let mutation = terminal.expire_agent_metadata_at(
            deadline + Duration::from_millis(1),
            deadline + Duration::from_millis(1),
        );

        assert!(mutation.is_none());
        assert!(!terminal.agent_metadata.contains_key("user:status"));
        assert_eq!(terminal.effective_title(), None);
    }

    #[test]
    fn late_metadata_expiry_reports_all_due_visible_changes() {
        let mut terminal = test_terminal();
        terminal.set_hook_authority(
            "herdr:claude".into(),
            "claude".into(),
            AgentState::Working,
            None,
            None,
        );
        terminal.set_agent_metadata(AgentMetadataReport {
            source: "user:first".into(),
            agent_label: Some("claude".into()),
            applies_to_source: Some("herdr:claude".into()),
            title: Some("First".into()),
            display_agent: None,
            state_labels: HashMap::new(),
            clear_title: false,
            clear_display_agent: false,
            clear_state_labels: false,
            ttl: Some(Duration::from_millis(1)),
            seq: None,
        });
        let first_deadline = terminal.next_agent_metadata_expiry().unwrap();
        terminal.set_agent_metadata(AgentMetadataReport {
            source: "user:second".into(),
            agent_label: Some("claude".into()),
            applies_to_source: Some("herdr:claude".into()),
            title: None,
            display_agent: Some("Second".into()),
            state_labels: HashMap::new(),
            clear_title: false,
            clear_display_agent: false,
            clear_state_labels: false,
            ttl: Some(Duration::from_millis(2)),
            seq: None,
        });
        let second_deadline = terminal
            .agent_metadata
            .get("user:second")
            .and_then(|metadata| terminal.agent_metadata_expiry(metadata))
            .unwrap();

        let mutation = terminal
            .expire_agent_metadata_at(first_deadline, second_deadline)
            .unwrap();
        let change = mutation.effective_state_change.unwrap();

        assert_eq!(change.previous_presentation.title.as_deref(), Some("First"));
        assert_eq!(
            change.previous_presentation.display_agent.as_deref(),
            Some("Second")
        );
        assert_eq!(change.presentation, EffectivePresentation::empty());
        assert!(terminal.agent_metadata.is_empty());
    }

    #[test]
    fn immediately_expired_visible_metadata_still_schedules_expiry_event() {
        let mut terminal = test_terminal();
        terminal.set_hook_authority(
            "herdr:claude".into(),
            "claude".into(),
            AgentState::Working,
            None,
            None,
        );
        terminal.set_agent_metadata(AgentMetadataReport {
            source: "user:status".into(),
            agent_label: Some("claude".into()),
            applies_to_source: Some("herdr:claude".into()),
            title: Some("Instant".into()),
            display_agent: None,
            state_labels: HashMap::new(),
            clear_title: false,
            clear_display_agent: false,
            clear_state_labels: false,
            ttl: Some(Duration::ZERO),
            seq: None,
        });

        let deadline = terminal
            .next_agent_metadata_expiry()
            .expect("pending expiry should be scheduled");
        let mutation = terminal
            .expire_agent_metadata_at(deadline, Instant::now())
            .unwrap();
        let change = mutation.effective_state_change.unwrap();

        assert_eq!(
            change.previous_presentation.title.as_deref(),
            Some("Instant")
        );
        assert_eq!(change.presentation.title, None);
        assert_eq!(terminal.effective_title(), None);
    }

    #[test]
    fn pending_metadata_expiry_clears_when_lifecycle_guard_hides_metadata() {
        let mut terminal = test_terminal();
        terminal.set_hook_authority(
            "herdr:claude".into(),
            "claude".into(),
            AgentState::Working,
            None,
            None,
        );
        terminal.set_agent_metadata(AgentMetadataReport {
            source: "user:status".into(),
            agent_label: Some("claude".into()),
            applies_to_source: Some("herdr:claude".into()),
            title: Some("Instant".into()),
            display_agent: None,
            state_labels: HashMap::new(),
            clear_title: false,
            clear_display_agent: false,
            clear_state_labels: false,
            ttl: Some(Duration::ZERO),
            seq: None,
        });
        assert!(terminal.next_agent_metadata_expiry().is_some());

        terminal.set_hook_authority(
            "herdr:codex".into(),
            "codex".into(),
            AgentState::Working,
            None,
            None,
        );
        terminal.set_hook_authority(
            "herdr:claude".into(),
            "claude".into(),
            AgentState::Working,
            None,
            None,
        );

        assert_eq!(terminal.next_agent_metadata_expiry(), None);
        assert_eq!(terminal.effective_title(), None);
    }

    #[test]
    fn partial_update_does_not_resurrect_expired_hidden_metadata_fields() {
        let mut terminal = test_terminal();
        terminal.set_hook_authority(
            "herdr:codex".into(),
            "codex".into(),
            AgentState::Working,
            None,
            None,
        );
        terminal.set_agent_metadata(AgentMetadataReport {
            source: "user:status".into(),
            agent_label: Some("claude".into()),
            applies_to_source: Some("herdr:claude".into()),
            title: Some("Expired title".into()),
            display_agent: None,
            state_labels: HashMap::new(),
            clear_title: false,
            clear_display_agent: false,
            clear_state_labels: false,
            ttl: Some(Duration::ZERO),
            seq: None,
        });
        assert_eq!(terminal.next_agent_metadata_expiry(), None);

        terminal.set_agent_metadata(AgentMetadataReport {
            source: "user:status".into(),
            agent_label: Some("claude".into()),
            applies_to_source: Some("herdr:claude".into()),
            title: None,
            display_agent: Some("Fresh display".into()),
            state_labels: HashMap::new(),
            clear_title: false,
            clear_display_agent: false,
            clear_state_labels: false,
            ttl: None,
            seq: None,
        });
        terminal.set_hook_authority(
            "herdr:claude".into(),
            "claude".into(),
            AgentState::Working,
            None,
            None,
        );

        let presentation = terminal.effective_presentation();
        assert_eq!(presentation.title, None);
        assert_eq!(presentation.display_agent.as_deref(), Some("Fresh display"));
    }

    fn opencode_tabs_owner() -> &'static str {
        "0123456789abcdef0123456789abcdef"
    }

    fn opencode_tabs_json(owner: &str, tabs: &str) -> String {
        format!(r#"{{"v":1,"owner":"{owner}","tabs":[{tabs}]}}"#)
    }

    fn opencode_focus_json(owner: &str, session: &str) -> String {
        format!(
            r#"{{"v":1,"owner":"{owner}","sessionID":"{session}","requestID":"{}"}}"#,
            "a".repeat(32)
        )
    }

    #[test]
    fn opencode_tabs_snapshot_stores_validates_and_clears_for_matching_owner() {
        let mut terminal = test_terminal();
        let now = Instant::now();
        let owner = opencode_tabs_owner();
        let raw = opencode_tabs_json(
            owner,
            r#"{"sessionID":"ses-1","title":"Root","active":true,"unread":true}"#,
        );
        assert_eq!(terminal.apply_opencode_tabs_token(&raw, now), Ok(true));

        let stored = terminal.metadata_tokens.values()[crate::opencode_tabs::TABS_TOKEN].clone();
        let decoded = crate::opencode_tabs::parse_snapshot(&stored).unwrap();
        assert!(decoded.expires_at_ms.is_some());
        assert_eq!(decoded.owner, owner);
        assert_eq!(
            terminal.opencode_tabs_snapshot(now).unwrap().tabs[0].session_id,
            "ses-1"
        );

        let clear = opencode_tabs_json(owner, "");
        assert_eq!(terminal.apply_opencode_tabs_token(&clear, now), Ok(true));
        assert!(!terminal
            .metadata_tokens
            .values()
            .contains_key(crate::opencode_tabs::TABS_TOKEN));
        assert!(terminal.opencode_tabs_snapshot(now).is_none());
    }

    #[test]
    fn opencode_tabs_empty_snapshot_from_other_owner_is_ignored() {
        let mut terminal = test_terminal();
        let now = Instant::now();
        let raw = opencode_tabs_json(
            opencode_tabs_owner(),
            r#"{"sessionID":"ses-1","title":"Root"}"#,
        );
        assert_eq!(terminal.apply_opencode_tabs_token(&raw, now), Ok(true));

        let clear = opencode_tabs_json("ffffffffffffffffffffffffffffffff", "");
        assert_eq!(terminal.apply_opencode_tabs_token(&clear, now), Ok(false));
        assert!(terminal.opencode_tabs_snapshot(now).is_some());
    }

    #[test]
    fn opencode_tabs_owner_change_replaces_snapshot_and_clears_focus() {
        let mut terminal = test_terminal();
        let now = Instant::now();
        let raw = opencode_tabs_json(
            opencode_tabs_owner(),
            r#"{"sessionID":"ses-1","title":"Root"}"#,
        );
        assert_eq!(terminal.apply_opencode_tabs_token(&raw, now), Ok(true));
        let focus = opencode_focus_json(opencode_tabs_owner(), "ses-1");
        assert_eq!(terminal.apply_opencode_focus_token(&focus, now), Ok(true));
        assert!(terminal
            .metadata_tokens
            .values()
            .contains_key(crate::opencode_tabs::FOCUS_TOKEN));

        let replacement = opencode_tabs_json(
            "ffffffffffffffffffffffffffffffff",
            r#"{"sessionID":"ses-2","title":"Second"}"#,
        );
        assert_eq!(
            terminal.apply_opencode_tabs_token(&replacement, now),
            Ok(true)
        );
        assert!(!terminal
            .metadata_tokens
            .values()
            .contains_key(crate::opencode_tabs::FOCUS_TOKEN));
        let snapshot = terminal.opencode_tabs_snapshot(now).unwrap();
        assert_eq!(snapshot.owner, "ffffffffffffffffffffffffffffffff");
        assert_eq!(snapshot.tabs[0].session_id, "ses-2");
    }

    #[test]
    fn opencode_tabs_focus_requires_live_matching_snapshot() {
        let mut terminal = test_terminal();
        let now = Instant::now();
        let missing = opencode_focus_json(opencode_tabs_owner(), "ses-1");
        assert!(terminal.apply_opencode_focus_token(&missing, now).is_err());

        let raw = opencode_tabs_json(
            opencode_tabs_owner(),
            r#"{"sessionID":"ses-1","title":"Root"}"#,
        );
        terminal.apply_opencode_tabs_token(&raw, now).unwrap();

        let unknown_session = opencode_focus_json(opencode_tabs_owner(), "ses-other");
        assert!(terminal
            .apply_opencode_focus_token(&unknown_session, now)
            .is_err());
        let wrong_owner = opencode_focus_json("ffffffffffffffffffffffffffffffff", "ses-1");
        assert!(terminal
            .apply_opencode_focus_token(&wrong_owner, now)
            .is_err());
        assert!(!terminal
            .metadata_tokens
            .values()
            .contains_key(crate::opencode_tabs::FOCUS_TOKEN));

        let accepted = opencode_focus_json(opencode_tabs_owner(), "ses-1");
        assert_eq!(
            terminal.apply_opencode_focus_token(&accepted, now),
            Ok(true)
        );
        let stored = terminal.metadata_tokens.values()[crate::opencode_tabs::FOCUS_TOKEN].clone();
        assert!(crate::opencode_tabs::parse_focus_request(&stored)
            .unwrap()
            .expires_at_ms
            .is_some());
    }

    #[test]
    fn opencode_tabs_snapshot_ignores_writes_after_opencode_process_exit() {
        let mut terminal = test_terminal();
        let now = Instant::now();
        terminal.set_detected_state_with_visible_blocker(
            Some(Agent::OpenCode),
            AgentState::Working,
            false,
            false,
            true,
        );
        let raw = opencode_tabs_json(
            opencode_tabs_owner(),
            r#"{"sessionID":"ses-1","title":"Root"}"#,
        );
        terminal.apply_opencode_tabs_token(&raw, now).unwrap();
        assert!(terminal.opencode_tabs_snapshot(now).is_none());
    }

    #[test]
    fn opencode_tabs_process_exit_immediately_clears_reserved_tokens() {
        let mut terminal = test_terminal();
        let now = Instant::now();
        let raw = opencode_tabs_json(
            opencode_tabs_owner(),
            r#"{"sessionID":"ses-1","title":"Root"}"#,
        );
        assert_eq!(terminal.apply_opencode_tabs_token(&raw, now), Ok(true));
        assert_eq!(
            terminal.apply_opencode_focus_token(
                &opencode_focus_json(opencode_tabs_owner(), "ses-1"),
                now
            ),
            Ok(true)
        );
        let revision_before = terminal.revision;

        terminal.set_detected_state_with_visible_blocker(
            Some(Agent::OpenCode),
            AgentState::Idle,
            false,
            false,
            true,
        );

        let tokens = terminal.metadata_tokens.values();
        assert!(!tokens.contains_key(crate::opencode_tabs::TABS_TOKEN));
        assert!(!tokens.contains_key(crate::opencode_tabs::FOCUS_TOKEN));
        assert!(terminal.revision > revision_before);
        assert!(terminal.opencode_tabs_snapshot(now).is_none());
    }

    #[test]
    fn opencode_tabs_focus_is_rejected_after_opencode_process_exit() {
        let mut terminal = test_terminal();
        let now = Instant::now();
        let raw = opencode_tabs_json(
            opencode_tabs_owner(),
            r#"{"sessionID":"ses-1","title":"Root"}"#,
        );
        assert_eq!(terminal.apply_opencode_tabs_token(&raw, now), Ok(true));

        terminal.set_detected_state_with_visible_blocker(
            Some(Agent::OpenCode),
            AgentState::Idle,
            false,
            false,
            true,
        );

        let error = terminal
            .apply_opencode_focus_token(&opencode_focus_json(opencode_tabs_owner(), "ses-1"), now)
            .unwrap_err();
        assert_eq!(error.code, "invalid_opencode_focus");
        assert!(error.message.contains("no live opencode process"));
        assert!(!terminal
            .metadata_tokens
            .values()
            .contains_key(crate::opencode_tabs::FOCUS_TOKEN));
    }

    #[test]
    fn opencode_tabs_reserved_tokens_expire_on_their_fixed_ttl() {
        let mut terminal = test_terminal();
        let now = Instant::now();
        let raw = opencode_tabs_json(
            opencode_tabs_owner(),
            r#"{"sessionID":"ses-1","title":"Root"}"#,
        );
        terminal.apply_opencode_tabs_token(&raw, now).unwrap();
        terminal
            .apply_opencode_focus_token(&opencode_focus_json(opencode_tabs_owner(), "ses-1"), now)
            .unwrap();

        let focus_expiry = now
            + Duration::from_millis(crate::opencode_tabs::FOCUS_TTL_MS)
            + Duration::from_millis(1);
        assert!(terminal.metadata_tokens.expire_at(focus_expiry));
        assert!(!terminal
            .metadata_tokens
            .values()
            .contains_key(crate::opencode_tabs::FOCUS_TOKEN));
        assert!(terminal
            .metadata_tokens
            .values()
            .contains_key(crate::opencode_tabs::TABS_TOKEN));

        let tabs_expiry = now
            + Duration::from_millis(crate::opencode_tabs::TABS_TTL_MS)
            + Duration::from_millis(1);
        assert!(terminal.metadata_tokens.expire_at(tabs_expiry));
        assert!(terminal.opencode_tabs_snapshot(tabs_expiry).is_none());
    }

    #[test]
    fn opencode_tabs_reserved_tokens_leave_generic_metadata_untouched() {
        let mut terminal = test_terminal();
        let now = Instant::now();
        terminal.metadata_tokens.patch(
            HashMap::from([("summary".to_string(), Some("keep me".to_string()))]),
            None,
            now,
        );
        let raw = opencode_tabs_json(
            opencode_tabs_owner(),
            r#"{"sessionID":"ses-1","title":"Root"}"#,
        );
        assert_eq!(terminal.apply_opencode_tabs_token(&raw, now), Ok(true));
        assert_eq!(terminal.metadata_tokens.values()["summary"], "keep me");
        assert!(terminal.clear_opencode_reserved_tokens());
        assert_eq!(terminal.metadata_tokens.values()["summary"], "keep me");
        assert!(!terminal
            .metadata_tokens
            .values()
            .contains_key(crate::opencode_tabs::TABS_TOKEN));
    }
}
