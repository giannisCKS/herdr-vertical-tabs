use std::collections::HashMap;

use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Modifier, Style},
    text::Line,
    widgets::{Paragraph, Widget},
};

use super::*;
use crate::protocol::ClientShellAgent;

pub(super) const OPENCODE_TABS_TOKEN: &str = crate::opencode_tabs::TABS_TOKEN;

pub(super) struct AgentRow {
    pub(super) identity: AgentIdentity,
    pub(super) status: crate::api::schema::AgentStatus,
    pub(super) focused: bool,
    pub(super) rows: Vec<Vec<crate::ui::ResolvedToken>>,
    /// Identity of the workspace this row hangs under, prefix included so two
    /// workspaces that happen to share a label never merge.
    pub(super) group_key: String,
    /// Leading token rows that name the workspace, counted on this row alone.
    pub(super) header_rows: usize,
    /// Column used by tab content after any retained workspace header. Tabs in
    /// a group with a header line up under it; a groupless row keeps the
    /// ordinary first-row column.
    pub(super) content_indent: usize,
}

/// Stable identity of a clickable agent sidebar entry.
///
/// A plain pane has one row. An OpenCode pane may expose several root tabs, each
/// with its own identity, without inventing new pane ids.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum AgentIdentity {
    Pane {
        pane_id: String,
    },
    OpenCodeTab {
        pane_id: String,
        owner: String,
        session_id: String,
    },
}

impl AgentIdentity {
    pub(super) fn pane_id(&self) -> &str {
        match self {
            Self::Pane { pane_id } | Self::OpenCodeTab { pane_id, .. } => pane_id,
        }
    }

    pub(super) fn opencode_tab(&self) -> Option<(&str, &str)> {
        match self {
            Self::OpenCodeTab {
                owner, session_id, ..
            } => Some((owner, session_id)),
            Self::Pane { .. } => None,
        }
    }
}

/// The tab the user last selected, held until the server reports that tab as
/// active. Rapid NextAgent/PreviousAgent presses walk from this intent instead
/// of from a snapshot whose `active` flag still names the previous tab.
///
/// This is presentation state, separate from the transport queue: sending the
/// focus request must not end the selection walk.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct AgentSelection {
    pub(super) endpoint_id: ClientEndpointId,
    pub(super) identity: AgentIdentity,
    pub(super) deadline: std::time::Instant,
    /// Snapshot the intent was formed against. A snapshot at or below this
    /// revision predates the focus request and cannot acknowledge it.
    pub(super) base_snapshot: Option<(String, u64)>,
}

/// Moves the highlight onto the intended tab before the authoritative snapshot
/// catches up. Ordinary pane rows keep their server-owned focus flag.
pub(super) fn apply_selection_intent(
    rows: &mut [AgentRow],
    endpoint_id: &ClientEndpointId,
    selection: Option<&AgentSelection>,
) {
    let Some(selection) = selection.filter(|selection| {
        &selection.endpoint_id == endpoint_id && std::time::Instant::now() < selection.deadline
    }) else {
        return;
    };
    for row in rows {
        if matches!(row.identity, AgentIdentity::OpenCodeTab { .. }) {
            row.focused = row.identity == selection.identity;
        } else {
            // The authoritative focused row may still name the previously
            // focused pane until the pending focus is acknowledged.
            row.focused = false;
        }
    }
}

/// True once the authoritative snapshot reports the selected tab as active.
pub(super) fn identity_is_active(snapshot: &ClientShellSnapshot, identity: &AgentIdentity) -> bool {
    let AgentIdentity::OpenCodeTab { pane_id, .. } = identity else {
        return false;
    };
    snapshot
        .agents
        .iter()
        .find(|agent| &agent.pane_id == pane_id)
        .filter(|agent| agent.focused)
        .and_then(expanded_tabs)
        .is_some_and(|tabs| {
            tabs.iter()
                .any(|tab| tab.active && tab.identity == *identity)
        })
}

struct TabEntry {
    identity: AgentIdentity,
    title: String,
    active: bool,
    status: crate::api::schema::AgentStatus,
}

/// Parses one agent's tab token. Callers expand a whole pane through this once
/// per pass; row count grows with tabs, parse count grows with panes only.
fn expanded_tabs(agent: &ClientShellAgent) -> Option<Vec<TabEntry>> {
    let (_, value) = agent
        .tokens
        .iter()
        .find(|(key, _)| key == OPENCODE_TABS_TOKEN)?;
    record_parse();
    let parsed = crate::opencode_tabs::parse_snapshot(value).ok()?;
    if parsed.tabs.is_empty()
        || !crate::opencode_tabs::snapshot_is_live(&parsed, crate::opencode_tabs::unix_millis())
    {
        return None;
    }
    let owner = parsed.owner;
    Some(
        parsed
            .tabs
            .into_iter()
            .map(|tab| TabEntry {
                status: tab_status(tab.attention, tab.busy),
                identity: AgentIdentity::OpenCodeTab {
                    pane_id: agent.pane_id.clone(),
                    owner: owner.clone(),
                    session_id: tab.session_id,
                },
                title: tab.title,
                active: tab.active,
            })
            .collect(),
    )
}

