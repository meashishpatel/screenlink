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
    /// How far (px) the user must travel *back* into the local screen before
    /// control returns. Prevents flapping right at the seam.
    hysteresis: i32,
    site: ControlSite,
    /// Accumulated penetration into the remote screen along the perpendicular
    /// axis while [`ControlSite::Remote`].
    perp_into: i32,
}

impl EdgeDetector {
    pub fn new(edge: ScreenEdge, hysteresis: i32) -> Self {
        Self {
            edge,
            hysteresis: hysteresis.max(1),
            site: ControlSite::Local,
            perp_into: 0,
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
            self.perp_into = 0;
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
        self.perp_into = self.hysteresis; // start a bit inside so a tiny wobble doesn't bounce back
        Some(Transition::ToRemote { entry_norm })
    }

    /// Call with each relative mouse delta while control is **remote**. Returns
    /// `ToLocal` once the user has pulled back across the seam by `hysteresis`.
    pub fn update_remote(&mut self, dx: i32, dy: i32) -> Option<Transition> {
        if self.site != ControlSite::Remote {
            return None;
        }
        // Positive = deeper into remote; negative = back toward the seam.
        let into_delta = match self.edge {
            ScreenEdge::Right => dx,
            ScreenEdge::Left => -dx,
            ScreenEdge::Bottom => dy,
            ScreenEdge::Top => -dy,
        };
        // Cap the positive side at `hysteresis` so no matter how far the user
        // has navigated into the remote, pushing back by `hysteresis` pixels is
        // always enough to return. Without this cap, `perp_into` accumulates
        // every rightward delta and the cursor effectively gets stuck on the
        // remote — the user has to push back by however far they went in.
        self.perp_into = (self.perp_into + into_delta).min(self.hysteresis);
        if self.perp_into <= 0 {
            self.site = ControlSite::Local;
            self.perp_into = 0;
            Some(Transition::ToLocal)
        } else {
            None
        }
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
    fn returns_home_after_pulling_back_past_hysteresis() {
        let mut d = EdgeDetector::new(ScreenEdge::Right, 20);
        d.update_local(1919, 540, screen()); // now remote, perp_into = 20
                                             // Pushing further right is capped at hysteresis: stays remote.
        assert_eq!(d.update_remote(50, 0), None);
        // Pulling left less than hysteresis: still remote.
        assert_eq!(d.update_remote(-10, 0), None);
        // Pulling left past the remaining hysteresis: returns home.
        let t = d.update_remote(-20, 0);
        assert_eq!(t, Some(Transition::ToLocal));
        assert_eq!(d.site(), ControlSite::Local);
    }

    #[test]
    fn deep_navigation_does_not_strand_user_on_remote() {
        // Regression: previously `perp_into` grew unbounded as the user moved
        // deeper into the remote, so returning home took an equally huge
        // back-push. With the cap, hysteresis pixels of back-push is enough.
        let mut d = EdgeDetector::new(ScreenEdge::Right, 25);
        d.update_local(1919, 540, screen()); // remote, perp_into = 25
        for _ in 0..50 {
            // Navigate far rightward on the remote (1000 px total).
            assert_eq!(d.update_remote(20, 0), None);
        }
        // A single back-push of hysteresis returns home.
        assert_eq!(d.update_remote(-25, 0), Some(Transition::ToLocal));
    }

    #[test]
    fn left_edge_uses_opposite_axis_sign() {
        let mut d = EdgeDetector::new(ScreenEdge::Left, 10);
        d.update_local(0, 540, screen()); // remote, perp_into = 10
                                          // Going further left (negative dx) is deeper into remote.
        assert_eq!(d.update_remote(-100, 0), None);
        // Coming back right (positive dx) returns home once past hysteresis.
        assert_eq!(d.update_remote(200, 0), Some(Transition::ToLocal));
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
