//! Sync tab: one named group, a Redis target, and manual pull or push.

use crossterm::event::KeyCode;
use ratatui::{
    Frame,
    layout::Rect,
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, Wrap},
};

use super::theme::{C_CYAN, C_GRAY, C_RED, C_YELLOW, base, highlight};
use crate::config::Language;
use crate::sync_store::{self, SyncSettings};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Focus {
    Group,
    Url,
    Password,
    Pull,
    Push,
}

const ORDER: &[Focus] = &[
    Focus::Group,
    Focus::Url,
    Focus::Password,
    Focus::Pull,
    Focus::Push,
];

pub struct SyncPage {
    pub settings: SyncSettings,
    focus: Focus,
    editing: bool,
    input: String,
    cursor: usize,
    pub busy: bool,
    notice: Option<String>,
    error: Option<String>,
    picker: Option<GroupPicker>,
}

pub enum SyncReport {
    Note(String),
    Changed(String),
    RemoteGroups(Vec<String>),
}

pub enum SyncCommand {
    None,
    ListRemote,
    PushGroups {
        groups: Vec<String>,
        replace_remote: bool,
    },
    PullGroups {
        groups: Vec<String>,
    },
}

struct GroupPicker {
    upload: bool,
    groups: Vec<String>,
    checked: Vec<bool>,
    cursor: usize,
}

impl SyncPage {
    pub fn load() -> Self {
        Self {
            settings: sync_store::load(),
            focus: Focus::Group,
            editing: false,
            input: String::new(),
            cursor: 0,
            busy: false,
            notice: None,
            error: None,
            picker: None,
        }
    }

    pub fn is_editing(&self) -> bool {
        self.editing
    }

    pub fn captures_keys(&self) -> bool {
        self.editing || self.picker.is_some()
    }

    pub fn group_names(&self) -> Vec<String> {
        sync_store::local_group_names(&self.settings.group_name)
            .unwrap_or_else(|_| vec![self.settings.group_name.clone()])
    }

    pub fn begin(&mut self) {
        self.busy = true;
        self.notice = None;
        self.error = None;
    }

    pub fn finish(&mut self, result: Result<SyncReport, String>, language: Language) -> bool {
        self.busy = false;
        match result {
            Ok(SyncReport::Note(message)) => {
                self.error = None;
                self.notice = Some(message);
                false
            }
            Ok(SyncReport::Changed(message)) => {
                self.error = None;
                self.notice = Some(message);
                true
            }
            Ok(SyncReport::RemoteGroups(groups)) => {
                self.show_remote_groups(groups, language);
                false
            }
            Err(message) => {
                self.notice = None;
                self.error = Some(message);
                false
            }
        }
    }

    pub fn show_remote_groups(&mut self, groups: Vec<String>, language: Language) {
        self.busy = false;
        if groups.is_empty() {
            self.error = Some(super::locale::no_remote_groups(language));
            return;
        }
        self.error = None;
        self.open_picker(false, groups);
    }

    pub fn handle_key(&mut self, code: KeyCode, language: Language) -> SyncCommand {
        if self.busy {
            return SyncCommand::None;
        }
        if self.picker.is_some() {
            return self.handle_picker(code);
        }
        if self.editing {
            return self.handle_edit(code, language);
        }
        match code {
            KeyCode::Up | KeyCode::Char('k') => self.move_focus(-1),
            KeyCode::Down | KeyCode::Char('j') => self.move_focus(1),
            KeyCode::Enter | KeyCode::Char('e') => match self.focus {
                Focus::Pull => return SyncCommand::ListRemote,
                Focus::Push => self.open_upload(),
                Focus::Group | Focus::Url | Focus::Password => self.begin_edit(),
            },
            KeyCode::Char('p') => return SyncCommand::ListRemote,
            KeyCode::Char('u') => self.open_upload(),
            _ => {}
        }
        SyncCommand::None
    }

    fn open_upload(&mut self) {
        match sync_store::local_group_names(&self.settings.group_name) {
            Ok(groups) => self.open_picker(true, groups),
            Err(error) => self.error = Some(error.to_string()),
        }
    }