fn tab_status(attention: bool, busy: bool) -> crate::api::schema::AgentStatus {
    use crate::api::schema::AgentStatus;
    if attention {
        AgentStatus::Blocked
    } else if busy {
        AgentStatus::Working
    } else {
        AgentStatus::Idle
    }
}

/// State words a plugin may key a token family by (`title_working`,
/// `logo_stale`), longest first so `idle_stale` matches before `stale`.
const TOKEN_STATE_WORDS: [&str; 9] = [
    "idle_fresh",
    "idle_stale",
    "working",
    "blocked",
    "unknown",
    "done",
    "idle",
    "fresh",
    "stale",
];

/// Freshness words describe how long the whole pane has been quiet, which holds
/// for every tab in it, so a per-tab row keeps whatever the pane published.
const PANE_LEVEL_STATE_WORDS: [&str; 4] = ["idle_fresh", "idle_stale", "fresh", "stale"];

fn token_state_word(status: crate::api::schema::AgentStatus) -> &'static str {
    use crate::api::schema::AgentStatus;
    match status {
        AgentStatus::Blocked => "blocked",
        AgentStatus::Working => "working",
        _ => "idle",
    }
}

/// Splits `title_idle_stale` into (`title`, `idle_stale`). A name with no known
/// state word is its own family key with an empty state.
fn split_token_state(name: &str) -> (&str, &str) {
    for word in TOKEN_STATE_WORDS {
        let Some(stem) = name
            .strip_suffix(word)
            .and_then(|stem| stem.strip_suffix('_'))
        else {
            continue;
        };
        if !stem.is_empty() {
            return (stem, word);
        }
    }
    (name, "")
}

type TokenFamilies = HashMap<String, HashMap<String, String>>;

/// Configured custom tokens grouped by family: `logo`, `logo_working` and
/// `logo_stale` all land under `logo`, each state word mapped to the exact
/// configured name. A family whose quiet member has no suffix keys `""`.
fn tab_token_families(
    config: &crate::config::AgentsSidebarConfig,
    canonical_agent: Option<crate::detect::Agent>,
) -> TokenFamilies {
    use crate::config::AgentSidebarToken;
    let mut families = TokenFamilies::new();
    for row in config.rows_for_agent(canonical_agent) {
        for token in row {
            // A configured entry may be a bare custom token or a styled one
            // (`{ token = "$title_working", fg = ... }`), which wraps it.
            let (token, _) = token.parts();
            let AgentSidebarToken::Custom(name) = token else {
                continue;
            };
            let (key, state) = split_token_state(name);
            families
                .entry(key.to_string())
                .or_default()
                .insert(state.to_string(), name.clone());
        }
    }
    families
}

/// Rewrites one pane's token map for one tab.
///
/// A plugin sets one member of each state-keyed family (`title_working`,
/// `state_blocked`, `logo_stale`) and only for the session the pane currently
/// shows, so every expanded row would otherwise repeat the focused tab's title
/// and colour. The client knows a tab's own title and state, so it moves the
/// published value to the family member named for the tab's state: `title_idle`
/// becomes `title_working` carrying the tab's own title. Values the client
/// cannot invent — a plugin's mark or logo glyph — carry over unchanged; only
/// the name, and so the configured style, follows the tab.
fn tab_token_projection(
    tokens: &HashMap<String, String>,
    families: &TokenFamilies,
    agent: &ClientShellAgent,
    title: &str,
    status: crate::api::schema::AgentStatus,
    workspace_label: &str,
) -> HashMap<String, String> {
    let tab_state = token_state_word(status);
    let mut projected = tokens.clone();
    for (name, value) in tokens {
        // A root tab is a peer row, not a child of the pane's split tree.
        // Drop the branch corner instead of keeping a blank cell for it, so
        // the row starts at the same column as an ordinary agent row.
        if name == "split_mark" {
            projected.remove(name);
            continue;
        }
        let (key, state) = split_token_state(name);
        // Freshness stays a pane-level signal: how long the pane has been
        // quiet says nothing tab by tab, so the name it published stands.
        let mut target = name.clone();
        if state != tab_state && !PANE_LEVEL_STATE_WORDS.contains(&state) {
            if let Some(family) = families.get(key) {
                // A family whose quiet member carries no suffix (`logo`) names
                // idle by omitting the state word.
                target = match family.get(tab_state) {
                    Some(sibling) => sibling.clone(),
                    None if tab_state == "idle" => family.get("").cloned().unwrap_or(target),
                    None => target,
                };
            }
        }
        let mut projected_value = value.clone();
        let title_family = key.to_ascii_lowercase().contains("title");
        if (title_family || !workspace_header_text(value, workspace_label))
            && projection_value_is_title(key, value, agent)
        {
            if title.is_empty() {
                // A tab with no title must not inherit the pane's session
                // title, so drop the token rather than leaving the old value.
                projected.remove(name);
                if target != *name {
                    projected.remove(&target);
                }
                continue;
            }
            projected_value = projected_title(value, title, agent, status);
        }
        if target == *name && projected_value == *value {
            continue;
        }
        projected.remove(name);
        projected.insert(target, projected_value);
    }
    projected
}

