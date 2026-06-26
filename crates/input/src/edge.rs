//! Edge-transition state machine with hysteresis.
//!
//! Pure logic, no OS calls — this is the part that decides *when* control crosses
//! between the local and remote screen, and it's fully unit-tested. The OS layer
//! (capture/injection) feeds it cursor positions and deltas and acts on the
//! [`Transition`]s it returns.

use screenlink_core::protocol::ScreenEdge;

/// A pixel rectangle (e.g. the virtual desktop or one monitor).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

impl Rect {
    pub fn new(x: i32, y: i32, w: i32, h: i32) -> Self {
        Self { x, y, w, h }
    }
}

/// Where control currently lives.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ControlSite {
    Local,
    Remote,
}

/// A control-location change the OS layer must act on.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Transition {
    /// Control moved onto the remote screen. `entry_norm` is the normalized
    /// position (0..1) along the shared edge where the cursor crossed, so the
    /// remote cursor can be placed to match.
    ToRemote { entry_norm: f32 },
    /// Control returned to the local screen.
    ToLocal,
}

/// Detects edge crossings with hysteresis to avoid jitter at the boundary.
#[derive(Clone, Debug)]
pub struct EdgeDetector {
    edge: ScreenEdge,
    /// How far (px) the user must push *into the return edge of the remote*
    /// before control returns. Prevents flapping right at the seam and stops
    /// arbitrary mid-remote motion from accidentally sending control home.
    hysteresis: i32,
    site: ControlSite,
    /// Accumulated leftover push toward the return edge while the cursor is
    /// already pinned against it. Reset by any forward motion or whenever the
    /// cursor moves away from the return edge.
    back_push: i32,
}

impl EdgeDetector {
    pub fn new(edge: ScreenEdge, hysteresis: i32) -> Self {
        Self {
            edge,
            hysteresis: hysteresis.max(1),
            site: ControlSite::Local,
            back_push: 0,
        }
    }

    pub fn site(&self) -> ControlSite {
        self.site
    }

    pub fn set_edge(&mut self, edge: ScreenEdge) {
        self.edge = edge;
    }

    /// Force control back to the local machine (the fail-safe / snap-home path).
    pub fn force_home(&mut self) -> Option<Transition> {
        if self.site == ControlSite::Remote {
            self.site = ControlSite::Local;
            self.back_push = 0;
            Some(Transition::ToLocal)
        } else {
            None
        }
    }

    /// Call with the absolute cursor position while control is **local**. Returns
    /// `ToRemote` when the cursor reaches the active edge.
    pub fn update_local(&mut self, cx: i32, cy: i32, screen: Rect) -> Option<Transition> {
        if self.site != ControlSite::Local {
            return None;
        }
        let at_edge = match self.edge {
            ScreenEdge::Left => cx <= screen.x,
            ScreenEdge::Right => cx >= screen.x + screen.w - 1,
            ScreenEdge::Top => cy <= screen.y,
            ScreenEdge::Bottom => cy >= screen.y + screen.h - 1,
        };
        if !at_edge {
            return None;
        }
        let entry_norm = match self.edge {
            ScreenEdge::Left | ScreenEdge::Right => norm(cy - screen.y, screen.h),
            ScreenEdge::Top | ScreenEdge::Bottom => norm(cx - screen.x, screen.w),
        };
        self.site = ControlSite::Remote;
        self.back_push = 0;
        Some(Transition::ToRemote { entry_norm })
    }

    /// Call with each relative mouse delta while control is **remote**.
    /// `at_return_edge` tells us whether the virtual remote cursor is currently
    /// pinned against the seam edge of the remote (i.e. the edge the user
    /// entered through). Only motion pushing further into that pinned edge
    /// counts toward returning home — that way ordinary mid-remote navigation
    /// can't accidentally send control back, and "press into the edge"
    /// mirrors how the user crossed in.
    pub fn update_remote(
        &mut self,
        dx: i32,
        dy: i32,
        at_return_edge: bool,
    ) -> Option<Transition> {
        if self.site != ControlSite::Remote {
            return None;
        }
        let into_delta = match self.edge {
            ScreenEdge::Right => dx,
            ScreenEdge::Left => -dx,
            ScreenEdge::Bottom => dy,
            ScreenEdge::Top => -dy,
        };
        if at_return_edge && into_delta < 0 {
            self.back_push += -into_delta;
            if self.back_push >= self.hysteresis {
                self.site = ControlSite::Local;
                self.back_push = 0;
                return Some(Transition::ToLocal);
            }
        } else {
            // Any forward motion, or moving away from the return edge, resets
            // the back-push streak — only sustained pressure at the edge
            // returns home.
            self.back_push = 0;
        }
        None
    }
}

