//! What a widget does with one frame's input.
//!
//! The input is the terminal's own [`Event`]s, drained once by the caller: there is no second
//! vocabulary here, only the four questions a widget asks of that list. A widget takes the
//! events it acts on and leaves the rest, so the same list can be offered to several widgets in
//! turn and whatever is left over afterwards is what nobody wanted.

use ratatui::layout::{Position, Rect};
use termina::event::{Event, KeyEvent, MouseButton, MouseEvent, MouseEventKind};

/// The pointer's last event, if this frame has one.
fn pointer(events: &[Event]) -> Option<MouseEvent> {
    events.iter().rev().find_map(|event| match event {
        Event::Mouse(mouse) => Some(*mouse),
        _ => None,
    })
}

/// Where the pointer is, if it said anything this frame.
///
/// `None` is not "the pointer left": a frame with no mouse event is a frame in which the pointer
/// did not move, so a widget must keep what it last heard rather than treat the silence as an exit.
pub fn pointer_at(events: &[Event]) -> Option<Position> {
    pointer(events).map(|mouse| Position::new(mouse.column, mouse.row))
}

/// Take the left press inside `area`.
///
/// Taking it is what stops the press reaching a widget further down: the widget that was
/// pressed is the one that acted.
pub fn take_press(events: &mut Vec<Event>, area: Rect) -> Option<Position> {
    let at = events.iter().position(|event| match event {
        Event::Mouse(mouse) => {
            matches!(mouse.kind, MouseEventKind::Down(MouseButton::Left))
                && area.contains(Position::new(mouse.column, mouse.row))
        }
        _ => false,
    })?;
    let Event::Mouse(mouse) = events.remove(at) else {
        return None;
    };
    Some(Position::new(mouse.column, mouse.row))
}

/// Take the release of the left button, wherever it happened.
///
/// Not limited to an area: a press that is dragged off a widget still ends when the button is
/// let go, and a widget that only heard about releases inside its own box would stay held down
/// forever.
pub fn take_release(events: &mut Vec<Event>) -> Option<Position> {
    let at = events.iter().position(|event| match event {
        Event::Mouse(mouse) => matches!(mouse.kind, MouseEventKind::Up(MouseButton::Left)),
        _ => false,
    })?;
    let Event::Mouse(mouse) = events.remove(at) else {
        return None;
    };
    Some(Position::new(mouse.column, mouse.row))
}

/// Take the first keypress `want` accepts, leaving the others for whoever looks next.
pub fn take_key(events: &mut Vec<Event>, want: impl Fn(&KeyEvent) -> bool) -> Option<KeyEvent> {
    let at = events.iter().position(|event| match event {
        Event::Key(key) => want(key),
        _ => false,
    })?;
    let Event::Key(key) = events.remove(at) else {
        return None;
    };
    Some(key)
}
