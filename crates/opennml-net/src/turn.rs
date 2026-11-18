//! Turn raster in ticks.
//!
//! Turn spacing `T` (`spacing`) and lead `V` (`lead`) in ticks, `1 <= V < T <= 40`: turn start
//! `b(k) = k·T`, execution tick `e(k) = b(k) + V`. The original works with `T = 8`, `V = 4` (`L = 2`, `T = 4·L`,
//! `V = 2·L`; re/multiplayer M.5). The stages [`RasterStage`] and the command line also choose finer
//! grids; execution ticks are then no longer necessarily even (the simulation accepts commands on every tick).

/// Grid of a match.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TurnConfig {
    /// Private: only the constructors and `Default` build a value, so that `T` is never 0 (division in `round_at`).
    spacing: u8,
    lead: u8,
}

impl Default for TurnConfig {
    fn default() -> Self {
        Self {
            spacing: 8,
            lead: 4,
        }
    }
}

impl TurnConfig {
    /// Turn spacing `t` and lead `v` in ticks: `1 <= v < t <= 40`.
    pub fn new(t: u8, v: u8) -> Result<Self, String> {
        if (1..t).contains(&v) && t <= 40 {
            Ok(Self {
                spacing: t,
                lead: v,
            })
        } else {
            Err(format!(
                "invalid turn raster {t}/{v}: lead at least 1 and smaller than the turn spacing, turn spacing at most 40"
            ))
        }
    }

    /// Old grid from the turn length `l` (1 ... 10): `T = 4·l`, `V = 2·l` (`--turn-steps`).
    pub fn from_turn_steps(l: u8) -> Result<Self, String> {
        if (1..=10).contains(&l) {
            Self::new(4 * l, 2 * l)
        } else {
            Err(format!("turn length {l} outside 1 to 10"))
        }
    }

    /// Raw values `(T, V)` for messages, savegame and status display.
    pub fn raw(&self) -> (u8, u8) {
        (self.spacing, self.lead)
    }

    /// Turn spacing `T` in ticks.
    pub fn spacing(&self) -> u64 {
        u64::from(self.spacing)
    }

    /// Lead `V` in ticks.
    pub fn lead(&self) -> u64 {
        u64::from(self.lead)
    }

    /// Closing deadline of the host in milliseconds: `max(3 000, (T + V)·50·20 / (game speed + 10) + 1 000)`.
    /// The deadline suffices for the grid, so that the last turns still arrive at a slow game speed.
    pub fn close_ms(&self, game_speed: u8) -> u64 {
        let ticks = u64::from(self.spacing) + u64::from(self.lead);
        (ticks * 50 * 20 / (u64::from(game_speed) + 10) + 1_000).max(3_000)
    }

    /// For how many turns after its execution a bundle is kept:
    /// `⌈(T + V) / T⌉ + 1`. For all grid stages and up to `T = 40` it is 3.
    pub fn kept_bundles(&self) -> u32 {
        let (t, v) = (u32::from(self.spacing), u32::from(self.lead));
        (t + v).div_ceil(t) + 1
    }

    /// Tick at which turn `k` begins.
    pub fn round_step(&self, k: u32) -> u64 {
        u64::from(k) * self.spacing()
    }

    /// Tick at which the commands of turn `k` are executed.
    pub fn exec_step(&self, k: u32) -> u64 {
        self.round_step(k) + self.lead()
    }

    /// Turn that begins at `step`.
    pub fn round_at(&self, step: u64) -> Option<u32> {
        if step.is_multiple_of(self.spacing()) {
            u32::try_from(step / self.spacing()).ok()
        } else {
            None
        }
    }

    /// Turn whose commands are executed at `step`.
    pub fn exec_at(&self, step: u64) -> Option<u32> {
        self.round_at(step.checked_sub(self.lead())?)
    }
}

/// Stage of the turn raster in `LAN3` (setting `NetTurnRaster`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RasterStage {
    /// 8/4: grid of the original.
    #[default]
    Original,
    /// 4/2: half grid.
    Lan,
    /// 2/1: finest grid.
    LanFast,
}

impl RasterStage {
    /// Grid of the stage.
    pub fn config(self) -> TurnConfig {
        let (t, v) = match self {
            Self::Original => (8, 4),
            Self::Lan => (4, 2),
            Self::LanFast => (2, 1),
        };
        TurnConfig {
            spacing: t,
            lead: v,
        }
    }

