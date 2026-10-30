//! Query results kept current as the index changes. Each request carries a sequence number,
//! and a reply applies only when it answers the latest request and observed an index
//! generation at least as new as the one shown, so a slow early reply never replaces a newer
//! one.

use crate::query_index::ask;
use gpui::{Context, SharedString, Task};
use quire_store::worker::{QueueError, Reply};

pub struct Live<T> {
    pub value: T,
    pub error: Option<SharedString>,
    requested: usize,
    applied: usize,
    generation: u64,
    task: Option<Task<()>>,
}

impl<T: Default> Default for Live<T> {
    fn default() -> Self {
        Self {
            value: T::default(),
            error: None,
            requested: 0,
            applied: 0,
            generation: 0,
            task: None,
        }
    }
}

pub type Answer<T> = Result<Reply<(u64, T)>, QueueError>;

impl<T: Send + 'static> Live<T> {
    /// Whether the shown value answers the latest request.
    #[cfg(test)]
    pub fn settled(&self) -> bool {
        self.applied == self.requested
    }

    /// Applies a reply unless a newer request was made or it saw an older index state than
    /// the value shown. Returns whether the value or error changed.
    pub fn accept(
        &mut self,
        request: usize,
        reply: Result<(u64, T), String>,
        describe: impl FnOnce(String) -> String,
    ) -> bool {
        if request != self.requested {
            return false;
        }
        self.applied = request;
        self.task = None;
        match reply {
            Ok((generation, _)) if generation < self.generation => false,
            Ok((generation, value)) => {
                self.generation = generation;
                self.value = value;
                self.error = None;
                true
            }
            Err(error) => {
                self.error = Some(describe(error).into());
                true
            }
        }
    }

    /// Sends a request whose reply `live` accepts on the view, superseding earlier ones.
    pub fn request<V: 'static>(
        &mut self,
        answer: Answer<T>,
        cx: &mut Context<V>,
        live: fn(&mut V) -> &mut Self,
        describe: fn(String) -> String,
    ) {
        self.requested += 1;
        let request = self.requested;
        self.task = Some(cx.spawn(async move |view, cx| {
            let reply = ask(answer).await;
            view.update(cx, |view, cx| {
                if live(view).accept(request, reply, describe) {
                    cx.notify();
                }
            })
            .ok();
        }));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replies_to_superseded_requests_or_older_generations_never_apply() {
        let mut live = Live::<Vec<&str>> {
            requested: 2,
            ..Default::default()
        };
        assert!(!live.accept(1, Ok((5, vec!["old request"])), |error| error));
        assert!(live.accept(2, Ok((5, vec!["latest"])), |error| error));
        live.requested = 3;
        assert!(!live.accept(3, Ok((4, vec!["older index"])), |error| error));
        assert_eq!(live.value, ["latest"]);
        assert!(
            live.settled(),
            "an older answer to the latest request still settles it"
        );
        live.requested = 4;
        assert!(
            live.accept(4, Err("worker stopped".into()), |error| format!(
                "Cannot list tags: {error}."
            ))
        );
        assert_eq!(
            live.error.as_deref(),
            Some("Cannot list tags: worker stopped.")
        );
        assert_eq!(live.value, ["latest"], "an error keeps the rows shown");
    }
}
