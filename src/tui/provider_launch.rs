//! Launch picker for a custom API provider: choose one saved model and the
//! reasoning effort for this session. The saved provider file is not written.

use std::time::{Duration, Instant};

use crossterm::event::KeyCode;
use ratatui::{
    Frame,
    layout::Rect,
    text::{Line, Span},
};

use super::hitmap::{HitMap, OverlayClick, OverlayHit};
use super::popup::{self, PopupState};
use super::provider_form::{REASONING_CHOICES, reasoning_choice};
use super::theme::{base, dim, header, key};
use crate::desktop::LaunchTarget;
use crate::provider::{ProviderProfile, ReasoningLaunch};
use crate::warmup::ModelEntry;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LaunchKind {
    Provider,
    Account,
}

#[derive(Debug, Clone)]
struct LaunchModel {
    /// An empty id means Codex's own default model (no `--model`).
    id: String,
    title: String,
    saved_reasoning: Option<String>,
    no_web_search: bool,
    is_default: bool,
}

pub struct ProviderLaunchState {
    pub popup: PopupState,
    kind: LaunchKind,
    alias: String,
    models: Vec<LaunchModel>,
    selected: usize,
    reasoning_idx: usize,
    custom_reasoning: Option<String>,
    target: LaunchTarget,
    last_model_click: Option<(usize, Instant)>,
    catalog_status: Option<String>,
}

pub enum LaunchPickerOutcome {
    Continue,
    Cancel,
    Launch {
        alias: String,
        model: String,
        reasoning: ReasoningLaunch,
        target: LaunchTarget,
    },
}

impl ProviderLaunchState {
    pub fn from_profile(profile: &ProviderProfile) -> Self {
        let models: Vec<LaunchModel> = profile
            .models
            .iter()
            .map(|model| LaunchModel {
                id: model.id.clone(),
                title: model.id.clone(),
                saved_reasoning: model.reasoning.clone(),
                no_web_search: model.no_web_search,
                is_default: model.id.trim() == profile.default_model.trim(),
            })
            .collect();
        let selected = models
            .iter()
            .position(|model| model.is_default)
            .unwrap_or(0);
        let (reasoning_idx, custom_reasoning) = models
            .get(selected)
            .map(|model| reasoning_choice(model.saved_reasoning.as_deref()))
            .unwrap_or((0, None));
        Self {
            popup: PopupState::new(),
            kind: LaunchKind::Provider,
            alias: profile.alias.clone(),
            models,
            selected,
            reasoning_idx,
            custom_reasoning,
            target: LaunchTarget::load(&profile.alias, true),
            last_model_click: None,
            catalog_status: None,
        }
    }

    /// Build the same launch picker for a ChatGPT account. The first row uses
    /// Codex's own default; cached authenticated `/models` entries follow it.
    pub fn from_chatgpt(alias: impl Into<String>, models: &[ModelEntry]) -> Self {
        let alias = alias.into();
        let mut launch_models = vec![LaunchModel {
            id: String::new(),
            title: "(Codex default)".into(),
            saved_reasoning: None,
            no_web_search: false,
            is_default: true,
        }];
        for model in crate::warmup::sorted_models_for_display(models) {
            let title = model
                .display_name
                .as_deref()
                .map(str::trim)
                .filter(|name| !name.is_empty())
                .unwrap_or(model.slug.as_str())
                .to_string();
            launch_models.push(LaunchModel {
                id: model.slug.clone(),
                title,
                saved_reasoning: model.default_reasoning_effort.clone(),
                no_web_search: false,
                is_default: false,
            });
        }
        Self {
            popup: PopupState::new(),
            kind: LaunchKind::Account,
            alias: alias.clone(),
            models: launch_models,
            selected: 0,
            reasoning_idx: 0,
            custom_reasoning: None,
            target: LaunchTarget::load(&alias, false),
            last_model_click: None,
            catalog_status: None,
        }
    }