/// Whether a family carries the session title, which the client can write per
/// tab, rather than a glyph it cannot. A plugin republishes Herdr's own terminal
/// title under a state-coloured name, so the value itself gives it away even
/// when the family is not named for it.
fn projection_value_is_title(family_key: &str, value: &str, agent: &ClientShellAgent) -> bool {
    if family_key.to_ascii_lowercase().contains("title") {
        return true;
    }
    [
        agent.terminal_title_stripped.as_deref(),
        agent.terminal_title.as_deref(),
        agent.title.as_deref(),
        agent.name.as_deref(),
        agent.display_agent.as_deref(),
    ]
    .into_iter()
    .flatten()
    .any(|candidate| !candidate.is_empty() && value.ends_with(candidate))
}

/// Replace the pane's own session title with the tab's, keeping whatever the
/// plugin put in front of it. Radar publishes `⡿ OC | title`: a spinner, the
/// OpenCode label, then the session title. The session title is the suffix the
/// client can swap; everything before it stays.
fn projected_title(
    value: &str,
    title: &str,
    agent: &ClientShellAgent,
    status: crate::api::schema::AgentStatus,
) -> String {
    let prefix = projected_prefix(value, agent);
    // Radar hangs its spinner on the title only while the pane is working.
    // A tab that is idle or waiting must not inherit the parent's working
    // frame; drop the mark cell but keep the indent and any tab label.
    let prefix = if status == crate::api::schema::AgentStatus::Working {
        prefix
    } else {
        strip_activity_mark_cell(&prefix)
    };
    format!("{prefix}{title}")
}

/// The prefix published in front of the pane's session title.
///
/// Prefer the title metadata or the segment after `OC | `: those name the
/// session itself, so the prefix keeps the vendor label. Fall back to the full
/// reported title, then a leading mark run.
fn projected_prefix(value: &str, agent: &ClientShellAgent) -> String {
    let stripped = agent.terminal_title_stripped.as_deref();
    let stripped_tail = stripped.and_then(|title| title.rsplit_once(" | ").map(|(_, tail)| tail));
    let session_candidates = [stripped_tail, agent.title.as_deref()];
    let full_candidates = [
        stripped,
        agent.name.as_deref(),
        agent.display_agent.as_deref(),
        agent.terminal_title.as_deref(),
    ];
    best_suffix_prefix(value, &session_candidates)
        .or_else(|| best_suffix_prefix(value, &full_candidates))
        .map(str::to_owned)
        .unwrap_or_else(|| fallback_title_prefix(value))
}

fn best_suffix_prefix<'a>(value: &'a str, candidates: &[Option<&str>]) -> Option<&'a str> {
    candidates
        .iter()
        .flatten()
        .copied()
        .filter(|candidate| !candidate.is_empty())
        .filter_map(|candidate| value.strip_suffix(candidate))
        .min_by_key(|prefix| prefix.len())
}

/// When no reported title matches byte for byte, keep the `OC | ` label if it
/// exists, else the leading marks. Never returns the session's own text.
fn fallback_title_prefix(value: &str) -> String {
    if let Some((head, _)) = value.rsplit_once(" | ") {
        return format!("{head} | ");
    }
    if let Some((index, character)) = value
        .char_indices()
        .find(|(_, character)| is_activity_glyph(*character))
    {
        let end = index + character.len_utf8();
        if value[end..].starts_with(' ') {
            return value[..=end].to_owned();
        }
    }
    leading_glyph_prefix(value)
}

fn is_activity_glyph(character: char) -> bool {
    matches!(character, '\u{2800}'..='\u{28ff}') || "·✢✳✶✻✽◐◓◑◒".contains(character)
}

