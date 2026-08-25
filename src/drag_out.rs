//! Drag audio files out of Tundra into other apps (DAWs, file managers, etc.).
//!
//! Windows and macOS use the [`drag`] crate. Linux/X11 uses an XDND source adapted
//! from [guth](https://docs.rs/guth) (Apache-2.0 / MIT).

use iced::window::raw_window_handle::{HasWindowHandle, RawWindowHandle};
use std::path::{Path, PathBuf};

#[cfg(any(windows, target_os = "macos"))]
pub fn start_blocking<W: HasWindowHandle>(window: &W, path: PathBuf) -> Result<(), String> {
    let canonical = std::fs::canonicalize(&path).map_err(|err| err.to_string())?;
    let item = drag::DragItem::Files(vec![canonical.clone()]);
    let preview = drag::Image::File(canonical);
    drag::start_drag(
        window,
        item,
        preview,
        |_, _| {},
        drag::Options::default(),
    )
    .map_err(|err| err.to_string())
}

pub fn x11_window_id(window: &dyn HasWindowHandle) -> Option<u32> {
    match window.window_handle().ok()?.as_raw() {
        RawWindowHandle::Xlib(handle) => u32::try_from(handle.window).ok(),
        RawWindowHandle::Xcb(handle) => Some(handle.window.get()),
        _ => None,
    }
}

#[cfg(all(unix, not(target_os = "macos")))]
mod x11 {
    use super::*;
    use std::time::{Duration, Instant};
    use x11rb::connection::Connection;
    use x11rb::protocol::xproto::{
        Atom, AtomEnum, ClientMessageEvent, ConfigureWindowAux, ConnectionExt as _,
        CreateWindowAux, EventMask, PropMode, SelectionNotifyEvent, SelectionRequestEvent,
        StackMode, Window, WindowClass, SELECTION_NOTIFY_EVENT,
    };
    use x11rb::protocol::Event;
    use x11rb::rust_connection::RustConnection;
    use x11rb::wrapper::ConnectionExt as _;

    const DRAG_PATH_LIMIT: usize = 1_024;
    const URI_LIST_BYTES_LIMIT: usize = 128 * 1024;
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

    #[derive(Default)]
    pub struct X11Drag {
        source: Option<X11DragSource>,
    }

    impl X11Drag {
        pub fn init_with_window_id(&mut self, app_window: u32) -> Result<(), String> {
            if self.source.is_some() {
                return Ok(());
            }
            self.source = Some(X11DragSource::new(app_window)?);
            Ok(())
        }

        pub fn is_active(&self) -> bool {
            self.source.as_ref().is_some_and(X11DragSource::is_active)
        }

        pub fn start(&mut self, path: PathBuf) -> Result<(), String> {
            let Some(source) = self.source.as_mut() else {
                return Err("X11 drag is unavailable on this display".to_string());
            };
            source.start(&[path])
        }

        pub fn update(&mut self, pointer_down: bool, pointer_released: bool) {
            if let Some(source) = self.source.as_mut() {
                source.update(pointer_down, pointer_released);
            }
        }
    }

    struct X11DragSource {
        connection: RustConnection,
        root: Window,
        app_window: Window,
        source_window: Window,
        atoms: Atoms,
        drag: Option<ActiveDrag>,
        failed_until_release: bool,
    }

    struct ActiveDrag {
        uri_list: Vec<u8>,
        target: Option<DragTarget>,
        accepted: bool,
        position_pending: bool,
        release_pending: bool,
        dropped: bool,
        deadline: Option<Instant>,
        timestamp: u32,
        owns_selection: bool,
    }

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    struct DragTarget {
        window: Window,
        recipient: Window,
        version: u32,
    }

