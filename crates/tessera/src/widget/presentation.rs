//! A taskbar HWND can exist well before Explorer is ready to present its contents.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct TaskbarIdentity {
    pub window: usize,
    pub process: u32,
    pub thread: u32,
}

#[derive(Clone, Copy, Debug)]
pub(super) struct Presentation {
    current: Option<TaskbarIdentity>,
    bootstrap: bool,
    notified: bool,
}

impl Presentation {
    /// An already-running taskbar's creation broadcast predates this app.
    pub fn new(initial: Option<TaskbarIdentity>) -> Self {
        Self {
            current: initial,
            bootstrap: initial.is_some(),
            notified: false,
        }
    }

    pub fn observe(&mut self, taskbar: Option<TaskbarIdentity>) {
        if self.current != taskbar {
            self.current = taskbar;
            self.bootstrap = false;
            self.notified = false;
        }
    }

    pub fn acknowledge(&mut self, taskbar: TaskbarIdentity) {
        if self.current == Some(taskbar) {
            self.notified = true;
            self.bootstrap = false;
        }
    }

    pub fn retire(&mut self, window: usize, thread: u32) -> bool {
        if self
            .current
            .is_some_and(|id| id.window == window && id.thread == thread)
        {
            self.bootstrap = false;
            self.notified = false;
            true
        } else {
            false
        }
    }

    pub fn matches_taskbar(self, taskbar: Option<TaskbarIdentity>) -> bool {
        self.current.is_some() && self.current == taskbar
    }

    pub fn can_present(self, surface_ready: bool) -> bool {
        self.current.is_some() && (self.bootstrap || self.notified) && surface_ready
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn taskbar(window: usize, process: u32) -> TaskbarIdentity {
        TaskbarIdentity {
            window,
            process,
            thread: process + 1,
        }
    }

    #[test]
    fn existing_taskbar_can_show_on_app_start_without_another_broadcast() {
        let initial = taskbar(100, 10);
        let presentation = Presentation::new(Some(initial));
        assert!(!presentation.can_present(false));
        assert!(presentation.can_present(true));
    }

    #[test]
    fn replacement_waits_for_broadcast_even_if_its_window_and_children_are_visible() {
        let mut presentation = Presentation::new(Some(taskbar(100, 10)));
        presentation.observe(None);
        assert!(!presentation.can_present(true));
        let replacement = taskbar(200, 20);
        presentation.observe(Some(replacement));
        assert!(!presentation.can_present(true));
        presentation.acknowledge(replacement);
        // A creation notification does not override missing layout or DWM cloaking.
        assert!(!presentation.can_present(false));
        assert!(presentation.can_present(true));
    }

    #[test]
    fn starting_without_explorer_does_not_bootstrap_a_half_built_taskbar() {
        let mut presentation = Presentation::new(None);
        let replacement = taskbar(200, 20);
        presentation.observe(Some(replacement));
        assert!(!presentation.can_present(true));
        presentation.acknowledge(replacement);
        assert!(presentation.can_present(true));
    }

    #[test]
    fn recycled_hwnd_and_stale_notifications_do_not_unlock_replacement() {
        let previous = taskbar(100, 10);
        let replacement = taskbar(100, 20);
        let mut presentation = Presentation::new(Some(previous));
        presentation.observe(Some(replacement));
        presentation.acknowledge(previous);
        assert!(!presentation.can_present(true));
        presentation.acknowledge(replacement);
        assert!(presentation.can_present(true));
        // A delayed destroy event from the old Explorer thread is harmless.
        assert!(!presentation.retire(previous.window, previous.thread));
        assert!(presentation.can_present(true));
        assert!(!presentation.matches_taskbar(Some(previous)));
        assert!(presentation.matches_taskbar(Some(replacement)));
    }

    #[test]
    fn retirement_blocks_early_rediscovery_and_repeated_restarts() {
        let mut current = taskbar(100, 10);
        let mut presentation = Presentation::new(Some(current));
        for index in 1..=3 {
            assert!(presentation.can_present(true));
            presentation.retire(current.window, current.thread);
            presentation.observe(Some(current));
            assert!(!presentation.can_present(true));
            current = taskbar(100 + index as usize, 10 + index);
            presentation.observe(Some(current));
            assert!(!presentation.can_present(true));
            presentation.acknowledge(current);
        }
        assert!(presentation.can_present(true));
    }
}
