//! Wayland edge barrier: a fully transparent layer-shell surface with a 1px
//! strip on the right boundary of the Linux desktop and padding outside it.
//! When the pointer enters, the compositor sends a pointer enter event. This
//! is precise, protocol-backed edge detection instead of delta-accumulation
//! cursor estimation, so it cannot drift.
//!
//! Only the desktop's right *boundary* carries a barrier. Seams between
//! monitors are interior edges: a barrier there would trap the local cursor
//! and hand control to the remote machine when the user merely wanted the
//! next monitor. Which outputs touch the boundary is not a property of any
//! single output, so it is derived from the layout:
//!
//! 1. every output gets a bufferless layer surface anchored to all four
//!    edges with a zero size; the compositor answers with the output's
//!    logical size,
//! 2. an output is on the boundary when its logical right edge equals the
//!    rightmost edge of the whole layout (`position.x + width`, where
//!    `position` comes from `wl_output.geometry` in the compositor's global
//!    logical space),
//! 3. boundary outputs are re-anchored into the 1px strip and get a
//!    transparent buffer; all others stay bufferless, stay unmapped and
//!    therefore receive no pointer events.
//!
//! A bufferless surface is the probe as well as the "inert" state. Any
//! output-layout change (hotplug, mode, scale) destroys and recreates the
//! surfaces, so every decision is made from a fresh configure.
//!
//! This module runs on its own thread (the wayland event loop is blocking)
//! and reports Enter/Leave to the main loop over a channel.

use std::error::Error;
use std::fs::File;
use std::io;
use std::io::ErrorKind::NotFound;
use std::os::fd::AsFd;
use std::path::PathBuf;
use std::sync::mpsc::Sender;
use std::thread;
use std::time::{Duration, Instant};

use wayland_client::protocol::wl_buffer::WlBuffer;
use wayland_client::protocol::wl_compositor::WlCompositor;
use wayland_client::protocol::wl_output::WlOutput;
use wayland_client::protocol::wl_pointer::WlPointer;
use wayland_client::protocol::wl_registry::WlRegistry;
use wayland_client::protocol::wl_seat::WlSeat;
use wayland_client::protocol::wl_shm::{Format, WlShm};
use wayland_client::protocol::wl_shm_pool::WlShmPool;
use wayland_client::protocol::wl_surface::WlSurface;
use wayland_client::protocol::{wl_output, wl_pointer, wl_registry, wl_seat};
use wayland_client::{Connection, Dispatch, EventQueue, QueueHandle, WEnum};
use wayland_protocols_wlr::layer_shell::v1::client::zwlr_layer_shell_v1::{
    Layer, ZwlrLayerShellV1,
};
use wayland_protocols_wlr::layer_shell::v1::client::zwlr_layer_surface_v1;
use wayland_protocols_wlr::layer_shell::v1::client::zwlr_layer_surface_v1::{
    Anchor, KeyboardInteractivity, ZwlrLayerSurfaceV1,
};

// The output space may be ceiled while layer-shell geometry is rounded.
// Extend outward by one logical pixel so the clamped pointer still hits the
// surface under fractional scaling, without widening the strip inside it.
const EDGE_PADDING: i32 = 1;
const BARRIER_WIDTH: i32 = 1 + EDGE_PADDING;
// Two outputs stacked on the same boundary may disagree by one pixel under
// fractional scaling. Interior seams are hundreds of pixels away, so the
// tolerance cannot resurrect a seam barrier.
const EDGE_TOLERANCE: i32 = 1;

#[derive(Debug, Clone, Copy)]
pub enum BarrierEvent {
    Enter(Instant),
    Leave,
}

/// Start the barrier thread. A Wayland session is required for edge
/// switching. Reconnects automatically on disconnect, so a compositor
/// restart does not take down the input pipe.
pub fn spawn(tx: Sender<BarrierEvent>) -> io::Result<()> {
    if std::env::var_os("WAYLAND_DISPLAY").is_none() {
        return Err(io::Error::new(
            NotFound,
            "WAYLAND_DISPLAY is not set; edge switching requires a Wayland session",
        ));
    }
    thread::spawn(move || loop {
        if let Err(e) = run(tx.clone()) {
            log!("(barrier) lost compositor ({e}), reconnecting in 5s");
        }
        thread::sleep(Duration::from_secs(5));
    });
    Ok(())
}

