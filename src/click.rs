pub const DEBOUNCE_MS: u64 = 30;
pub const MULTI_CLICK_MS: u64 = 650;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ClickAction {
    TypeText,
    MountStorage,
}

#[derive(Debug)]
pub struct ClickDetector {
    candidate_pressed: bool,
    candidate_since_ms: u64,
    stable_pressed: bool,
    release_count: u8,
    last_release_ms: u64,
}

impl ClickDetector {
    pub const fn new(now_ms: u64, initially_pressed: bool) -> Self {
        Self {
            candidate_pressed: initially_pressed,
            candidate_since_ms: now_ms,
            stable_pressed: initially_pressed,
            release_count: 0,
            last_release_ms: now_ms,
        }
    }

    pub fn update(&mut self, pressed: bool, now_ms: u64) -> Option<ClickAction> {
        if pressed != self.candidate_pressed {
            self.candidate_pressed = pressed;
            self.candidate_since_ms = now_ms;
        }

        if self.candidate_pressed != self.stable_pressed
            && now_ms.saturating_sub(self.candidate_since_ms) >= DEBOUNCE_MS
        {
            self.stable_pressed = self.candidate_pressed;
            if !self.stable_pressed {
                self.release_count = self.release_count.saturating_add(1);
                self.last_release_ms = now_ms;
                if self.release_count == 3 {
                    self.release_count = 0;
                    return Some(ClickAction::MountStorage);
                }
            }
        }

        let window_expired = self.release_count != 0
            && !self.candidate_pressed
            && !self.stable_pressed
            && now_ms.saturating_sub(self.last_release_ms) >= MULTI_CLICK_MS;
        if window_expired {
            let releases = self.release_count;
            self.release_count = 0;
            if releases == 1 {
                return Some(ClickAction::TypeText);
            }
        }

        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn click(detector: &mut ClickDetector, at_ms: u64) -> Option<ClickAction> {
        assert_eq!(detector.update(true, at_ms), None);
        assert_eq!(detector.update(true, at_ms + DEBOUNCE_MS), None);
        assert_eq!(detector.update(false, at_ms + 80), None);
        detector.update(false, at_ms + 80 + DEBOUNCE_MS)
    }

    #[test]
    fn single_click_should_wait_before_typing() {
        let mut detector = ClickDetector::new(0, false);
        assert_eq!(click(&mut detector, 10), None);
        assert_eq!(detector.update(false, 769), None);
        assert_eq!(detector.update(false, 770), Some(ClickAction::TypeText));
    }

    #[test]
    fn triple_click_should_mount_without_typing() {
        let mut detector = ClickDetector::new(0, false);
        assert_eq!(click(&mut detector, 10), None);
        assert_eq!(click(&mut detector, 180), None);
        assert_eq!(click(&mut detector, 350), Some(ClickAction::MountStorage));
        assert_eq!(detector.update(false, 2_000), None);
    }

    #[test]
    fn contact_bounce_should_not_count_as_a_click() {
        let mut detector = ClickDetector::new(0, false);
        assert_eq!(detector.update(true, 10), None);
        assert_eq!(detector.update(false, 20), None);
        assert_eq!(detector.update(true, 25), None);
        assert_eq!(detector.update(false, 35), None);
        assert_eq!(detector.update(false, 1_000), None);
    }

    #[test]
    fn double_click_should_be_a_safe_noop() {
        let mut detector = ClickDetector::new(0, false);
        assert_eq!(click(&mut detector, 10), None);
        assert_eq!(click(&mut detector, 180), None);
        assert_eq!(detector.update(false, 1_000), None);
    }
}
