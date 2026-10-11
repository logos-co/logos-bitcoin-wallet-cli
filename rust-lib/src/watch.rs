//! What the poll thread turns the backend's send list into: a prompt per send that awaits
//! approval, a settlement per send seen open, and the size of the queue. Logos-free.

use std::collections::BTreeSet;

pub fn is_final(state: &str) -> bool {
    matches!(state, "sent" | "failed" | "cancelled" | "unknown")
}

/// One row of `list_sends`. A restarted backend counts from s1 again, so a send is known by
/// its id and the second it was created.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Row {
    pub id: String,
    pub state: String,
    pub created: u64,
}

impl Row {
    fn key(&self) -> String {
        format!("{}@{}", self.id, self.created)
    }
}

#[derive(Default)]
pub struct Watch {
    prompted: BTreeSet<String>,
    open: BTreeSet<(String, String)>,
    awaiting: usize,
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct Turn {
    /// Sends newly awaiting approval: prompt for each.
    pub prompt: Vec<String>,
    /// Sends seen open that reached a final state, or left the list.
    pub settled: Vec<(String, String)>,
    /// The number awaiting approval, when it changed.
    pub awaiting: Option<usize>,
}

impl Watch {
    pub fn turn(&mut self, rows: &[Row]) -> Turn {
        let mut t = Turn::default();
        let mut awaiting = 0;
        for r in rows {
            let entry = (r.key(), r.id.clone());
            if r.state == "awaiting_approval" {
                awaiting += 1;
                if self.prompted.insert(r.key()) {
                    t.prompt.push(r.id.clone());
                }
            }
            if !is_final(&r.state) {
                self.open.insert(entry);
            } else if self.open.remove(&entry) {
                t.settled.push((r.id.clone(), r.state.clone()));
            }
        }
        let present: BTreeSet<String> = rows.iter().map(Row::key).collect();
        let gone: Vec<(String, String)> = self.open.iter().filter(|(k, _)| !present.contains(k)).cloned().collect();
        for e in gone {
            self.open.remove(&e);
            t.settled.push((e.1, "gone".into()));
        }
        // Only what the backend still lists is remembered.
        self.prompted.retain(|k| present.contains(k));
        if awaiting != self.awaiting {
            self.awaiting = awaiting;
            t.awaiting = Some(awaiting);
        }
        t
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn l(rows: &[(&str, &str, u64)]) -> Vec<Row> {
        rows.iter().map(|(a, b, c)| Row { id: a.to_string(), state: b.to_string(), created: *c }).collect()
    }

    #[test]
    fn a_send_is_prompted_once_and_settled_once() {
        let mut w = Watch::default();
        assert_eq!(w.turn(&l(&[("s1", "preparing", 9)])), Turn::default());
        let t = w.turn(&l(&[("s1", "awaiting_approval", 9)]));
        assert_eq!((t.prompt, t.awaiting), (vec!["s1".to_string()], Some(1)));
        assert_eq!(w.turn(&l(&[("s1", "awaiting_approval", 9)])), Turn::default(), "prompted once");
        let t = w.turn(&l(&[("s1", "committing", 9)]));
        assert_eq!((t.settled.len(), t.awaiting), (0, Some(0)), "committing is not settled");
        let t = w.turn(&l(&[("s1", "sent", 9)]));
        assert_eq!(t.settled, vec![("s1".to_string(), "sent".to_string())]);
        assert_eq!(w.turn(&l(&[("s1", "sent", 9)])), Turn::default(), "settled once");
    }

    #[test]
    fn old_final_sends_are_not_announced() {
        let mut w = Watch::default();
        assert_eq!(w.turn(&l(&[("s1", "failed", 1), ("s2", "cancelled", 2)])), Turn::default());
    }

    #[test]
    fn a_send_that_vanishes_while_open_is_settled_as_gone() {
        let mut w = Watch::default();
        w.turn(&l(&[("s1", "awaiting_approval", 5)]));
        let t = w.turn(&[]);
        assert_eq!((t.settled, t.awaiting), (vec![("s1".to_string(), "gone".to_string())], Some(0)));
    }

    #[test]
    fn a_restarted_backend_counting_from_s1_again_is_prompted_again() {
        let mut w = Watch::default();
        w.turn(&l(&[("s1", "awaiting_approval", 100)]));
        w.turn(&l(&[("s1", "sent", 100)]));
        // The backend restarted between two polls and already has a new s1 waiting.
        let t = w.turn(&l(&[("s1", "awaiting_approval", 160)]));
        assert_eq!(t.prompt, vec!["s1".to_string()]);
    }

    #[test]
    fn unknown_is_final() {
        assert!(is_final("unknown") && is_final("sent") && !is_final("committing") && !is_final("awaiting_approval"));
    }
}
