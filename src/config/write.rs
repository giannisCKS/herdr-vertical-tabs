#[derive(Clone, Copy)]
pub(crate) enum ConfigEdit<'a> {
    Theme(&'a str),
    StatusIndicators(super::StatusIndicatorStyle),
    Sound(bool),
    ToastDelivery(super::ToastDelivery),
    TabsOrientation(super::TabsOrientationConfig),
    SidebarSectionOrder([super::SidebarSectionConfig; 3]),
}

impl ConfigEdit<'_> {
    pub(crate) fn description(self) -> &'static str {
        match self {
            Self::Theme(_) => "theme",
            Self::StatusIndicators(_) => "status indicators",
            Self::Sound(_) => "sound setting",
            Self::ToastDelivery(_) => "toast setting",
            Self::TabsOrientation(_) => "tabs orientation",
            Self::SidebarSectionOrder(_) => "sidebar section order",
        }
    }

    pub(crate) fn apply(self, content: &str) -> String {
        match self {
            Self::Theme(name) => {
                let content =
                    super::upsert_section_value(content, "theme", "name", &format!("\"{name}\""));
                super::upsert_section_bool(&content, "theme", "auto_switch", false)
            }
            Self::StatusIndicators(style) => super::upsert_section_value(
                content,
                "ui",
                "status_indicators",
                &format!("\"{}\"", style.as_str()),
            ),
            Self::Sound(enabled) => {
                super::upsert_section_bool(content, "ui.sound", "enabled", enabled)
            }
            Self::ToastDelivery(delivery) => {
                let value = match delivery {
                    super::ToastDelivery::Off => "\"off\"",
                    super::ToastDelivery::Herdr => "\"herdr\"",
                    super::ToastDelivery::Terminal => "\"terminal\"",
                    super::ToastDelivery::System => "\"system\"",
                };
                let content = super::upsert_section_value(content, "ui.toast", "delivery", value);
                super::remove_section_key(&content, "ui.toast", "enabled")
            }
            Self::TabsOrientation(orientation) => super::upsert_section_value(
                content,
                "ui",
                "tabs_orientation",
                &format!("\"{}\"", orientation.as_str()),
            ),
            Self::SidebarSectionOrder(order) => {
                let entries = order
                    .iter()
                    .map(|section| format!("\"{}\"", section.as_str()))
                    .collect::<Vec<_>>()
                    .join(", ");
                super::upsert_section_value(
                    content,
                    "ui",
                    "sidebar_section_order",
                    &format!("[{entries}]"),
                )
            }
        }
    }
}

pub(crate) fn update_file_at(
    path: &std::path::Path,
    description: &str,
    update: impl FnOnce(&str) -> String,
) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| format!("failed to create config directory: {error}"))?;
    }
    let content = match super::io::read_optional_config(path) {
        Ok(Some(content)) => content,
        Ok(None) => String::new(),
        Err(error) => {
            return Err(format!(
                "failed to read config before saving {description}: {error}"
            ));
        }
    };
    std::fs::write(path, update(&content))
        .map_err(|error| format!("failed to save {description}: {error}"))
}

pub(crate) fn write_edit(edit: ConfigEdit<'_>) -> Result<(), String> {
    update_file_at(&super::config_path(), edit.description(), |content| {
        edit.apply(content)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn update_file_at_does_not_move_a_leading_bom_into_the_file() {
        let dir = std::env::temp_dir().join(format!("herdr-config-bom-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.toml");
        std::fs::write(
            &path,
            b"\xEF\xBB\xBF[terminal]\ndefault_shell = \"pwsh.exe\"\n",
        )
        .unwrap();

        update_file_at(&path, "onboarding setting", |content| {
            crate::config::upsert_top_level_bool(content, "onboarding", false)
        })
        .unwrap();

        let written = std::fs::read_to_string(&path).unwrap();
        let _ = std::fs::remove_dir_all(dir);

        assert!(
            !written.contains('\u{feff}'),
            "unexpected BOM in {written:?}"
        );
        assert!(
            toml::from_str::<toml::Value>(&written).is_ok(),
            "written config is not valid TOML: {written:?}"
        );
    }

    #[test]
    fn config_edits_write_the_tab_list_settings() {
        let content =
            ConfigEdit::TabsOrientation(crate::config::TabsOrientationConfig::Vertical).apply("");
        assert_eq!(content.matches("tabs_orientation").count(), 1, "{content}");
        let parsed: crate::config::Config = toml::from_str(&content).unwrap();
        assert_eq!(
            parsed.ui.tabs_orientation,
            crate::config::TabsOrientationConfig::Vertical
        );

        // Applying the same key again updates it in place.
        let content = ConfigEdit::TabsOrientation(crate::config::TabsOrientationConfig::Horizontal)
            .apply(&content);
        assert_eq!(content.matches("tabs_orientation").count(), 1, "{content}");
        let parsed: crate::config::Config = toml::from_str(&content).unwrap();
        assert_eq!(
            parsed.ui.tabs_orientation,
            crate::config::TabsOrientationConfig::Horizontal
        );

        let order = [
            crate::config::SidebarSectionConfig::Agents,
            crate::config::SidebarSectionConfig::Spaces,
            crate::config::SidebarSectionConfig::Tabs,
        ];
        let content = ConfigEdit::SidebarSectionOrder(order).apply(&content);
        assert_eq!(
            content.matches("sidebar_section_order").count(),
            1,
            "{content}"
        );
        let parsed: crate::config::Config = toml::from_str(&content).unwrap();
        assert_eq!(parsed.ui.sidebar_order(), Some(order));
        assert_eq!(
            parsed.ui.tabs_orientation,
            crate::config::TabsOrientationConfig::Horizontal
        );
    }
}
