//! Drag audio files out of Tundra into other apps (DAWs, file managers, etc.).
//!
//! Windows and macOS use the [`drag`] crate. Linux/X11 uses an XDND source adapted
//! from [guth](https://docs.rs/guth) (Apache-2.0 / MIT).

use iced::window::raw_window_handle::{HasWindowHandle, RawWindowHandle};

#[cfg(any(windows, target_os = "macos"))]
pub fn start_blocking(window: &dyn HasWindowHandle, path: std::path::PathBuf) -> Result<(), String> {
    let item = drag::DragItem::Files(vec![path.clone()]);
    // `&dyn HasWindowHandle` is itself a sized `HasWindowHandle`, as `start_drag` requires.
    drag::start_drag(&window, item, drag::Image::File(path), |_, _| {}, drag::Options::default())
        .map_err(|err| err.to_string())
}

pub fn x11_window_id(window: &dyn HasWindowHandle) -> Option<u32> {
    match window.window_handle().ok()?.as_raw() {
        // `c_ulong`: 32 bits on Windows, 64 on Linux.
        #[allow(clippy::useless_conversion)]
        RawWindowHandle::Xlib(handle) => u32::try_from(handle.window).ok(),
        RawWindowHandle::Xcb(handle) => Some(handle.window.get()),
        _ => None,
    }
}

#[cfg(all(unix, not(target_os = "macos")))]
mod x11 {
    use std::path::{Path, PathBuf};
    use std::time::{Duration, Instant};
    use x11rb::connection::Connection;
    use x11rb::protocol::Event;
    use x11rb::protocol::xproto::{
        Atom, AtomEnum, ClientMessageEvent, ConfigureWindowAux, ConnectionExt as _, CreateWindowAux, EventMask,
        PropMode, SELECTION_NOTIFY_EVENT, SelectionNotifyEvent, SelectionRequestEvent, StackMode, Window, WindowClass,
    };
    use x11rb::rust_connection::RustConnection;
    use x11rb::wrapper::ConnectionExt as _;

    const REPLACE: PropMode = PropMode::REPLACE;
    const EVENT_LIMIT: usize = 64;
    const TIMESTAMP_POLL_LIMIT: usize = 256;
    const WINDOW_HIERARCHY_LIMIT: usize = 64;
    const STATUS_RESPONSE_TIMEOUT: Duration = Duration::from_secs(1);
    const DROP_RESPONSE_TIMEOUT: Duration = Duration::from_secs(10);

    x11rb::atom_manager! {
        Atoms: AtomsCookie {
            TARGETS,
            TUNDRA_DRAG_TIMESTAMP,
            _NET_WM_WINDOW_TYPE,
            _NET_WM_WINDOW_TYPE_DND,
            XdndActionCopy,
            XdndActionList,
            XdndAware,
            XdndDrop,
            XdndEnter,
            XdndFinished,
            XdndLeave,
            XdndPosition,
            XdndProxy,
            XdndSelection,
            XdndStatus,
            XdndTypeList,
            TextUriList: b"text/uri-list",
        }
    }

    /// XDND drag source, driven by polling from the UI's ticks.
    #[derive(Default)]
    pub struct X11Drag(Option<Source>);

    impl X11Drag {
        pub fn init_with_window_id(&mut self, app_window: u32) -> Result<(), String> {
            if self.0.is_none() {
                self.0 = Some(Source::new(app_window).map_err(|X11Error(message)| message)?);
            }
            Ok(())
        }

        pub fn is_active(&self) -> bool {
            self.0.as_ref().is_some_and(|source| source.drag.is_some())
        }

        pub fn start(&mut self, path: PathBuf) -> Result<(), String> {
            let source = self.0.as_mut().ok_or("X11 drag is unavailable on this display")?;
            source.start(&path).map_err(|X11Error(message)| message)
        }

        /// Advances the drag; `released` once the pointer button is up.
        pub fn update(&mut self, released: bool) {
            if let Some(source) = self.0.as_mut() {
                source.update(released);
            }
        }
    }

    struct Source {
        connection: RustConnection,
        root: Window,
        app_window: Window,
        /// Small override-redirect window that follows the pointer and owns the selection.
        source_window: Window,
        atoms: Atoms,
        drag: Option<ActiveDrag>,
    }

    #[derive(Default)]
    struct ActiveDrag {
        uri_list: Vec<u8>,
        timestamp: u32,
        target: Option<DragTarget>,
        accepted: bool,
        /// An `XdndPosition` is waiting for its `XdndStatus`.
        position_pending: bool,
        /// The pointer was released; drop once the pending status arrives.
        release_pending: bool,
        dropped: bool,
        deadline: Option<Instant>,
    }

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    struct DragTarget {
        window: Window,
        /// `window`, or the proxy it names in `XdndProxy`.
        recipient: Window,
        version: u32,
    }

