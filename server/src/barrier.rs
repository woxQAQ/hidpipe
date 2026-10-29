//! Wayland edge barrier: a 1px fully transparent layer-shell surface on the
//! right edge of every output. When the pointer enters, the compositor sends
//! a pointer enter event. This is precise, protocol-backed edge detection
//! instead of delta-accumulation cursor estimation, so it cannot drift.
//!
//! This module runs on its own thread (the wayland event loop is blocking)
//! and reports Enter/Leave to the main loop over a channel.

use std::error::Error;
use std::fs::File;
use std::io;
use std::os::fd::AsFd;
use std::path::PathBuf;
use std::sync::mpsc::Sender;
use std::thread;
use std::time::Duration;

use wayland_client::protocol::{
    wl_buffer, wl_compositor, wl_output, wl_pointer, wl_registry, wl_seat, wl_shm, wl_shm_pool,
    wl_surface,
};
use wayland_client::{Connection, Dispatch, EventQueue, QueueHandle, WEnum};
use wayland_protocols_wlr::layer_shell::v1::client::{zwlr_layer_shell_v1, zwlr_layer_surface_v1};

#[derive(Debug, Clone, Copy)]
pub enum BarrierEvent {
    Enter,
    Leave,
}

/// Start the barrier thread. A Wayland session is required for edge
/// switching. Reconnects automatically on disconnect, so a compositor
/// restart does not take down the input pipe.
pub fn spawn(tx: Sender<BarrierEvent>) -> io::Result<()> {
    if std::env::var_os("WAYLAND_DISPLAY").is_none() {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
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
    state.create_barriers(&qh);
    // One more roundtrip to get seat capabilities and the first configure.
    queue.roundtrip(&mut state)?;
    log!(
        "(barrier) edge barrier in place: right edge x {}",
        state.barriers.len()
    );

    loop {
        queue.blocking_dispatch(&mut state)?;
    }
}

struct State {
    tx: Sender<BarrierEvent>,
    compositor: Option<wl_compositor::WlCompositor>,
    shm: Option<wl_shm::WlShm>,
    layer_shell: Option<zwlr_layer_shell_v1::ZwlrLayerShellV1>,
    seat: Option<wl_seat::WlSeat>,
    pointer: Option<wl_pointer::WlPointer>,
    outputs: Vec<wl_output::WlOutput>,
    barriers: Vec<Barrier>,
}

struct Barrier {
    surface: wl_surface::WlSurface,
    layer: zwlr_layer_surface_v1::ZwlrLayerSurfaceV1,
    /// An all-zero pixel is fully transparent; the file handle must live
    /// as long as the buffer.
    backing: Option<(File, wl_buffer::WlBuffer)>,
    height: u32,
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
            barriers: Vec::new(),
        }
    }

    fn create_barriers(&mut self, qh: &QueueHandle<Self>) {
        let (Some(compositor), Some(layer_shell)) = (&self.compositor, &self.layer_shell) else {
            return;
        };
        for output in &self.outputs {
            let surface = compositor.create_surface(qh, ());
            let layer = layer_shell.get_layer_surface(
                &surface,
                Some(output),
                zwlr_layer_shell_v1::Layer::Top,
                "hidpipe-barrier".to_string(),
                qh,
                (),
            );
            use zwlr_layer_surface_v1::Anchor;
            // Anchor to the right edge and stretch top to bottom; 1px wide.
            // No exclusive space, no keyboard focus.
            layer.set_anchor(Anchor::Top | Anchor::Bottom | Anchor::Right);
            layer.set_size(1, 0);
            layer.set_exclusive_zone(0);
            layer.set_keyboard_interactivity(zwlr_layer_surface_v1::KeyboardInteractivity::None);
            surface.commit();
            self.barriers.push(Barrier {
                surface,
                layer,
                backing: None,
                height: 0,
            });
        }
    }

    fn is_barrier(&self, surface: &wl_surface::WlSurface) -> bool {
        self.barriers.iter().any(|b| b.surface == *surface)
    }
}

/// A surface without a buffer has an empty input region and receives no
/// pointer events; attach an all-zero (fully transparent) shm buffer to
/// give it geometry.
fn make_transparent_buffer(
    shm: &wl_shm::WlShm,
    height: u32,
    qh: &QueueHandle<State>,
) -> Result<(File, wl_buffer::WlBuffer), Box<dyn Error>> {
    let height = height.max(1);
    let size = u64::from(height) * 4; // 1px wide, ARGB8888
    let file = unlinked_tempfile()?;
    file.set_len(size)?;
    let pool = shm.create_pool(file.as_fd(), size as i32, qh, ());
    let buffer = pool.create_buffer(0, 1, height as i32, 4, wl_shm::Format::Argb8888, qh, ());
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

impl Dispatch<wl_registry::WlRegistry, ()> for State {
    fn event(
        state: &mut Self,
        registry: &wl_registry::WlRegistry,
        event: wl_registry::Event,
        _: &(),
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        if let wl_registry::Event::Global {
            name,
            interface,
            version,
        } = event
        {
            match interface.as_str() {
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
                    state
                        .outputs
                        .push(registry.bind(name, version.min(2), qh, ()));
                }
                _ => {}
            }
        }
    }
}

impl Dispatch<wl_seat::WlSeat, ()> for State {
    fn event(
        state: &mut Self,
        seat: &wl_seat::WlSeat,
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

impl Dispatch<wl_pointer::WlPointer, ()> for State {
    fn event(
        state: &mut Self,
        _: &wl_pointer::WlPointer,
        event: wl_pointer::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            wl_pointer::Event::Enter { surface, .. } if state.is_barrier(&surface) => {
                let _ = state.tx.send(BarrierEvent::Enter);
            }
            wl_pointer::Event::Leave { surface, .. } if state.is_barrier(&surface) => {
                let _ = state.tx.send(BarrierEvent::Leave);
            }
            _ => {}
        }
    }
}

impl Dispatch<zwlr_layer_surface_v1::ZwlrLayerSurfaceV1, ()> for State {
    fn event(
        state: &mut Self,
        layer: &zwlr_layer_surface_v1::ZwlrLayerSurfaceV1,
        event: zwlr_layer_surface_v1::Event,
        _: &(),
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        let zwlr_layer_surface_v1::Event::Configure { serial, height, .. } = event else {
            return;
        };
        layer.ack_configure(serial);
        let Some(shm) = state.shm.clone() else { return };
        let Some(barrier) = state.barriers.iter_mut().find(|b| b.layer == *layer) else {
            return;
        };
        if barrier.height != height {
            match make_transparent_buffer(&shm, height, qh) {
                Ok(backing) => barrier.backing = Some(backing),
                Err(e) => {
                    log!("(barrier) buffer creation failed: {e}");
                    return;
                }
            }
            barrier.height = height;
        }
        if let Some((_, buffer)) = &barrier.backing {
            barrier.surface.attach(Some(buffer), 0, 0);
            barrier.surface.commit();
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
    wl_compositor::WlCompositor,
    wl_output::WlOutput,
    wl_shm::WlShm,
    wl_shm_pool::WlShmPool,
    wl_buffer::WlBuffer,
    wl_surface::WlSurface,
    zwlr_layer_shell_v1::ZwlrLayerShellV1
);
