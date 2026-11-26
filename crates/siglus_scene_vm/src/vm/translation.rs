use super::SceneVm;
use crate::platform_time::Instant;
use crate::runtime::Value;
use anyhow::Result;
use std::collections::HashMap;
use std::time::Duration;

const RESPONSE_WINDOW: Duration = Duration::from_secs(3);
const MAX_TEXT_BYTES: usize = 16 * 1024;

#[derive(Default)]
pub(super) struct Translation {
    replacements: HashMap<String, String>,
    online: bool,
    next_id: u64,
    pub(super) pending: Option<Pending>,
}

pub(super) enum Instruction {
    Text {
        source: String,
        read_flag: i32,
    },
    Print {
        element: Vec<i32>,
        arg_list: i32,
        return_form: i32,
        args: Vec<Value>,
        text_arg: usize,
    },
}

pub(super) struct Pending {
    site: (Option<usize>, usize, usize),
    instruction: Instruction,
    id: u64,
    began: Instant,
    polled: bool,
    ready: bool,
    result: Option<String>,
}

fn translatable(text: &str) -> bool {
    !text.trim().is_empty()
        && text.len() <= MAX_TEXT_BYTES
        && !text.contains('＄')
        && !text
            .chars()
            .any(|ch| ch.is_control() && !matches!(ch, '\n' | '\r' | '\t'))
}

impl Translation {
    pub(super) fn is_configured(&self) -> bool {
        self.online || !self.replacements.is_empty()
    }
}

impl Instruction {
    fn source(&self) -> &str {
        match self {
            Self::Text { source, .. } => source,
            Self::Print { args, text_arg, .. } => args[*text_arg].unwrap_named().as_str().unwrap(),
        }
    }
}

impl Pending {
    fn expired(&self) -> bool {
        self.began.elapsed() >= RESPONSE_WINDOW
    }
}

impl SceneVm<'_> {
    /// Replaces the session's translation settings and releases any paused text.
    pub fn configure_text_translation(
        &mut self,
        replacements: HashMap<String, String>,
        online: bool,
    ) {
        self.translation.replacements = replacements;
        self.translation.online = online;
        if let Some(pending) = &mut self.translation.pending {
            pending.ready = true;
            pending.result = None;
        }
    }

    /// Inspect the next UTF-8 request. Call `consume_text_translation` once it has been copied.
    pub fn text_translation_request(&self) -> Option<(u64, &str)> {
        let pending = self.translation.pending.as_ref()?;
        (!pending.ready && !pending.polled && !pending.expired())
            .then(|| (pending.id, pending.instruction.source()))
    }

    pub fn consume_text_translation(&mut self, id: u64) {
        if let Some(pending) = &mut self.translation.pending
            && pending.id == id
        {
            pending.polled = true;
        }
    }

    /// Submit a reply to a request from this session. None releases the original text.
    pub fn complete_text_translation(&mut self, id: u64, text: Option<String>) -> bool {
        let Some(pending) = &mut self.translation.pending else {
            return false;
        };
        if pending.id != id || !pending.polled || pending.ready || pending.expired() {
            return false;
        }
        pending.ready = true;
        pending.result = text.filter(|text| translatable(text));
        true
    }

    pub(crate) fn text_translation_can_resume(&self) -> bool {
        self.translation.pending.as_ref().is_some_and(|pending| {
            pending.site == self.translation_site() && (pending.ready || pending.expired())
        })
    }

    fn translation_site(&self) -> (Option<usize>, usize, usize) {
        (
            self.current_scene_no,
            self.stream.get_prg_cntr(),
            self.call_stack.len(),
        )
    }

    pub(super) fn translation_blocked(&self) -> bool {
        self.translation.pending.as_ref().is_some_and(|pending| {
            pending.site == self.translation_site() && !pending.ready && !pending.expired()
        })
    }

    pub(super) fn resolve_translation(
        &mut self,
        instruction: Instruction,
        can_wait: bool,
    ) -> Result<bool> {
        let source = instruction.source();
        if self.translation.is_configured() && translatable(source) {
            if let Some(text) = self
                .translation
                .replacements
                .get(source)
                .filter(|t| translatable(t))
            {
                return self.execute_translated(instruction, Some(text.clone()), can_wait);
            }
            if can_wait
                && self.translation.online
                && self.translation.pending.is_none()
                && let Some(id) = self.translation.next_id.checked_add(1)
            {
                self.translation.next_id = id;
                self.translation.pending = Some(Pending {
                    site: self.translation_site(),
                    instruction,
                    id,
                    began: Instant::now(),
                    polled: false,
                    ready: false,
                    result: None,
                });
                return Ok(true);
            }
        }
        self.execute_translated(instruction, None, can_wait)
    }

    pub(super) fn resume_translation(&mut self) -> Result<Option<bool>> {
        let Some(pending) = self.translation.pending.as_ref() else {
            return Ok(None);
        };
        if pending.site != self.translation_site() {
            return Ok(None);
        }
        if self.translation_blocked() {
            return Ok(Some(true));
        }
        let pending = self.translation.pending.take().unwrap();
        if let Some(value) = &pending.result {
            self.translation
                .replacements
                .insert(pending.instruction.source().to_owned(), value.clone());
        }
        self.execute_translated(pending.instruction, pending.result, true)
            .map(Some)
    }
}