fn run(tx: Sender<BarrierEvent>) -> Result<(), Box<dyn Error>> {
    let conn = Connection::connect_to_env()?;
    let mut queue: EventQueue<State> = conn.new_event_queue();
    let qh = queue.handle();
    conn.display().get_registry(&qh, ());

    let mut state = State::new(tx);
    queue.roundtrip(&mut state)?;
    if state.layer_shell.is_none() {
        return Err("compositor does not support layer-shell".into());
    }
    loop {
        // Layout changes are applied between event batches, never in the
        // middle of one: a burst of output events must produce exactly one
        // probe round.
        state.sync(&qh);
        queue.blocking_dispatch(&mut state)?;
    }
}

struct State {
    tx: Sender<BarrierEvent>,
    compositor: Option<WlCompositor>,
    shm: Option<WlShm>,
    layer_shell: Option<ZwlrLayerShellV1>,
    seat: Option<WlSeat>,
    pointer: Option<WlPointer>,
    outputs: Vec<Output>,
    /// Set by any output-layout event. The next sync drops every surface so
    /// each decision is made from a fresh configure.
    layout_dirty: bool,
}

struct Output {
    /// Registry name, used to match a global removal.
    global: u32,
    wl: WlOutput,
    /// Compositor-assigned name (`wl_output.name`, v4), for logs only.
    name: String,
    /// Logical top-left in the compositor's global space, from
    /// `wl_output.geometry`.
    position: (i32, i32),
    /// Logical size, learned from a bufferless full-anchor configure.
    logical: Option<(i32, i32)>,
    surface: Option<BarrierSurface>,
    /// True while the surface carries the strip buffer and therefore
    /// captures pointer input.
    edge: bool,
}

struct BarrierSurface {
    surface: WlSurface,
    layer: ZwlrLayerSurfaceV1,
    /// An all-zero pixel is fully transparent; the file handle must live
    /// as long as the buffer.
    backing: Option<(File, WlBuffer)>,
    /// Height of `backing`; 0 before the first strip configure.
    buffer_height: u32,
}

impl Output {
    fn label(&self) -> String {
        if self.name.is_empty() {
            format!("output@{}", self.position.0)
        } else {
            self.name.clone()
        }
    }

    /// Drop the surface objects, returning the output to "no surface" so the
    /// next sync probes it again from scratch.
    fn drop_surface(&mut self) {
        if let Some(surface) = self.surface.take() {
            surface.layer.destroy();
            surface.surface.destroy();
        }
        self.edge = false;
        self.logical = None;
    }
}

impl State {
    fn new(tx: Sender<BarrierEvent>) -> Self {
        Self {
            tx,
            compositor: None,
            shm: None,
            layer_shell: None,
            seat: None,
            pointer: None,
            outputs: Vec::new(),
            layout_dirty: true,
        }
    }

    /// Apply pending layout work: recreate probes after a layout change,
    /// learn sizes from their configures, then place the boundary barrier.
    fn sync(&mut self, qh: &QueueHandle<Self>) {
        if self.layout_dirty {
            self.layout_dirty = false;
            for output in &mut self.outputs {
                output.drop_surface();
            }
        }
        self.create_probes(qh);
        if self.outputs.is_empty() {
            return;
        }
        // Wait until every output has answered its probe; placing edges from
        // a partial layout would light up an interior seam.
        if self.outputs.iter().any(|output| output.logical.is_none()) {
            return;
        }
        self.place_edges();
    }

    /// A bufferless surface is unmapped, so it can neither be seen nor take
    /// input. Anchoring it to all four edges with a zero size asks the
    /// compositor for the output's logical size in the configure.
    fn create_probes(&mut self, qh: &QueueHandle<Self>) {
        let (Some(compositor), Some(layer_shell)) =
            (self.compositor.clone(), self.layer_shell.clone())
        else {
            return;
        };
        for output in self.outputs.iter_mut().filter(|o| o.surface.is_none()) {
            let surface = compositor.create_surface(qh, ());
            let layer = layer_shell.get_layer_surface(
                &surface,
                Some(&output.wl),
                Layer::Top,
                "hidpipe-barrier".to_string(),
                qh,
                (),
            );
            layer.set_anchor(Anchor::Top | Anchor::Bottom | Anchor::Left | Anchor::Right);
            layer.set_size(0, 0);
            layer.set_exclusive_zone(-1);
            layer.set_keyboard_interactivity(KeyboardInteractivity::None);
            surface.commit();
            output.surface = Some(BarrierSurface {
                surface,
                layer,
                backing: None,
                buffer_height: 0,
            });
        }
    }

