//! Keyboard-oriented projection of published inspection-session evidence.
use std::sync::Arc;

use crate::inspection::{
    CellIdentity, EntityIdentity, InspectionGraph, InspectionSession, SessionState,
};

mod evidence;
mod navigation;
mod runtime;
pub use runtime::run;
mod deep;

#[derive(Clone, Copy, Debug)]
pub enum Key {
    Up,
    Down,
    Enter,
    Back,
    PageUp,
    PageDown,
    Char(char),
    Quit,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Focus {
    Database,
    Schema,
    Object(CellIdentity),
    Btrees,
    Freelist,
    PointerMaps,
    Diagnostics,
    Page(u32),
    Cell(CellIdentity),
    Help,
}

struct Location {
    focus: Focus,
    label: String,
    selected: usize,
}
struct Entry {
    label: String,
    focus: Focus,
}

/// User-visible terminal boundary. The actual terminal and harness use identical keys and frames.
pub struct TerminalFlow {
    session: Arc<InspectionSession>,
    graph: Option<Arc<InspectionGraph>>,
    path: Vec<Location>,
    revision: u64,
    evidence_offset: usize,
    evidence_column: usize,
    evidence_page_size: usize,
    viewport_width: usize,
    deep_budget: crate::inspection::DeepBudget,
    message: String,
    quit: bool,
    prompt: Option<String>,
    deep: Option<deep::PendingDeep>,
    values: Option<crate::inspection::DeepResult>,
}

impl TerminalFlow {
    #[must_use]
    pub fn new(session: Arc<InspectionSession>) -> Self {
        let mut flow = Self {
            session,
            graph: None,
            path: vec![Location {
                focus: Focus::Database,
                label: "Database".into(),
                selected: 0,
            }],
            revision: 1,
            evidence_offset: 0,
            evidence_column: 0,
            evidence_page_size: 1,
            viewport_width: 80,
            deep_budget: crate::inspection::DeepBudget::default(),
            message: String::new(),
            quit: false,
            prompt: None,
            deep: None,
            values: None,
        };
        flow.refresh();
        flow
    }

    /// Configures explicit request budgets; the inspection session enforces its admission ceilings.
    #[must_use]
    pub fn with_deep_budget(mut self, budget: crate::inspection::DeepBudget) -> Self {
        self.deep_budget = budget;
        self
    }

    fn refresh(&mut self) {
        let status = self.session.status();
        if status.state == SessionState::Invalidated {
            self.graph = None;
            self.withhold_values();
            self.message = "Invalidated snapshot: navigation and values withheld".into();
        } else if self.graph.is_none() && status.available_revisions.contains(&self.revision) {
            match self.session.revision(self.revision) {
                Ok(graph) => self.graph = Some(graph),
                Err(_) => self.message = "Revision unavailable".into(),
            }
        }
    }

    fn change_revision(&mut self, forward: bool) {
        let revisions = self.session.status().available_revisions;
        let target = if forward {
            revisions
                .into_iter()
                .find(|revision| *revision > self.revision)
        } else {
            revisions
                .into_iter()
                .rev()
                .find(|revision| *revision < self.revision)
        };
        if let Some(target) = target {
            self.withhold_values();
            match self.session.revision(target) {
                Ok(graph) => {
                    self.graph = Some(graph);
                    self.revision = target;
                    self.message = "Revision changed; values withheld".into();
                }
                Err(_) => self.message = "Revision unavailable".into(),
            }
        }
    }