    impl X11DragSource {
        /// Opens a dedicated X11 connection for XDND. Iced owns the window connection;
        /// a separate client is standard for drag sources and must target the same display.
        fn new(app_window: Window) -> Result<Self, String> {
            let (connection, screen_number) = x11rb::connect(None).map_err(display_error)?;
            let screen = &connection.setup().roots[screen_number];
            let root = screen.root;
            let source_window = connection.generate_id().map_err(display_error)?;
            connection
                .create_window(
                    screen.root_depth,
                    source_window,
                    root,
                    0,
                    0,
                    42,
                    28,
                    1,
                    WindowClass::INPUT_OUTPUT,
                    screen.root_visual,
                    &CreateWindowAux::new()
                        .override_redirect(1)
                        .background_pixel(screen.white_pixel)
                        .border_pixel(screen.black_pixel)
                        .event_mask(EventMask::PROPERTY_CHANGE),
                )
                .map_err(display_error)?
                .check()
                .map_err(display_error)?;
            let atoms = Atoms::new(&connection)
                .map_err(display_error)?
                .reply()
                .map_err(display_error)?;
            connection
                .change_property32(
                    PropMode::REPLACE,
                    source_window,
                    atoms._NET_WM_WINDOW_TYPE,
                    AtomEnum::ATOM,
                    &[atoms._NET_WM_WINDOW_TYPE_DND],
                )
                .map_err(display_error)?
                .check()
                .map_err(display_error)?;
            connection.flush().map_err(display_error)?;
            Ok(Self {
                connection,
                root,
                app_window,
                source_window,
                atoms,
                drag: None,
                failed_until_release: false,
            })
        }

        fn is_active(&self) -> bool {
            self.drag.is_some()
        }

        fn update(&mut self, pointer_down: bool, pointer_released: bool) {
            if self.poll_events().is_err() {
                self.reset(true);
                return;
            }
            if self
                .drag
                .as_ref()
                .is_some_and(|drag| drag.deadline.is_some_and(|time| time <= Instant::now()))
            {
                self.reset(true);
                return;
            }
            if self.failed_until_release {
                if !pointer_down {
                    self.failed_until_release = false;
                }
                return;
            }
            if self.drag.as_ref().is_some_and(|drag| drag.release_pending) {
                self.advance_release();
                return;
            }
            if self.drag.as_ref().is_some_and(|drag| drag.dropped) {
                return;
            }
            if self.drag.is_some() && (pointer_released || !pointer_down) {
                if let Some(drag) = self.drag.as_mut() {
                    drag.release_pending = true;
                    drag.deadline = Some(Instant::now() + STATUS_RESPONSE_TIMEOUT);
                }
                self.advance_release();
                return;
            }
            if self.drag.is_none() {
                return;
            }
            if self.update_target().is_err() {
                self.cancel();
            }
        }

        fn start(&mut self, paths: &[PathBuf]) -> Result<(), String> {
            if paths.is_empty() || paths.len() > DRAG_PATH_LIMIT {
                return Err(format!(
                    "drag must contain between 1 and {DRAG_PATH_LIMIT} paths"
                ));
            }
            if paths.iter().any(|path| !path.is_absolute()) {
                return Err("all drag paths must be absolute".to_string());
            }
            let paths = paths.to_vec();
            let uri_list = encode_uri_list(&paths)?;
            self.connection
                .change_property32(
                    PropMode::REPLACE,
                    self.source_window,
                    self.atoms.XdndTypeList,
                    AtomEnum::ATOM,
                    &[self.atoms.TextUriList],
                )
                .map_err(display_error)?
                .check()
                .map_err(display_error)?;
            self.connection
                .change_property32(
                    PropMode::REPLACE,
                    self.source_window,
                    self.atoms.XdndActionList,
                    AtomEnum::ATOM,
                    &[self.atoms.XdndActionCopy],
                )
                .map_err(display_error)?
                .check()
                .map_err(display_error)?;
            self.connection
                .set_selection_owner(
                    self.source_window,
                    self.atoms.XdndSelection,
                    x11rb::CURRENT_TIME,
                )
                .map_err(display_error)?
                .check()
                .map_err(display_error)?;
            let timestamp = self.server_timestamp()?;
            let owner = self
                .connection
                .get_selection_owner(self.atoms.XdndSelection)
                .map_err(display_error)?
                .reply()
                .map_err(display_error)?
                .owner;
            if owner != self.source_window {
                return Err("could not own the XDND selection".to_string());
            }
            self.drag = Some(ActiveDrag {
                uri_list,
                target: None,
                accepted: false,
                position_pending: false,
                release_pending: false,
                dropped: false,
                deadline: None,
                timestamp,
                owns_selection: true,
            });
            self.connection.flush().map_err(display_error)?;
            let pointer = self
                .connection
                .query_pointer(self.root)
                .map_err(display_error)?
                .reply()
                .map_err(display_error)?;
            self.move_icon(pointer.root_x, pointer.root_y)?;
            self.connection
                .map_window(self.source_window)
                .map_err(display_error)?
                .check()
                .map_err(display_error)?;
            self.connection.flush().map_err(display_error)?;
            Ok(())
        }