/// Remove the first activity-mark cell (`⡿ `) from a title prefix. Other
/// glyph cells, such as a branch corner, are content and stay.
fn strip_activity_mark_cell(prefix: &str) -> String {
    for (index, character) in prefix.char_indices() {
        if !is_activity_glyph(character) {
            continue;
        }
        let after = &prefix[index + character.len_utf8()..];
        if let Some(rest) = after.strip_prefix(' ') {
            return format!("{}{rest}", &prefix[..index]);
        }
    }
    prefix.to_owned()
}

/// Keep a leading run of marks and spaces when nothing better is known.
fn leading_glyph_prefix(value: &str) -> String {
    let end = value
        .char_indices()
        .take_while(|(_, character)| !character.is_alphanumeric())
        .map(|(index, character)| index + character.len_utf8())
        .last()
        .unwrap_or(0);
    value[..end].to_owned()
}

/// Whether a token value names the workspace rather than the tab.
///
/// Radar writes the group label behind a branch corner and a zero-width space
/// (`\u{200b}└─ diagnosis`), which Herdr would otherwise trim. The corner is
/// short and fixed, so a suffix match with a small allowance distinguishes the
/// header from a title that merely mentions the workspace.
fn workspace_header_text(value: &str, workspace_label: &str) -> bool {
    if workspace_label.is_empty() {
        return false;
    }
    let trimmed = value.trim_start_matches(&['\u{200b}', ' ', '\t'][..]);
    let label_len = workspace_label.chars().count();
    trimmed.ends_with(workspace_label) && trimmed.chars().count() <= label_len + 8
}

fn is_workspace_header_row(tokens: &[crate::ui::ResolvedToken], workspace_label: &str) -> bool {
    let mut names_workspace = false;
    let names_header = tokens.iter().all(|token| match &token.kind {
        crate::ui::ResolvedTokenKind::Workspace(value) => {
            names_workspace |= value == workspace_label;
            true
        }
        crate::ui::ResolvedTokenKind::Custom(value) => {
            if workspace_header_text(value, workspace_label) {
                names_workspace = true;
                true
            } else {
                // Group rows carry a leader and a branch corner beside the label.
                is_blank_or_glyph(value)
            }
        }
        _ => false,
    });
    names_workspace && names_header
}

fn is_blank_or_glyph(value: &str) -> bool {
    let trimmed = value.trim_start_matches(&['\u{200b}', ' ', '\t'][..]);
    trimmed.is_empty() || trimmed.chars().count() <= 2
}

fn leading_workspace_headers(
    rows: &[Vec<crate::ui::ResolvedToken>],
    workspace_label: &str,
) -> usize {
    rows.iter()
        .take_while(|tokens| is_workspace_header_row(tokens, workspace_label))
        .count()
}

/// A row that paints nothing, such as Radar's zero-width gap between groups.
fn is_blank_row(tokens: &[crate::ui::ResolvedToken]) -> bool {
    tokens.iter().all(|token| {
        let text = match &token.kind {
            crate::ui::ResolvedTokenKind::StateIcon
            | crate::ui::ResolvedTokenKind::GitStatus { .. } => return false,
            crate::ui::ResolvedTokenKind::StateText(value)
            | crate::ui::ResolvedTokenKind::Machine(value)
            | crate::ui::ResolvedTokenKind::Workspace(value)
            | crate::ui::ResolvedTokenKind::Tab(value)
            | crate::ui::ResolvedTokenKind::Pane(value)
            | crate::ui::ResolvedTokenKind::Agent(value)
            | crate::ui::ResolvedTokenKind::TerminalTitle(value)
            | crate::ui::ResolvedTokenKind::Branch(value)
            | crate::ui::ResolvedTokenKind::Custom(value) => value.as_str(),
        };
        display_width(text) == 0
            || text
                .trim_matches(|character: char| {
                    character.is_whitespace() || character == '\u{200b}'
                })
                .is_empty()
    })
}