    impl Source {
        /// Opens a dedicated connection: iced owns the window's, and a separate
        /// client on the same display is standard for drag sources.
        fn new(app_window: Window) -> Result<Self, X11Error> {
            let (connection, screen_number) = x11rb::connect(None)?;
            let screen = &connection.setup().roots[screen_number];
            let root = screen.root;
            let source_window = connection.generate_id()?;
            let aux = CreateWindowAux::new()
                .override_redirect(1)
                .background_pixel(screen.white_pixel)
                .border_pixel(screen.black_pixel)
                .event_mask(EventMask::PROPERTY_CHANGE);
            let (depth, visual, class) = (screen.root_depth, screen.root_visual, WindowClass::INPUT_OUTPUT);
            connection.create_window(depth, source_window, root, 0, 0, 42, 28, 1, class, visual, &aux)?.check()?;
            let atoms = Atoms::new(&connection)?.reply()?;
            let source = Self { connection, root, app_window, source_window, atoms, drag: None };
            let (window_type, dnd) = (source.atoms._NET_WM_WINDOW_TYPE, source.atoms._NET_WM_WINDOW_TYPE_DND);
            source.set_atoms(window_type, &[dnd])?;
            Ok(source)
        }

        fn set_atoms(&self, property: Atom, values: &[Atom]) -> Result<(), X11Error> {
            let window = self.source_window;
            self.connection.change_property32(REPLACE, window, property, AtomEnum::ATOM, values)?.check()?;
            Ok(self.connection.flush()?)
        }

        fn update(&mut self, released: bool) {
            let expired = |drag: &ActiveDrag| drag.deadline.is_some_and(|time| time <= Instant::now());
            if self.poll_events().is_err() || self.drag.as_ref().is_some_and(expired) {
                return self.reset(true);
            }
            let Some(drag) = self.drag.as_mut().filter(|drag| !drag.dropped) else {
                return;
            };
            if released && !drag.release_pending {
                drag.release_pending = true;
                drag.deadline = Some(Instant::now() + STATUS_RESPONSE_TIMEOUT);
            }
            let (release_pending, position_pending) = (drag.release_pending, drag.position_pending);
            if !release_pending {
                if self.update_target().is_err() {
                    self.cancel();
                }
            } else if !position_pending {
                self.drop_or_cancel();
            }
        }

        fn start(&mut self, path: &Path) -> Result<(), X11Error> {
            let uri_list = file_uri(path)?.into_bytes();
            self.set_atoms(self.atoms.XdndTypeList, &[self.atoms.TextUriList])?;
            self.set_atoms(self.atoms.XdndActionList, &[self.atoms.XdndActionCopy])?;
            self.connection
                .set_selection_owner(self.source_window, self.atoms.XdndSelection, x11rb::CURRENT_TIME)?
                .check()?;
            let timestamp = self.server_timestamp()?;
            let owner = self.connection.get_selection_owner(self.atoms.XdndSelection)?;
            if owner.reply()?.owner != self.source_window {
                return Err("could not own the XDND selection".into());
            }
            self.drag = Some(ActiveDrag { uri_list, timestamp, ..ActiveDrag::default() });
            let pointer = self.connection.query_pointer(self.root)?.reply()?;
            self.move_icon(pointer.root_x, pointer.root_y)?;
            self.connection.map_window(self.source_window)?.check()?;
            Ok(self.connection.flush()?)
        }

        /// The server's current time, read from the `PropertyNotify` of a dummy write.
        fn server_timestamp(&self) -> Result<u32, X11Error> {
            let (window, atom) = (self.source_window, self.atoms.TUNDRA_DRAG_TIMESTAMP);
            self.connection.change_property8(REPLACE, window, atom, AtomEnum::INTEGER, &[0])?.check()?;
            self.connection.flush()?;
            for _ in 0..TIMESTAMP_POLL_LIMIT {
                match self.connection.poll_for_event()? {
                    Some(Event::PropertyNotify(event)) if event.window == window && event.atom == atom => {
                        return Ok(event.time);
                    }
                    Some(_) => {}
                    None => std::thread::sleep(Duration::from_millis(1)),
                }
            }
            Err("timed out waiting for X11 server timestamp".into())
        }