        fn server_timestamp(&self) -> Result<u32, String> {
            self.connection
                .change_property8(
                    PropMode::REPLACE,
                    self.source_window,
                    self.atoms.TUNDRA_DRAG_TIMESTAMP,
                    AtomEnum::INTEGER,
                    &[0],
                )
                .map_err(display_error)?
                .check()
                .map_err(display_error)?;
            self.connection.flush().map_err(display_error)?;
            for _ in 0..TIMESTAMP_POLL_LIMIT {
                let Some(event) = self.connection.poll_for_event().map_err(display_error)? else {
                    std::thread::sleep(Duration::from_millis(1));
                    continue;
                };
                if let Event::PropertyNotify(event) = event
                    && event.window == self.source_window
                    && event.atom == self.atoms.TUNDRA_DRAG_TIMESTAMP
                {
                    return Ok(event.time);
                }
            }
            Err("timed out waiting for X11 server timestamp".to_string())
        }

        fn pack_xdnd_coords(x: i16, y: i16) -> u32 {
            let x = i32::from(x) as u32;
            let y = i32::from(y) as u32;
            ((x & 0xFFFF) << 16) | (y & 0xFFFF)
        }

        fn update_target(&mut self) -> Result<bool, String> {
            let pointer = self
                .connection
                .query_pointer(self.root)
                .map_err(display_error)?
                .reply()
                .map_err(display_error)?;
            self.move_icon(pointer.root_x, pointer.root_y)?;
            let target = self.find_target(pointer.child)?;
            let previous = self.drag.as_ref().and_then(|drag| drag.target);
            if target != previous {
                if let Some(previous) = previous {
                    self.send_target(
                        previous,
                        self.atoms.XdndLeave,
                        [self.source_window, 0, 0, 0, 0],
                    )?;
                }
                if let Some(target) = target {
                    self.send_target(
                        target,
                        self.atoms.XdndEnter,
                        [
                            self.source_window,
                            target.version.min(5) << 24,
                            self.atoms.TextUriList,
                            0,
                            0,
                        ],
                    )?;
                }
                if let Some(drag) = self.drag.as_mut() {
                    drag.target = target;
                    drag.accepted = false;
                    drag.position_pending = false;
                }
            }
            let send_position = target.is_some()
                && self
                    .drag
                    .as_ref()
                    .is_some_and(|drag| !drag.position_pending);
            if let Some(target) = target.filter(|_| send_position) {
                let coordinates = Self::pack_xdnd_coords(pointer.root_x, pointer.root_y);
                let timestamp = self
                    .drag
                    .as_ref()
                    .map(|drag| drag.timestamp)
                    .unwrap_or(x11rb::CURRENT_TIME);
                self.send_target(
                    target,
                    self.atoms.XdndPosition,
                    [
                        self.source_window,
                        0,
                        coordinates,
                        timestamp,
                        self.atoms.XdndActionCopy,
                    ],
                )?;
                if let Some(drag) = self.drag.as_mut() {
                    drag.accepted = false;
                    drag.position_pending = true;
                }
                return Ok(true);
            }
            Ok(false)
        }

