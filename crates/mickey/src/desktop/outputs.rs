//! This PC's monitors as Wayland itself describes them (xdg-output): each
//! output's name and its place and size in the logical layout the cursor
//! moves in. For desktops where neither a portal nor the compositor's own
//! IPC names them, such as COSMIC without RemoteDesktop (#429).

use crate::monitors::Monitor;
use wayland_client::protocol::{wl_output::WlOutput, wl_registry};
use wayland_client::{Connection, Dispatch, QueueHandle};
use wayland_protocols::xdg::xdg_output::zv1::client::{
    zxdg_output_manager_v1::ZxdgOutputManagerV1,
    zxdg_output_v1::{self, ZxdgOutputV1},
};

/// What one output said about itself so far.
#[derive(Debug, Default, Clone, PartialEq)]
struct Seen {
    name: Option<String>,
    at: Option<(i32, i32)>,
    size: Option<(i32, i32)>,
}

#[derive(Default)]
struct Outputs {
    manager: Option<ZxdgOutputManagerV1>,
    outputs: Vec<WlOutput>,
    seen: Vec<Seen>,
}

/// The outputs with their names, in the logical layout. An error when the
/// session is not Wayland or the compositor has no xdg-output.
pub fn outputs() -> Result<Vec<(String, Monitor)>, String> {
    let connection = Connection::connect_to_env().map_err(|err| err.to_string())?;
    let mut queue = connection.new_event_queue();
    let handle = queue.handle();
    connection.display().get_registry(&handle, ());
    let mut state = Outputs::default();
    queue.roundtrip(&mut state).map_err(|err| err.to_string())?;
    let manager = state
        .manager
        .clone()
        .ok_or("this compositor has no xdg-output")?;
    state.seen = vec![Seen::default(); state.outputs.len()];
    for (index, output) in state.outputs.iter().enumerate() {
        manager.get_xdg_output(output, &handle, index);
    }
    queue.roundtrip(&mut state).map_err(|err| err.to_string())?;
    Ok(complete(&state.seen))
}

/// The outputs that said everything Mickey needs; one that did not (being
/// set up, or turned off) is left out.
fn complete(seen: &[Seen]) -> Vec<(String, Monitor)> {
    seen.iter()
        .filter_map(|output| {
            let ((x, y), (width, height)) = (output.at?, output.size?);
            (width > 0 && height > 0).then(|| {
                (
                    output.name.clone().unwrap_or_default(),
                    Monitor {
                        x,
                        y,
                        width,
                        height,
                    },
                )
            })
        })
        .collect()
}

impl Dispatch<wl_registry::WlRegistry, ()> for Outputs {
    fn event(
        state: &mut Self,
        registry: &wl_registry::WlRegistry,
        event: wl_registry::Event,
        _: &(),
        _: &Connection,
        handle: &QueueHandle<Self>,
    ) {
        if let wl_registry::Event::Global {
            name,
            interface,
            version,
        } = event
        {
            match interface.as_str() {
                "wl_output" => state.outputs.push(registry.bind(name, 1, handle, ())),
                // Version 2 adds the output's name.
                "zxdg_output_manager_v1" => {
                    state.manager = Some(registry.bind(name, version.min(3), handle, ()));
                }
                _ => {}
            }
        }
    }
}

impl Dispatch<ZxdgOutputV1, usize> for Outputs {
    fn event(
        state: &mut Self,
        _: &ZxdgOutputV1,
        event: zxdg_output_v1::Event,
        index: &usize,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let Some(seen) = state.seen.get_mut(*index) else {
            return;
        };
        match event {
            zxdg_output_v1::Event::LogicalPosition { x, y } => seen.at = Some((x, y)),
            zxdg_output_v1::Event::LogicalSize { width, height } => {
                seen.size = Some((width, height))
            }
            zxdg_output_v1::Event::Name { name } => seen.name = Some(name),
            _ => {}
        }
    }
}

wayland_client::delegate_noop!(Outputs: ignore WlOutput);
wayland_client::delegate_noop!(Outputs: ignore ZxdgOutputManagerV1);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn outputs_that_said_where_and_how_big_are_monitors() {
        let seen = [
            Seen {
                name: Some("DP-1".into()),
                at: Some((0, 0)),
                size: Some((2560, 1080)),
            },
            Seen {
                name: Some("HDMI-A-1".into()),
                at: Some((2560, -840)),
                size: Some((1080, 1920)),
            },
            // Still being set up: no size yet.
            Seen {
                name: Some("eDP-1".into()),
                at: Some((0, 1080)),
                size: None,
            },
        ];
        assert_eq!(
            complete(&seen),
            [
                (
                    "DP-1".to_string(),
                    Monitor {
                        x: 0,
                        y: 0,
                        width: 2560,
                        height: 1080
                    }
                ),
                (
                    "HDMI-A-1".to_string(),
                    Monitor {
                        x: 2560,
                        y: -840,
                        width: 1080,
                        height: 1920
                    }
                ),
            ]
        );
    }
}