        fn update_target(&mut self) -> Result<(), X11Error> {
            let pointer = self.connection.query_pointer(self.root)?.reply()?;
            self.move_icon(pointer.root_x, pointer.root_y)?;
            let target = self.find_target(pointer.child)?;
            let Self { connection, atoms, source_window: me, drag: Some(drag), .. } = self else {
                return Ok(());
            };
            if target != drag.target {
                if let Some(previous) = drag.target {
                    send(connection, previous, atoms.XdndLeave, [*me, 0, 0, 0, 0])?;
                }
                if let Some(target) = target {
                    let data = [*me, target.version.min(5) << 24, atoms.TextUriList, 0, 0];
                    send(connection, target, atoms.XdndEnter, data)?;
                }
                drag.target = target;
                drag.accepted = false;
                drag.position_pending = false;
            }
            if let Some(target) = target
                && !drag.position_pending
            {
                let coordinates = (u32::from(pointer.root_x as u16) << 16) | u32::from(pointer.root_y as u16);
                let data = [*me, 0, coordinates, drag.timestamp, atoms.XdndActionCopy];
                send(connection, target, atoms.XdndPosition, data)?;
                drag.accepted = false;
                drag.position_pending = true;
            }
            Ok(())
        }

        fn move_icon(&self, root_x: i16, root_y: i16) -> Result<(), X11Error> {
            let aux = ConfigureWindowAux::new()
                .x(i32::from(root_x) + 16)
                .y(i32::from(root_y) + 16)
                .stack_mode(StackMode::ABOVE);
            self.connection.configure_window(self.source_window, &aux)?;
            Ok(self.connection.flush()?)
        }

        fn drop_or_cancel(&mut self) {
            let Some(drag) = self.drag.as_mut() else {
                return;
            };
            if let Some(target) = drag.target.filter(|_| drag.accepted) {
                let data = [self.source_window, 0, drag.timestamp, 0, 0];
                if send(&self.connection, target, self.atoms.XdndDrop, data).is_ok() {
                    drag.dropped = true;
                    drag.deadline = Some(Instant::now() + DROP_RESPONSE_TIMEOUT);
                    return;
                }
            }
            self.cancel();
        }

        fn cancel(&mut self) {
            let drag = self.drag.as_ref().filter(|drag| !drag.dropped);
            if let Some(target) = drag.and_then(|drag| drag.target) {
                let _ = send(&self.connection, target, self.atoms.XdndLeave, [self.source_window, 0, 0, 0, 0]);
            }
            self.reset(true);
        }

        fn reset(&mut self, release_selection: bool) {
            if release_selection && let Some(drag) = &self.drag {
                let _ = self.connection.set_selection_owner(x11rb::NONE, self.atoms.XdndSelection, drag.timestamp);
            }
            let _ = self.connection.unmap_window(self.source_window);
            let _ = self.connection.flush();
            self.drag = None;
        }

        /// The innermost XDND-aware window (or its proxy) under the pointer,
        /// unless that is Tundra itself.
        fn find_target(&self, child: Window) -> Result<Option<DragTarget>, X11Error> {
            let mut current = if child == x11rb::NONE { self.root } else { child };
            for _ in 0..WINDOW_HIERARCHY_LIMIT {
                let reply = self.connection.query_pointer(current)?.reply()?;
                if reply.child == x11rb::NONE || reply.child == current {
                    break;
                }
                current = reply.child;
            }
            let mut ancestors = Vec::new();
            for _ in 0..WINDOW_HIERARCHY_LIMIT {
                if current == self.app_window || current == self.source_window {
                    return Ok(None);
                }
                ancestors.push(current);
                if current == self.root {
                    break;
                }
                let tree = self.connection.query_tree(current)?.reply()?;
                if tree.parent == x11rb::NONE || tree.parent == current {
                    break;
                }
                current = tree.parent;
            }
            let proxy_of = |window| self.property32(window, self.atoms.XdndProxy, AtomEnum::WINDOW);
            for window in ancestors {
                // A proxy counts only when it names itself as its own proxy.
                let recipient = match proxy_of(window)? {
                    Some(proxy) if proxy_of(proxy)? == Some(proxy) => proxy,
                    _ => window,
                };
                let version = self.property32(recipient, self.atoms.XdndAware, AtomEnum::ATOM)?;
                if let Some(version) = version.filter(|version| *version >= 3) {
                    return Ok(Some(DragTarget { window, recipient, version }));
                }
            }
            Ok(None)
        }

        /// The first value of a 32-bit property of type `kind`.
        fn property32(&self, window: Window, property: Atom, kind: AtomEnum) -> Result<Option<u32>, X11Error> {
            let reply = self.connection.get_property(false, window, property, kind, 0, 1)?;
            let reply = reply.reply()?;
            let matches = reply.type_ == u32::from(kind) && reply.format == 32;
            Ok(reply.value32().filter(|_| matches).and_then(|mut values| values.next()))
        }