    pub(crate) fn update_chatgpt_catalog(
        &mut self,
        alias: &str,
        models: Option<&[ModelEntry]>,
        status: Option<String>,
    ) {
        if self.kind != LaunchKind::Account || self.alias != alias {
            return;
        }
        if let Some(models) = models {
            let selected_id = self.models.get(self.selected).map(|model| model.id.clone());
            let previous_reasoning = (self.reasoning_idx, self.custom_reasoning.clone());
            let mut replacement = Self::from_chatgpt(alias, models);
            if let Some(index) = selected_id
                .and_then(|id| replacement.models.iter().position(|model| model.id == id))
            {
                replacement.selected = index;
                replacement.reasoning_idx = previous_reasoning.0;
                replacement.custom_reasoning = previous_reasoning.1;
            }
            replacement.target = self.target;
            replacement.popup.scroll = self.popup.scroll;
            // A prior row index may refer to a different model after a refreshed
            // catalog is sorted or replaced, so it cannot safely complete a double-click.
            replacement.last_model_click = None;
            *self = replacement;
        }
        self.catalog_status = status;
    }

    pub(crate) fn selected_index(&self) -> usize {
        self.selected
    }

    /// Click a model row. A second click on the same row within 500ms launches.
    pub(crate) fn click_model(&mut self, idx: usize) -> bool {
        if idx >= self.models.len() {
            return false;
        }
        let now = Instant::now();
        let double = self.last_model_click.is_some_and(|(prev, at)| {
            prev == idx && now.duration_since(at) <= Duration::from_millis(500)
        });
        self.select(idx);
        if double {
            self.last_model_click = None;
            true
        } else {
            self.last_model_click = Some((idx, now));
            false
        }
    }

    pub(crate) fn click_reasoning(&mut self) {
        self.nudge_reasoning(1);
    }

    pub(crate) fn toggle_target(&mut self) {
        self.target = self.target.toggled();
    }

    pub(crate) fn handle_wheel(&mut self, down: bool) {
        let _ = self.handle_key(if down { KeyCode::Down } else { KeyCode::Up });
    }

    fn select(&mut self, idx: usize) {
        if idx >= self.models.len() {
            return;
        }
        self.selected = idx;
        let (reasoning_idx, custom_reasoning) =
            reasoning_choice(self.models[idx].saved_reasoning.as_deref());
        self.reasoning_idx = reasoning_idx;
        self.custom_reasoning = custom_reasoning;
    }

    fn reasoning_for_launch(&self) -> ReasoningLaunch {
        if let Some(custom) = &self.custom_reasoning {
            return ReasoningLaunch::Effort(custom.clone());
        }
        if self.reasoning_idx == 0 {
            ReasoningLaunch::Skip
        } else {
            ReasoningLaunch::Effort(REASONING_CHOICES[self.reasoning_idx].to_string())
        }
    }

    fn launch_outcome(&mut self) -> LaunchPickerOutcome {
        let Some(model) = self.models.get(self.selected) else {
            return LaunchPickerOutcome::Continue;
        };
        LaunchPickerOutcome::Launch {
            alias: self.alias.clone(),
            model: model.id.clone(),
            reasoning: self.reasoning_for_launch(),
            target: self.target,
        }
    }

    fn reasoning_label(&self) -> &str {
        self.custom_reasoning
            .as_deref()
            .unwrap_or(REASONING_CHOICES[self.reasoning_idx])
    }

    pub fn handle_key(&mut self, code: KeyCode) -> LaunchPickerOutcome {
        match code {
            KeyCode::Esc => LaunchPickerOutcome::Cancel,
            KeyCode::Tab | KeyCode::BackTab => {
                self.toggle_target();
                LaunchPickerOutcome::Continue
            }
            KeyCode::Enter | KeyCode::Char('o') => self.launch_outcome(),
            KeyCode::Down | KeyCode::Char('j') => {
                if self.selected + 1 < self.models.len() {
                    self.select(self.selected + 1);
                }
                LaunchPickerOutcome::Continue
            }
            KeyCode::Up | KeyCode::Char('k') => {
                if self.selected > 0 {
                    self.select(self.selected - 1);
                }
                LaunchPickerOutcome::Continue
            }
            KeyCode::Left => {
                self.nudge_reasoning(-1);
                LaunchPickerOutcome::Continue
            }
            KeyCode::Right => {
                self.nudge_reasoning(1);
                LaunchPickerOutcome::Continue
            }
            _ => LaunchPickerOutcome::Continue,
        }
    }

    fn nudge_reasoning(&mut self, delta: i32) {
        if self.custom_reasoning.take().is_some() {
            if delta > 0 {
                self.reasoning_idx = 0;
            } else {
                self.reasoning_idx = REASONING_CHOICES.len() - 1;
            }
            return;
        }
        if delta < 0 {
            if self.reasoning_idx > 0 {
                self.reasoning_idx -= 1;
            } else {
                self.reasoning_idx = REASONING_CHOICES.len() - 1;
            }
        } else {
            self.reasoning_idx = (self.reasoning_idx + 1) % REASONING_CHOICES.len();
        }
    }
}

