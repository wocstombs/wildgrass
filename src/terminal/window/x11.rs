//! X11 window management using EWMH protocol messages via `x11rb`.

use x11rb::connection::Connection;
use x11rb::errors::ReplyError;
use x11rb::protocol::xproto::{
    AtomEnum, ClientMessageEvent, ConfigureWindowAux, ConnectionExt as _, EventMask,
    Window as XWindow,
};
use x11rb::rust_connection::RustConnection;

use std::env;

use super::action::{Direction, Layer};
use super::geometry::{self, Insets, Movement, Rect, Screen, STEP};

/// Checks if the desktop environment is a native X11 session (not Wayland/XWayland).
pub(super) fn is_x11_session() -> bool {
    let set = |name| env::var_os(name).is_some_and(|value| !value.is_empty());
    set("DISPLAY")
        && !set("WAYLAND_DISPLAY")
        && env::var("XDG_SESSION_TYPE").map_or(true, |kind| kind != "wayland")
}

x11rb::atom_manager! {
    /// Extended Window Manager Hints (EWMH) atom manager.
    Atoms: AtomsCookie {
        _NET_SUPPORTED,
        _NET_ACTIVE_WINDOW,
        _NET_WORKAREA,
        _NET_CURRENT_DESKTOP,
        _NET_FRAME_EXTENTS,
        _GTK_FRAME_EXTENTS,
        _NET_MOVERESIZE_WINDOW,
        _NET_WM_STATE,
        _NET_WM_STATE_ABOVE,
        _NET_WM_STATE_BELOW,
        _NET_WM_STATE_MAXIMIZED_VERT,
        _NET_WM_STATE_MAXIMIZED_HORZ,
    }
}

/// EWMH `_NET_WM_STATE` action constants.
const STATE_REMOVE: u32 = 0;
const STATE_ADD: u32 = 1;
/// Pager client type indicator for EWMH messages.
const SOURCE_PAGER: u32 = 2;
/// `_NET_MOVERESIZE_WINDOW` flags: Static gravity with specified x and y coordinates.
const MOVE_FLAGS: u32 = 10 | (1 << 8) | (1 << 9) | (SOURCE_PAGER << 12);

/// X11 connection and window management state.
pub(super) struct X11 {
    connection: RustConnection,
    root: XWindow,
    atoms: Atoms,
    /// The whole screen, for when the monitors cannot be listed.
    whole_screen: Rect,
    /// One step in pixels, scaled by the desktop's `Xft.dpi`.
    step: i32,
    /// Whether the window manager takes `_NET_MOVERESIZE_WINDOW`.
    moveresize: bool,
    /// The window put above or below the others, so that it can be put back wherever the
    /// focus is by then.
    layered: Option<XWindow>,
}

impl X11 {
    pub(super) fn connect() -> Option<Self> {
        let (connection, screen_number) = x11rb::connect(None).ok()?;
        let screen = connection.setup().roots.get(screen_number)?;
        let root = screen.root;
        let whole_screen = Rect {
            x: 0,
            y: 0,
            width: screen.width_in_pixels.into(),
            height: screen.height_in_pixels.into(),
        };
        let atoms = Atoms::new(&connection).ok()?.reply().ok()?;
        let supported = property_values(
            &connection,
            root,
            atoms._NET_SUPPORTED,
            AtomEnum::ATOM,
            1024,
        )
        .unwrap_or_default();
        let dpi = connection
            .get_property(
                false,
                root,
                AtomEnum::RESOURCE_MANAGER,
                AtomEnum::STRING,
                0,
                1 << 16,
            )
            .ok()
            .and_then(|cookie| cookie.reply().ok())
            .and_then(|reply| xft_dpi(&String::from_utf8_lossy(&reply.value)))
            .unwrap_or(96.0);
        Some(Self {
            moveresize: supported.contains(&atoms._NET_MOVERESIZE_WINDOW),
            step: step_for_dpi(dpi),
            connection,
            root,
            atoms,
            whole_screen,
            layered: None,
        })
    }