        fn move_icon(&self, root_x: i16, root_y: i16) -> Result<(), String> {
            self.connection
                .configure_window(
                    self.source_window,
                    &ConfigureWindowAux::new()
                        .x(i32::from(root_x) + 16)
                        .y(i32::from(root_y) + 16)
                        .stack_mode(StackMode::ABOVE),
                )
                .map_err(display_error)?;
            self.connection.flush().map_err(display_error)
        }

        fn advance_release(&mut self) {
            let Some(drag) = self.drag.as_ref() else {
                return;
            };
            if drag.position_pending {
                return;
            }
            if !self.initiate_drop() {
                self.failed_until_release = true;
            }
        }

        fn initiate_drop(&mut self) -> bool {
            let Some(drag) = self.drag.as_ref() else {
                return false;
            };
            let target = drag.target;
            let accepted = drag.accepted;
            let timestamp = drag.timestamp;
            if let Some(target) = target.filter(|_| accepted)
                && self
                    .send_target(
                        target,
                        self.atoms.XdndDrop,
                        [self.source_window, 0, timestamp, 0, 0],
                    )
                    .is_ok()
            {
                if let Some(drag) = self.drag.as_mut() {
                    drag.release_pending = false;
                    drag.dropped = true;
                    drag.deadline = Some(Instant::now() + DROP_RESPONSE_TIMEOUT);
                }
                return true;
            }
            self.cancel();
            false
        }

        fn cancel(&mut self) {
            if let Some(target) = self
                .drag
                .as_ref()
                .filter(|drag| !drag.dropped)
                .and_then(|drag| drag.target)
            {
                let _ = self.send_target(
                    target,
                    self.atoms.XdndLeave,
                    [self.source_window, 0, 0, 0, 0],
                );
            }
            self.reset(true);
        }

        fn reset(&mut self, release_selection: bool) {
            if release_selection
                && let Some(drag) = self.drag.as_ref().filter(|drag| drag.owns_selection)
            {
                let _ = self.connection.set_selection_owner(
                    x11rb::NONE,
                    self.atoms.XdndSelection,
                    drag.timestamp,
                );
                let _ = self.connection.flush();
            }
            let _ = self.connection.unmap_window(self.source_window);
            let _ = self.connection.flush();
            self.drag = None;
        }

        fn find_target(&self, child: Window) -> Result<Option<DragTarget>, String> {
            let mut current = if child == x11rb::NONE {
                self.root
            } else {
                child
            };
            for _ in 0..WINDOW_HIERARCHY_LIMIT {
                let reply = self
                    .connection
                    .query_pointer(current)
                    .map_err(display_error)?
                    .reply()
                    .map_err(display_error)?;
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
                let tree = self
                    .connection
                    .query_tree(current)
                    .map_err(display_error)?
                    .reply()
                    .map_err(display_error)?;
                if tree.parent == x11rb::NONE || tree.parent == current {
                    break;
                }
                current = tree.parent;
            }
            for window in ancestors {
                let recipient = self.xdnd_proxy(window)?.unwrap_or(window);
                if let Some(version) = self.xdnd_version(recipient)? {
                    return Ok(Some(DragTarget {
                        window,
                        recipient,
                        version,
                    }));
                }
            }
            Ok(None)
        }

        fn xdnd_version(&self, window: Window) -> Result<Option<u32>, String> {
            let property = self
                .connection
                .get_property(false, window, self.atoms.XdndAware, AtomEnum::ATOM, 0, 1)
                .map_err(display_error)?
                .reply()
                .map_err(display_error)?;
            if property.type_ != u32::from(AtomEnum::ATOM) || property.format != 32 {
                return Ok(None);
            }
            Ok(property
                .value32()
                .and_then(|mut values| values.next())
                .filter(|version| *version >= 3))
        }

