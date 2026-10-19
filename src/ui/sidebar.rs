mod tokens;

use ratatui::{
    layout::Rect,
    style::{Modifier, Style},
    text::Span,
};

pub(crate) use self::tokens::{
    agent_rows as sidebar_agent_rows, space_rows as sidebar_space_rows, AgentTokenContext,
    ResolvedToken, ResolvedTokenKind, SpaceTokenContext,
};
use super::text::{display_width, truncate_end};
use crate::app::state::Palette;
use crate::app::AppState;
use crate::config::SidebarSectionConfig;
use crate::detect::AgentState;
use crate::terminal::TerminalRuntimeRegistry;

pub(crate) struct AgentPanelEntry {
    pub ws_idx: usize,
    pub tab_idx: usize,
    pub pane_id: crate::layout::PaneId,
    pub agent_kind_label: Option<String>,
    pub state: AgentState,
    pub seen: bool,
    pub last_agent_state_change_seq: Option<u64>,
    pub tokens: std::collections::HashMap<String, String>,
}

fn sidebar_section_heights(total_height: u16, split_ratio: f32) -> (u16, u16) {
    if total_height == 0 {
        return (0, 0);
    }
    if total_height < 6 {
        let workspace_height = total_height.div_ceil(2);
        return (
            workspace_height,
            total_height.saturating_sub(workspace_height),
        );
    }

    let workspace_height = ((total_height as f32) * split_ratio.clamp(0.1, 0.9)).round() as u16;
    let workspace_height = workspace_height.clamp(3, total_height.saturating_sub(3));
    (
        workspace_height,
        total_height.saturating_sub(workspace_height),
    )
}

/// A draggable boundary between two sections that sit next to each other.
///
/// A drag moves rows between `above` and `below` only. The third section keeps
/// its rows, so its far edge stays put. [`ExpandedSidebarSections::dividers`]
/// holds one descriptor per visible boundary, top to bottom.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct SidebarDivider {
    /// The draggable row: the first row of the section below the boundary.
    pub(crate) row: Rect,
    /// Section above the boundary.
    pub(crate) above: SidebarSectionConfig,
    /// Section below the boundary.
    pub(crate) below: SidebarSectionConfig,
    /// Section the drag leaves alone.
    pub(crate) fixed: SidebarSectionConfig,
    /// First row of the section above the boundary.
    pub(crate) top: u16,
    /// Rows `above` and `below` share: a constant while the drag runs.
    pub(crate) pair_rows: u16,
    /// Rows the `fixed` section keeps.
    pub(crate) fixed_rows: u16,
    /// Fewest rows `above` can take without breaking a sidebar minimum.
    pub(crate) min_above_rows: u16,
    /// Most rows `above` can take without breaking a sidebar minimum.
    pub(crate) max_above_rows: u16,
}

impl SidebarDivider {
    /// The rows and the splits a drag to `above_rows` rows above the pair
    /// produces.
    ///
    /// `above_rows` is clamped to the rows the layout can reproduce, so the
    /// boundary follows the pointer only as far as the sidebar minimums allow
    /// and repeated events at one extreme cannot drift.
    pub(crate) fn drag_result(
        self,
        above_rows: u16,
        total: u16,
        spaces_split: f32,
    ) -> (SectionRows, f32, f32) {
        let above_rows = above_rows.clamp(self.min_above_rows, self.max_above_rows);
        let rows = self.rows_at(above_rows);
        let (split, spaces) = rows.splits(total, spaces_split);
        (rows, split, spaces)
    }

    fn rows_at(self, above_rows: u16) -> SectionRows {
        let below_rows = self.pair_rows.saturating_sub(above_rows);
        SectionRows::default()
            .with_row(self.above, above_rows)
            .with_row(self.below, below_rows)
            .with_row(self.fixed, self.fixed_rows)
    }

    /// Rows above the pair the layout can reproduce, as `(min, max)`.
    ///
    /// The layout produced the current position, so it always sits inside the
    /// range. Reproducible rows are contiguous, so the bounds are found by
    /// binary search outwards from the current position.
    fn bounds(self, total: u16, spaces_split: f32, tab_count: usize) -> (u16, u16) {
        let current = self.row.y.saturating_sub(self.top).min(self.pair_rows);
        if self.pair_rows < 2 || total == 0 {
            return (current, current);
        }
        let reproducible = |above_rows: u16| {
            let rows = self.rows_at(above_rows);
            let (split, spaces) = rows.splits(total, spaces_split);
            section_heights(total, split, spaces, tab_count) == rows
        };
        let mut low = 1;
        let mut high = current;
        while low < high {
            let middle = low + (high - low) / 2;
            if reproducible(middle) {
                high = middle;
            } else {
                low = middle + 1;
            }
        }
        let min = if reproducible(low) { low } else { current };
        let mut low = current;
        let mut high = self.pair_rows - 1;
        while low < high {
            let middle = low + (high - low).div_ceil(2);
            if reproducible(middle) {
                low = middle;
            } else {
                high = middle - 1;
            }
        }
        let max = if reproducible(low) { low } else { current };
        (min, max)
    }
}