pub fn render_provider_launch(
    f: &mut Frame,
    state: &mut ProviderLaunchState,
    area: Rect,
    hitmap: &mut HitMap,
) {
    let mut lines: Vec<Line<'static>> = Vec::new();
    let mut hits: Vec<Option<OverlayClick>> = Vec::new();

    let (kind_label, title, hint) = match state.kind {
        LaunchKind::Provider => (
            "Provider  ",
            "Launch provider",
            "Choose a model and launch target.",
        ),
        LaunchKind::Account => (
            "Account   ",
            "Launch account",
            "Choose a Codex model or use the default.",
        ),
    };
    lines.push(Line::from(vec![
        Span::styled(kind_label, dim()),
        Span::styled(state.alias.clone(), header()),
    ]));
    hits.push(None);
    lines.push(Line::from(vec![
        Span::styled("Tab       ", dim()),
        Span::styled(state.target.label(), header()),
        Span::styled("   (switch Desktop / CLI)", dim()),
    ]));
    hits.push(Some(OverlayClick::LaunchTarget));
    lines.push(Line::from(Span::styled(hint, dim())));
    hits.push(None);
    if state.kind == LaunchKind::Account && state.target == LaunchTarget::Desktop {
        let note = match crate::config::get().language {
            crate::config::Language::En => {
                "Desktop windows share local chats and keep separate logins."
            }
            crate::config::Language::Zh => "桌面窗口共用本地会话，各自登录。",
        };
        lines.push(Line::from(Span::styled(note, dim())));
        hits.push(None);
    }
    lines.push(Line::from(""));
    hits.push(None);

    if let Some(status) = &state.catalog_status {
        lines.push(Line::from(Span::styled(status.clone(), dim())));
        hits.push(None);
    }

    for (idx, model) in state.models.iter().enumerate() {
        let selected = idx == state.selected;
        let marker = if selected { "▶ " } else { "  " };
        let default = if model.is_default { " ● default" } else { "" };
        let search = if model.no_web_search {
            "  no-web-search"
        } else {
            ""
        };
        let style = if selected { header() } else { base() };
        lines.push(Line::from(vec![
            Span::styled(marker, key()),
            Span::styled(format!("{}{default}{search}", model.title), style),
        ]));
        hits.push(Some(OverlayClick::LaunchModel(idx)));
    }

    lines.push(Line::from(""));
    hits.push(None);
    lines.push(Line::from(vec![
        Span::styled("reasoning  ", dim()),
        Span::styled(state.reasoning_label().to_string(), header()),
        Span::styled("   (this session)", dim()),
    ]));
    hits.push(Some(OverlayClick::LaunchReasoning));
    lines.push(Line::from(""));
    hits.push(None);
    lines.push(Line::from(vec![
        Span::styled("j/k", key()),
        Span::styled(" model  ", dim()),
        Span::styled("←/→", key()),
        Span::styled(" reasoning  ", dim()),
        Span::styled("tab", key()),
        Span::styled(" Desktop/CLI  ", dim()),
        Span::styled("enter/o", key()),
        Span::styled(" launch  ", dim()),
        Span::styled("esc", key()),
        Span::styled(" cancel", dim()),
    ]));
    hits.push(Some(OverlayClick::Key(KeyCode::Enter)));

    let layout = popup::render_popup(f, title, &lines, &mut state.popup, area);
    hitmap.overlay = OverlayHit::Modal;
    hitmap.overlay_clicks.clear();
    if let Some(layout) = layout {
        hitmap.overlay_panel = Some(layout.panel);
        layout.push_line_hits(&mut hitmap.overlay_clicks, &hits);
    }
}

#[cfg(test)]
mod tests {
    use crossterm::event::KeyCode;

    use super::{LaunchPickerOutcome, ProviderLaunchState};
    use crate::provider::{ProviderModel, ProviderProfile, ReasoningLaunch};
    use crate::warmup::ModelEntry;

    fn profile() -> ProviderProfile {
        let mut p = ProviderProfile::build(
            "or",
            "https://openrouter.ai/api/v1",
            vec![
                ProviderModel::from_id("minimax/minimax-m3:free"),
                ProviderModel {
                    id: "liquid/lfm-2.5-2.6b:free".into(),
                    reasoning: Some("high".into()),
                    no_web_search: true,
                },
            ],
            "sk-test",
        );
        p.default_model = "minimax/minimax-m3:free".into();
        p
    }

