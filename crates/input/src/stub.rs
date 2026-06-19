//! No-op input backend for non-Windows targets, so the crate compiles
//! everywhere and the loopback dev mode has something to bind to. It logs what it
//! *would* inject, which is handy when debugging the wire protocol off-device.

use crate::edge::Rect;
use crate::{CapturedEvent, Capturer, Injector};
use screenlink_core::protocol::InputEvent;
use std::time::Duration;

#[derive(Default)]
pub struct StubInjector;

impl Injector for StubInjector {
    fn inject(&mut self, ev: InputEvent) -> anyhow::Result<()> {
        tracing::trace!("stub inject: {ev:?}");
        Ok(())
    }
    fn desktop_rect(&self) -> Rect {
        Rect::new(0, 0, 1920, 1080)
    }
    fn set_cursor_norm(&mut self, _x: f32, _y: f32) -> anyhow::Result<()> {
        Ok(())
    }
}

#[derive(Default)]
pub struct StubCapturer;

impl Capturer for StubCapturer {
    fn poll(&self, timeout: Duration) -> Option<CapturedEvent> {
        std::thread::sleep(timeout.min(Duration::from_millis(50)));
        None
    }
    fn set_suppress(&self, _suppress: bool) {}
    fn park_cursor(&self, _x: i32, _y: i32) {}
}