/// Rows the sidebar tab section reserves above its first tab row: the section
/// divider and the ` tabs` header.
pub(crate) const SIDEBAR_TABS_CHROME_ROWS: u16 = 2;
/// Smallest tab section that can still list one tab.
const SIDEBAR_TABS_MIN_ROWS: u16 = SIDEBAR_TABS_CHROME_ROWS + 1;
/// Smallest spaces section that still lists one space: header, one row, footer.
const SIDEBAR_SPACES_MIN_ROWS: u16 = 4;

/// Rows each sidebar section takes.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct SectionRows {
    pub(crate) spaces: u16,
    pub(crate) tabs: u16,
    pub(crate) agents: u16,
}

impl SectionRows {
    /// Rows the spaces and the tab list share: the upper allocation.
    fn group(self) -> u16 {
        self.spaces.saturating_add(self.tabs)
    }

    fn row_of(self, section: SidebarSectionConfig) -> u16 {
        match section {
            SidebarSectionConfig::Spaces => self.spaces,
            SidebarSectionConfig::Tabs => self.tabs,
            SidebarSectionConfig::Agents => self.agents,
        }
    }

    fn with_row(self, section: SidebarSectionConfig, rows: u16) -> Self {
        match section {
            SidebarSectionConfig::Spaces => Self {
                spaces: rows,
                ..self
            },
            SidebarSectionConfig::Tabs => Self { tabs: rows, ..self },
            SidebarSectionConfig::Agents => Self {
                agents: rows,
                ..self
            },
        }
    }

    /// The splits a sidebar with these rows stores.
    ///
    /// The tab list only takes a share of the upper allocation while it is
    /// visible, so the spaces split stays as it was when the tabs are hidden.
    fn splits(self, total: u16, spaces_split: f32) -> (f32, f32) {
        if total == 0 {
            return (0.5, spaces_split);
        }
        let group = self.group();
        let split = (f32::from(group) / f32::from(total)).clamp(0.1, 0.9);
        if self.tabs == 0 || group == 0 {
            return (split, spaces_split);
        }
        let spaces = (f32::from(self.spaces) / f32::from(group)).clamp(0.1, 0.9);
        (split, spaces)
    }
}

/// Rows a sidebar of `total` height gives each section.
fn section_heights(
    total: u16,
    split_ratio: f32,
    spaces_split: f32,
    tab_count: usize,
) -> SectionRows {
    let (upper_height, lower_height) = sidebar_section_heights(total, split_ratio);
    let tabs_visible =
        tab_count > 0 && upper_height >= SIDEBAR_SPACES_MIN_ROWS + SIDEBAR_TABS_MIN_ROWS;
    if !tabs_visible {
        return SectionRows {
            spaces: upper_height,
            tabs: 0,
            agents: lower_height,
        };
    }
    let spaces_split = if spaces_split.is_finite() {
        spaces_split.clamp(0.1, 0.9)
    } else {
        0.5
    };
    let spaces = ((f32::from(upper_height) * spaces_split).round() as u16).clamp(
        SIDEBAR_SPACES_MIN_ROWS,
        upper_height.saturating_sub(SIDEBAR_TABS_MIN_ROWS),
    );
    SectionRows {
        spaces,
        tabs: upper_height.saturating_sub(spaces),
        agents: lower_height,
    }
}

/// Expanded sidebar sections in the configured top-to-bottom order.
pub(crate) struct ExpandedSidebarSections {
    /// Spaces (or machines) rows.
    pub(crate) primary: Rect,
    /// Tab list rows, when tabs are vertical and the upper area has room.
    pub(crate) tabs: Option<Rect>,
    /// Agent panel rows.
    pub(crate) agents: Rect,
    /// Draggable boundaries between sections that sit next to each other, top
    /// to bottom. A hidden tab list leaves a single boundary.
    pub(crate) dividers: [Option<SidebarDivider>; 2],
}

fn sidebar_section_slot(section: SidebarSectionConfig) -> usize {
    match section {
        SidebarSectionConfig::Spaces => 0,
        SidebarSectionConfig::Tabs => 1,
        SidebarSectionConfig::Agents => 2,
    }
}

/// Split an expanded sidebar into the configured section order.
///
/// Named heights stay with their section: the tab list takes half of the
/// spaces allocation (the spaces keep the odd row), and the agent panel keeps
/// the lower allocation. Ordering only moves the sections; each visible
/// boundary between two neighbours is draggable. Without a tab list the sidebar
/// shows spaces and agents in their relative order.
pub(crate) fn expanded_sidebar_tabs_sections(
    area: Rect,
    split_ratio: f32,
    tab_count: usize,
    order: [SidebarSectionConfig; 3],
) -> ExpandedSidebarSections {
    expanded_sidebar_tabs_sections_split(area, split_ratio, 0.5, tab_count, order)
}