/// Folds the repeated workspace header of expanded tab rows into one header.
///
/// Every expanded tab carries its pane's tokens, including the workspace header
/// a plugin publishes once per group. The first tab of a workspace keeps that
/// header; later tabs drop the same leading rows. Groups are keyed by endpoint
/// plus workspace id, so two workspaces that display the same label stay
/// separate. A trailing blank spacer is kept only on a group's last visible
/// tab.
// Indexed loops are needed: the fold matches rows by position and mutates each
// row's token lines through the accessor closure.
#[allow(clippy::needless_range_loop)]
pub(super) fn collapse_workspace_headers<T>(
    rows: &mut [T],
    mut agent: impl FnMut(&mut T) -> &mut AgentRow,
) {
    let mut header_by_group: HashMap<String, usize> = HashMap::new();
    for index in 0..rows.len() {
        let row = agent(&mut rows[index]);
        if !matches!(row.identity, AgentIdentity::OpenCodeTab { .. }) {
            continue;
        }
        header_by_group
            .entry(row.group_key.clone())
            .and_modify(|header| *header = (*header).max(row.header_rows))
            .or_insert(row.header_rows);
    }

    let mut current_group: Option<String> = None;
    for index in 0..rows.len() {
        let (is_tab, group_key, header_rows) = {
            let row = agent(&mut rows[index]);
            (
                matches!(row.identity, AgentIdentity::OpenCodeTab { .. }),
                row.group_key.clone(),
                row.header_rows,
            )
        };
        if !is_tab {
            current_group = None;
            continue;
        }
        let first_in_run = current_group.as_deref() != Some(group_key.as_str());
        current_group = Some(group_key.clone());
        let header = header_by_group.get(&group_key).copied().unwrap_or(0);
        let next_in_run = index + 1 < rows.len() && {
            let next = agent(&mut rows[index + 1]);
            matches!(next.identity, AgentIdentity::OpenCodeTab { .. })
                && next.group_key == group_key
        };
        let row = agent(&mut rows[index]);
        if !first_in_run {
            let drop = header.min(header_rows);
            if drop < row.rows.len() {
                row.rows.drain(..drop);
            }
            // The header is gone from this row, so every remaining line is
            // content. Keep that visible to the renderer's indent choice.
            row.header_rows = 0;
        }
        if next_in_run {
            // Keep the plugin's group spacer on the last tab only.
            while row.rows.len() > 1 && row.rows.last().is_some_and(|tokens| is_blank_row(tokens)) {
                row.rows.pop();
            }
        }
    }
}

/// Validates that an identity still refers to a live pane/tab in `snapshot`.
///
/// Used before focus so a stale click fails closed instead of focusing a
/// vanished OpenCode tab.
pub(super) fn identity_is_current(
    snapshot: &ClientShellSnapshot,
    identity: &AgentIdentity,
) -> bool {
    let AgentIdentity::OpenCodeTab {
        pane_id,
        owner,
        session_id,
    } = identity
    else {
        return snapshot
            .agents
            .iter()
            .any(|agent| agent.pane_id == identity.pane_id());
    };
    let Some(agent) = snapshot
        .agents
        .iter()
        .find(|agent| &agent.pane_id == pane_id)
    else {
        return false;
    };
    expanded_tabs(agent).is_some_and(|tabs| {
        tabs.iter().any(|tab| {
            tab.identity
                .opencode_tab()
                .is_some_and(|(current_owner, id)| current_owner == owner && id == session_id)
        })
    })
}

/// Expanded keyboard-navigation entries for one pane: each OpenCode tab plus
/// its active flag, or a single ordinary pane entry.
pub(super) fn agent_identity_entries(
    snapshot: &ClientShellSnapshot,
    pane_id: &str,
) -> Vec<(AgentIdentity, bool)> {
    let Some(agent) = snapshot
        .agents
        .iter()
        .find(|agent| agent.pane_id == pane_id)
    else {
        return Vec::new();
    };
    match expanded_tabs(agent) {
        Some(tabs) if !tabs.is_empty() => tabs
            .into_iter()
            .map(|tab| (tab.identity, tab.active))
            .collect(),
        _ => vec![(
            AgentIdentity::Pane {
                pane_id: agent.pane_id.clone(),
            },
            true,
        )],
    }
}

pub(super) fn ordered_agent_pane_ids(
    snapshot: &ClientShellSnapshot,
    sort: crate::config::AgentPanelSortConfig,
) -> Vec<String> {
    if snapshot.agent_view_label.is_some() {
        return snapshot
            .agent_order
            .iter()
            .filter(|pane_id| {
                snapshot
                    .agents
                    .iter()
                    .any(|agent| agent.pane_id == pane_id.as_str())
            })
            .cloned()
            .collect();
    }
    let mut agents = snapshot.agents.iter().collect::<Vec<_>>();
    if sort == crate::config::AgentPanelSortConfig::Priority {
        agents.sort_by_key(|agent| {
            (
                std::cmp::Reverse(status_priority(agent.agent_status)),
                std::cmp::Reverse(agent.state_change_seq),
            )
        });
    }
    agents
        .into_iter()
        .map(|agent| agent.pane_id.clone())
        .collect()
}

pub(super) fn render_agent_panel(
    buffer: &mut Buffer,
    area: Rect,
    snapshot: &ClientShellSnapshot,
    config: &ClientShellConfig,
    agent_scroll: &mut usize,
    endpoint_id: &ClientEndpointId,
    agent_selection: Option<&AgentSelection>,
    hits: &mut ShellHitMap,
) {
    if !render_agent_panel_header(
        buffer,
        area,
        snapshot.agent_view_label.as_deref(),
        config,
        hits,
    ) {
        return;
    }

    let mut rows = agent_rows(snapshot, config, None);
    apply_selection_intent(&mut rows, endpoint_id, agent_selection);
    render_agent_list(
        buffer,
        area,
        &rows,
        snapshot
            .agent_view_label
            .as_ref()
            .map(|_| " no matching agents"),
        config,
        agent_scroll,
        hits,
        |row| row.rows.len(),
        |buffer, rect, row, hits| {
            hits.agents.push((rect, row.identity.clone()));
            render_agent_row(buffer, rect, row, config);
        },
    );
}