    /// Activate exactly the outputs that touch the desktop's right boundary
    /// and deactivate the rest. Idempotent: only role changes touch the
    /// protocol.
    fn place_edges(&mut self) {
        let Some(max_right) = self
            .outputs
            .iter()
            .filter_map(|output| Some(output.position.0 + output.logical?.0))
            .max()
        else {
            return;
        };
        let mut changed = false;
        for output in &mut self.outputs {
            let Some((width, _)) = output.logical else {
                continue;
            };
            let on_boundary = (output.position.0 + width - max_right).abs() <= EDGE_TOLERANCE;
            if on_boundary == output.edge {
                continue;
            }
            changed = true;
            let Some(surface) = output.surface.as_ref() else {
                continue;
            };
            if on_boundary {
                // Keep one pixel inside the output and cover rounding at its
                // edge. Ignore reserved zones so the barrier follows the
                // output boundary. The strip buffer arrives with the
                // configure this commit asks for.
                surface
                    .layer
                    .set_anchor(Anchor::Top | Anchor::Bottom | Anchor::Right);
                surface.layer.set_size(BARRIER_WIDTH as u32, 0);
                surface.layer.set_margin(0, -EDGE_PADDING, 0, 0);
                surface.layer.set_exclusive_zone(-1);
            } else {
                // Back to a probe: unmap first, then restore full anchoring.
                surface.surface.attach(None, 0, 0);
                surface
                    .layer
                    .set_anchor(Anchor::Top | Anchor::Bottom | Anchor::Left | Anchor::Right);
                surface.layer.set_size(0, 0);
                surface.layer.set_margin(0, 0, 0, 0);
            }
            surface.surface.commit();
            output.edge = on_boundary;
        }
        if changed {
            let edges: Vec<String> = self
                .outputs
                .iter()
                .filter(|output| output.edge)
                .map(Output::label)
                .collect();
            log!(
                "(barrier) right-edge barrier on {} of {} outputs: {}",
                edges.len(),
                self.outputs.len(),
                edges.join(", ")
            );
        }
    }

    fn edge_surface(&self, surface: &WlSurface) -> bool {
        self.outputs.iter().any(|output| {
            output.edge
                && output
                    .surface
                    .as_ref()
                    .is_some_and(|candidate| candidate.surface == *surface)
        })
    }

    fn index_of_layer(&self, layer: &ZwlrLayerSurfaceV1) -> Option<usize> {
        self.outputs.iter().position(|output| {
            output
                .surface
                .as_ref()
                .is_some_and(|candidate| candidate.layer == *layer)
        })
    }
}

/// A surface without a buffer has an empty input region and receives no
/// pointer events; attach an all-zero (fully transparent) shm buffer to
/// give it geometry.
fn make_transparent_buffer(
    shm: &WlShm,
    height: u32,
    qh: &QueueHandle<State>,
) -> Result<(File, WlBuffer), Box<dyn Error>> {
    let height = height.max(1);
    let stride = BARRIER_WIDTH * 4; // ARGB8888
    let size = u64::from(height) * stride as u64;
    let file = unlinked_tempfile()?;
    file.set_len(size)?;
    let pool = shm.create_pool(file.as_fd(), size as i32, qh, ());
    let buffer = pool.create_buffer(
        0,
        BARRIER_WIDTH,
        height as i32,
        stride,
        Format::Argb8888,
        qh,
        (),
    );
    Ok((file, buffer))
}

/// A tempfile unlinked right after creation: the fd stays valid, the
/// filesystem keeps no trace.
fn unlinked_tempfile() -> std::io::Result<File> {
    let dir = std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/tmp"));
    let path = dir.join(format!("hidpipe-barrier-{}", std::process::id()));
    // O_RDWR is required: the compositor mmaps this fd read-only, and
    // File::create's O_WRONLY makes that mmap fail (protocol error 2).
    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(true)
        .open(&path)?;
    let _ = std::fs::remove_file(&path);
    Ok(file)
}

impl Dispatch<WlRegistry, ()> for State {
    fn event(
        state: &mut Self,
        registry: &WlRegistry,
        event: wl_registry::Event,
        _: &(),
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        match event {
            wl_registry::Event::Global {
                name,
                interface,
                version,
            } => match interface.as_str() {
                "wl_compositor" => {
                    state.compositor = Some(registry.bind(name, version.min(4), qh, ()));
                }
                "wl_shm" => {
                    state.shm = Some(registry.bind(name, 1, qh, ()));
                }
                "zwlr_layer_shell_v1" => {
                    state.layer_shell = Some(registry.bind(name, version.min(4), qh, ()));
                }
                "wl_seat" => {
                    if state.seat.is_none() {
                        state.seat = Some(registry.bind(name, version.min(5), qh, ()));
                    }
                }
                "wl_output" => {
                    // v2 is enough for geometry, mode, scale and done; v4
                    // adds the name used in logs.
                    let wl = registry.bind(name, version.min(4), qh, ());
                    state.outputs.push(Output {
                        global: name,
                        wl,
                        name: String::new(),
                        position: (0, 0),
                        logical: None,
                        surface: None,
                        edge: false,
                    });
                    state.layout_dirty = true;
                }
                _ => {}
            },
            wl_registry::Event::GlobalRemove { name } => {
                if let Some(index) = state.outputs.iter().position(|o| o.global == name) {
                    // Dropping the output drops its proxies; the compositor
                    // has already closed the removed output's layer surface.
                    state.outputs.remove(index);
                    state.layout_dirty = true;
                }
            }
            _ => {}
        }
    }
}