/// Like [`expanded_sidebar_tabs_sections`], but the spaces keep `spaces_split`
/// of the upper allocation instead of an even share with the tab list.
///
/// Every boundary between two neighbouring sections is draggable: a drag moves
/// rows between those two sections only and leaves the third section alone.
pub(crate) fn expanded_sidebar_tabs_sections_split(
    area: Rect,
    split_ratio: f32,
    spaces_split: f32,
    tab_count: usize,
    order: [SidebarSectionConfig; 3],
) -> ExpandedSidebarSections {
    let content = Rect::new(area.x, area.y, area.width.saturating_sub(1), area.height);
    if content.is_empty() {
        return ExpandedSidebarSections {
            primary: Rect::default(),
            tabs: None,
            agents: Rect::default(),
            dividers: [None, None],
        };
    }

    let heights = section_heights(content.height, split_ratio, spaces_split, tab_count);
    let tabs_height = heights.tabs;
    let mut rects = [Rect::default(); 3];
    let mut y = content.y;
    for section in order {
        let height = heights.row_of(section);
        rects[sidebar_section_slot(section)] = Rect::new(content.x, y, content.width, height);
        y = y.saturating_add(height);
    }

    let active = order
        .iter()
        .copied()
        .filter(|section| *section != SidebarSectionConfig::Tabs || tabs_height > 0)
        .collect::<Vec<_>>();
    let mut dividers = [None, None];
    if content.height >= 6 {
        for (index, pair) in active.windows(2).enumerate().take(dividers.len()) {
            let (above, below) = (pair[0], pair[1]);
            let above_rect = rects[sidebar_section_slot(above)];
            let below_rect = rects[sidebar_section_slot(below)];
            let Some(fixed) = order
                .iter()
                .copied()
                .find(|section| *section != above && *section != below)
            else {
                continue;
            };
            let mut divider = SidebarDivider {
                row: Rect::new(content.x, below_rect.y, content.width, 1),
                above,
                below,
                fixed,
                top: above_rect.y,
                pair_rows: above_rect.height.saturating_add(below_rect.height),
                fixed_rows: rects[sidebar_section_slot(fixed)].height,
                min_above_rows: 0,
                max_above_rows: 0,
            };
            let (min_above_rows, max_above_rows) =
                divider.bounds(content.height, spaces_split, tab_count);
            divider.min_above_rows = min_above_rows;
            divider.max_above_rows = max_above_rows;
            dividers[index] = Some(divider);
        }
    }

    ExpandedSidebarSections {
        primary: rects[sidebar_section_slot(SidebarSectionConfig::Spaces)],
        tabs: (tabs_height > 0).then_some(rects[sidebar_section_slot(SidebarSectionConfig::Tabs)]),
        agents: rects[sidebar_section_slot(SidebarSectionConfig::Agents)],
        dividers,
    }
}
pub(crate) fn agent_panel_entries_from(
    app: &AppState,
    _terminal_runtimes: &TerminalRuntimeRegistry,
) -> Vec<AgentPanelEntry> {
    let mut entries = app
        .workspaces
        .iter()
        .enumerate()
        .flat_map(|(ws_idx, workspace)| {
            workspace
                .pane_details(&app.terminals)
                .into_iter()
                .map(move |detail| AgentPanelEntry {
                    ws_idx,
                    tab_idx: detail.tab_idx,
                    pane_id: detail.pane_id,
                    agent_kind_label: detail.agent_kind_label,
                    state: detail.state,
                    seen: detail.seen,
                    last_agent_state_change_seq: detail.last_agent_state_change_seq,
                    tokens: detail.tokens,
                })
        })
        .collect();
    crate::app::agent_view::apply_agent_view(app, &mut entries);
    entries
}