pub(super) fn render_agent_panel_header(
    buffer: &mut Buffer,
    area: Rect,
    agent_view_label: Option<&str>,
    config: &ClientShellConfig,
    hits: &mut ShellHitMap,
) -> bool {
    if area.height == 0 {
        return false;
    }
    put_text(
        buffer,
        area.x,
        area.y,
        area.width,
        &"─".repeat(area.width as usize),
        Style::default().fg(config.palette.surface_dim),
    );
    if area.height < 2 {
        return false;
    }
    put_text(
        buffer,
        area.x,
        area.y + 1,
        area.width,
        " agents",
        Style::default()
            .fg(config.palette.overlay0)
            .add_modifier(Modifier::BOLD),
    );
    let sort_label = agent_view_label.unwrap_or(match config.agent_panel_sort {
        crate::config::AgentPanelSortConfig::Spaces => "grouped",
        crate::config::AgentPanelSortConfig::Priority => "priority",
    });
    let sort_width = display_width(sort_label).min(area.width as usize) as u16;
    let sort_rect = Rect::new(
        area.right().saturating_sub(sort_width),
        area.y + 1,
        sort_width,
        1,
    );
    hits.agent_sort_toggle = if config.mouse_capture && agent_view_label.is_none() {
        sort_rect
    } else {
        Rect::default()
    };
    put_text(
        buffer,
        sort_rect.x,
        sort_rect.y,
        sort_rect.width,
        sort_label,
        Style::default()
            .fg(if agent_view_label.is_some() {
                config.palette.accent
            } else {
                config.palette.overlay0
            })
            .add_modifier(Modifier::BOLD),
    );
    true
}

pub(super) fn render_agent_list<T>(
    buffer: &mut Buffer,
    area: Rect,
    rows: &[T],
    empty_message: Option<&str>,
    config: &ClientShellConfig,
    agent_scroll: &mut usize,
    hits: &mut ShellHitMap,
    row_lines: impl Fn(&T) -> usize,
    mut render_row: impl FnMut(&mut Buffer, Rect, &T, &mut ShellHitMap),
) {
    let body = Rect::new(
        area.x,
        area.y.saturating_add(3),
        area.width,
        area.height.saturating_sub(3),
    );
    hits.agent_body = body;
    if body.is_empty() || rows.is_empty() {
        *agent_scroll = 0;
        if let Some(message) = empty_message.filter(|_| !body.is_empty()) {
            put_text(
                buffer,
                body.x,
                body.y,
                body.width,
                message,
                Style::default()
                    .fg(config.palette.overlay0)
                    .add_modifier(Modifier::DIM),
            );
        }
        return;
    }

    let row_heights = rows
        .iter()
        .map(|row| row_lines(row).max(1).min(u16::MAX as usize) as u16)
        .collect::<Vec<_>>();
    let gaps = rows
        .iter()
        .enumerate()
        .map(|(index, _)| {
            if index + 1 < rows.len() {
                config.agents.row_gap
            } else {
                0
            }
        })
        .collect::<Vec<_>>();
    let metrics =
        super::scroll::list_scroll_metrics(&row_heights, &gaps, body.height, *agent_scroll);
    hits.agent_max_scroll = metrics.max_offset_from_bottom;
    hits.agent_scroll_metrics = Some(metrics);
    *agent_scroll = metrics
        .max_offset_from_bottom
        .saturating_sub(metrics.offset_from_bottom);
    let show_scrollbar = metrics.max_offset_from_bottom > 0 && body.width > 1;
    let content_width = body.width.saturating_sub(u16::from(show_scrollbar));
    let mut y = body.y;
    for (index, row) in rows.iter().enumerate().skip(*agent_scroll) {
        let height = row_heights[index].min(body.height);
        if y.saturating_add(height) > body.bottom() {
            break;
        }
        let rect = Rect::new(body.x, y, content_width, height);
        render_row(buffer, rect, row, hits);
        y = y
            .saturating_add(height)
            .saturating_add(if index + 1 < rows.len() {
                config.agents.row_gap
            } else {
                0
            });
    }

    if show_scrollbar {
        let track = Rect::new(body.right().saturating_sub(1), body.y, 1, body.height);
        hits.agent_scrollbar = track;
        super::scroll::render_list_scrollbar(buffer, track, metrics, &config.palette);
    }
}