        fn xdnd_proxy(&self, window: Window) -> Result<Option<Window>, String> {
            let proxy = self
                .connection
                .get_property(false, window, self.atoms.XdndProxy, AtomEnum::WINDOW, 0, 1)
                .map_err(display_error)?
                .reply()
                .map_err(display_error)?;
            if proxy.type_ != u32::from(AtomEnum::WINDOW) || proxy.format != 32 {
                return Ok(None);
            }
            let Some(proxy) = proxy.value32().and_then(|mut values| values.next()) else {
                return Ok(None);
            };
            let confirmation = self
                .connection
                .get_property(false, proxy, self.atoms.XdndProxy, AtomEnum::WINDOW, 0, 1)
                .map_err(display_error)?
                .reply()
                .map_err(display_error)?;
            Ok(
                (confirmation.type_ == u32::from(AtomEnum::WINDOW) && confirmation.format == 32)
                    .then(|| confirmation.value32().and_then(|mut values| values.next()))
                    .flatten()
                    .filter(|confirmed| *confirmed == proxy),
            )
        }

        fn poll_events(&mut self) -> Result<(), String> {
            for _ in 0..EVENT_LIMIT {
                let Some(event) = self.connection.poll_for_event().map_err(display_error)? else {
                    break;
                };
                match event {
                    Event::ClientMessage(event)
                        if event.type_ == self.atoms.XdndStatus
                            && event.format == 32
                            && event.window == self.source_window =>
                    {
                        let data = event.data.as_data32();
                        if let Some(drag) = self.drag.as_mut()
                            && drag.position_pending
                            && drag.target.is_some_and(|target| target.window == data[0])
                        {
                            drag.position_pending = false;
                            drag.accepted =
                                data[1] & 1 != 0 && data[4] == self.atoms.XdndActionCopy;
                        }
                    }
                    Event::ClientMessage(event)
                        if event.type_ == self.atoms.XdndFinished
                            && event.format == 32
                            && event.window == self.source_window =>
                    {
                        self.reset(true);
                    }
                    Event::SelectionRequest(event) => self.answer_selection_request(event)?,
                    Event::SelectionClear(event)
                        if event.selection == self.atoms.XdndSelection && self.drag.is_some() =>
                    {
                        self.reset(false);
                    }
                    _ => {}
                }
            }
            Ok(())
        }

        fn answer_selection_request(&self, request: SelectionRequestEvent) -> Result<(), String> {
            if request.owner != self.source_window || request.selection != self.atoms.XdndSelection
            {
                return Ok(());
            }
            let property = if request.property == x11rb::NONE {
                request.target
            } else {
                request.property
            };
            let active = self.drag.as_ref().filter(|drag| {
                drag.dropped
                    && drag.owns_selection
                    && timestamp_not_older(request.time, drag.timestamp)
            });
            let result = if request.target == self.atoms.TextUriList {
                active.map_or(Err(()), |drag| {
                    self.connection
                        .change_property8(
                            PropMode::REPLACE,
                            request.requestor,
                            property,
                            self.atoms.TextUriList,
                            &drag.uri_list,
                        )
                        .map_err(|_| ())?
                        .check()
                        .map_err(|_| ())
                })
            } else if request.target == self.atoms.TARGETS && active.is_some() {
                self.connection
                    .change_property32(
                        PropMode::REPLACE,
                        request.requestor,
                        property,
                        AtomEnum::ATOM,
                        &[self.atoms.TextUriList, self.atoms.TARGETS],
                    )
                    .map_err(|_| ())
                    .and_then(|cookie| cookie.check().map_err(|_| ()))
            } else {
                Err(())
            };
            let notify = SelectionNotifyEvent {
                response_type: SELECTION_NOTIFY_EVENT,
                sequence: 0,
                time: request.time,
                requestor: request.requestor,
                selection: request.selection,
                target: request.target,
                property: if result.is_ok() {
                    property
                } else {
                    x11rb::NONE
                },
            };
            self.connection
                .send_event(false, request.requestor, EventMask::NO_EVENT, notify)
                .map_err(display_error)?
                .check()
                .map_err(display_error)?;
            self.connection.flush().map_err(display_error)
        }