impl Dispatch<WlOutput, ()> for State {
    fn event(
        state: &mut Self,
        output: &WlOutput,
        event: wl_output::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let Some(index) = state.outputs.iter().position(|o| o.wl == *output) else {
            return;
        };
        match event {
            wl_output::Event::Geometry { x, y, .. } => {
                state.outputs[index].position = (x, y);
                state.layout_dirty = true;
            }
            wl_output::Event::Name { name } => state.outputs[index].name = name,
            // Mode and scale change the logical size; done closes a burst of
            // output state events.
            wl_output::Event::Mode { .. }
            | wl_output::Event::Scale { .. }
            | wl_output::Event::Done => state.layout_dirty = true,
            _ => {}
        }
    }
}

impl Dispatch<WlSeat, ()> for State {
    fn event(
        state: &mut Self,
        seat: &WlSeat,
        event: wl_seat::Event,
        _: &(),
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        if let wl_seat::Event::Capabilities {
            capabilities: WEnum::Value(caps),
        } = event
        {
            if caps.contains(wl_seat::Capability::Pointer) && state.pointer.is_none() {
                state.pointer = Some(seat.get_pointer(qh, ()));
            }
        }
    }
}

impl Dispatch<WlPointer, ()> for State {
    fn event(
        state: &mut Self,
        _: &WlPointer,
        event: wl_pointer::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            wl_pointer::Event::Enter { surface, .. } if state.edge_surface(&surface) => {
                let _ = state.tx.send(BarrierEvent::Enter(Instant::now()));
            }
            wl_pointer::Event::Leave { surface, .. } if state.edge_surface(&surface) => {
                let _ = state.tx.send(BarrierEvent::Leave);
            }
            _ => {}
        }
    }
}

impl Dispatch<ZwlrLayerSurfaceV1, ()> for State {
    fn event(
        state: &mut Self,
        layer: &ZwlrLayerSurfaceV1,
        event: zwlr_layer_surface_v1::Event,
        _: &(),
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        match event {
            zwlr_layer_surface_v1::Event::Configure {
                serial,
                width,
                height,
                ..
            } => {
                layer.ack_configure(serial);
                let Some(index) = state.index_of_layer(layer) else {
                    return;
                };
                if state.outputs[index].edge {
                    let Some(shm) = state.shm.clone() else { return };
                    if state.outputs[index]
                        .surface
                        .as_ref()
                        .is_none_or(|s| s.buffer_height != height)
                    {
                        let Ok(backing) = make_transparent_buffer(&shm, height, qh) else {
                            log!("(barrier) buffer creation failed");
                            return;
                        };
                        let surface = state.outputs[index].surface.as_mut().expect("indexed");
                        surface.backing = Some(backing);
                        surface.buffer_height = height;
                    }
                    let surface = state.outputs[index].surface.as_ref().expect("indexed");
                    if let Some((_, buffer)) = &surface.backing {
                        surface.surface.attach(Some(buffer), 0, 0);
                        surface.surface.commit();
                    }
                } else if width > 0 && height > 0 {
                    // Probe answer: the output's logical size.
                    state.outputs[index].logical = Some((width as i32, height as i32));
                }
            }
            zwlr_layer_surface_v1::Event::Closed => {
                let Some(index) = state.index_of_layer(layer) else {
                    return;
                };
                state.outputs[index].drop_surface();
                state.layout_dirty = true;
            }
            _ => {}
        }
    }
}

// We do not care about these objects' events, but wayland-client requires
// a Dispatch impl for every created object.
macro_rules! empty_dispatch {
    ($($t:ty),*) => {
        $(
            impl Dispatch<$t, ()> for State {
                fn event(
                    _: &mut Self,
                    _: &$t,
                    _: <$t as wayland_client::Proxy>::Event,
                    _: &(),
                    _: &Connection,
                    _: &QueueHandle<Self>,
                ) {
                }
            }
        )*
    };
}

empty_dispatch!(
    WlCompositor,
    WlShm,
    WlShmPool,
    WlBuffer,
    WlSurface,
    ZwlrLayerShellV1
);