    #[test]
    fn picker_starts_on_default_and_can_select_another_model_with_reasoning() {
        let mut picker = ProviderLaunchState::from_profile(&profile());
        assert!(matches!(
            picker.handle_key(KeyCode::Down),
            LaunchPickerOutcome::Continue
        ));
        assert!(matches!(
            picker.handle_key(KeyCode::Right),
            LaunchPickerOutcome::Continue
        ));
        let LaunchPickerOutcome::Launch {
            alias,
            model,
            reasoning,
            target,
        } = picker.handle_key(KeyCode::Enter)
        else {
            panic!("enter should launch");
        };
        assert_eq!(alias, "or");
        assert_eq!(model, "liquid/lfm-2.5-2.6b:free");
        // saved high (index of "high") then one Right → xhigh
        assert_eq!(reasoning, ReasoningLaunch::Effort("xhigh".into()));
        assert_eq!(target, crate::desktop::LaunchTarget::Desktop);
    }

    #[test]
    fn late_catalog_update_preserves_account_picker_input_and_selection() {
        let initial = [ModelEntry {
            slug: "existing-model".into(),
            default_reasoning_effort: Some("high".into()),
            ..ModelEntry::default()
        }];
        let mut picker = ProviderLaunchState::from_chatgpt("work", &initial);
        picker.handle_key(KeyCode::Down);
        picker.handle_key(KeyCode::Right);
        picker.handle_key(KeyCode::Tab);
        picker.popup.scroll = 3;

        let updated = [
            initial[0].clone(),
            ModelEntry {
                slug: "gpt-6.1-sol".into(),
                ..ModelEntry::default()
            },
        ];
        picker.update_chatgpt_catalog("work", Some(&updated), None);

        assert_eq!(picker.models[picker.selected].id, "existing-model");
        assert_eq!(picker.reasoning_label(), "xhigh");
        assert_eq!(picker.target, crate::desktop::LaunchTarget::Cli);
        assert_eq!(picker.popup.scroll, 3);
        assert!(picker.models.iter().any(|model| model.id == "gpt-6.1-sol"));
    }

    #[test]
    fn account_catalog_error_is_visible_and_alias_guard_preserves_provider_picker() {
        let mut account = ProviderLaunchState::from_chatgpt("work", &[]);
        account.target = crate::desktop::LaunchTarget::Desktop;
        account.update_chatgpt_catalog("other", None, Some("wrong account".into()));
        assert!(account.catalog_status.is_none());
        account.update_chatgpt_catalog(
            "work",
            None,
            Some("Loading model catalog… Codex default remains available.".into()),
        );
        let backend = TestBackend::new(90, 20);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| {
                super::render_provider_launch(
                    frame,
                    &mut account,
                    frame.area(),
                    &mut crate::tui::hitmap::HitMap::default(),
                )
            })
            .unwrap();
        let joined = (0..20)
            .map(|y| row_text(terminal.backend(), y))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(joined.contains("Loading model catalog"));
        assert!(joined.contains("(Codex default)"));
        assert!(joined.contains("share local chats") || joined.contains("共用本地会话"));

        account.update_chatgpt_catalog(
            "work",
            None,
            Some("Model catalog unavailable: offline. Codex default remains available.".into()),
        );

        terminal
            .draw(|frame| {
                super::render_provider_launch(
                    frame,
                    &mut account,
                    frame.area(),
                    &mut crate::tui::hitmap::HitMap::default(),
                )
            })
            .unwrap();
        let joined = (0..20)
            .map(|y| row_text(terminal.backend(), y))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(joined.contains("Model catalog unavailable: offline"));
        assert!(joined.contains("(Codex default)"));
        assert!(
            matches!(account.handle_key(KeyCode::Enter), LaunchPickerOutcome::Launch { model, .. } if model.is_empty())
        );

