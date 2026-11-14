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
}

pub enum SyncCommand {
    None,
    Pull,
    Push,
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
        }
    }

    pub fn is_editing(&self) -> bool {
        self.editing
    }

    pub fn group_names(&self) -> Vec<String> {
        vec![self.settings.group_name.clone()]
    }

    pub fn begin(&mut self) {
        self.busy = true;
        self.notice = None;
        self.error = None;
    }

    pub fn finish(&mut self, result: Result<String, String>) {
        self.busy = false;
        match result {
            Ok(message) => {
                self.error = None;
                self.notice = Some(message);
            }
            Err(message) => {
                self.notice = None;
                self.error = Some(message);
            }
        }
    }

    pub fn handle_key(&mut self, code: KeyCode, language: Language) -> SyncCommand {
        if self.busy {
            return SyncCommand::None;
        }
        if self.editing {
            return self.handle_edit(code, language);
        }
        match code {
            KeyCode::Up | KeyCode::Char('k') => self.move_focus(-1),
            KeyCode::Down | KeyCode::Char('j') => self.move_focus(1),
            KeyCode::Enter | KeyCode::Char('e') => match self.focus {
                Focus::Pull => return SyncCommand::Pull,
                Focus::Push => return SyncCommand::Push,
                Focus::Group | Focus::Url | Focus::Password => self.begin_edit(),
            },
            KeyCode::Char('p') => return SyncCommand::Pull,
            KeyCode::Char('u') => return SyncCommand::Push,
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
                self.settings.group_name = value;
            }
            Focus::Url => self.settings.redis_url = value,
            Focus::Password => self.settings.password = value,
            Focus::Pull | Focus::Push => {}
        }
        sync_store::save(&self.settings).map_err(|error| error.to_string())
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
}

fn char_to_byte(text: &str, char_index: usize) -> usize {
    text.char_indices()
        .nth(char_index)
        .map(|(index, _)| index)
        .unwrap_or(text.len())
}
