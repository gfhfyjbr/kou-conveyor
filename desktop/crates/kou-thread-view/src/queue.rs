//! What waits for the agent, as kou-conveyor's cockpits keep it: prompts
//! written while a turn runs wait in a queue and run one after another when
//! it ends; a forced prompt goes into the running turn instead, and leaves
//! the queue once the agent has it. A turn that is stopped or fails pauses
//! the queue until the user resumes it.

use crate::draft::Draft;
use kou_thread::Stop;

/// One prompt of the queue.
#[derive(Clone, Debug)]
pub(crate) struct Item {
    /// Unique within the view, kept while the prompt waits.
    pub id: u64,
    pub draft: Draft,
    /// Handed to the running agent, which has not taken it yet.
    pub forced: bool,
}

/// The prompts waiting: forced ones first, then queued ones in the order they run.
#[derive(Clone, Debug, Default)]
pub(crate) struct Queue {
    items: Vec<Item>,
    /// Keeps the queued prompts from running when a turn ends.
    pub paused: bool,
    next_id: u64,
}

impl Queue {
    /// Every prompt, forced first.
    pub(crate) fn items(&self) -> &[Item] {
        &self.items
    }

    /// Whether nothing waits.
    pub(crate) fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// How many prompts the running agent has been handed.
    pub(crate) fn forced(&self) -> usize {
        self.items.iter().take_while(|item| item.forced).count()
    }

    /// How many prompts wait for the turn to end.
    pub(crate) fn queued(&self) -> usize {
        self.items.len() - self.forced()
    }

    /// Queues a prompt at the end and returns its id.
    pub(crate) fn add(&mut self, draft: Draft) -> u64 {
        let id = self.fresh_id();
        self.items.push(Item {
            id,
            draft,
            forced: false,
        });
        id
    }

    /// Records a prompt handed to the running agent and returns its id.
    pub(crate) fn add_forced(&mut self, draft: Draft) -> u64 {
        let id = self.fresh_id();
        let at = self.forced();
        self.items.insert(
            at,
            Item {
                id,
                draft,
                forced: true,
            },
        );
        id
    }

    fn fresh_id(&mut self) -> u64 {
        self.next_id += 1;
        self.next_id
    }

    /// A queued prompt's place among the queued ones.
    pub(crate) fn position(&self, id: u64) -> Option<usize> {
        let forced = self.forced();
        self.items[forced..].iter().position(|item| item.id == id)
    }

    /// Takes a queued prompt out, with its place, to edit it or drop it. A
    /// forced prompt is the agent's already and stays.
    pub(crate) fn take(&mut self, id: u64) -> Option<(usize, Item)> {
        let position = self.position(id)?;
        let item = self.items.remove(self.forced() + position);
        Some((position, item))
    }

    /// Puts a queued prompt back at `position` among the queued ones, as an
    /// edit that took it out leaves it.
    pub(crate) fn insert(&mut self, position: usize, mut item: Item) {
        item.forced = false;
        let at = self.forced() + position.min(self.queued());
        self.items.insert(at, item);
    }

    /// Moves a queued prompt to `position` among the queued ones.
    pub(crate) fn move_to(&mut self, id: u64, position: usize) -> bool {
        let Some((from, item)) = self.take(id) else {
            return false;
        };
        let to = position.min(self.queued());
        self.insert(to, item);
        from != to
    }

    /// Forces a queued prompt into the running turn: it goes after the
    /// prompts forced before it. Returns what to hand the agent.
    pub(crate) fn force(&mut self, id: u64) -> Option<Draft> {
        let (_, mut item) = self.take(id)?;
        item.forced = true;
        let draft = item.draft.clone();
        let at = self.forced();
        self.items.insert(at, item);
        Some(draft)
    }

    /// Drops the queued prompts; forced ones stay. Returns how many went.
    pub(crate) fn clear(&mut self) -> usize {
        let forced = self.forced();
        let dropped = self.items.len() - forced;
        self.items.truncate(forced);
        dropped
    }

    /// The agent took the oldest forced prompt.
    pub(crate) fn delivered(&mut self) -> Option<Item> {
        (self.forced() > 0).then(|| self.items.remove(0))
    }