        let mut provider = ProviderLaunchState::from_profile(&profile());
        provider.update_chatgpt_catalog(
            "or",
            None,
            Some("should not affect provider picker".into()),
        );
        assert!(provider.catalog_status.is_none());
        assert_eq!(provider.models.len(), 2);
    }

    use ratatui::{Terminal, backend::TestBackend};

    fn row_text(backend: &TestBackend, y: u16) -> String {
        let area = backend.buffer().area;
        (0..area.width)
            .map(|x| {
                backend
                    .buffer()
                    .cell((x, y))
                    .expect("cell")
                    .symbol()
                    .to_string()
            })
            .collect()
    }

    #[test]
    fn picker_renders_models_and_session_reasoning() {
        let mut picker = ProviderLaunchState::from_profile(&profile());
        picker.handle_key(KeyCode::Down);
        let backend = TestBackend::new(80, 20);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| {
                super::render_provider_launch(
                    frame,
                    &mut picker,
                    frame.area(),
                    &mut crate::tui::hitmap::HitMap::default(),
                )
            })
            .unwrap();
        let joined = (0..20)
            .map(|y| row_text(terminal.backend(), y))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(joined.contains("Launch provider"));
        assert!(joined.contains("liquid/lfm-2.5-2.6b:free"));
        assert!(joined.contains("high"));
        assert!(joined.contains("this session"));
        assert!(joined.contains("enter/o"));
        assert!(!joined.contains("sk-test"));
    }

    #[test]
    fn picker_can_skip_saved_reasoning_for_this_launch() {
        let mut picker = ProviderLaunchState::from_profile(&profile());
        picker.handle_key(KeyCode::Down);
        // saved high is index 4; four Left steps reach (skip)
        for _ in 0..4 {
            picker.handle_key(KeyCode::Left);
        }
        let LaunchPickerOutcome::Launch { reasoning, .. } = picker.handle_key(KeyCode::Enter)
        else {
            panic!("enter should launch");
        };
        assert_eq!(reasoning, ReasoningLaunch::Skip);
    }

    #[test]
    fn chatgpt_picker_starts_with_codex_default_without_model_or_reasoning() {
        let mut picker = ProviderLaunchState::from_chatgpt("work", &[]);
        let LaunchPickerOutcome::Launch {
            alias,
            model,
            reasoning,
            target,
        } = picker.handle_key(KeyCode::Enter)
        else {
            panic!("enter should launch");
        };
        assert_eq!(alias, "work");
        assert!(model.is_empty());
        assert_eq!(reasoning, ReasoningLaunch::Skip);
        assert_eq!(target, crate::desktop::LaunchTarget::Desktop);
    }

    #[test]
    fn chatgpt_picker_uses_cached_model_and_default_reasoning() {
        let models = [ModelEntry {
            slug: "gpt-5.4".into(),
            display_name: Some("GPT-5.4".into()),
            default_reasoning_effort: Some("high".into()),
            ..ModelEntry::default()
        }];
        let mut picker = ProviderLaunchState::from_chatgpt("work", &models);
        picker.handle_key(KeyCode::Down);
        let LaunchPickerOutcome::Launch {
            alias,
            model,
            reasoning,
            ..
        } = picker.handle_key(KeyCode::Enter)
        else {
            panic!("enter should launch");
        };
        assert_eq!(alias, "work");
        assert_eq!(model, "gpt-5.4");
        assert_eq!(reasoning, ReasoningLaunch::Effort("high".into()));
    }

    #[test]
    fn picker_keeps_custom_saved_reasoning_until_nudged() {
        let mut p = profile();
        p.models[0].reasoning = Some("custom-effort".into());
        p.default_model = p.models[0].id.clone();
        let mut picker = ProviderLaunchState::from_profile(&p);
        let LaunchPickerOutcome::Launch { reasoning, .. } = picker.handle_key(KeyCode::Enter)
        else {
            panic!("enter should launch");
        };
        assert_eq!(reasoning, ReasoningLaunch::Effort("custom-effort".into()));
    }

    #[test]
    fn picker_tab_toggles_target_without_launching() {
        let mut picker = ProviderLaunchState::from_chatgpt("work", &[]);
        picker.handle_key(KeyCode::Tab);
        assert_eq!(picker.target, crate::desktop::LaunchTarget::Cli);
        picker.handle_key(KeyCode::BackTab);
        assert_eq!(picker.target, crate::desktop::LaunchTarget::Desktop);
    }

    #[test]
    fn picker_o_confirms_launch_like_enter() {
        let mut picker = ProviderLaunchState::from_profile(&profile());
        let LaunchPickerOutcome::Launch { alias, model, .. } =
            picker.handle_key(KeyCode::Char('o'))
        else {
            panic!("o should launch from the picker");
        };
        assert_eq!(alias, "or");
        assert_eq!(model, "minimax/minimax-m3:free");
    }
}
