use crate::state::sessions::{VoiceReceipt, VoiceRunEnd};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct TurnToken {
    pub epoch: u64,
    generation: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum RecognitionPath {
    Streaming,
    Batch,
}

#[derive(Default)]
pub(super) struct VoiceTurn {
    generation: u64,
    current: Option<TurnToken>,
    pub stream_id: Option<String>,
    pub frame_requests: usize,
    pub frame_loss: bool,
    pub capture_sealed: bool,
    pub frame_drain: Option<futures::channel::oneshot::Sender<()>>,
    pub receipt: Option<VoiceReceipt>,
    pub run: Option<String>,
    pub terminal: Option<VoiceRunEnd>,
    recognition: Option<RecognitionPath>,
    submitted: bool,
}

impl VoiceTurn {
    pub fn begin(&mut self, epoch: u64) -> TurnToken {
        debug_assert!(self.current.is_none());
        self.generation += 1;
        let token = TurnToken {
            epoch,
            generation: self.generation,
        };
        self.current = Some(token);
        token
    }

    pub fn token(&self) -> Option<TurnToken> {
        self.current
    }

    pub fn owns(&self, token: TurnToken) -> bool {
        self.current == Some(token)
    }

    pub fn recognizing(&self) -> bool {
        self.recognition.is_some()
    }

    pub fn capture_live(&self) -> bool {
        !self.capture_sealed
    }

    /// Claim the single transition that detaches capture and emits its tail.
    pub fn seal_capture(&mut self) -> bool {
        if self.capture_sealed {
            return false;
        }
        self.capture_sealed = true;
        true
    }

    pub fn finish_recognition(
        &mut self,
        token: TurnToken,
        usable_preview: bool,
        stream_intact: bool,
    ) -> Option<RecognitionPath> {
        if !self.owns(token) || self.recognizing() {
            return None;
        }
        let path = if usable_preview && stream_intact {
            RecognitionPath::Streaming
        } else {
            RecognitionPath::Batch
        };
        self.recognition = Some(path);
        Some(path)
    }

    pub fn begin_send(&mut self, token: TurnToken) -> bool {
        if !self.owns(token) || !self.recognizing() || self.submitted {
            return false;
        }
        self.submitted = true;
        true
    }

    pub fn accept_run(
        &mut self,
        token: TurnToken,
        run: String,
        terminal: Option<VoiceRunEnd>,
    ) -> bool {
        if !self.owns(token) || !self.submitted || self.run.is_some() {
            return false;
        }
        self.run = Some(run);
        self.terminal = terminal;
        true
    }

    pub fn observe_terminal(&mut self, run: &str, terminal: VoiceRunEnd) {
        if self.run.as_deref() == Some(run) && self.terminal.is_none() {
            self.terminal = Some(terminal);
        }
    }

    pub fn invalidate(&mut self) -> Self {
        let generation = self.generation;
        std::mem::replace(
            self,
            Self {
                generation,
                ..Self::default()
            },
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_submission_per_capture() {
        let mut turn = VoiceTurn::default();
        let token = turn.begin(1);
        assert!(!turn.begin_send(token));
        assert_eq!(
            turn.finish_recognition(token, true, true),
            Some(RecognitionPath::Streaming)
        );
        assert_eq!(turn.finish_recognition(token, false, false), None);
        assert!(turn.begin_send(token));
        assert!(!turn.begin_send(token));
    }

    #[test]
    fn revocation_releases_once_and_late_receipt_cannot_replace_successor() {
        let mut turn = VoiceTurn::default();
        let token = turn.begin(1);
        turn.finish_recognition(token, false, false).unwrap();
        assert!(turn.begin_send(token));
        assert!(turn.accept_run(token, "first".into(), None));
        let revoked = turn.invalidate();
        assert!(!turn.owns(token));
        assert_eq!(revoked.run.as_deref(), Some("first"));
        assert!(turn.invalidate().run.is_none());
        let next = turn.begin(1);
        assert!(!turn.accept_run(token, "late".into(), None));
        assert_eq!(turn.token(), Some(next));
    }

    #[test]
    fn capture_seal_emits_only_one_tail() {
        let mut turn = VoiceTurn::default();
        turn.begin(1);
        assert!(turn.seal_capture());
        assert!(!turn.seal_capture());
        assert!(!turn.capture_live());
    }

    #[test]
    fn unhealthy_stream_falls_back_once() {
        for (preview, intact) in [(false, true), (true, false), (false, false)] {
            let mut turn = VoiceTurn::default();
            let token = turn.begin(9);
            assert_eq!(
                turn.finish_recognition(token, preview, intact),
                Some(RecognitionPath::Batch)
            );
            assert_eq!(turn.finish_recognition(token, true, true), None);
        }
    }

    #[test]
    fn terminal_must_match_the_owned_run() {
        let mut turn = VoiceTurn::default();
        let token = turn.begin(1);
        turn.finish_recognition(token, true, true).unwrap();
        assert!(turn.begin_send(token));
        assert!(turn.accept_run(token, "mine".into(), None));
        turn.observe_terminal("other", VoiceRunEnd::Failed);
        assert_eq!(turn.terminal, None);
        turn.observe_terminal("mine", VoiceRunEnd::Failed);
        turn.observe_terminal("mine", VoiceRunEnd::Complete);
        assert_eq!(turn.terminal, Some(VoiceRunEnd::Failed));
    }
}