fn norm(v: i32, span: i32) -> f32 {
    if span <= 0 {
        0.5
    } else {
        (v as f32 / span as f32).clamp(0.0, 1.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn screen() -> Rect {
        Rect::new(0, 0, 1920, 1080)
    }

    #[test]
    fn crosses_to_remote_at_right_edge() {
        let mut d = EdgeDetector::new(ScreenEdge::Right, 20);
        assert_eq!(d.update_local(960, 540, screen()), None);
        let t = d.update_local(1919, 270, screen());
        match t {
            Some(Transition::ToRemote { entry_norm }) => {
                assert!((entry_norm - 0.25).abs() < 0.01);
            }
            other => panic!("expected ToRemote, got {other:?}"),
        }
        assert_eq!(d.site(), ControlSite::Remote);
    }

    #[test]
    fn local_updates_ignored_once_remote() {
        let mut d = EdgeDetector::new(ScreenEdge::Right, 20);
        d.update_local(1919, 540, screen());
        assert_eq!(d.site(), ControlSite::Remote);
        // Further local-position reports do nothing while remote.
        assert_eq!(d.update_local(5, 5, screen()), None);
    }

    #[test]
    fn returns_home_only_when_pushing_into_return_edge() {
        let mut d = EdgeDetector::new(ScreenEdge::Right, 20);
        d.update_local(1919, 540, screen()); // now remote
                                             // Pushing further right is forward motion: stays remote.
        assert_eq!(d.update_remote(50, 0, false), None);
        // Leftward motion mid-remote (not yet at the return edge) does NOT
        // count — accidental wobble during navigation must not send us home.
        assert_eq!(d.update_remote(-100, 0, false), None);
        // Once pinned at the return edge, leftward motion accumulates...
        assert_eq!(d.update_remote(-10, 0, true), None);
        // ...and past hysteresis it returns home.
        let t = d.update_remote(-20, 0, true);
        assert_eq!(t, Some(Transition::ToLocal));
        assert_eq!(d.site(), ControlSite::Local);
    }

    #[test]
    fn mid_remote_motion_never_strands_user() {
        // Regression: ordinary navigation on the remote (cursor not pinned at
        // the seam edge) must never return control home, no matter how big
        // the leftward deltas are.
        let mut d = EdgeDetector::new(ScreenEdge::Right, 25);
        d.update_local(1919, 540, screen());
        for _ in 0..1000 {
            assert_eq!(d.update_remote(-50, 0, false), None);
            assert_eq!(d.update_remote(50, 0, false), None);
        }
        assert_eq!(d.site(), ControlSite::Remote);
    }

    #[test]
    fn forward_motion_resets_back_push() {
        let mut d = EdgeDetector::new(ScreenEdge::Right, 20);
        d.update_local(1919, 540, screen());
        // Accumulate some back-push at the return edge.
        assert_eq!(d.update_remote(-15, 0, true), None);
        // A flick forward resets the streak: still remote.
        assert_eq!(d.update_remote(10, 0, true), None);
        // Now back-push has to restart from zero.
        assert_eq!(d.update_remote(-15, 0, true), None);
        let t = d.update_remote(-10, 0, true);
        assert_eq!(t, Some(Transition::ToLocal));
    }

    #[test]
    fn left_edge_uses_opposite_axis_sign() {
        let mut d = EdgeDetector::new(ScreenEdge::Left, 10);
        d.update_local(0, 540, screen());
        // Going further left (negative dx) is deeper into remote for a
        // Left-edge config — no return.
        assert_eq!(d.update_remote(-100, 0, false), None);
        // Coming back right while pinned at the return edge returns home.
        assert_eq!(d.update_remote(20, 0, true), Some(Transition::ToLocal));
    }

    #[test]
    fn force_home_only_acts_when_remote() {
        let mut d = EdgeDetector::new(ScreenEdge::Bottom, 10);
        assert_eq!(d.force_home(), None);
        d.update_local(960, 1079, screen());
        assert_eq!(d.force_home(), Some(Transition::ToLocal));
        assert_eq!(d.site(), ControlSite::Local);
    }
}
