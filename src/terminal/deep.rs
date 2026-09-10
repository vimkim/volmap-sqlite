use super::{Focus, TerminalFlow};
use crate::inspection::{DeepJob, DeepSelector, DeepState, ScanControl, SessionState};
use std::sync::Arc;

pub(super) struct PendingDeep {
    job: Arc<DeepJob>,
    target: DeepSelector,
    handled: bool,
}

impl TerminalFlow {
    pub(super) fn withhold_values(&mut self) {
        self.values = None;
        if let Some(deep) = &mut self.deep {
            deep.handled = true;
        }
    }

    pub(super) fn start_deep(&mut self) {
        if self
            .deep
            .as_ref()
            .is_some_and(|deep| deep.job.status().state == DeepState::Pending)
        {
            self.message = "Deep inspection pending; c cancels it".into();
            return;
        }
        let Focus::Cell(identity) = self.focus() else {
            self.message = "Select a cell before requesting deep inspection".into();
            return;
        };
        let status = self.session.status();
        let target = DeepSelector {
            session_id: status.session_id,
            snapshot_id: status.snapshot_id,
            revision: self.revision,
            page_number: identity.page_number,
            cell_index: identity.index,
        };
        self.values = None;
        self.deep = Some(PendingDeep {
            job: self.session.request_deep(target.clone(), self.deep_budget),
            target,
            handled: false,
        });
        self.message = "Deep inspection requested for the selected cell".into();
    }

    pub(super) fn cancel_work(&mut self) {
        self.values = None;
        if let Some(deep) = &mut self.deep {
            deep.job.cancel();
            deep.handled = true;
        }
        if self.session.status().state == SessionState::Scanning {
            let _ = self.session.stop(ScanControl::Cancel);
        }
        self.message = "Cancellation requested; values withheld".into();
    }

    pub(super) fn refresh_deep(&mut self) {
        let Some(deep) = &self.deep else {
            return;
        };
        let status = deep.job.status();
        if deep.handled || status.state == DeepState::Pending {
            return;
        }
        let selected = matches!(self.focus(), Focus::Cell(identity) if identity.page_number == deep.target.page_number && identity.index == deep.target.cell_index)
            && self.revision == deep.target.revision;
        if status.state == DeepState::Completed
            && selected
            && let Ok(result) = self.session.deep_result(&deep.job.id, &deep.target)
            && let Ok(graph) = self.session.revision(result.revision)
        {
            self.revision = result.revision;
            self.graph = Some(graph);
            self.values = Some(result);
            self.message = "Selected-cell values; leaving this cell or revision hides them".into();
        }
        self.deep.as_mut().unwrap().handled = true;
    }

    pub(super) fn compact_deep_summary(&self) -> String {
        self.deep.as_ref().map_or_else(
            || "Deep: idle".into(),
            |deep| {
                let status = deep.job.status();
                format!(
                    "Deep: {:?} {}B {}V",
                    status.state,
                    status.coverage.reconstructed_bytes,
                    status.coverage.decoded_values
                )
            },
        )
    }

    pub(super) fn deep_summary(&self) -> String {
        self.deep.as_ref().map_or_else(
            || "Deep: idle | d requests selected cell; c cancels work".into(),
            |deep| {
                let status = deep.job.status();
                format!(
                    "Deep: {:?} | {} | {} bytes | {} values | {} decoded bytes | cell {}:{}",
                    status.state,
                    status.coverage.reason,
                    status.coverage.reconstructed_bytes,
                    status.coverage.decoded_values,
                    status.coverage.decoded_bytes,
                    deep.target.page_number,
                    deep.target.cell_index
                )
            },
        )
    }

    pub(super) fn value_lines(&self) -> Vec<String> {
        let Some(result) = &self.values else {
            return Vec::new();
        };
        let mut lines = vec![format!(
            "Explicit selected values: cell {}:{} revision {}",
            result.target.page_number, result.target.cell_index, result.revision
        )];
        for value in &result.values {
            lines.push(format!("Field {}: {:?}", value.field.ordinal, value.value));
        }
        lines
    }
}
