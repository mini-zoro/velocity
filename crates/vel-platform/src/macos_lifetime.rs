//! Ownership bookkeeping, independent of AppKit and Metal.

pub(super) struct Windows<W, T> {
    live: Vec<(u64, W)>,
    template: Option<T>,
}

impl<W, T> Windows<W, T> {
    pub const fn new() -> Self {
        Self {
            live: Vec::new(),
            template: None,
        }
    }

    pub fn insert(&mut self, id: u64, owner: W) {
        self.live.push((id, owner));
    }

    pub fn owners(&self) -> impl Iterator<Item = &W> {
        self.live.iter().map(|(_, owner)| owner)
    }

    pub fn owner(&self, id: u64) -> Option<&W> {
        self.live
            .iter()
            .find(|(key, _)| *key == id)
            .map(|(_, owner)| owner)
    }

    /// Return the owner for teardown outside the registry borrow.
    pub fn remove(&mut self, id: u64) -> Option<(W, bool)> {
        let index = self.live.iter().position(|(key, _)| *key == id)?;
        let (_, owner) = self.live.remove(index);
        Some((owner, self.live.is_empty()))
    }

    pub fn keep_template(&mut self, template: T) -> Option<T> {
        self.template.replace(template)
    }

    pub fn template(&self) -> Option<&T> {
        self.template.as_ref()
    }

    /// Do not consume the template until the new window has started successfully.
    pub fn launched(&mut self) -> Option<T> {
        self.template.take()
    }

    pub fn into_owners(self) -> impl Iterator<Item = W> {
        self.live.into_iter().map(|(_, owner)| owner)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::rc::Rc;

    #[test]
    fn window_close_lifecycle_retires_one_owner_while_another_stays_live() {
        let mut windows = Windows::<_, ()>::new();
        let owner = Rc::new(());
        let weak = Rc::downgrade(&owner);
        windows.insert(1, owner);
        windows.insert(2, Rc::new(()));

        let (retired, last) = windows.remove(1).unwrap();
        assert!(!last);
        assert!(windows.owner(1).is_none());
        assert!(windows.owner(2).is_some());
        assert!(windows.template().is_none());
        drop(retired);
        assert!(weak.upgrade().is_none());
    }

    #[test]
    fn window_close_lifecycle_keeps_only_the_template_until_a_successful_launch() {
        let mut windows = Windows::new();
        let owner = Rc::new((7, String::from("window resources")));
        let weak = Rc::downgrade(&owner);
        windows.insert(7, owner);
        let (retired, last) = windows.remove(7).unwrap();
        assert!(last);
        assert!(windows.keep_template(retired.0).is_none());
        drop(retired);
        assert!(weak.upgrade().is_none());
        assert_eq!(windows.owners().count(), 0);
        assert_eq!(windows.template(), Some(&7));

        windows.insert(8, Rc::new((8, String::new())));
        windows.remove(8); // Failed construction leaves the existing template.
        assert_eq!(windows.template(), Some(&7));

        windows.insert(9, Rc::new((9, String::new())));
        assert_eq!(windows.launched(), Some(7));
        assert!(windows.template().is_none());
        assert!(windows.owner(9).is_some());
    }
}