    fn goto(&mut self, input: &str) {
        let mut parts = input.split(':');
        let page = parts.next().and_then(|part| part.parse::<u32>().ok());
        let cell = parts.next().map(str::parse::<u16>);
        if parts.next().is_some() {
            self.message = "Invalid selector: use page or page:cell".into();
            return;
        }
        let Some(page) = page.and_then(|number| {
            self.graph
                .as_ref()?
                .pages
                .iter()
                .find(|page| page.number == number)
        }) else {
            self.message = "Invalid selector: page unavailable in this revision".into();
            return;
        };
        let number = page.number;
        let identity = match cell {
            None => None,
            Some(Ok(index)) => {
                if let Some(cell) = page
                    .detail
                    .cells
                    .iter()
                    .find(|cell| cell.identity.index == index)
                {
                    Some(cell.identity.clone())
                } else {
                    self.message = "Invalid selector: cell unavailable".into();
                    return;
                }
            }
            Some(Err(_)) => {
                self.message = "Invalid selector: cell unavailable".into();
                return;
            }
        };
        self.path.truncate(1);
        self.enter(Focus::Page(number), format!("Page {number}"));
        if let Some(identity) = identity {
            let label = format!("Cell {}", identity.index);
            self.enter(Focus::Cell(identity), label);
        }
        self.message.clear();
    }

    fn location(&self) -> &Location {
        self.path.last().expect("database path always exists")
    }
    fn location_mut(&mut self) -> &mut Location {
        self.path.last_mut().expect("database path always exists")
    }
    fn focus(&self) -> &Focus {
        &self.location().focus
    }

    pub fn key(&mut self, key: Key) {
        self.refresh();
        if let Some(mut prompt) = self.prompt.take() {
            match key {
                Key::Enter => self.goto(&prompt),
                Key::Back => (),
                Key::Quit | Key::Char('q') => {
                    self.cancel_work();
                    self.quit = true;
                }
                Key::Char(c) if (c.is_ascii_digit() || c == ':') && prompt.len() < 24 => {
                    prompt.push(c);
                    self.prompt = Some(prompt);
                }
                _ => self.prompt = Some(prompt),
            }
            return;
        }
        match key {
            Key::Quit | Key::Char('q') => {
                self.cancel_work();
                self.quit = true;
            }
            Key::Char('h') => {
                self.evidence_column = self
                    .evidence_column
                    .saturating_sub((self.viewport_width / 2).max(1));
            }
            Key::Char('l') => {
                self.evidence_column = self
                    .evidence_column
                    .saturating_add((self.viewport_width / 2).max(1));
            }
            Key::Char('0') => {
                self.evidence_offset = 0;
                self.evidence_column = 0;
            }
            Key::Char('g') => {
                self.withhold_values();
                self.prompt = Some(String::new());
            }
            Key::Char('d') => self.start_deep(),
            Key::Char('c') => self.cancel_work(),
            Key::Char('s') => {
                let _ = self.session.stop(crate::inspection::ScanControl::Stop);
            }
            Key::Char('[') => self.change_revision(false),
            Key::Char(']') => self.change_revision(true),
            Key::Back => {
                self.withhold_values();
                if self.path.len() > 1 {
                    self.path.pop();
                }
                self.evidence_offset = 0;
            }
            Key::Char('?') => self.enter(Focus::Help, "Help".into()),
            Key::Char('!') => self.enter(Focus::Diagnostics, "Diagnostics".into()),
            Key::Up | Key::Char('k') => {
                let location = self.location_mut();
                location.selected = location.selected.saturating_sub(1);
            }
            Key::Down | Key::Char('j') => {
                let count = self.entries().len();
                let location = self.location_mut();
                location.selected = (location.selected + 1).min(count.saturating_sub(1));
            }
            Key::Enter => {
                let selected = self.location().selected;
                if let Some(entry) = self.entries().into_iter().nth(selected) {
                    self.enter(entry.focus, entry.label);
                }
            }
            Key::PageUp => {
                self.evidence_offset = self.evidence_offset.saturating_sub(self.evidence_page_size);
            }
            Key::PageDown => {
                self.evidence_offset = self.evidence_offset.saturating_add(self.evidence_page_size);
            }
            Key::Char(_) => (),
        }
    }