    /// Moves the focused window one step in `direction`, or un-maximizes it if it is maximized.
    pub(super) fn move_window(&self, direction: Direction) -> Result<(), ReplyError> {
        let Some(window) = self.active_window()? else {
            return Ok(());
        };
        let atoms = &self.atoms;
        let state = property_values(
            &self.connection,
            window,
            atoms._NET_WM_STATE,
            AtomEnum::ATOM,
            64,
        )?;
        let (maximized_vertically, maximized_horizontally) = (
            atoms._NET_WM_STATE_MAXIMIZED_VERT,
            atoms._NET_WM_STATE_MAXIMIZED_HORZ,
        );
        if state.contains(&maximized_vertically) || state.contains(&maximized_horizontally) {
            // The window manager resizes it later, so this press only un-maximizes it
            self.change_state(
                window,
                STATE_REMOVE,
                maximized_vertically,
                maximized_horizontally,
            )?;
            self.connection.flush()?;
            return Ok(());
        }

        let window_geometry = self.connection.get_geometry(window)?.reply()?;
        let origin = self
            .connection
            .translate_coordinates(window, self.root, 0, 0)?
            .reply()?;
        let client = Rect {
            x: origin.dst_x.into(),
            y: origin.dst_y.into(),
            width: window_geometry.width.into(),
            height: window_geometry.height.into(),
        };
        // With the title bar and borders the window manager draws around it
        let frame = client.grown(self.extents(window, atoms._NET_FRAME_EXTENTS)?);
        // Without the invisible shadow margins of a window that draws its own title bar
        let visible = frame.shrunk(self.extents(window, atoms._GTK_FRAME_EXTENTS)?);
        let movement = geometry::step_within(visible, direction, self.step, &self.screens()?);
        if movement == Movement::NONE {
            return Ok(());
        }

        if self.moveresize {
            // Static gravity places the window itself, so it does not matter how the window
            // manager counts its frame (WSLg's counts an invisible shadow it then leaves out)
            let (x, y) = (client.x + movement.right, client.y + movement.down);
            self.send_to_root(
                window,
                atoms._NET_MOVERESIZE_WINDOW,
                [MOVE_FLAGS, x as u32, y as u32, 0, 0],
            )?;
        } else {
            // Without a gravity of its own, a window's requested position is its frame's
            let (x, y) = (frame.x + movement.right, frame.y + movement.down);
            self.connection
                .configure_window(window, &ConfigureWindowAux::new().x(x).y(y))?;
        }
        self.connection.flush()?;
        Ok(())
    }

    /// Puts the focused window above or below the others, or back to normal. A window this put
    /// above or below before goes back first, wherever the focus is by now, so `Normal` acts on
    /// that window and only that one.
    pub(super) fn set_layer(&mut self, layer: Layer) -> Result<(), ReplyError> {
        let (above, below) = (
            self.atoms._NET_WM_STATE_ABOVE,
            self.atoms._NET_WM_STATE_BELOW,
        );
        // The state to add, and the opposite one to take off
        let states = match layer {
            Layer::Normal => None,
            Layer::Top => Some((above, below)),
            Layer::Bottom => Some((below, above)),
        };
        // The focused window to put there; with none, everything stays as it is
        let wanted = match states {
            None => None,
            Some((add, remove)) => match self.active_window()? {
                Some(window) => Some((window, add, remove)),
                None => return Ok(()),
            },
        };
        // A window layered before goes back, unless it is the one to layer now
        let put_back = self
            .layered
            .take()
            .filter(|&earlier| wanted.is_none_or(|(window, ..)| window != earlier));
        if let Some(earlier) = put_back {
            self.change_state(earlier, STATE_REMOVE, above, below)?;
        }
        if let Some((window, add, remove)) = wanted {
            self.change_state(window, STATE_REMOVE, remove, x11rb::NONE)?;
            self.change_state(window, STATE_ADD, add, x11rb::NONE)?;
            self.layered = Some(window);
        }
        self.connection.flush()?;
        Ok(())
    }

    /// The window that has the focus: wildgrass's whenever a key reaches it.
    fn active_window(&self) -> Result<Option<XWindow>, ReplyError> {
        let ids = property_values(
            &self.connection,
            self.root,
            self.atoms._NET_ACTIVE_WINDOW,
            AtomEnum::WINDOW,
            1,
        )?;
        Ok(ids.first().copied().filter(|&id| id != x11rb::NONE))
    }

    /// The monitors with the part of each that panels and docks leave free, or the whole
    /// screen when the monitors cannot be listed.
    fn screens(&self) -> Result<Vec<Screen>, ReplyError> {
        use x11rb::protocol::randr::ConnectionExt as _;

        let mut monitors: Vec<Rect> = self
            .connection
            .randr_get_monitors(self.root, true)
            .ok()
            .and_then(|cookie| cookie.reply().ok())
            .map(|reply| {
                reply
                    .monitors
                    .iter()
                    .map(|monitor| Rect {
                        x: monitor.x.into(),
                        y: monitor.y.into(),
                        width: monitor.width.into(),
                        height: monitor.height.into(),
                    })
                    .collect()
            })
            .unwrap_or_default();
        if monitors.is_empty() {
            monitors.push(self.whole_screen);
        }
        let work_area = self.work_area()?;
        Ok(monitors
            .into_iter()
            .map(|bounds| Screen {
                bounds,
                work: work_area
                    .and_then(|area| bounds.intersection(area))
                    .unwrap_or(bounds),
            })
            .collect())
    }