    fn open_picker(&mut self, upload: bool, groups: Vec<String>) {
        let checked = vec![false; groups.len()];
        self.picker = Some(GroupPicker {
            upload,
            groups,
            checked,
            cursor: 0,
        });
        self.notice = None;
        self.error = None;
    }

    fn handle_picker(&mut self, code: KeyCode) -> SyncCommand {
        let Some(picker) = self.picker.as_mut() else {
            return SyncCommand::None;
        };
        match code {
            KeyCode::Esc | KeyCode::Char('q') => {
                self.picker = None;
            }
            KeyCode::Up | KeyCode::Char('k') if picker.cursor > 0 => picker.cursor -= 1,
            KeyCode::Down | KeyCode::Char('j') if picker.cursor + 1 < picker.groups.len() => {
                picker.cursor += 1;
            }
            KeyCode::Char(' ') => {
                let cursor = picker.cursor;
                if let Some(checked) = picker.checked.get_mut(cursor) {
                    *checked = !*checked;
                }
            }
            KeyCode::Enter => {
                let groups = selected_groups(picker);
                let upload = picker.upload;
                self.picker = None;
                if groups.is_empty() {
                    return SyncCommand::None;
                }
                if upload {
                    return SyncCommand::PushGroups {
                        groups,
                        replace_remote: false,
                    };
                }
                return SyncCommand::PullGroups { groups };
            }
            KeyCode::Char('a') => {
                let groups = picker.groups.clone();
                let upload = picker.upload;
                self.picker = None;
                if upload {
                    return SyncCommand::PushGroups {
                        groups,
                        replace_remote: true,
                    };
                }
                return SyncCommand::PullGroups { groups };
            }
            _ => {}
        }
        SyncCommand::None
    }

    fn move_focus(&mut self, delta: isize) {
        let index = ORDER.iter().position(|item| *item == self.focus).unwrap_or(0);
        let len = ORDER.len() as isize;
        let next = (index as isize + delta).rem_euclid(len) as usize;
        self.focus = ORDER[next];
    }

    fn begin_edit(&mut self) {
        self.input = match self.focus {
            Focus::Group => self.settings.group_name.clone(),
            Focus::Url => self.settings.redis_url.clone(),
            Focus::Password => self.settings.password.clone(),
            Focus::Pull | Focus::Push => return,
        };
        self.cursor = self.input.chars().count();
        self.editing = true;
        self.error = None;
    }

    fn handle_edit(&mut self, code: KeyCode, language: Language) -> SyncCommand {
        match code {
            KeyCode::Esc => {
                self.editing = false;
                self.input.clear();
            }
            KeyCode::Enter => {
                if let Err(error) = self.commit_edit() {
                    self.error = Some(error);
                } else {
                    self.editing = false;
                    self.notice = Some(super::locale::t(language).sync_saved.to_string());
                    self.error = None;
                }
            }
            KeyCode::Backspace => {
                if self.cursor > 0 {
                    self.cursor -= 1;
                    let byte = char_to_byte(&self.input, self.cursor);
                    self.input.remove(byte);
                }
            }
            KeyCode::Char(ch) => {
                let byte = char_to_byte(&self.input, self.cursor);
                self.input.insert(byte, ch);
                self.cursor += 1;
            }
            _ => {}
        }
        SyncCommand::None
    }

    fn commit_edit(&mut self) -> Result<(), String> {
        let value = self.input.trim().to_string();
        match self.focus {
            Focus::Group => {
                crate::profile::validate_alias(&value).map_err(|error| error.to_string())?;
                let previous = self.settings.group_name.clone();
                self.settings.group_name = value;
                if let Err(error) = sync_store::save(&self.settings) {
                    self.settings.group_name = previous;
                    return Err(error.to_string());
                }
                if let Err(error) = sync_store::rename_group(&previous, &self.settings.group_name) {
                    return Err(error.to_string());
                }
                return Ok(());
            }
            Focus::Url => self.settings.redis_url = value,
            Focus::Password => self.settings.password = value,
            Focus::Pull | Focus::Push => {}
        }
        if !matches!(self.focus, Focus::Group) {
            sync_store::save(&self.settings).map_err(|error| error.to_string())?;
        }
        Ok(())
    }
}