pub(super) fn agent_rows(
    snapshot: &ClientShellSnapshot,
    config: &ClientShellConfig,
    machine: Option<&str>,
) -> Vec<AgentRow> {
    let mut rows = ordered_agent_pane_ids(snapshot, config.agent_panel_sort)
        .into_iter()
        .flat_map(|pane_id| agent_rows_for_pane(snapshot, &pane_id, config, machine, "local"))
        .collect::<Vec<_>>();
    collapse_workspace_headers(&mut rows, |row| row);
    rows
}

/// One row per live pane, or one row per OpenCode root tab when the pane
/// publishes `herdr_oc_tabs`. Malformed or stale tokens fall back to one pane row.
///
/// `group_prefix` names the endpoint so workspaces on different endpoints keep
/// separate headers even when their ids and labels coincide.
pub(super) fn agent_rows_for_pane(
    snapshot: &ClientShellSnapshot,
    pane_id: &str,
    config: &ClientShellConfig,
    machine: Option<&str>,
    group_prefix: &str,
) -> Vec<AgentRow> {
    let Some(agent) = snapshot
        .agents
        .iter()
        .find(|agent| agent.pane_id == pane_id)
    else {
        return Vec::new();
    };
    let base_identity = AgentIdentity::Pane {
        pane_id: agent.pane_id.clone(),
    };
    let Some(base) = rendered_agent_row(
        snapshot,
        agent,
        config,
        machine,
        None,
        None,
        agent.agent_status,
        None,
        agent.focused,
        base_identity,
        group_prefix,
    ) else {
        return Vec::new();
    };
    let Some(tabs) = expanded_tabs(agent) else {
        return vec![base];
    };
    let header_rows = base.header_rows;
    let workspace_label = snapshot
        .workspaces
        .iter()
        .find(|workspace| workspace.workspace_id == agent.workspace_id)
        .map(|workspace| workspace.label.as_str())
        .unwrap_or_default();
    let tokens = agent
        .tokens
        .iter()
        .cloned()
        .collect::<HashMap<String, String>>();
    let families = tab_token_families(
        &config.agents,
        agent
            .agent
            .as_deref()
            .and_then(crate::detect::parse_agent_label),
    );
    tabs.into_iter()
        .filter_map(|tab| {
            let status = tab.status;
            let projected = tab_token_projection(
                &tokens,
                &families,
                agent,
                &tab.title,
                status,
                workspace_label,
            );
            let mut row = rendered_agent_row(
                snapshot,
                agent,
                config,
                machine,
                Some(&projected),
                Some(&tab.title),
                status,
                Some(status_text(status)),
                agent.focused && tab.active,
                tab.identity,
                group_prefix,
            )?;
            // Detect the header on the pane's unprojected lines: per-tab title
            // rewrites must not turn an ordinary title into a workspace header.
            row.header_rows = header_rows;
            Some(row)
        })
        .collect()
}

#[allow(clippy::too_many_arguments)]
fn rendered_agent_row(
    snapshot: &ClientShellSnapshot,
    agent: &ClientShellAgent,
    config: &ClientShellConfig,
    machine: Option<&str>,
    token_override: Option<&HashMap<String, String>>,
    tab_title: Option<&str>,
    status: crate::api::schema::AgentStatus,
    state_override: Option<&str>,
    focused: bool,
    identity: AgentIdentity,
    group_prefix: &str,
) -> Option<AgentRow> {
    let workspace = snapshot
        .workspaces
        .iter()
        .find(|workspace| workspace.workspace_id == agent.workspace_id)?;
    let tab = snapshot.tabs.iter().find(|tab| tab.tab_id == agent.tab_id);
    let pane = snapshot
        .panes
        .iter()
        .find(|pane| pane.pane_id == agent.pane_id);
    let tab_count = snapshot
        .tabs
        .iter()
        .filter(|candidate| candidate.workspace_id == agent.workspace_id)
        .count();
    let tab_label = tab
        .filter(|tab| tab_count > 1 || tab.custom_label)
        .map(|tab| tab.label.as_str());
    let agent_label = tab_title
        .or(agent.display_agent.as_deref())
        .or(agent.name.as_deref())
        .or(agent.agent.as_deref())
        .or(agent.title.as_deref());
    let labels = agent
        .state_labels
        .iter()
        .cloned()
        .collect::<HashMap<_, _>>();
    let tokens = token_override
        .cloned()
        .unwrap_or_else(|| agent.tokens.iter().cloned().collect::<HashMap<_, _>>());
    let state_text = state_override.map(str::to_owned).unwrap_or_else(|| {
        labels
            .get(status_text(status))
            .cloned()
            .unwrap_or_else(|| sidebar_status_text(status).to_owned())
    });
    let canonical_agent = agent
        .agent
        .as_deref()
        .and_then(crate::detect::parse_agent_label);
    let pane_label = tab_title
        .or(agent.title.as_deref())
        .or_else(|| pane.and_then(|pane| pane.label.as_deref()));
    let rows = crate::ui::sidebar_agent_rows(
        &config.agents,
        crate::ui::AgentTokenContext {
            machine,
            workspace: &workspace.label,
            tab: tab_label,
            pane: pane_label,
            agent_label,
            terminal_title: agent.terminal_title.as_deref(),
            terminal_title_stripped: agent.terminal_title_stripped.as_deref(),
            canonical_agent,
            tokens: &tokens,
        },
        &state_text,
    );
    let header_rows = leading_workspace_headers(&rows, &workspace.label);
    Some(AgentRow {
        identity,
        status,
        focused,
        header_rows,
        content_indent: if header_rows == 0 { 1 } else { 3 },
        group_key: format!("{group_prefix}\u{0}{}", agent.workspace_id),
        rows,
    })
}