    /// Stage from the index of the setting (0/1/2, otherwise `Original`).
    pub fn from_index(i: i32) -> Self {
        match i {
            1 => Self::Lan,
            2 => Self::LanFast,
            _ => Self::Original,
        }
    }

    /// Index of the setting (0/1/2).
    pub fn index(self) -> i32 {
        match self {
            Self::Original => 0,
            Self::Lan => 1,
            Self::LanFast => 2,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // QUIRK E-255: deviation
    #[test]
    fn stages_have_the_planned_rasters() {
        assert_eq!(RasterStage::Original.config().raw(), (8, 4));
        assert_eq!(RasterStage::Lan.config().raw(), (4, 2));
        assert_eq!(RasterStage::LanFast.config().raw(), (2, 1));
        assert_eq!(RasterStage::default(), RasterStage::Original);
        assert_eq!(TurnConfig::default(), RasterStage::Original.config());
        for i in 0..3 {
            assert_eq!(RasterStage::from_index(i).index(), i);
        }
        assert_eq!(RasterStage::from_index(7), RasterStage::Original);
        assert_eq!(RasterStage::from_index(-1), RasterStage::Original);
    }

    // QUIRK E-255, E-298: deviation
    #[test]
    fn raster_bounds() {
        assert!(TurnConfig::new(2, 1).is_ok());
        assert!(TurnConfig::new(40, 39).is_ok());
        assert!(TurnConfig::new(2, 2).is_err());
        assert!(TurnConfig::new(1, 0).is_err());
        assert!(TurnConfig::new(41, 4).is_err());
        assert_eq!(
            TurnConfig::from_turn_steps(2).unwrap(),
            TurnConfig::new(8, 4).unwrap()
        );
        assert!(TurnConfig::from_turn_steps(0).is_err());
        assert!(TurnConfig::from_turn_steps(11).is_err());
        assert_eq!(TurnConfig::from_turn_steps(10).unwrap().raw(), (40, 20));
    }

    // QUIRK E-255: deviation
    #[test]
    fn fine_raster_executes_on_odd_steps() {
        let c = TurnConfig::new(2, 1).unwrap();
        assert_eq!(c.exec_step(0), 1);
        assert_eq!(c.exec_step(3), 7);
        assert_eq!(c.exec_at(7), Some(3));
        assert_eq!(c.exec_at(6), None);
    }

    #[test]
    fn close_ms_covers_the_raster() {
        assert_eq!(TurnConfig::default().close_ms(10), 3_000);
        assert_eq!(TurnConfig::new(40, 20).unwrap().close_ms(0), 7_000);
    }

    // QUIRK E-255: deviation
    #[test]
    fn default_raster_matches_the_original() {
        let c = TurnConfig::default();
        assert_eq!(c.exec_step(0), 4);
        assert_eq!(c.exec_step(1), 12);
        assert_eq!(c.round_step(2), 16);
        assert_eq!(c.exec_at(4), Some(0));
        assert_eq!(c.exec_at(5), None);
        assert_eq!(c.exec_at(3), None);
        assert_eq!(c.round_at(8), Some(1));
        assert_eq!(c.round_at(9), None);
        for l in 1..=10 {
            let c = TurnConfig::from_turn_steps(l).unwrap();
            for k in 0..50 {
                assert_eq!(c.exec_at(c.exec_step(k)), Some(k));
                assert_eq!(c.round_at(c.round_step(k)), Some(k));
            }
        }
        for (t, v) in [(2, 1), (3, 2), (7, 3)] {
            let c = TurnConfig::new(t, v).unwrap();
            for k in 0..50 {
                assert_eq!(c.exec_at(c.exec_step(k)), Some(k));
            }
        }
    }

    #[test]
    fn kept_bundles_cover_the_lead() {
        for (t, v) in [(8, 4), (4, 2), (2, 1), (40, 39)] {
            assert_eq!(TurnConfig::new(t, v).unwrap().kept_bundles(), 3, "{t}/{v}");
        }
        for t in 2..=40u8 {
            for v in 1..t {
                assert!(
                    TurnConfig::new(t, v).unwrap().kept_bundles() <= 3,
                    "{t}/{v}"
                );
            }
        }
    }
}