pub(crate) fn resolved_token_spans(
    resolved: &[ResolvedToken],
    state_icon: (&str, Style),
    state_text_style: Style,
    workspace_style: Style,
    secondary_style: Style,
    custom_style: Style,
    palette: &Palette,
    max_width: usize,
) -> Vec<Span<'static>> {
    let fixed_widths = resolved
        .iter()
        .map(|token| match &token.kind {
            ResolvedTokenKind::StateIcon => display_width(state_icon.0),
            ResolvedTokenKind::GitStatus { ahead, behind } => {
                usize::from(*ahead > 0) * display_width(&format!("↑{ahead}"))
                    + usize::from(*behind > 0) * display_width(&format!("↓{behind}"))
                    + usize::from(*ahead > 0 && *behind > 0)
            }
            _ => 0,
        })
        .collect::<Vec<_>>();
    let flexible_widths = resolved
        .iter()
        .map(|token| match &token.kind {
            ResolvedTokenKind::StateText(text)
            | ResolvedTokenKind::Machine(text)
            | ResolvedTokenKind::Workspace(text)
            | ResolvedTokenKind::Tab(text)
            | ResolvedTokenKind::Pane(text)
            | ResolvedTokenKind::Agent(text)
            | ResolvedTokenKind::TerminalTitle(text)
            | ResolvedTokenKind::Branch(text)
            | ResolvedTokenKind::Custom(text) => display_width(text),
            _ => 0,
        })
        .collect::<Vec<_>>();
    let minimum_width = |active: &[bool]| {
        let indices = active
            .iter()
            .enumerate()
            .filter_map(|(index, active)| active.then_some(index))
            .collect::<Vec<_>>();
        let content = indices
            .iter()
            .map(|index| fixed_widths[*index] + usize::from(flexible_widths[*index] > 0))
            .sum::<usize>();
        let separators = indices
            .windows(2)
            .map(|pair| display_width(tokens::separator(&resolved[pair[0]], &resolved[pair[1]])))
            .sum::<usize>();
        content + separators
    };
    let mut active = resolved.iter().map(|_| true).collect::<Vec<_>>();
    if minimum_width(&active) > max_width {
        for (index, width) in flexible_widths.iter().enumerate() {
            if *width > 0 {
                active[index] = false;
            }
        }
        for index in (0..resolved.len()).rev() {
            if flexible_widths[index] == 0 {
                continue;
            }
            active[index] = true;
            if minimum_width(&active) > max_width {
                active[index] = false;
            }
        }
    }
    let visible_indices = active
        .iter()
        .enumerate()
        .filter_map(|(index, active)| active.then_some(index))
        .collect::<Vec<_>>();
    let separator_width = visible_indices
        .windows(2)
        .map(|pair| display_width(tokens::separator(&resolved[pair[0]], &resolved[pair[1]])))
        .sum::<usize>();
    let fixed_width = visible_indices
        .iter()
        .map(|index| fixed_widths[*index])
        .sum::<usize>();
    let mut budgets = flexible_widths
        .iter()
        .enumerate()
        .map(|(index, width)| usize::from(active[index] && *width > 0))
        .collect::<Vec<_>>();
    let minimum = budgets.iter().sum::<usize>();
    let mut remaining = max_width
        .saturating_sub(separator_width + fixed_width)
        .saturating_sub(minimum);
    while remaining > 0 {
        let mut grew = false;
        for (budget, width) in budgets.iter_mut().zip(&flexible_widths) {
            if *budget > 0 && *budget < *width {
                *budget += 1;
                remaining -= 1;
                grew = true;
                if remaining == 0 {
                    break;
                }
            }
        }
        if !grew {
            break;
        }
    }

    let mut spans = Vec::new();
    for (position, index) in visible_indices.iter().copied().enumerate() {
        let token = &resolved[index];
        if position > 0 {
            let previous = &resolved[visible_indices[position - 1]];
            spans.push(Span::styled(
                tokens::separator(previous, token),
                Style::default().fg(palette.overlay0),
            ));
        }
        match &token.kind {
            ResolvedTokenKind::StateIcon => spans.push(Span::styled(
                state_icon.0.to_string(),
                apply_token_style(state_icon.1, token.style),
            )),
            ResolvedTokenKind::StateText(text) => spans.push(Span::styled(
                truncate_end(text, budgets[index]),
                apply_token_style(state_text_style, token.style),
            )),
            ResolvedTokenKind::Workspace(text) => spans.push(Span::styled(
                truncate_end(text, budgets[index]),
                apply_token_style(workspace_style, token.style),
            )),
            ResolvedTokenKind::Machine(text)
            | ResolvedTokenKind::Tab(text)
            | ResolvedTokenKind::Pane(text)
            | ResolvedTokenKind::Agent(text)
            | ResolvedTokenKind::Branch(text) => spans.push(Span::styled(
                truncate_end(text, budgets[index]),
                apply_token_style(secondary_style, token.style),
            )),
            ResolvedTokenKind::GitStatus { ahead, behind } => {
                if *ahead > 0 {
                    spans.push(Span::styled(
                        format!("↑{ahead}"),
                        apply_token_style(Style::default().fg(palette.green), token.style),
                    ));
                }
                if *ahead > 0 && *behind > 0 {
                    spans.push(Span::styled(
                        " ",
                        apply_token_style(Style::default(), token.style),
                    ));
                }
                if *behind > 0 {
                    spans.push(Span::styled(
                        format!("↓{behind}"),
                        apply_token_style(Style::default().fg(palette.red), token.style),
                    ));
                }
            }
            ResolvedTokenKind::TerminalTitle(text) | ResolvedTokenKind::Custom(text) => {
                spans.push(Span::styled(
                    truncate_end(text, budgets[index]),
                    apply_token_style(custom_style, token.style),
                ));
            }
        }
    }
    spans
}

