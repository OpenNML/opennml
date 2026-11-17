//! Formation and dragging of a move command from the UI (re/groups-formations 2.2 and 12.6). Base module
//! without recourse to the unit bar; [`FormationOrder::new`] from the bar lives in `unit_bar.rs`.

use nml_sim::command::CommandKind;

/// Formation and dragging of a move command from the UI: the formation selected in the bar and
/// direction/spread from [`crate::game::unit_bar::formation_drag`]. Default: no formation,
/// no dragging (the command stays as it is).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct FormationOrder {
    pub formation: i32,
    pub drag: Option<(i32, i32)>,
}

impl FormationOrder {
    /// Move commands get formation and dragging ([`with_formation`]), all others do not.
    pub fn apply(self, kind: CommandKind) -> CommandKind {
        with_formation(kind, self.formation, self.drag)
    }
}

/// Move command with the selected formation and optionally direction and spread from
/// dragging; other commands stay unchanged.
pub fn with_formation(kind: CommandKind, formation: i32, drag: Option<(i32, i32)>) -> CommandKind {
    match kind {
        CommandKind::Move {
            units,
            x,
            z,
            target,
            direction,
            spread,
            flags,
            ..
        } => {
            let (direction, spread) = drag.unwrap_or((direction, spread));
            CommandKind::Move {
                units,
                x,
                z,
                target,
                formation,
                direction,
                spread,
                flags,
            }
        }
        other => other,
    }
}
