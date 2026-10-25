//! Pending approvals waiting for a human answer. Whoever removes an entry first owns the
//! outcome, so an answer racing a timeout resolves the approval exactly once.

use std::collections::HashMap;
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;

use crate::ids::ApprovalId;
use crate::runs::Answer;

/// How a wait ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Waited {
    Answered(Answer),
    Expired,
    Cancelled,
}

#[derive(Default)]
pub struct ApprovalWaiter {
    /// A std mutex: it is never held across an await.
    pending: Mutex<HashMap<ApprovalId, oneshot::Sender<Answer>>>,
}

impl ApprovalWaiter {
    /// Registered before `approval_requested` is emitted, so an answer can never arrive first.
    pub fn register(&self, id: ApprovalId) -> oneshot::Receiver<Answer> {
        let (tx, rx) = oneshot::channel();
        self.pending().insert(id, tx);
        rx
    }

    pub fn forget(&self, id: ApprovalId) {
        self.pending().remove(&id);
    }

    pub async fn wait(
        &self,
        id: ApprovalId,
        mut rx: oneshot::Receiver<Answer>,
        timeout: Duration,
        cancel: &CancellationToken,
    ) -> Waited {
        let ended = tokio::select! {
            answer = &mut rx => return answered(answer),
            () = tokio::time::sleep(timeout) => Waited::Expired,
            () = cancel.cancelled() => Waited::Cancelled,
        };
        if self.pending().remove(&id).is_some() {
            return ended;
        }
        // `answer` removed the entry first, so its value is in the channel.
        answered(rx.await)
    }

    /// False when the approval is no longer pending: already answered, expired or cancelled.
    pub fn answer(&self, id: ApprovalId, answer: Answer) -> bool {
        match self.pending().remove(&id) {
            Some(tx) => tx.send(answer).is_ok(),
            None => false,
        }
    }

    fn pending(&self) -> MutexGuard<'_, HashMap<ApprovalId, oneshot::Sender<Answer>>> {
        self.pending.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// A dropped sender only happens if the waiter itself is dropped; treat it as cancelled.
fn answered(result: Result<Answer, oneshot::error::RecvError>) -> Waited {
    match result {
        Ok(answer) => Waited::Answered(answer),
        Err(_) => Waited::Cancelled,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn approve() -> Answer {
        Answer::Approve { note: None }
    }

    #[tokio::test]
    async fn an_answer_already_sent_wins_over_an_elapsed_timeout() {
        let waiter = ApprovalWaiter::default();
        let id = ApprovalId::new();
        let rx = waiter.register(id);
        assert!(waiter.answer(id, approve()));

        let waited = waiter
            .wait(id, rx, Duration::ZERO, &CancellationToken::new())
            .await;
        assert_eq!(waited, Waited::Answered(approve()));
    }

    #[tokio::test]
    async fn an_answer_after_expiry_is_refused() {
        let waiter = ApprovalWaiter::default();
        let id = ApprovalId::new();
        let rx = waiter.register(id);

        let waited = waiter
            .wait(id, rx, Duration::ZERO, &CancellationToken::new())
            .await;
        assert_eq!(waited, Waited::Expired);
        assert!(!waiter.answer(id, approve()));
    }

    #[tokio::test]
    async fn cancel_ends_the_wait_and_refuses_a_later_answer() {
        let waiter = ApprovalWaiter::default();
        let id = ApprovalId::new();
        let rx = waiter.register(id);
        let cancel = CancellationToken::new();
        cancel.cancel();

        let waited = waiter.wait(id, rx, Duration::from_secs(60), &cancel).await;
        assert_eq!(waited, Waited::Cancelled);
        assert!(!waiter.answer(id, approve()));
    }
}
