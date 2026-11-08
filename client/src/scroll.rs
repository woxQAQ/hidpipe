//! Connection-scoped ordinary wheel injection. The wire remains 120 units/notch.
use core_graphics::event::{CGEvent, CGEventFlags, ScrollEventUnit};
use core_graphics::event_source::CGEventSource;
use hidpipe_proto::{Frame, PointerEvent};

use crate::diagnostics::{measure, Stage};

/// Destination-side shaping; a notch maps to ten Quartz points before shaping.
#[derive(Clone, Copy)]
pub struct ScrollModifier {
    pub scale_x: f64,
    pub scale_y: f64,
    pub invert_x: bool,
    pub invert_y: bool,
}

pub struct Scroll {
    modifier: ScrollModifier,
    remainder: [f64; 2],
    compensation: [f64; 2],
}

impl Scroll {
    pub fn new(modifier: ScrollModifier) -> Self {
        Self {
            modifier,
            remainder: [0.0; 2],
            compensation: [0.0; 2],
        }
    }

    /// Build but never post. Reset frames discard all subpixel state.
    /// Candidate state is committed only after successful event creation.
    pub fn event(
        &mut self,
        source: &CGEventSource,
        frame: &Frame,
        flags: CGEventFlags,
    ) -> Result<Option<CGEvent>, ()> {
        let (dx, dy) = match frame {
            Frame::Wheel { dx, dy } => (*dx, *dy),
            Frame::Pointer(PointerEvent::Wheel { dx, dy }) => (*dx, *dy),
            Frame::ReleaseAll | Frame::Pointer(PointerEvent::Reset) => {
                self.remainder = [0.0; 2];
                self.compensation = [0.0; 2];
                return Ok(None);
            }
            _ => return Ok(None),
        };
        let mut remainder = [0.0; 2];
        let mut compensation = [0.0; 2];
        let mut pixels = [0; 2];
        for (axis, (raw, scale, invert)) in [
            (dx, self.modifier.scale_x, self.modifier.invert_x),
            (dy, self.modifier.scale_y, self.modifier.invert_y),
        ]
        .into_iter()
        .enumerate()
        {
            // Divide before multiplying so tiny input and large finite scales
            // remain useful. Overflow saturates and leaves no compensation queue.
            let signed = f64::from(raw) / 12.0 * if invert { -1.0 } else { 1.0 };
            let delta = signed * scale - self.compensation[axis];
            let total = self.remainder[axis] + delta;
            if total <= f64::from(i32::MIN) || total >= f64::from(i32::MAX) {
                pixels[axis] = total.clamp(f64::from(i32::MIN), f64::from(i32::MAX)) as i32;
                continue;
            }
            // Kahan compensation survives integer extraction as well as reports
            // too small to emit. It bounds accumulated addition error without
            // rounding each tiny physical input up to an artificial minimum.
            compensation[axis] = (total - self.remainder[axis]) - delta;
            // Division/multiplication can still land a few rounding errors below
            // an integer. Normalize only nonzero integer boundaries, within four
            // relative machine epsilons. Preserve any advance as signed debt in
            // the remainder, rather than discarding it on every emitted point.
            let nearest = total.round();
            let at_boundary =
                nearest != 0.0 && (total - nearest).abs() <= 4.0 * f64::EPSILON * total.abs();
            pixels[axis] = if at_boundary { nearest } else { total.trunc() } as i32;
            remainder[axis] = total - f64::from(pixels[axis]);
        }
        if pixels == [0, 0] {
            self.remainder = remainder;
            self.compensation = compensation;
            return Ok(None);
        }
        let event = measure(Stage::Create, || {
            CGEvent::new_scroll_event(
                source.clone(),
                ScrollEventUnit::PIXEL,
                2,
                pixels[1],
                pixels[0],
                0,
            )
        })?;
        measure(Stage::Configure, || event.set_flags(flags));
        self.remainder = remainder;
        self.compensation = compensation;
        Ok(Some(event))
    }
}
