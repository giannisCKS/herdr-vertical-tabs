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

/// What sits above the draggable section divider inside the sidebar column.
///
/// The agent panel keeps its named height; the divider only moves the
/// spaces/tabs group's share of the column.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum SidebarSectionAbove {
    /// The whole spaces and tabs group: the group share is the pointer row.
    #[default]
    Group,
    /// Half the group (spaces or tabs): the group is twice the height above
    /// the divider.
    GroupHalf,
    /// The agent panel: the group fills the rows below the divider.
    Agents,
}

impl SidebarSectionAbove {
    /// The spaces/tabs group share for a pointer `above` rows below the
    /// sidebar top, inside a column `height` rows tall.
    pub(crate) fn split_ratio(self, above: u16, height: u16) -> f32 {
        if height == 0 {
            return 0.0;
        }
        let above = f32::from(above.min(height));
        let height = f32::from(height);
        let group_rows = match self {
            Self::Group => above,
            Self::GroupHalf => above * 2.0,
            Self::Agents => height - above,
        };
        (group_rows / height).clamp(0.1, 0.9)
    }
}

/// Rows the sidebar tab section reserves above its first tab row: the section
/// divider and the ` tabs` header.
pub(crate) const SIDEBAR_TABS_CHROME_ROWS: u16 = 2;
/// Smallest tab section that can still list one tab.
const SIDEBAR_TABS_MIN_ROWS: u16 = SIDEBAR_TABS_CHROME_ROWS + 1;
/// Smallest spaces section that still lists one space: header, one row, footer.
const SIDEBAR_SPACES_MIN_ROWS: u16 = 4;

/// Expanded sidebar sections in the configured top-to-bottom order.
pub(crate) struct ExpandedSidebarSections {
    /// Spaces (or machines) rows.
    pub(crate) primary: Rect,
    /// Tab list rows, when tabs are vertical and the upper area has room.
    pub(crate) tabs: Option<Rect>,
    /// Agent panel rows.
    pub(crate) agents: Rect,
    /// Visible boundary row between the agent panel and the spaces/tabs
    /// group: the draggable section divider.
    pub(crate) divider: Rect,
    /// Which section sits above `divider`.
    pub(crate) divider_above: SidebarSectionAbove,
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
/// the lower allocation. Ordering only moves the sections, so the divider
/// always follows the visible boundary between the agent panel and the
/// spaces/tabs group. Without a tab list the sidebar shows spaces and agents
/// in their relative order.
pub(crate) fn expanded_sidebar_tabs_sections(
    area: Rect,
    split_ratio: f32,
    tab_count: usize,
    order: [SidebarSectionConfig; 3],
) -> ExpandedSidebarSections {
    let content = Rect::new(area.x, area.y, area.width.saturating_sub(1), area.height);
    if content.is_empty() {
        return ExpandedSidebarSections {
            primary: Rect::default(),
            tabs: None,
            agents: Rect::default(),
            divider: Rect::default(),
            divider_above: SidebarSectionAbove::Group,
        };
    }

    let (upper_height, lower_height) = sidebar_section_heights(content.height, split_ratio);
    let tabs_height =
        if tab_count > 0 && upper_height >= SIDEBAR_SPACES_MIN_ROWS + SIDEBAR_TABS_MIN_ROWS {
            upper_height / 2
        } else {
            0
        };
    let height_of = |section: SidebarSectionConfig| match section {
        SidebarSectionConfig::Spaces => upper_height.saturating_sub(tabs_height),
        SidebarSectionConfig::Tabs => tabs_height,
        SidebarSectionConfig::Agents => lower_height,
    };
    let mut rects = [Rect::default(); 3];
    let mut y = content.y;
    for section in order {
        let height = height_of(section);
        rects[sidebar_section_slot(section)] = Rect::new(content.x, y, content.width, height);
        y = y.saturating_add(height);
    }

    let active = order
        .iter()
        .copied()
        .filter(|section| *section != SidebarSectionConfig::Tabs || tabs_height > 0)
        .collect::<Vec<_>>();
    let agents_position = active
        .iter()
        .position(|section| *section == SidebarSectionConfig::Agents)
        .unwrap_or(0);
    let divider_above = match agents_position {
        0 => SidebarSectionAbove::Agents,
        1 if tabs_height > 0 => SidebarSectionAbove::GroupHalf,
        _ => SidebarSectionAbove::Group,
    };
    let divider_y = if active.first() == Some(&SidebarSectionConfig::Agents) {
        active
            .get(1)
            .map_or(content.y, |section| rects[sidebar_section_slot(*section)].y)
    } else {
        rects[sidebar_section_slot(SidebarSectionConfig::Agents)].y
    };

    ExpandedSidebarSections {
        primary: rects[sidebar_section_slot(SidebarSectionConfig::Spaces)],
        tabs: (tabs_height > 0).then_some(rects[sidebar_section_slot(SidebarSectionConfig::Tabs)]),
        agents: rects[sidebar_section_slot(SidebarSectionConfig::Agents)],
        divider: if content.height < 6 {
            Rect::default()
        } else {
            Rect::new(content.x, divider_y, content.width, 1)
        },
        divider_above,
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
                    assert_eq!(sections.divider.y, sections.agents.y);
                } else {
                    assert_eq!(sections.divider, Rect::default());
                }
                assert_eq!(sections.divider_above, SidebarSectionAbove::Group);
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
        use SidebarSectionConfig::Agents;

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

            let agents_position = order
                .iter()
                .position(|section| *section == Agents)
                .expect("agents in every order");
            assert_eq!(
                sections.divider_above,
                match agents_position {
                    0 => SidebarSectionAbove::Agents,
                    1 => SidebarSectionAbove::GroupHalf,
                    _ => SidebarSectionAbove::Group,
                },
                "{order:?}"
            );
            let expected_row = if order[0] == Agents {
                lower
            } else {
                sections.agents.y
            };
            assert_eq!(sections.divider.y, expected_row, "{order:?}");
            assert_eq!(sections.divider.height, 1, "{order:?}");

            let above = sections.divider.y;
            let restored = sections.divider_above.split_ratio(above, height);
            assert!(
                (restored - 0.5).abs() < 0.06,
                "{order:?}: divider restores split, got {restored}"
            );
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
        assert_eq!(spaces_first.divider.y, 10);
        assert_eq!(spaces_first.divider_above, SidebarSectionAbove::Group);

        let agents_first = expanded_sidebar_tabs_sections(area, 0.5, 0, [Tabs, Agents, Spaces]);
        assert!(agents_first.tabs.is_none());
        assert_eq!(agents_first.agents, Rect::new(0, 0, 23, 10));
        assert_eq!(agents_first.primary, Rect::new(0, 10, 23, 10));
        assert_eq!(agents_first.divider.y, 10);
        assert_eq!(agents_first.divider_above, SidebarSectionAbove::Agents);

        let restored = agents_first
            .divider_above
            .split_ratio(agents_first.divider.y, 20);
        assert!((restored - 0.5).abs() < f32::EPSILON);
    }
}
