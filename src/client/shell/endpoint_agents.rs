use super::render::put_text;
use super::*;

pub(super) fn render_collapsed(
    buffer: &mut Buffer,
    area: Rect,
    endpoints: &[ClientShellEndpoint],
    active_endpoint_id: &ClientEndpointId,
    config: &ClientShellConfig,
    selection: Option<&super::agent_sidebar::AgentSelection>,
    hits: &mut ShellHitMap,
) {
    let rows = agent_rows(endpoints, active_endpoint_id, config, selection);
    for (index, row) in rows.into_iter().take(area.height as usize).enumerate() {
        let rect = Rect::new(area.x, area.y + index as u16, area.width, 1);
        if row.agent.focused {
            buffer.set_style(rect, Style::default().bg(config.palette.active_row_bg));
        }
        let initial = row.machine_label.chars().next().unwrap_or('?');
        put_text(
            buffer,
            rect.x,
            rect.y,
            rect.width,
            &format!(
                "{initial}{}",
                status_icon(row.agent.status, config.status_indicators)
            ),
            Style::default()
                .fg(if row.stale {
                    config.palette.overlay0
                } else {
                    status_color(row.agent.status, &config.palette)
                })
                .add_modifier(if row.stale {
                    Modifier::DIM
                } else {
                    Modifier::empty()
                }),
        );
        hits.endpoint_agents
            .push((rect, row.endpoint_id, row.agent.identity));
    }
}

pub(super) fn render_expanded(
    buffer: &mut Buffer,
    area: Rect,
    agent_view_label: Option<&str>,
    endpoints: &[ClientShellEndpoint],
    active_endpoint_id: &ClientEndpointId,
    config: &ClientShellConfig,
    agent_scroll: &mut usize,
    selection: Option<&super::agent_sidebar::AgentSelection>,
    hits: &mut ShellHitMap,
) {
    if !super::agent_sidebar::render_agent_panel_header(
        buffer,
        area,
        agent_view_label,
        config,
        hits,
    ) {
        return;
    }
    let rows = agent_rows(endpoints, active_endpoint_id, config, selection);
    super::agent_sidebar::render_agent_list(
        buffer,
        area,
        &rows,
        agent_view_label.map(|_| " no matching agents"),
        config,
        agent_scroll,
        hits,
        |row| row.agent.rows.len(),
        |buffer, rect, row, hits| {
            super::agent_sidebar::render_agent_row(buffer, rect, &row.agent, config);
            if row.stale {
                buffer.set_style(
                    rect,
                    Style::default()
                        .fg(config.palette.overlay0)
                        .add_modifier(Modifier::DIM),
                );
            }
            hits.endpoint_agents
                .push((rect, row.endpoint_id.clone(), row.agent.identity.clone()));
        },
    );
}

impl ClientShellState {
    pub(super) fn reveal_endpoint_agent(
        &mut self,
        endpoint_id: &ClientEndpointId,
        identity: &super::agent_sidebar::AgentIdentity,
        body_height: u16,
    ) {
        if body_height == 0 {
            return;
        }
        let rows = agent_rows(
            &self.endpoints,
            &self.active_endpoint_id,
            &self.config,
            None,
        );
        let Some(target) = rows
            .iter()
            .position(|row| &row.endpoint_id == endpoint_id && &row.agent.identity == identity)
        else {
            return;
        };
        let heights = rows
            .iter()
            .map(|row| row.agent.rows.len().max(1).min(u16::MAX as usize) as u16)
            .collect::<Vec<_>>();
        let mut gaps = vec![self.config.agents.row_gap; rows.len()];
        if let Some(last) = gaps.last_mut() {
            *last = 0;
        }
        self.agent_scroll = super::scroll::list_scroll_start_to_reveal(
            &heights,
            &gaps,
            body_height,
            self.agent_scroll,
            target,
        );
    }
}

struct EndpointAgentRow {
    endpoint_id: ClientEndpointId,
    machine_label: String,
    stale: bool,
    agent: super::agent_sidebar::AgentRow,
}

fn agent_rows(
    endpoints: &[ClientShellEndpoint],
    active_endpoint_id: &ClientEndpointId,
    config: &ClientShellConfig,
    selection: Option<&super::agent_sidebar::AgentSelection>,
) -> Vec<EndpointAgentRow> {
    let mut rendered_rows = HashMap::new();
    for endpoint in endpoints {
        let Some(snapshot) = endpoint.snapshot.as_deref() else {
            continue;
        };
        for agent in &snapshot.agents {
            let rows = super::agent_sidebar::agent_rows_for_pane(
                snapshot,
                &agent.pane_id,
                config,
                Some(&endpoint.label),
                &endpoint.endpoint_id.storage_key(),
            );
            if !rows.is_empty() {
                rendered_rows.insert((endpoint.endpoint_id.clone(), agent.pane_id.clone()), rows);
            }
        }
    }

    let mut rows: Vec<EndpointAgentRow> = super::aggregate_navigation::aggregate_agent_rows(
        endpoints,
        active_endpoint_id,
        config.agent_panel_sort,
    )
    .into_iter()
    .flat_map(|row| {
        let key = (row.endpoint.endpoint_id.clone(), row.agent.pane_id.clone());
        let Some(rows) = rendered_rows.remove(&key) else {
            return Vec::new();
        };
        let active = row.endpoint.endpoint_id == active_endpoint_id;
        let endpoint_id = row.endpoint.endpoint_id.clone();
        let machine_label = row.endpoint.label.to_owned();
        let stale = row.endpoint.stale();
        rows.into_iter()
            .map(|mut agent| {
                agent.focused &= active;
                EndpointAgentRow {
                    endpoint_id: endpoint_id.clone(),
                    machine_label: machine_label.clone(),
                    stale,
                    agent,
                }
            })
            .collect()
    })
    .collect();
    super::agent_sidebar::collapse_workspace_headers(&mut rows, |row| &mut row.agent);
    if let Some(selection) = selection {
        for row in &mut rows {
            let endpoint_id = row.endpoint_id.clone();
            super::agent_sidebar::apply_selection_intent(
                std::slice::from_mut(&mut row.agent),
                &endpoint_id,
                Some(selection),
            );
        }
    }
    rows
}