    /// The turn ended: forced prompts the agent never took go back to the
    /// head of the queue, and a turn that did not end well pauses it.
    pub(crate) fn settle(&mut self, stop: Option<&Stop>) {
        for item in &mut self.items {
            item.forced = false;
        }
        if matches!(
            stop,
            Some(Stop::Cancelled | Stop::Failed(_) | Stop::Refused)
        ) && !self.items.is_empty()
        {
            self.paused = true;
        }
    }

    /// The prompt to run now, if the queue runs and nothing is forced.
    pub(crate) fn next(&mut self) -> Option<Item> {
        if self.paused || self.items.first().is_none_or(|item| item.forced) {
            return None;
        }
        Some(self.items.remove(0))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn draft(text: &str) -> Draft {
        Draft {
            text: text.into(),
            ..Draft::default()
        }
    }

    fn texts(queue: &Queue) -> Vec<(String, bool)> {
        queue
            .items()
            .iter()
            .map(|item| (item.draft.text.clone(), item.forced))
            .collect()
    }

    #[test]
    fn prompts_wait_in_order_and_run_one_by_one() {
        let mut queue = Queue::default();
        let a = queue.add(draft("a"));
        queue.add(draft("b"));
        let c = queue.add(draft("c"));
        assert_eq!((queue.forced(), queue.queued()), (0, 3));
        assert!(queue.move_to(c, 0));
        assert_eq!(queue.position(c), Some(0));
        assert!(!queue.move_to(a, 1), "already there");
        assert_eq!(
            texts(&queue),
            [
                ("c".into(), false),
                ("a".into(), false),
                ("b".into(), false)
            ]
        );
        assert_eq!(queue.next().map(|item| item.draft.text), Some("c".into()));
        assert_eq!(queue.queued(), 2);
    }

    #[test]
    fn a_forced_prompt_goes_first_and_leaves_when_delivered() {
        let mut queue = Queue::default();
        queue.add(draft("later"));
        let urgent = queue.add(draft("urgent"));
        assert_eq!(
            queue.force(urgent).map(|draft| draft.text),
            Some("urgent".into())
        );
        let direct = queue.add_forced(draft("direct"));
        assert_eq!(
            texts(&queue),
            [
                ("urgent".into(), true),
                ("direct".into(), true),
                ("later".into(), false)
            ]
        );
        // A forced prompt cannot be taken back, moved or run as a prompt.
        assert!(queue.take(direct).is_none());
        assert!(queue.next().is_none(), "the run has the forced ones first");
        assert_eq!(
            queue.delivered().map(|item| item.draft.text),
            Some("urgent".into())
        );
        assert_eq!(queue.clear(), 1, "only queued prompts are cleared");
        assert_eq!(texts(&queue), [("direct".into(), true)]);
    }

    #[test]
    fn a_turn_that_ends_badly_pauses_and_returns_what_it_never_read() {
        let mut queue = Queue::default();
        queue.add(draft("next"));
        queue.add_forced(draft("unread"));
        queue.settle(Some(&Stop::Cancelled));
        assert!(queue.paused);
        assert_eq!(
            texts(&queue),
            [("unread".into(), false), ("next".into(), false)]
        );
        assert!(queue.next().is_none(), "paused");
        queue.paused = false;
        assert_eq!(
            queue.next().map(|item| item.draft.text),
            Some("unread".into())
        );

        // A turn that finished lets the queue go on.
        let mut queue = Queue::default();
        queue.add(draft("next"));
        queue.settle(Some(&Stop::Finished));
        assert!(!queue.paused);
        // An empty queue is not paused by a failure.
        let mut empty = Queue::default();
        empty.settle(Some(&Stop::Failed("x".into())));
        assert!(!empty.paused);
    }

    #[test]
    fn an_edit_takes_a_prompt_out_and_puts_it_back_in_its_place() {
        let mut queue = Queue::default();
        queue.add(draft("a"));
        let b = queue.add(draft("b"));
        queue.add(draft("c"));
        let (position, mut item) = queue.take(b).expect("queued");
        assert_eq!(position, 1);
        item.draft.text = "b, edited".into();
        queue.insert(position, item);
        assert_eq!(
            texts(&queue),
            [
                ("a".into(), false),
                ("b, edited".into(), false),
                ("c".into(), false)
            ]
        );
    }
}