        fn send_target(
            &self,
            target: DragTarget,
            message_type: Atom,
            data: [u32; 5],
        ) -> Result<(), String> {
            let event = ClientMessageEvent::new(32, target.window, message_type, data);
            self.connection
                .send_event(false, target.recipient, EventMask::NO_EVENT, event)
                .map_err(display_error)?
                .check()
                .map_err(display_error)?;
            self.connection.flush().map_err(display_error)
        }
    }

    impl Drop for X11DragSource {
        fn drop(&mut self) {
            self.cancel();
            let _ = self.connection.destroy_window(self.source_window);
            let _ = self.connection.flush();
        }
    }

    fn encode_uri_list(paths: &[PathBuf]) -> Result<Vec<u8>, String> {
        if paths.is_empty() || paths.len() > DRAG_PATH_LIMIT {
            return Err(format!(
                "drag must contain between 1 and {DRAG_PATH_LIMIT} paths"
            ));
        }
        let mut output = Vec::new();
        for path in paths {
            let uri = file_uri(path)?;
            if output.len().saturating_add(uri.len()).saturating_add(2) > URI_LIST_BYTES_LIMIT {
                return Err("drag URI list exceeds the supported size".to_string());
            }
            output.extend_from_slice(uri.as_bytes());
            output.extend_from_slice(b"\r\n");
        }
        Ok(output)
    }

    fn timestamp_not_older(candidate: u32, reference: u32) -> bool {
        candidate == x11rb::CURRENT_TIME || candidate.wrapping_sub(reference) < (1_u32 << 31)
    }

    fn file_uri(path: &Path) -> Result<String, String> {
        if !path.is_absolute() {
            return Err("drag path must be absolute".to_string());
        }
        // Percent-encode non-unreserved bytes for file:// URIs (RFC 8089).
        use std::os::unix::ffi::OsStrExt;
        let bytes = path.as_os_str().as_bytes();
        let mut uri = String::from("file://");
        for byte in bytes {
            if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~' | b'/') {
                uri.push(char::from(*byte));
            } else {
                const HEX: &[u8; 16] = b"0123456789ABCDEF";
                uri.push('%');
                uri.push(char::from(HEX[usize::from(byte >> 4)]));
                uri.push(char::from(HEX[usize::from(byte & 0x0f)]));
            }
        }
        Ok(uri)
    }

    fn display_error(error: impl std::fmt::Display) -> String {
        error.to_string()
    }
}

#[cfg(all(unix, not(target_os = "macos")))]
pub use x11::X11Drag;

#[cfg(all(unix, not(target_os = "macos")))]
pub struct NativeDrag {
    x11: X11Drag,
}

#[cfg(not(all(unix, not(target_os = "macos"))))]
pub struct NativeDrag;

impl Default for NativeDrag {
    fn default() -> Self {
        Self::new()
    }
}

impl NativeDrag {
    pub fn new() -> Self {
        #[cfg(all(unix, not(target_os = "macos")))]
        {
            Self { x11: X11Drag::default() }
        }
        #[cfg(not(all(unix, not(target_os = "macos"))))]
        {
            Self
        }
    }

    pub fn init_with_window_id(&mut self, app_window: u32) -> Result<(), String> {
        #[cfg(all(unix, not(target_os = "macos")))]
        {
            self.x11.init_with_window_id(app_window)
        }
        #[cfg(not(all(unix, not(target_os = "macos"))))]
        {
            let _ = app_window;
            Ok(())
        }
    }

    pub fn is_active(&self) -> bool {
        #[cfg(all(unix, not(target_os = "macos")))]
        {
            self.x11.is_active()
        }
        #[cfg(not(all(unix, not(target_os = "macos"))))]
        {
            false
        }
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    pub fn start(&mut self, path: PathBuf) -> Result<(), String> {
        self.x11.start(path)
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    pub fn update(&mut self, pointer_down: bool, pointer_released: bool) {
        self.x11.update(pointer_down, pointer_released);
    }
}