pub(super) fn render_agent_row(
    buffer: &mut Buffer,
    rect: Rect,
    row: &AgentRow,
    config: &ClientShellConfig,
) {
    let palette = &config.palette;
    let row_style = if row.focused {
        Style::default().bg(palette.active_row_bg)
    } else {
        Style::default()
    };
    let name_style = if row.focused {
        Style::default()
            .fg(palette.text)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default()
            .fg(palette.subtext0)
            .add_modifier(Modifier::BOLD)
    };
    let status_style = Style::default().fg(status_color(row.status, palette));
    let secondary = Style::default().fg(palette.overlay0);
    let icon = (
        status_icon(row.status, config.status_indicators),
        Style::default().fg(status_color(row.status, palette)),
    );
    let rows = if row.rows.is_empty() {
        vec![vec![crate::ui::ResolvedToken {
            kind: crate::ui::ResolvedTokenKind::StateIcon,
            style: Default::default(),
        }]]
    } else {
        row.rows.clone()
    };
    for (index, tokens) in rows.iter().take(rect.height as usize).enumerate() {
        // An expanded tab is one logical row even when it keeps the workspace
        // header: its content starts in the same column as the first tab and as
        // an ordinary pane row. Header lines keep the leading column; every
        // tab content line uses the ordinary continuation column.
        let indent = if matches!(row.identity, AgentIdentity::OpenCodeTab { .. }) {
            if index < row.header_rows {
                1
            } else {
                row.content_indent
            }
        } else if index == 0 {
            1
        } else {
            3
        };
        let mut spans = vec![ratatui::text::Span::raw(" ".repeat(indent)).style(secondary)];
        spans.extend(crate::ui::resolved_token_spans(
            tokens,
            icon,
            status_style,
            name_style,
            secondary,
            secondary,
            palette,
            rect.width.saturating_sub(indent as u16) as usize,
        ));
        Paragraph::new(Line::from(spans)).style(row_style).render(
            Rect::new(rect.x, rect.y + index as u16, rect.width, 1),
            buffer,
        );
    }
}

fn put_text(buffer: &mut Buffer, x: u16, y: u16, width: u16, text: &str, style: Style) {
    for (offset, character) in text.chars().take(width as usize).enumerate() {
        if let Some(cell) = buffer.cell_mut((x + offset as u16, y)) {
            cell.set_char(character).set_style(style);
        }
    }
}

fn display_width(text: &str) -> usize {
    unicode_width::UnicodeWidthStr::width(text)
}

fn sidebar_status_text(status: crate::api::schema::AgentStatus) -> &'static str {
    use crate::api::schema::AgentStatus;
    match status {
        AgentStatus::Blocked => "blocked",
        AgentStatus::Done => "done",
        AgentStatus::Working => "working",
        AgentStatus::Idle | AgentStatus::Unknown => "idle",
    }
}

#[cfg(test)]
fn record_parse() {
    parse_probe::hit();
}

#[cfg(not(test))]
fn record_parse() {}

/// Test-only probe that counts snapshot parses during one client frame.
///
/// Render, hit, scroll and keyboard paths must not re-parse the same pane token
/// more than once per expansion pass; a repeat would show up here.
#[cfg(test)]
pub(super) mod parse_probe {
    use std::cell::Cell;

    thread_local! {
        static COUNT: Cell<usize> = const { Cell::new(0) };
    }

    pub(crate) fn reset() {
        COUNT.with(|count| count.set(0));
    }

    pub(crate) fn count() -> usize {
        COUNT.with(Cell::get)
    }

    pub(crate) fn hit() {
        COUNT.with(|count| count.set(count.get() + 1));
    }
}