pub fn render(frame: &mut Frame, page: &SyncPage, area: Rect, language: Language) {
    let text = super::locale::t(language);
    let block = Block::default()
        .title(text.sync_title)
        .borders(Borders::ALL)
        .border_style(base().fg(C_CYAN))
        .style(base());
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let group = page.settings.group_name.clone();
    let url = if page.settings.redis_url.is_empty() {
        text.sync_unset.to_string()
    } else {
        crate::remote_store::display_url(&page.settings.redis_url)
    };
    let password = if page.settings.password.is_empty() && !page.settings.redis_url.contains('@') {
        text.sync_unset.to_string()
    } else {
        text.sync_set.to_string()
    };
    let rows = [
        (Focus::Group, format!("{}    {group}", text.sync_group)),
        (Focus::Url, format!("{}  {url}", text.sync_url)),
        (Focus::Password, format!("{}      {password}", text.sync_password)),
        (Focus::Pull, text.sync_pull.to_string()),
        (Focus::Push, text.sync_push.to_string()),
    ];
    let mut lines = vec![
        Line::from(Span::styled(text.sync_intro_1, base().fg(C_GRAY))),
        Line::from(Span::styled(text.sync_intro_2, base().fg(C_GRAY))),
        Line::from(""),
    ];
    for (focus, shown_text) in rows {
        let style = if page.focus == focus {
            highlight()
        } else {
            base()
        };
        let shown = if page.editing && page.focus == focus {
            format!("{shown_text}  ← {}", page.input)
        } else {
            shown_text
        };
        lines.push(Line::from(Span::styled(shown, style)));
    }
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(text.sync_help, base().fg(C_GRAY))));
    if page.busy {
        lines.push(Line::from(Span::styled(text.sync_busy, base().fg(C_YELLOW))));
    }
    if let Some(notice) = &page.notice {
        lines.push(Line::from(Span::styled(notice.clone(), base().fg(C_CYAN))));
    }
    if let Some(error) = &page.error {
        lines.push(Line::from(Span::styled(error.clone(), base().fg(C_RED))));
    }
    frame.render_widget(
        Paragraph::new(lines).style(base()).wrap(Wrap { trim: false }),
        inner,
    );
    if let Some(picker) = &page.picker {
        render_picker(frame, picker, inner, language);
    }
}

fn render_picker(frame: &mut Frame, picker: &GroupPicker, area: Rect, language: Language) {
    let text = super::locale::t(language);
    let title = if picker.upload {
        text.picker_upload_title
    } else {
        text.picker_download_title
    };
    let help = if picker.upload {
        text.picker_upload_help
    } else {
        text.picker_download_help
    };
    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_style(base().fg(C_YELLOW))
        .style(base());
    let height = area.height.min(14).max(5);
    let width = area.width.min(72).max(20);
    let popup = Rect {
        x: area.x + area.width.saturating_sub(width) / 2,
        y: area.y,
        width,
        height,
    };
    let inner = block.inner(popup);
    frame.render_widget(ratatui::widgets::Clear, popup);
    frame.render_widget(block, popup);
    let visible = usize::from(inner.height.saturating_sub(2)).max(1);
    let scroll = if picker.cursor + 1 > visible {
        picker.cursor + 1 - visible
    } else {
        0
    };
    let mut lines = vec![Line::from(Span::styled(help, base().fg(C_GRAY))), Line::from("")];
    for (index, name) in picker.groups.iter().enumerate().skip(scroll).take(visible) {
        let mark = if picker.checked[index] { "[x]" } else { "[ ]" };
        let style = if index == picker.cursor { highlight() } else { base() };
        lines.push(Line::from(Span::styled(format!("{mark} {name}"), style)));
    }
    frame.render_widget(Paragraph::new(lines).style(base()), inner);
}

fn selected_groups(picker: &GroupPicker) -> Vec<String> {
    let checked: Vec<String> = picker
        .groups
        .iter()
        .enumerate()
        .filter(|(index, _)| picker.checked[*index])
        .map(|(_, name)| name.clone())
        .collect();
    if !checked.is_empty() {
        return checked;
    }
    picker.groups.get(picker.cursor).cloned().into_iter().collect()
}

fn char_to_byte(text: &str, char_index: usize) -> usize {
    text.char_indices()
        .nth(char_index)
        .map(|(index, _)| index)
        .unwrap_or(text.len())
}