    /// `_NET_WORKAREA` of the current desktop: the screen without panels and docks. It is one
    /// rectangle around all monitors, so each monitor's share is where the two overlap.
    fn work_area(&self) -> Result<Option<Rect>, ReplyError> {
        let (connection, root, atoms) = (&self.connection, self.root, &self.atoms);
        let areas = property_values(
            connection,
            root,
            atoms._NET_WORKAREA,
            AtomEnum::CARDINAL,
            4 * 64,
        )?;
        let desktop = property_values(
            connection,
            root,
            atoms._NET_CURRENT_DESKTOP,
            AtomEnum::CARDINAL,
            1,
        )?;
        let desktop = desktop.first().map_or(0, |&desktop| desktop as usize);
        // x, y, width and height for each desktop in turn
        let (areas, _) = areas.as_chunks::<4>();
        let area = areas.get(desktop).or(areas.first());
        Ok(area.map(|&[x, y, width, height]| Rect {
            x: x as i32,
            y: y as i32,
            width: width as i32,
            height: height as i32,
        }))
    }

    /// `_NET_FRAME_EXTENTS` or `_GTK_FRAME_EXTENTS` of `window`; none when it is not set.
    fn extents(&self, window: XWindow, property: u32) -> Result<Insets, ReplyError> {
        let values = property_values(&self.connection, window, property, AtomEnum::CARDINAL, 4)?;
        Ok(match values[..] {
            [left, right, top, bottom] => Insets {
                left: left as i32,
                right: right as i32,
                top: top as i32,
                bottom: bottom as i32,
            },
            _ => Insets::default(),
        })
    }

    /// Asks the window manager to add or remove one or two `_NET_WM_STATE` states of `window`.
    fn change_state(
        &self,
        window: XWindow,
        action: u32,
        first: u32,
        second: u32,
    ) -> Result<(), ReplyError> {
        self.send_to_root(
            window,
            self.atoms._NET_WM_STATE,
            [action, first, second, SOURCE_PAGER, 0],
        )
    }

    /// Sends the window manager a request about `window`, the way EWMH says to: as a message
    /// to the root window.
    fn send_to_root(
        &self,
        window: XWindow,
        message: u32,
        data: [u32; 5],
    ) -> Result<(), ReplyError> {
        let event = ClientMessageEvent::new(32, window, message, data);
        self.connection.send_event(
            false,
            self.root,
            EventMask::SUBSTRUCTURE_REDIRECT | EventMask::SUBSTRUCTURE_NOTIFY,
            event,
        )?;
        Ok(())
    }
}

/// The 32-bit values of `window`'s `property` (of type `kind`), at most `max`; none when it is
/// not set.
fn property_values(
    connection: &RustConnection,
    window: XWindow,
    property: u32,
    kind: AtomEnum,
    max: u32,
) -> Result<Vec<u32>, ReplyError> {
    let reply = connection
        .get_property(false, window, property, kind, 0, max)?
        .reply()?;
    Ok(reply
        .value32()
        .map(|values| values.collect())
        .unwrap_or_default())
}

/// `Xft.dpi` from the X resources (`RESOURCE_MANAGER`), which desktops set for high-resolution
/// screens.
fn xft_dpi(resources: &str) -> Option<f64> {
    resources
        .lines()
        .find_map(|line| line.strip_prefix("Xft.dpi:"))
        .and_then(|value| value.trim().parse().ok())
        .filter(|&dpi| dpi > 0.0)
}

/// One step in pixels at `dpi`.
fn step_for_dpi(dpi: f64) -> i32 {
    ((f64::from(STEP) * dpi / 96.0).round() as i32).max(1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_xft_dpi_is_read_from_the_x_resources() {
        assert_eq!(xft_dpi("Xft.antialias:\t1\nXft.dpi:\t192\n"), Some(192.0));
        assert_eq!(xft_dpi("Xft.dpi: 96"), Some(96.0));
        assert_eq!(xft_dpi("Xft.antialias:\t1\n"), None);
        assert_eq!(xft_dpi("Xft.dpi:\tlots\n"), None);
        assert_eq!(step_for_dpi(96.0), 50);
        assert_eq!(step_for_dpi(192.0), 100);
        assert_eq!(step_for_dpi(144.0), 75);
    }
}