    fn enter(&mut self, focus: Focus, label: String) {
        self.withhold_values();
        self.path.push(Location {
            focus,
            label,
            selected: 0,
        });
        self.evidence_offset = 0;
        self.evidence_column = 0;
    }

    fn status_lines(&self, status: &crate::inspection::SessionStatus) -> Vec<String> {
        vec![
            format!("Volmap | {}", status.source.display_name),
            format!(
                "{:?} | Revision {}",
                status.state,
                if status.state == SessionState::Invalidated {
                    "unavailable".into()
                } else {
                    self.revision.to_string()
                }
            ),
            "Main-file image; sidecars not applied".into(),
            format!(
                "Fast coverage: {}",
                status.coverage.as_ref().map_or_else(
                    || format!(
                        "pending {}/{} pages",
                        status.progress.completed,
                        status
                            .progress
                            .total
                            .map_or_else(|| "?".into(), |total| total.to_string())
                    ),
                    |c| format!(
                        "{:?} {}/{} pages",
                        c.reason,
                        c.evaluated,
                        c.total.map_or_else(|| "?".into(), |n| n.to_string())
                    )
                )
            ),
            format!(
                "Sidecars: {}",
                self.graph.as_ref().map_or_else(
                    || "pending; never applied".into(),
                    |graph| graph
                        .sidecars
                        .iter()
                        .map(|s| format!("{} {}", s.kind, s.state))
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            ),
            self.deep_summary(),
            self.path
                .iter()
                .map(|location| location.label.as_str())
                .collect::<Vec<_>>()
                .join(" / "),
        ]
    }

    fn full_path(&self) -> String {
        self.path
            .iter()
            .map(|location| location.label.as_str())
            .collect::<Vec<_>>()
            .join(" / ")
    }

    fn breadcrumb(&self, width: usize) -> String {
        use unicode_width::UnicodeWidthStr;
        let path = self.full_path();
        if safe_line(&path, width + 1).width() <= width {
            return path;
        }
        let suffix = self
            .path
            .iter()
            .skip(self.path.len().saturating_sub(2))
            .map(|location| location.label.as_str())
            .collect::<Vec<_>>()
            .join(" / ");
        let abbreviated = format!("Database / ... / {suffix}");
        if safe_line(&abbreviated, width + 1).width() <= width {
            return abbreviated;
        }
        let abbreviated = format!("... / {suffix}");
        if safe_line(&abbreviated, width + 1).width() <= width {
            return abbreviated;
        }
        match self.focus() {
            Focus::Object(identity) => format!(
                "... / Schema cell {}:{}",
                identity.page_number, identity.index
            ),
            _ => format!("... / {}", self.location().label),
        }
    }

    fn focused_lines(&self, status: &crate::inspection::SessionStatus) -> Vec<String> {
        let mut evidence = self.value_lines();
        evidence.push(format!("Path: {}", self.full_path()));
        if status.state == SessionState::Invalidated {
            evidence.push(
                "Snapshot invalidated; retained observations are diagnostic evidence only.".into(),
            );
            if let Some(observed) = self.session.evidence() {
                evidence.push(format!(
                    "Retained {} pages | coverage {:?}",
                    observed.pages.len(),
                    observed.coverage.reason
                ));
                evidence.extend(
                    observed
                        .diagnostics
                        .iter()
                        .map(|finding| format!("Diagnostic: {}", finding.code)),
                );
                evidence.extend(
                    observed
                        .sidecars
                        .iter()
                        .map(|sidecar| format!("{}: {}", sidecar.kind, sidecar.state)),
                );
            }
        } else {
            evidence.extend(evidence::lines(self.focus(), self.graph.as_deref()));
        }
        if let Some(diagnostic) = &status.diagnostic {
            evidence.push(format!("{}: {}", diagnostic.code, diagnostic.message));
        }
        evidence.extend(self.status_lines(status));
        evidence
    }

    /// Renders a bounded frame, without terminal escape sequences or raw payload bytes.
    pub fn screen(&mut self, width: u16, height: u16) -> Vec<String> {
        self.refresh();
        self.refresh_deep();
        let status = self.session.status();
        if status.state == SessionState::Invalidated {
            self.graph = None;
            self.withhold_values();
            self.message = "Invalidated snapshot: navigation and values withheld".into();
        }
        self.viewport_width = usize::from(width);
        let mut lines = self.status_lines(&status);
        if width < 40 || height < 16 {
            lines = vec![
                format!("Narrow terminal | {:?} r{}", status.state, self.revision),
                "Main-file; sidecars not applied".into(),
                format!(
                    "Fast {} {}/{}",
                    status
                        .coverage
                        .as_ref()
                        .map_or_else(|| "pending".into(), |c| format!("{:?}", c.reason)),
                    status.progress.completed,
                    status
                        .progress
                        .total
                        .map_or_else(|| "?".into(), |n| n.to_string())
                ),
                self.compact_deep_summary(),
                self.breadcrumb(usize::from(width)),
            ];
        } else if let Some(path) = lines.last_mut() {
            *path = self.breadcrumb(usize::from(width));
        }
        let entries = self.entries();
        let selected = self.location().selected;
        let body = usize::from(height).saturating_sub(lines.len() + 3);
        let list_height = (body / 3).max(1);
        let start = selected.saturating_sub(list_height - 1);
        if entries.is_empty() {
            lines.push("(No targets in this context)".into());
        } else {
            lines.extend(
                entries
                    .iter()
                    .enumerate()
                    .skip(start)
                    .take(list_height)
                    .map(|(i, e)| format!("{} {}", if i == selected { ">" } else { " " }, e.label)),
            );
        }
        lines.push(format!(
            "Evidence row {} col {} (PgUp/PgDn h/l)",
            self.evidence_offset + 1,
            self.evidence_column + 1
        ));
        let evidence = self.focused_lines(&status);
        self.evidence_offset = self.evidence_offset.min(evidence.len().saturating_sub(1));
        let remaining = usize::from(height).saturating_sub(lines.len() + 2);
        self.evidence_page_size = remaining.max(1);
        lines.extend(
            evidence
                .into_iter()
                .skip(self.evidence_offset)
                .take(remaining)
                .map(|line| safe_window(&line, usize::from(width), self.evidence_column)),
        );
        lines.push(self.prompt.as_ref().map_or_else(
            || self.message.clone(),
            |prompt| format!("Go to page[:cell]: {prompt} (Enter / Esc)"),
        ));
        lines
            .push("? help q quit | Up/Down Enter Esc | d deep c cancel [ ] rev g go ! diag".into());
        lines
            .into_iter()
            .take(usize::from(height))
            .map(|line| safe_line(&line, usize::from(width)))
            .collect()
    }
}

fn entity_page(identity: &EntityIdentity) -> u32 {
    match identity {
        EntityIdentity::Page { page_number } | EntityIdentity::Cell { page_number, .. } => {
            *page_number
        }
    }
}

fn safe_line(text: &str, width: usize) -> String {
    safe_window(text, width, 0)
}

fn safe_window(text: &str, width: usize, offset: usize) -> String {
    use unicode_width::UnicodeWidthChar;
    let mut result = String::new();
    let mut columns = 0;
    let mut output_columns = 0;
    for ch in text.chars() {
        let display = if ch.is_control()
            || ch.width() == Some(0)
            || matches!(ch, '\u{061c}' | '\u{200e}' | '\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}')
        {
            ch.escape_default().to_string()
        } else {
            ch.to_string()
        };
        for ch in display.chars() {
            let size = ch.width().unwrap_or(0);
            columns += size;
            if columns <= offset {
                continue;
            }
            if output_columns + size > width {
                return result;
            }
            result.push(ch);
            output_columns += size;
        }
    }
    result
}