        fn poll_events(&mut self) -> Result<(), X11Error> {
            for _ in 0..EVENT_LIMIT {
                let Some(event) = self.connection.poll_for_event()? else {
                    break;
                };
                match event {
                    Event::ClientMessage(event) if event.format == 32 && event.window == self.source_window => {
                        let data = event.data.as_data32();
                        if event.type_ == self.atoms.XdndFinished {
                            self.reset(true);
                        } else if event.type_ == self.atoms.XdndStatus
                            && let Some(drag) = self.drag.as_mut()
                            && drag.position_pending
                            && drag.target.is_some_and(|target| target.window == data[0])
                        {
                            drag.position_pending = false;
                            drag.accepted = data[1] & 1 != 0 && data[4] == self.atoms.XdndActionCopy;
                        }
                    }
                    Event::SelectionRequest(event) => self.answer_selection_request(event)?,
                    Event::SelectionClear(event) if event.selection == self.atoms.XdndSelection => {
                        self.reset(false);
                    }
                    _ => {}
                }
            }
            Ok(())
        }

        /// Hands the URI list to the drop target, only after the drop and only
        /// for a request no older than the drag.
        fn answer_selection_request(&self, request: SelectionRequestEvent) -> Result<(), X11Error> {
            if request.owner != self.source_window || request.selection != self.atoms.XdndSelection {
                return Ok(());
            }
            let property = if request.property == x11rb::NONE { request.target } else { request.property };
            let not_older = |drag: &&ActiveDrag| {
                request.time == x11rb::CURRENT_TIME || request.time.wrapping_sub(drag.timestamp) < (1 << 31)
            };
            let written = self.drag.as_ref().filter(|drag| drag.dropped).filter(not_older).is_some_and(|drag| {
                let (requestor, atoms) = (request.requestor, &self.atoms);
                let cookie = if request.target == atoms.TextUriList {
                    self.connection.change_property8(REPLACE, requestor, property, atoms.TextUriList, &drag.uri_list)
                } else if request.target == atoms.TARGETS {
                    let targets = [atoms.TextUriList, atoms.TARGETS];
                    self.connection.change_property32(REPLACE, requestor, property, AtomEnum::ATOM, &targets)
                } else {
                    return false;
                };
                cookie.is_ok_and(|cookie| cookie.check().is_ok())
            });
            let notify = SelectionNotifyEvent {
                response_type: SELECTION_NOTIFY_EVENT,
                sequence: 0,
                time: request.time,
                requestor: request.requestor,
                selection: request.selection,
                target: request.target,
                property: if written { property } else { x11rb::NONE },
            };
            self.connection.send_event(false, request.requestor, EventMask::NO_EVENT, notify)?.check()?;
            Ok(self.connection.flush()?)
        }
    }

    impl Drop for Source {
        fn drop(&mut self) {
            self.cancel();
            let _ = self.connection.destroy_window(self.source_window);
            let _ = self.connection.flush();
        }
    }

    fn send(connection: &RustConnection, target: DragTarget, kind: Atom, data: [u32; 5]) -> Result<(), X11Error> {
        let event = ClientMessageEvent::new(32, target.window, kind, data);
        connection.send_event(false, target.recipient, EventMask::NO_EVENT, event)?.check()?;
        Ok(connection.flush()?)
    }

    /// `file://` URI with every byte outside the unreserved set percent-encoded (RFC 8089),
    /// as a `text/uri-list` line.
    fn file_uri(path: &Path) -> Result<String, X11Error> {
        use std::os::unix::ffi::OsStrExt;
        if !path.is_absolute() {
            return Err("drag path must be absolute".into());
        }
        let mut uri = String::from("file://");
        for &byte in path.as_os_str().as_bytes() {
            if byte.is_ascii_alphanumeric() || b"-._~/".contains(&byte) {
                uri.push(char::from(byte));
            } else {
                uri.push_str(&format!("%{byte:02X}"));
            }
        }
        Ok(uri + "\r\n")
    }

    /// Why an X11 request failed. Any displayable error converts with `?`;
    /// `X11Drag` hands callers the message.
    struct X11Error(String);

    impl<E: std::fmt::Display> From<E> for X11Error {
        fn from(error: E) -> Self {
            Self(error.to_string())
        }
    }
}

#[cfg(all(unix, not(target_os = "macos")))]
pub type NativeDrag = x11::X11Drag;

/// Windows and macOS drags block in `start_blocking`, so there is nothing to poll.
#[cfg(not(all(unix, not(target_os = "macos"))))]
#[derive(Default)]
pub struct NativeDrag;

#[cfg(not(all(unix, not(target_os = "macos"))))]
impl NativeDrag {
    pub fn init_with_window_id(&mut self, _app_window: u32) -> Result<(), String> {
        Ok(())
    }

    pub fn is_active(&self) -> bool {
        false
    }

    pub fn update(&mut self, _released: bool) {}
}