fn apply_token_style(mut style: Style, patch: crate::config::SidebarTokenStyle) -> Style {
    if let Some(foreground) = patch.fg {
        style = style.fg(foreground.ratatui());
    }
    if let Some(bold) = patch.bold {
        style = if bold {
            style.add_modifier(Modifier::BOLD)
        } else {
            style.remove_modifier(Modifier::BOLD)
        };
    }
    if let Some(dim) = patch.dim {
        style = if dim {
            style.add_modifier(Modifier::DIM)
        } else {
            style.remove_modifier(Modifier::DIM)
        };
    }
    style
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::SidebarSectionConfig;

    /// The sidebar split without the optional tab list.
    fn expanded_sidebar_sections(area: Rect, split_ratio: f32) -> (Rect, Rect) {
        let sections = expanded_sidebar_tabs_sections(
            area,
            split_ratio,
            0,
            SidebarSectionConfig::DEFAULT_ORDER,
        );
        (sections.primary, sections.agents)
    }

    /// The split preferences a drag on `divider` stores for a pointer on the
    /// divider row, mirroring the shell drag handler.
    fn stored_splits(divider: SidebarDivider, total: u16) -> (f32, f32) {
        let above = divider
            .row
            .y
            .saturating_sub(divider.top)
            .min(divider.pair_rows);
        let below = divider.pair_rows - above;
        let mut spaces = 0u16;
        let mut agents = 0u16;
        for (section, rows) in [
            (divider.above, above),
            (divider.below, below),
            (divider.fixed, divider.fixed_rows),
        ] {
            match section {
                SidebarSectionConfig::Spaces => spaces = rows,
                SidebarSectionConfig::Agents => agents = rows,
                SidebarSectionConfig::Tabs => {}
            }
        }
        let group = total.saturating_sub(agents);
        if total == 0 || group == 0 {
            return (0.5, 0.5);
        }
        (
            (f32::from(group) / f32::from(total)).clamp(0.1, 0.9),
            (f32::from(spaces) / f32::from(group)).clamp(0.1, 0.9),
        )
    }

    #[test]
    fn expanded_sidebar_tabs_sections_stack_without_overlap() {
        for split in [0.1_f32, 0.5, 0.9] {
            for height in 1..=40 {
                let area = Rect::new(0, 0, 30, height);
                let (spaces, agents) = expanded_sidebar_sections(area, split);
                let sections = expanded_sidebar_tabs_sections(
                    area,
                    split,
                    2,
                    SidebarSectionConfig::DEFAULT_ORDER,
                );
                let tabs_height = sections.tabs.map_or(0, |tabs| tabs.height);
                assert_eq!(
                    sections.primary.height + tabs_height + sections.agents.height,
                    height,
                    "split={split}, height={height}"
                );
                assert_eq!(
                    sections.agents, agents,
                    "agent panel keeps its size and position"
                );
                let Some(tabs) = sections.tabs else {
                    assert_eq!(sections.primary, spaces, "spaces stay whole");
                    continue;
                };
                assert_eq!(tabs.y, sections.primary.bottom());
                assert_eq!(sections.primary.height + tabs.height, spaces.height);
                assert_eq!(tabs.height, spaces.height / 2);
                assert_eq!(sections.primary.height, spaces.height.div_ceil(2));
                assert!(tabs.height >= SIDEBAR_TABS_MIN_ROWS);
                assert!(sections.primary.height >= SIDEBAR_SPACES_MIN_ROWS);
                if height >= 6 {
                    let boundary = sections.dividers[1].expect("tab/agent boundary");
                    assert_eq!(boundary.above, SidebarSectionConfig::Tabs);
                    assert_eq!(boundary.below, SidebarSectionConfig::Agents);
                    assert_eq!(boundary.row.y, sections.agents.y);
                } else {
                    assert_eq!(sections.dividers, [None, None]);
                }
            }
        }
    }

    #[test]
    fn sidebar_tabs_split_preserves_the_original_sections() {
        for split in [0.1_f32, 0.35, 0.5, 0.75, 0.9] {
            for height in 1..=40 {
                let area = Rect::new(0, 0, 24, height);
                let (spaces, agents) = expanded_sidebar_sections(area, split);
                for tab_count in [0_usize, 1, 3, 12] {
                    let sections = expanded_sidebar_tabs_sections(
                        area,
                        split,
                        tab_count,
                        SidebarSectionConfig::DEFAULT_ORDER,
                    );
                    assert_eq!(
                        sections.agents, agents,
                        "split={split}, height={height}, tabs={tab_count}"
                    );
                    if tab_count == 0 {
                        assert_eq!(sections.primary, spaces);
                        assert!(sections.tabs.is_none());
                    }
                    if let Some(tabs) = sections.tabs {
                        assert_eq!(tabs.bottom(), spaces.bottom());
                        assert_eq!(tabs.width, spaces.width);
                    }
                }
            }
        }
    }

    #[test]
    fn sidebar_tabs_section_needs_a_tab_and_room_for_spaces() {
        let area = |height: u16| Rect::new(0, 4, 24, height);
        let sections = |height: u16, tab_count: usize| {
            expanded_sidebar_tabs_sections(
                area(height),
                0.5,
                tab_count,
                SidebarSectionConfig::DEFAULT_ORDER,
            )
        };

        assert!(sections(30, 0).tabs.is_none());
        assert!(sections(12, 1).tabs.is_none());
        assert_eq!(
            sections(14, 1).tabs.expect("split upper area"),
            Rect::new(0, 8, 23, 3)
        );
        assert_eq!(
            sections(14, 1).primary,
            Rect::new(0, 4, 23, 4),
            "spaces keep the odd row"
        );
        assert_eq!(
            sections(16, 1).tabs.expect("even split"),
            Rect::new(0, 8, 23, 4)
        );
        assert_eq!(
            sections(18, 1).tabs.expect("odd split"),
            Rect::new(0, 9, 23, 4)
        );
    }

    #[test]
    fn expanded_sidebar_tabs_sections_follow_every_order() {
        let height = 30;
        let area = Rect::new(0, 0, 24, height);
        let (upper, lower) = sidebar_section_heights(height, 0.5);
        // split 0.5 rounds to 15, so the tab list keeps the odd row halves.
        assert_eq!((upper, lower), (15, 15));
        let tabs_height = upper / 2;
        let spaces_height = upper - tabs_height;

        for order in crate::config::SIDEBAR_SECTION_ORDERS {
            let sections = expanded_sidebar_tabs_sections(area, 0.5, 3, order);
            let rects = [sections.primary, sections.tabs.unwrap(), sections.agents];
            let heights = [spaces_height, tabs_height, lower];
            let mut y = 0;
            for section in order.iter() {
                let slot = sidebar_section_slot(*section);
                assert_eq!(rects[slot].y, y, "{order:?}");
                assert_eq!(rects[slot].height, heights[slot], "{order:?}");
                assert_eq!(rects[slot].width, area.width - 1, "{order:?}");
                y += heights[slot];
            }
            assert_eq!(y, height, "{order:?}");

            for (index, pair) in order.windows(2).enumerate() {
                let divider = sections.dividers[index].expect("visible section boundary");
                assert_eq!(
                    (divider.above, divider.below),
                    (pair[0], pair[1]),
                    "{order:?}"
                );
                assert_eq!(
                    divider.row.y,
                    rects[sidebar_section_slot(pair[1])].y,
                    "{order:?}"
                );
                assert_eq!(divider.row.height, 1, "{order:?}");
                assert_eq!(
                    divider.top,
                    rects[sidebar_section_slot(pair[0])].y,
                    "{order:?}"
                );
                assert_eq!(
                    divider.pair_rows,
                    rects[sidebar_section_slot(pair[0])].height
                        + rects[sidebar_section_slot(pair[1])].height,
                    "{order:?}"
                );
            }

            // The boundary rows restore the default split.
            for divider in sections.dividers.into_iter().flatten() {
                let (restored, _) = stored_splits(divider, height);
                assert!(
                    (restored - 0.5).abs() < 0.06,
                    "{order:?}: divider restores split, got {restored}"
                );
            }
        }
    }

    #[test]
    fn sidebar_sections_without_tabs_follow_the_relative_order() {
        use SidebarSectionConfig::{Agents, Spaces, Tabs};

        let area = Rect::new(0, 0, 24, 20);
        let (upper, lower) = sidebar_section_heights(20, 0.5);
        assert_eq!((upper, lower), (10, 10));

        let spaces_first = expanded_sidebar_tabs_sections(area, 0.5, 0, [Spaces, Tabs, Agents]);
        assert!(spaces_first.tabs.is_none());
        assert_eq!(spaces_first.primary, Rect::new(0, 0, 23, 10));
        assert_eq!(spaces_first.agents, Rect::new(0, 10, 23, 10));
        let divider = spaces_first.dividers[0].expect("spaces/agents boundary");
        assert_eq!((divider.above, divider.below), (Spaces, Agents));
        assert_eq!(divider.fixed, Tabs);
        assert_eq!(divider.fixed_rows, 0);
        assert_eq!(divider.row.y, 10);
        assert!(spaces_first.dividers[1].is_none());

        let agents_first = expanded_sidebar_tabs_sections(area, 0.5, 0, [Tabs, Agents, Spaces]);
        assert!(agents_first.tabs.is_none());
        assert_eq!(agents_first.agents, Rect::new(0, 0, 23, 10));
        assert_eq!(agents_first.primary, Rect::new(0, 10, 23, 10));
        let divider = agents_first.dividers[0].expect("agents/spaces boundary");
        assert_eq!((divider.above, divider.below), (Agents, Spaces));
        assert_eq!(divider.row.y, 10);

        // The boundary row restores the split a drag on it stores.
        let (restored, spaces) = stored_splits(divider, 20);
        assert!((restored - 0.5).abs() < f32::EPSILON, "restored {restored}");
        let round_trip =
            expanded_sidebar_tabs_sections_split(area, restored, spaces, 0, [Tabs, Agents, Spaces]);
        assert_eq!(round_trip.agents, agents_first.agents);
        assert_eq!(round_trip.primary, agents_first.primary);
    }

    #[test]
    fn spaces_split_moves_only_the_spaces_and_tab_rows() {
        use SidebarSectionConfig::{Agents, Spaces, Tabs};

        let area = Rect::new(0, 0, 24, 30);
        let sections =
            expanded_sidebar_tabs_sections_split(area, 0.5, 0.75, 3, [Spaces, Tabs, Agents]);
        assert_eq!(sections.primary.height, 11);
        assert_eq!(sections.tabs.expect("tab list").height, 4);
        assert_eq!(sections.agents.height, 15, "agent panel keeps its size");
        let dividers = sections
            .dividers
            .map(|divider| divider.expect("visible boundary"));
        assert_eq!((dividers[0].above, dividers[0].below), (Spaces, Tabs));
        assert_eq!(dividers[0].row.y, sections.tabs.expect("tab list").y);
        assert_eq!(dividers[0].top, sections.primary.y);
        assert_eq!(dividers[0].pair_rows, 15);
        assert_eq!((dividers[0].fixed, dividers[0].fixed_rows), (Agents, 15));
        assert_eq!((dividers[1].above, dividers[1].below), (Tabs, Agents));
        assert_eq!((dividers[1].fixed, dividers[1].fixed_rows), (Spaces, 11));

        let sections =
            expanded_sidebar_tabs_sections_split(area, 0.5, 0.25, 3, [Spaces, Tabs, Agents]);
        assert_eq!(sections.primary.height, 4);
        assert_eq!(sections.tabs.expect("tab list").height, 11);

        // A touch past the edge still leaves one space row and one tab row.
        let sections =
            expanded_sidebar_tabs_sections_split(area, 0.5, 0.01, 3, [Spaces, Tabs, Agents]);
        assert_eq!(sections.primary.height, SIDEBAR_SPACES_MIN_ROWS);
        let sections =
            expanded_sidebar_tabs_sections_split(area, 0.5, 0.99, 3, [Spaces, Tabs, Agents]);
        assert_eq!(
            sections.tabs.expect("tab list").height,
            SIDEBAR_TABS_MIN_ROWS
        );

        // The even default keeps the original geometry.
        let sections =
            expanded_sidebar_tabs_sections_split(area, 0.5, 0.5, 3, [Spaces, Tabs, Agents]);
        let even = expanded_sidebar_tabs_sections(area, 0.5, 3, [Spaces, Tabs, Agents]);
        assert_eq!(sections.primary, even.primary);
        assert_eq!(sections.tabs, even.tabs);
    }

    #[test]
    fn dividers_meet_the_two_neighbouring_sections_in_every_order() {
        use SidebarSectionConfig::{Agents, Spaces, Tabs};

        let area = Rect::new(0, 0, 24, 30);
        for order in crate::config::SIDEBAR_SECTION_ORDERS {
            for tab_count in [0usize, 3] {
                let sections =
                    expanded_sidebar_tabs_sections_split(area, 0.5, 0.5, tab_count, order);
                let rect_of = |section: SidebarSectionConfig| match section {
                    Spaces => sections.primary,
                    Tabs => sections.tabs.unwrap_or_default(),
                    Agents => sections.agents,
                };
                let visible = order
                    .iter()
                    .copied()
                    .filter(|section| *section != Tabs || sections.tabs.is_some())
                    .collect::<Vec<_>>();
                assert_eq!(
                    sections.dividers.iter().flatten().count(),
                    visible.len() - 1,
                    "{order:?} tabs={tab_count}"
                );
                for (index, pair) in visible.windows(2).enumerate() {
                    let divider = sections.dividers[index].expect("visible boundary");
                    assert_eq!(
                        (divider.above, divider.below),
                        (pair[0], pair[1]),
                        "{order:?} tabs={tab_count}"
                    );
                    assert_eq!(divider.row.y, rect_of(pair[1]).y, "{order:?}");
                    assert_eq!(divider.row.height, 1, "{order:?}");
                    assert_eq!(divider.row.width, area.width - 1, "{order:?}");
                    assert_eq!(divider.top, rect_of(pair[0]).y, "{order:?}");
                    assert_eq!(
                        divider.pair_rows,
                        rect_of(pair[0]).height + rect_of(pair[1]).height,
                        "{order:?}"
                    );
                    let fixed = order
                        .iter()
                        .copied()
                        .find(|section| *section != pair[0] && *section != pair[1])
                        .expect("a third section");
                    assert_eq!(divider.fixed, fixed, "{order:?}");
                    assert_eq!(divider.fixed_rows, rect_of(fixed).height, "{order:?}");
                }
            }
        }

        // A sidebar too short for the sections keeps every drag off.
        let tiny = expanded_sidebar_tabs_sections_split(
            Rect::new(0, 0, 24, 4),
            0.5,
            0.5,
            3,
            SidebarSectionConfig::DEFAULT_ORDER,
        );
        assert_eq!(tiny.dividers, [None, None]);
    }

    #[test]
    fn every_divider_restores_the_geometry_it_describes() {
        let area = Rect::new(0, 0, 24, 40);
        for order in crate::config::SIDEBAR_SECTION_ORDERS {
            for tab_count in [0usize, 3] {
                for section_split in [0.35_f32, 0.5, 0.65] {
                    for spaces_split in [0.35_f32, 0.5, 0.65] {
                        let sections = expanded_sidebar_tabs_sections_split(
                            area,
                            section_split,
                            spaces_split,
                            tab_count,
                            order,
                        );
                        for divider in sections.dividers.into_iter().flatten() {
                            let (split, spaces) = stored_splits(divider, area.height);
                            let restored = expanded_sidebar_tabs_sections_split(
                                area, split, spaces, tab_count, order,
                            );
                            assert_eq!(restored.primary, sections.primary, "{order:?}");
                            assert_eq!(restored.tabs, sections.tabs, "{order:?}");
                            assert_eq!(restored.agents, sections.agents, "{order:?}");
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn divider_drags_stop_at_the_rows_the_layout_can_reproduce() {
        for order in crate::config::SIDEBAR_SECTION_ORDERS {
            for tab_count in [0usize, 3] {
                for total in [12_u16, 20, 40] {
                    let area = Rect::new(0, 0, 24, total);
                    let sections =
                        expanded_sidebar_tabs_sections_split(area, 0.5, 0.5, tab_count, order);
                    let tabs_visible = sections.tabs.is_some();
                    for divider in sections.dividers.into_iter().flatten() {
                        let current = divider.row.y - divider.top;
                        assert!(
                            current >= divider.min_above_rows
                                && current <= divider.max_above_rows,
                            "{order:?} tabs={tab_count} rows={total}: the current rows are reachable"
                        );
                        for wanted in [0_u16, 1, current, divider.pair_rows, u16::MAX] {
                            let (rows, split, spaces) = divider.drag_result(wanted, total, 0.5);
                            assert_eq!(
                                rows.spaces + rows.tabs + rows.agents,
                                total,
                                "{order:?} rows={total}: every row stays with a section"
                            );
                            assert_eq!(
                                rows.row_of(divider.fixed),
                                divider.fixed_rows,
                                "{order:?} rows={total}: the third section keeps its rows"
                            );
                            assert_eq!(
                                rows.row_of(divider.above) + rows.row_of(divider.below),
                                divider.pair_rows,
                                "{order:?} rows={total}: the pair keeps its rows"
                            );
                            assert_eq!(
                                rows.tabs > 0,
                                tabs_visible,
                                "{order:?} tabs={tab_count} rows={total}: the tab list keeps its place"
                            );
                            let restored = expanded_sidebar_tabs_sections_split(
                                area, split, spaces, tab_count, order,
                            );
                            assert_eq!(restored.primary.height, rows.spaces, "{order:?}");
                            assert_eq!(
                                restored.tabs.map_or(0, |tabs| tabs.height),
                                rows.tabs,
                                "{order:?} rows={total}"
                            );
                            assert_eq!(restored.agents.height, rows.agents, "{order:?}");
                        }

                        // A row past the range collapses onto the bound, and
                        // asking twice for it lands in the same place.
                        let (once, _, _) = divider.drag_result(u16::MAX, total, 0.5);
                        let (twice, _, _) = divider.drag_result(u16::MAX, total, 0.5);
                        assert_eq!(once, twice, "{order:?}: repeated extremes do not drift");
                        assert_eq!(
                            once.row_of(divider.above),
                            divider.max_above_rows,
                            "{order:?}"
                        );
                        let (low, _, _) = divider.drag_result(0, total, 0.5);
                        assert_eq!(
                            low.row_of(divider.above),
                            divider.min_above_rows,
                            "{order:?}"
                        );
                        let (mid, _, _) = divider.drag_result(current, total, 0.5);
                        assert_eq!(
                            mid,
                            divider.rows_at(current),
                            "{order:?}: the current rows stay put"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn divider_bounds_keep_the_spaces_at_their_share() {
        use SidebarSectionConfig::{Agents, Spaces, Tabs};

        let order = [Spaces, Tabs, Agents];
        let area = Rect::new(0, 0, 24, 40);
        let sections = expanded_sidebar_tabs_sections_split(area, 0.5, 0.5, 3, order);
        assert_eq!(sections.primary.height, 10);
        let divider = sections.dividers[1].expect("tabs/agents boundary");
        assert_eq!((divider.top, divider.pair_rows), (10, 30));

        // A drag far past the bottom stops where the layout still holds.
        let (rows, split, spaces) = divider.drag_result(39 - divider.top, area.height, 0.5);
        assert_eq!(rows.spaces, 10, "the spaces keep their share");
        assert_eq!(rows.agents, 4, "the agent panel keeps the lower minimum");
        assert_eq!((rows.spaces, rows.tabs, rows.agents), (10, 26, 4));
        let restored = expanded_sidebar_tabs_sections_split(area, split, spaces, 3, order);
        assert_eq!(
            restored.primary.height, 10,
            "the drag cannot shrink the spaces"
        );
        assert_eq!(restored.dividers[1].expect("boundary").row.y, 36);

        // 20 rows: the tab list keeps its rows and never disappears.
        let area = Rect::new(0, 0, 24, 20);
        let sections = expanded_sidebar_tabs_sections_split(area, 0.5, 0.5, 3, order);
        assert_eq!(sections.primary.height, 5);
        let divider = sections.dividers[1].expect("tabs/agents boundary");
        for wanted in [7 - divider.top, 6 - divider.top, 0] {
            let (rows, _, _) = divider.drag_result(wanted, area.height, 0.5);
            assert_eq!(rows.spaces, 5, "the spaces keep their share");
            assert_eq!(
                rows.tabs, SIDEBAR_TABS_MIN_ROWS,
                "the tab list stays visible"
            );
            assert_eq!(rows.spaces + rows.tabs + rows.agents, area.height);
        }
    }
}
