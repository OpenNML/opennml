//! Named view rectangles, clearing "explored" and the build button lock.
//!
//! - Action 20 (`GameWorld_ClearExploredArea(p, name, reveal = 1, create = 1)`, `0x00487440`, via
//!   `FUN_0042D950`): appends `{name, rectangle of the area, p, clear: true}` to [`crate::fog::Fog::area_views`],
//!   only for `0 <= p < 8` and an existing area, without a duplicate check. Action 24 (`FUN_00484A10`) removes
//!   the **first** entry with the same slot and name, without any check. Stamping happens in the fog cycle
//!   ([`crate::fog::cycle`]).
//! - Action 372 (`GameWorld_ClearExploredRect`, `0x00479270`): clear "explored" of a rectangle, corners
//!   swapped, both bounds included; action 347 (`ClearExploredArea(..., 1, 0)`, branch "do not create",
//!   `0x00487559`): only for `0 <= p < 8`, without swapping, `z` half-open.
//! - Actions 207/208 (`FUN_00482240`, `0x00482240`): set the entry in [`crate::state::SimState::button_locks`]
//!   (1/0), global for all slots. It is read through [`crate::world::is_guid_disabled`].
//!
//! Source: re/vision 3.4.

use crate::fog::AreaView;
use crate::id::PLAYER_SLOTS;
use crate::sim::Sim;

use super::super::{area, area_key};

/// Slot `0...7` or nothing (`0 <= p < 8`).
fn player_slot(p: i32) -> Option<u8> {
    u8::try_from(p)
        .ok()
        .filter(|&p| usize::from(p) < PLAYER_SLOTS)
}

/// Action 20: create a view rectangle. Name lowercased and trimmed (`FUN_00435A60`); rectangle like the
/// area at run time ([`area`], changes of the savegame first), unnormalized. An empty name would yield an
/// entry that the load check used to reject; an area without a name does not exist.
pub fn add_area_view(sim: &mut Sim, p: i32, name: &str) {
    let Some(player) = player_slot(p) else {
        return;
    };
    let Some(rect) = area(sim, name) else {
        return;
    };
    let key = area_key(name);
    if key.is_empty() {
        return;
    }
    sim.state.fog.area_views.push(AreaView {
        name: key,
        rect,
        player,
        clear: true,
    });
}

/// Action 24 (`FUN_00484A10`): remove the first entry with slot `p` and the same (lowercased) name;
/// no range, slot or existence check.
pub fn remove_area_view(sim: &mut Sim, p: i32, name: &str) {
    let key = area_key(name);
    let views = &mut sim.state.fog.area_views;
    if let Some(i) = views
        .iter()
        .position(|v| i32::from(v.player) == p && v.name == key)
    {
        views.remove(i);
    }
}

/// Action 372 (`GameWorld_ClearExploredRect`, `0x00479270`): clear bit `p` in "explored" over the rectangle
/// of the area. Corners swapped, `x` and `z` each **included**, only tiles inside the grid. The bit mask is
/// `MOV DL,1; SHL DL,CL` with `CL = p`: the counter has 5 bits, an 8-bit value becomes 0 for counter 8...31 -
/// slots 8...31 and negative ones (-1 -> 31) clear nothing, 32...39 act like 0...7. No range or slot check.
pub fn clear_explored(sim: &mut Sim, p: i32, name: &str) {
    let Some([x0, z0, x1, z1]) = area(sim, name) else {
        return;
    };
    let shift = (p & 31) as u32;
    let mask: u8 = if shift < 8 { 1 << shift } else { 0 };
    let (xs, xe) = (x0.min(x1), x0.max(x1));
    let (zs, ze) = (z0.min(z1), z0.max(z1));
    clear_rect(sim, mask, xs, xe, zs, ze);
}

/// Action 347 (`ClearExploredArea(p, name, 1, 0)`, branch "do not create", `0x00487559...0x004875AF`): only
/// for `0 <= p < 8`. **No** swapping of corners; `x = l...r` included, `z = t...b-1` **excluded** - the bottom
/// row of the area stays explored (E-201); for `l > r` or `t >= b` nothing. Only "explored".
pub fn clear_explored_open(sim: &mut Sim, p: i32, name: &str) {
    let Some(player) = player_slot(p) else {
        return;
    };
    let Some([x0, z0, x1, z1]) = area(sim, name) else {
        return;
    };
    if z1 <= z0 {
        return;
    }
    clear_rect(sim, 1 << player, x0, x1, z0, z1 - 1);
}

/// `&= !mask` on "explored" over `x in xs...=xe`, `z in zs...=ze`, clipped to the grid (the original checks
/// tiles outside per tile and skips them).
fn clear_rect(sim: &mut Sim, mask: u8, xs: i32, xe: i32, zs: i32, ze: i32) {
    if mask == 0 {
        return;
    }
    let grid = &mut sim.state.fog.explored;
    let (xs, xe) = (xs.max(0), xe.min(grid.width - 1));
    let (zs, ze) = (zs.max(0), ze.min(grid.height - 1));
    for z in zs..=ze {
        for x in xs..=xe {
            grid.clear_bits(x, z, mask);
        }
    }
}

/// Actions 207 (`locked`) and 208: `m_ButtonLocked[guid] = 1` and `0`. The entry is always written
/// (`operator[]` creates it); the original does not need to redraw the command bar - it reads the state on every
/// query anyway. 208 never lifts a level-data lock ([`crate::world::is_guid_disabled`]).
pub fn lock_button(sim: &mut Sim, guid: u32, locked: bool) {
    sim.state.button_locks.insert(guid, u8::from(locked));
}
