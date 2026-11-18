//! Player interface: [`View`] is the view of a slot onto the world, through which the AI queries (`ki::query`) and
//! later other players (bots) read. Two sight levels: [`Sight::Omniscient`] reads everything like the original's AI
//! (E-25), [`Sight::Fog`] only what the slot sees (reimplementation extension).
//!
//! **Read-only is the contract**: `View` holds `&mut Sim` because the building blocks (search, filters, object
//! messages) take `&mut Sim`; no method changes the state (checksum test `views_leave_the_checksum_unchanged`).
//! Object messages are restricted: [`View::message`] accepts only the query messages from [`QUERY_MESSAGES`] (checked
//! in the debug build). Quirks of individual request codes are not listed here but in the facade `ki::query`.
//!
//! Sight rule under [`Sight::Fog`] (`crate::filter::seen_by`): own objects always, foreign ones only if the slot's fog
//! shows them (state `& 2` at the tile, stealth like `MM_CHECKFOG`). Searches check sight per candidate in the
//! search, collections and counts filter their list afterwards. Foreign accounts, stocks, fog and reports are hidden
//! (`None` or 0). Slots, diplomacy, map and match, running counter-upgrades ([`View::counter_running`]) and the
//! offered headhunters ([`View::headhunter`]) stay public. The path queries ([`View::reachable_cell`],
//! [`View::free_cell`], [`View::tile_path_len`]) read the path grid, which unseen foreign buildings also block; they
//! therefore reveal these indirectly - acceptable, since the pathfinder plans the units of human players the same
//! way.
//!
//! Do not hold references from [`View::object`] and [`View::attack_reports`] across calls with `&mut self`
//! ([`View::message`], [`View::find_nearest`], [`View::matches`] ...): copy the needed fields beforehand.

use opennml_formats::targets::MatchFilter;
use serde::{Deserialize, Serialize};

use crate::command::{Command, Origin, Outcome, execute};
use crate::id::{ObjectId, PLAYER_SLOTS};
use crate::message::Message;
use crate::mfc::MfcMap;
use crate::object::GameObject;
use crate::rules::Rules;
use crate::sim::Sim;
use crate::state::{Control, ReportTile};

/// Object messages that [`View::message`] accepts: pure queries that only read (reply in the return value or in the
/// arguments). The list grows with every new query group; commands (e.g. `0x500` path, `0x600`) never belong in it.
pub const QUERY_MESSAGES: &[u32] = &[
    0x222,  // Headhunter: price, target player, target GUID, customer (`headhunter::query`).
    0x711,  // Attack module: attack mode (`a.mode`).
    0xC11, // Attack module: status (return value) and target ID (only into the message, `args[0]`).
    0xD11, // Attack module: greatest range (`max_reach`: `0xB13` to the ranged combat, read-only).
    0xF03, // Building: running order and progress into the message (`on_progress`).
    0xF11, // Attack module: stubbornness (`get_stubbornness`: `0x2400` to walk, read-only).
    0x1003, // Building: build list into the message (`on_list`).
    0x1400, // Walk: layer of the unit.
    0x1411, // Attack module: command target (only compares, C-11).
    0x1511, // Attack module: current target.
    0x2217, // Container: number of occupants (`OCCUPANT_COUNT`).
];

/// Sight level of a [`View`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Sight {
    /// Everything visible like the original's AI (E-25).
    Omniscient,
    /// Only what the slot sees; foreign accounts hidden.
    Fog,
}

/// Public information of a player slot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PlayerInfo {
    pub slot: u8,
    pub control: Control,
    pub nation: i32,
    pub color: i32,
    /// Slot present in the `.lvl` ([`crate::state::Player::slot_active`]).
    pub defined: bool,
}

/// View of slot `player` onto the world (read-only, see the module documentation).
pub struct View<'s, 'a> {
    sim: &'s mut Sim<'a>,
    player: u8,
    sight: Sight,
}

impl<'s, 'a> View<'s, 'a> {
    pub fn new(sim: &'s mut Sim<'a>, player: u8, sight: Sight) -> Self {
        Self { sim, player, sight }
    }

    pub fn player_slot(&self) -> u8 {
        self.player
    }

    pub fn sight(&self) -> Sight {
        self.sight
    }

    pub fn rules(&self) -> &'a Rules {
        self.sim.rules
    }

    /// Under [`Sight::Fog`] the own slot, otherwise `None` (sight check of the searches).
    fn seen_by(&self) -> Option<u8> {
        (self.sight == Sight::Fog).then_some(self.player)
    }

    /// Does the view see the (existing) object?
    fn sees(&self, id: ObjectId) -> bool {
        self.seen_by()
            .is_none_or(|p| crate::filter::seen_by(self.sim, id, p))
    }

    /// Foreign slot under [`Sight::Fog`]: account, stocks, fog and reports hidden.
    fn hides_account(&self, p: i32) -> bool {
        self.sight == Sight::Fog && p != i32::from(self.player)
    }

    /// `GameWorld_GetObjectById(id, alive_only)`: 0 and -1 are never an object (re/ki-queries 5.1); with `alive_only`
    /// without dying ones. Under [`Sight::Fog`] an unseen foreign one -> `None`.
    pub fn object(&self, id: ObjectId, alive_only: bool) -> Option<&GameObject> {
        if id.0 == 0 || id.is_none() || !self.sees(id) {
            return None;
        }
        self.sim.state.object(id, alive_only)
    }

    /// Query message to an object ([`crate::messaging::send_mut`], also finds dying objects): reply; queries also
    /// reply in `msg`. `None`: object missing or hidden.
    ///
    /// **Precondition:** `msg.code` is in [`QUERY_MESSAGES`] (only reading queries; the list grows with every new
    /// query group). Another message causes a panic in the debug build, in release the result is `None` (no message
    /// is sent).
    pub fn message(&mut self, id: ObjectId, msg: &mut Message) -> Option<i32> {
        if !QUERY_MESSAGES.contains(&msg.code) {
            debug_assert!(
                false,
                "View::message: {:#X} is not a query message",
                msg.code
            );
            return None;
        }
        self.object(id, false)?;
        Some(crate::messaging::send_mut(self.sim, id, msg))
    }

    /// Current attribute value ([`Sim::attribute`]); `None`: object missing or hidden.
    pub fn attribute(&self, id: ObjectId, attr: u32) -> Option<i32> {
        self.object(id, false)?;
        Some(self.sim.attribute(id, attr))
    }

    /// Fill level of an attribute 0...100,000 (`AttributeSet_GetCurrentFraction`).
    pub fn fraction(&self, id: ObjectId, attr: u32) -> Option<i32> {
        self.object(id, false)
            .map(|o| o.attributes.current_fraction(attr))
    }

    /// Nearest matching object around `pos_fx` (fixed point) within the radius (< 0 = whole map) by center distance
    /// ([`crate::collision::find_nearest_matching_seen`], `edge_ofs = −1`); `ref_player` is the filter's reference
    /// player.
    pub fn find_nearest(
        &mut self,
        pos_fx: (i32, i32),
        f: &MatchFilter,
        radius: i32,
        ref_player: i32,
    ) -> Option<ObjectId> {
        let seen_by = self.seen_by();
        crate::collision::find_nearest_matching_seen(
            self.sim, pos_fx, f, radius, ref_player, -1, seen_by,
        )
    }

    /// Matching objects in the tile rectangle `[x0, z0, x1, z1]` (inclusive, [`crate::area::collect_in_tile_rect`]);
    /// under [`Sight::Fog`] only seen ones.
    pub fn collect_in_rect(
        &mut self,
        rect: [i32; 4],
        f: &MatchFilter,
        ref_player: i32,
    ) -> Vec<ObjectId> {
        let ids = crate::area::collect_in_tile_rect(self.sim, rect, f, ref_player);
        self.retain_seen(ids)
    }

    /// Matching objects of the object table of slot `slot` in table order
    /// ([`crate::economy::collect_matching_in_table`]); under [`Sight::Fog`] only seen ones.
    pub fn objects_in_table(&mut self, slot: i32, f: &MatchFilter) -> Vec<ObjectId> {
        let ids = crate::economy::collect_matching_in_table(self.sim, slot, f);
        self.retain_seen(ids)
    }

    /// Matching objects of the object table of slot `slot` (0-10) in table order, reference player = owner
    /// ([`crate::economy::collect_matching_in_slot_table`], `FUN_0047EA20`, only `0x50D2`); under [`Sight::Fog`] only
    /// seen ones.
    pub fn objects_in_slot_table(&mut self, slot: i32, f: &MatchFilter) -> Vec<ObjectId> {
        let ids = crate::economy::collect_matching_in_slot_table(self.sim, slot, f);
        self.retain_seen(ids)
    }

    /// Count of [`Self::objects_in_table`] ([`crate::economy::count_matching_in_table`]).
    pub fn count_in_table(&mut self, slot: i32, f: &MatchFilter) -> i32 {
        if self.sight == Sight::Omniscient {
            return crate::economy::count_matching_in_table(self.sim, slot, f);
        }
        i32::try_from(self.objects_in_table(slot, f).len()).unwrap_or(i32::MAX)
    }

    /// GUID object counter or filter count of the slot ([`crate::economy::count_guid_or_filter`]). Under
    /// [`Sight::Fog`] for a foreign slot: GUID (`id ≥ 1000`) -> 0 (foreign account), filter ID -> the seen objects like
    /// [`Self::count_in_table`] with [`crate::filter::from_id`].
    pub fn count_guid_or_filter(&mut self, slot: i32, id: i32, with_pending: bool) -> i32 {
        if self.hides_account(slot) {
            if id >= 1000 {
                return 0;
            }
            let f = crate::filter::from_id(self.sim.rules, id);
            return self.count_in_table(slot, &f);
        }
        crate::economy::count_guid_or_filter(self.sim, slot, id, with_pending)
    }

    /// Does the object pass the filter ([`crate::filter::matches`])? 0, -1, missing or hidden: no.
    pub fn matches(&mut self, id: ObjectId, f: &MatchFilter, ref_player: i32) -> bool {
        self.object(id, false).is_some() && crate::filter::matches(self.sim, id, f, ref_player)
    }

    fn retain_seen(&self, mut ids: Vec<ObjectId>) -> Vec<ObjectId> {
        if self.seen_by().is_some() {
            ids.retain(|&id| self.sees(id));
        }
        ids
    }

    /// Public information of slot `p` (0...7), otherwise `None`.
    pub fn players(&self, p: i32) -> Option<PlayerInfo> {
        let slot = u8::try_from(p)
            .ok()
            .filter(|&s| usize::from(s) < PLAYER_SLOTS)?;
        let pl = self.sim.state.players.get(usize::from(slot))?;
        Some(PlayerInfo {
            slot,
            control: pl.control,
            nation: pl.nation,
            color: pl.color,
            defined: pl.slot_active,
        })
    }

    /// Resource `r` of slot `p` (`Player_GetResource` with living space limit); slot outside or index outside 0...4
    /// -> 0. `None` only under [`Sight::Fog`] for foreign slots.
    pub fn resource(&self, p: i32, r: i32) -> Option<i32> {
        if self.hides_account(p) {
            return None;
        }
        Some(match (usize::try_from(p), usize::try_from(r)) {
            (Ok(p), Ok(r)) => self.sim.state.resource(p, r),
            _ => 0,
        })
    }

    /// `Player_GetGuidCount` ([`crate::economy::guid_count`]); `None` under [`Sight::Fog`] for foreign slots.
    pub fn guid_count(&self, p: i32, guid: u32) -> Option<i32> {
        (!self.hides_account(p)).then(|| crate::economy::guid_count(self.sim, p, guid))
    }

    /// `Player_GetPendingGuidCount` ([`crate::economy::pending_guid_count`]); `None` under
    /// [`Sight::Fog`] for foreign slots.
    pub fn pending_count(&self, p: i32, guid: u32) -> Option<i32> {
        (!self.hides_account(p)).then(|| crate::economy::pending_guid_count(self.sim, p, guid))
    }

    /// `Player_CanBuildOrMakeGuid` ([`crate::command::validate::can_build_or_make_guid`], silent);
    /// `None` under [`Sight::Fog`] for foreign slots.
    pub fn can_build_or_make(&mut self, p: i32, guid: u32) -> Option<bool> {
        if self.hides_account(p) {
            return None;
        }
        Some(crate::command::validate::can_build_or_make_guid(
            self.sim, p, guid,
        ))
    }

    /// Diplomacy from `p` to `q`: (friend, enemy, `p` sees what `q` sees).
    pub fn relation(&self, p: i32, q: i32) -> (bool, bool, bool) {
        let d = &self.sim.state.diplomacy;
        (d.is_friend(p, q), d.is_enemy(p, q), d.shares_vision(p, q))
    }

    /// `GameWorld_GetFogOfWarStateAt(x, z, p)` (7/3/1/0, [`crate::fog::FogView::state_at`]); under
    /// [`Sight::Fog`] 0 for a foreign slot.
    pub fn fog_state(&self, x: i32, z: i32, p: i32) -> u8 {
        if self.hides_account(p) {
            return 0;
        }
        crate::fog::FogView::new(self.sim.state).state_at(x, z, p)
    }

    /// Map size in tiles (`TERRAIN_DIM_X/Z`).
    pub fn map_size(&self) -> (i32, i32) {
        (self.sim.rules.map.dim_x, self.sim.rules.map.dim_z)
    }

    /// Mode of the match (`SimState::mode`, < 0 = single player).
    pub fn mode(&self) -> i32 {
        self.sim.state.mode
    }

    /// Living space limit of the match (`GameEngine+0x5EC`).
    pub fn max_living_space(&self) -> i32 {
        self.sim.state.economy.max_living_space
    }

    /// Global AI difficulty (`SimState::campaign_difficulty`).
    pub fn campaign_difficulty(&self) -> i32 {
        self.sim.state.campaign_difficulty
    }

    /// Water tiles and all tiles of the map ([`Rules::water_tiles`], [`Rules::tile_count`]).
    pub fn water_share(&self) -> (i32, i32) {
        (self.sim.rules.water_tiles, self.sim.rules.tile_count)
    }

    /// Running counter-upgrades with source on slot `p` (`FUN_00479690`,
    /// [`crate::upgrades::counter::count_of_slot`]); `p` outside 0...255 -> 0, not restricted to 0...7.
    /// Under [`Sight::Fog`] a foreign slot -> 0.
    pub fn counter_upgrades(&self, p: i32) -> i32 {
        if self.hides_account(p) {
            return 0;
        }
        u8::try_from(p).map_or(0, |s| {
            let n = crate::upgrades::counter::count_of_slot(self.sim.state, s);
            i32::try_from(n).unwrap_or(i32::MAX)
        })
    }

    /// Does the counter-upgrade `guid` run anywhere ([`crate::upgrades::counter::is_running`])?
    pub fn counter_running(&self, guid: u32) -> bool {
        crate::upgrades::counter::is_running(self.sim.state, guid)
    }

    /// Offered headhunter of kind `art` (0...2, `SimState::headhunters`); public like the hiring menu, the object
    /// itself is read by [`Self::object`].
    pub fn headhunter(&self, art: i32) -> Option<ObjectId> {
        let art = usize::try_from(art).ok()?;
        self.sim.state.headhunters.get(art).copied().flatten()
    }

    /// Layer 0...8 as an index, otherwise `None` (the message `0x1400` returns -1 without walk).
    fn layer(layer: i32) -> Option<usize> {
        usize::try_from(layer)
            .ok()
            .filter(|&l| l < crate::path::LAYER_COUNT)
    }

    /// Nearest cell of the region of cell `start` around cell `goal` (`PathPlanner_FindNearestReachableCell`
    /// without ring limit, filter and target, [`crate::path::reach::find_nearest_reachable_cell`]).
    pub fn reachable_cell(
        &mut self,
        layer: i32,
        start: (i32, i32),
        goal: (i32, i32),
    ) -> Option<(i32, i32)> {
        Self::layer(layer)?;
        crate::path::reach::find_nearest_reachable_cell(
            self.sim, layer, start, goal, -1, None, None,
        )
    }

    /// Free cell near `cell` (`RegionLayer_FindFreeCellNear`,
    /// [`crate::path::region::RegionLayer::find_free_cell_near`]).
    pub fn free_cell(&self, layer: i32, cell: (i32, i32)) -> Option<(i32, i32)> {
        let layer = Self::layer(layer)?;
        self.sim
            .state
            .path
            .layers
            .get(layer)?
            .find_free_cell_near(cell.0, cell.1)
    }

    /// Cell count of the tile path from cell `start` to cell `goal` (`PathPlanner_FindTilePath`,
    /// `crate::path::highpath::tile_path_cells`); `None` without a path and above 1000 cells.
    pub fn tile_path_len(&self, layer: i32, start: (i32, i32), goal: (i32, i32)) -> Option<usize> {
        let layer = Self::layer(layer)?;
        crate::path::highpath::tile_path_cells(&self.sim.state.path, layer, start, goal)
    }

    /// Attack reports of slot `p` (0...7); under [`Sight::Fog`] foreign -> `None`.
    pub fn attack_reports(&self, p: i32) -> Option<&MfcMap<ReportTile>> {
        if self.hides_account(p) {
            return None;
        }
        self.sim.state.attack_reports.get(usize::try_from(p).ok()?)
    }
}

/// Command of a player to the simulation: sets the origin [`Sim::origin`] for the duration of the execution, calls
/// [`execute`] like the input (`direct`) and restores the old origin. The result reports whether the command took
/// effect fully, partially or not.
///
/// [`Origin::Script`] exists only from the simulation itself ([`submit_script`]): from outside - host, network,
/// control channel, bots - the origin is rejected without executing anything.
pub fn submit(sim: &mut Sim, cmd: &Command, origin: Origin) -> Outcome {
    if origin == Origin::Script {
        return Outcome::Rejected("script path only from the simulation".to_owned());
    }
    run_as(sim, cmd, origin)
}

/// Command of a script action: [`Origin::Script`], immediately in the script tick and `direct` (the units' command
/// queues are cleared like on input). Only for `script::command`; the command appears in no command log (E-162).
pub(crate) fn submit_script(sim: &mut Sim, cmd: &Command) -> Outcome {
    run_as(sim, cmd, Origin::Script)
}

fn run_as(sim: &mut Sim, cmd: &Command, origin: Origin) -> Outcome {
    let before = std::mem::replace(&mut sim.origin, origin);
    let outcome = execute(sim, cmd, true);
    sim.origin = before;
    outcome
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::combat::testkit::{WARRIOR, rules as combat_rules};
    use crate::command::testkit::{account, fogged_state, spawn};
    use crate::save::checksum;
    use crate::state::SimState;
    use opennml_formats::targets::{parse_target_defines, parse_targets};

    /// The player layer does not depend on the AI: `MfcMap` lives in `crate::mfc`. The search text is written in
    /// pieces so that the test does not find itself.
    #[test]
    fn player_does_not_depend_on_ki() {
        let needle = concat!("crate::", "ki");
        assert!(
            !include_str!("player.rs").contains(needle),
            "player.rs must not import anything from `{needle}`"
        );
    }

    /// `submit` sets the origin only for the duration of the command and returns the result.
    #[test]
    fn submit_restores_the_origin() {
        let rules = crate::command::testkit::rules();
        let mut state = crate::command::testkit::state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        assert_eq!(sim.origin, Origin::External);
        let unit = spawn(&mut sim, crate::command::testkit::COLONIST, 0, (10, 10));
        let halt = |player| Command {
            player,
            kind: crate::command::CommandKind::Halt { units: vec![unit] },
        };
        assert_eq!(submit(&mut sim, &halt(0), Origin::Ki), Outcome::Accepted);
        assert_eq!(sim.origin, Origin::External);
        // Also after a rejection, and a different starting origin is preserved.
        assert!(matches!(
            submit(&mut sim, &halt(1), Origin::Ki),
            Outcome::Rejected(_)
        ));
        assert_eq!(sim.origin, Origin::External);
        sim.origin = Origin::Ki;
        submit(&mut sim, &halt(0), Origin::External);
        assert_eq!(sim.origin, Origin::Ki);
    }

    /// The script path comes only from the simulation: `submit` rejects `Origin::Script` without executing anything
    /// or changing the origin.
    // QUIRK E-152: kept
    #[test]
    fn submit_rejects_the_script_origin_from_outside() {
        let rules = crate::command::testkit::rules();
        let mut state = crate::command::testkit::state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let foreign = spawn(&mut sim, crate::command::testkit::COLONIST, 1, (10, 10));
        let halt = Command {
            player: 0,
            kind: crate::command::CommandKind::Halt {
                units: vec![foreign],
            },
        };
        // With script origin the halt would have hit the foreign unit; this way it is rejected.
        let before = checksum(sim.state);
        let outcome = submit(&mut sim, &halt, Origin::Script);
        assert!(
            matches!(&outcome, Outcome::Rejected(why) if why.contains("script path")),
            "{outcome:?}"
        );
        assert_eq!(sim.origin, Origin::External);
        assert_eq!(checksum(sim.state), before);
        // A different starting origin is preserved here too.
        sim.origin = Origin::Ki;
        assert!(matches!(
            submit(&mut sim, &halt, Origin::Script),
            Outcome::Rejected(_)
        ));
        assert_eq!(sim.origin, Origin::Ki);
    }

    /// Fog world (nothing seen): own warrior at (10, 10), foreign one at (14, 10); filter ID 5 = "objects of the
    /// reference player" (`flowcontroltragets.ini`).
    fn world() -> (Rules, SimState, ObjectId, ObjectId) {
        let mut rules = combat_rules();
        let text = "BEGIN
BUTTONMSG 5
MY_MM
MM_MODE MM_HL_MYLIST
MM_END
TARGET_MM
MM_END
END
";
        let defines = parse_target_defines(crate::command::testkit::DEFINES);
        rules.flowcontrol_targets = parse_targets(text, &defines).entries;
        let mut state = fogged_state(&rules);
        let (own, foreign) = {
            let mut sim = Sim::new(&mut state, &rules);
            account(&mut sim, 0, [100, 200, 300, 4, 50]);
            account(&mut sim, 1, [700, 800, 900, 6, 50]);
            (
                spawn(&mut sim, WARRIOR, 0, (10, 10)),
                spawn(&mut sim, WARRIOR, 1, (14, 10)),
            )
        };
        state.fog.seen.clear();
        state.fog.cleared.clear();
        (rules, state, own, foreign)
    }

    /// Own and enemy objects of the reference player, OR mode (`MYLIST | ENEMYLIST`).
    fn own_or_enemy() -> MatchFilter {
        MatchFilter {
            mode: 0x100 | 0x400,
            ..MatchFilter::constructed()
        }
    }

    /// Fixed-point center of a tile.
    fn centre(t: (i32, i32)) -> (i32, i32) {
        ((t.0 << 12) + 0x800, (t.1 << 12) + 0x800)
    }

    fn walk_layer() -> Message {
        Message::new(0x1400, &[])
    }

    // QUIRK E-25: kept
    #[test]
    fn omniscient_view_sees_foreign_objects_and_accounts() {
        let (rules, mut state, own, foreign) = world();
        state.fog.seen.or(14, 10, 1 << 1);
        let mut sim = Sim::new(&mut state, &rules);
        let f = own_or_enemy();
        let mut v = View::new(&mut sim, 0, Sight::Omniscient);
        assert_eq!((v.player_slot(), v.sight()), (0, Sight::Omniscient));
        assert_eq!(v.object(foreign, true).map(|o| o.id), Some(foreign));
        assert!(v.object(ObjectId(0), false).is_none());
        assert!(v.object(ObjectId::NONE, false).is_none());
        assert_eq!(v.message(foreign, &mut walk_layer()), Some(0));
        assert!(v.attribute(foreign, 14).is_some());
        assert!(v.fraction(foreign, 14).is_some());
        assert!(v.matches(foreign, &f, 0) && v.matches(own, &f, 0));
        assert!(!v.matches(ObjectId(0), &f, 0) && !v.matches(ObjectId::NONE, &f, 0));
        assert_eq!(v.find_nearest(centre((14, 10)), &f, -1, 0), Some(foreign));
        let mut both = v.collect_in_rect([0, 0, 63, 63], &f, 0);
        both.sort();
        let mut want = vec![own, foreign];
        want.sort();
        assert_eq!(both, want);
        assert_eq!(v.objects_in_table(1, &f), vec![foreign]);
        assert_eq!(v.count_in_table(1, &f), 1);
        assert_eq!(v.count_guid_or_filter(1, WARRIOR as i32, false), 1);
        assert_eq!(v.count_guid_or_filter(1, 5, false), 1);
        assert_eq!(v.resource(1, 0), Some(700));
        assert_eq!(v.resource(1, 9), Some(0));
        assert_eq!(v.resource(-1, 0), Some(0));
        assert_eq!(v.guid_count(1, WARRIOR), Some(1));
        assert_eq!(v.pending_count(1, WARRIOR), Some(0));
        assert!(v.can_build_or_make(1, WARRIOR).is_some());
        assert_eq!(v.fog_state(14, 10, 1), 3);
        assert_eq!(v.fog_state(14, 10, 0), 0);
        assert!(v.attack_reports(1).is_some_and(|m| m.is_empty()));
        assert!(v.attack_reports(8).is_none() && v.attack_reports(-1).is_none());
        assert_eq!(v.counter_upgrades(1), 0);
        let p = v.players(1).expect("slot 1");
        assert_eq!((p.slot, p.color), (1, 1));
        assert!(v.players(8).is_none() && v.players(-1).is_none());
    }

    // QUIRK E-25: kept
    #[test]
    fn fog_view_hides_unseen_foreign_objects_everywhere() {
        let (rules, mut state, own, foreign) = world();
        // Sight of another slot reveals nothing for slot 0.
        state.fog.seen.or(14, 10, 1 << 2);
        let f = own_or_enemy();
        let pos = centre((14, 10));
        {
            let mut sim = Sim::new(&mut state, &rules);
            let mut v = View::new(&mut sim, 0, Sight::Fog);
            assert!(v.object(foreign, false).is_none());
            assert_eq!(v.message(foreign, &mut walk_layer()), None);
            assert_eq!(v.attribute(foreign, 14), None);
            assert_eq!(v.fraction(foreign, 14), None);
            assert!(!v.matches(foreign, &f, 0));
            // Sight works in the search: the hidden nearer hit does not hide the own one.
            assert_eq!(v.find_nearest(pos, &f, -1, 0), Some(own));
            assert_eq!(v.find_nearest(pos, &f, 4096, 0), None);
            assert_eq!(v.collect_in_rect([0, 0, 63, 63], &f, 0), vec![own]);
            assert!(v.objects_in_table(1, &f).is_empty());
            assert_eq!(v.count_in_table(1, &f), 0);
            assert_eq!(v.count_guid_or_filter(1, 5, false), 0);
            // Own objects are visible without sight of their tile.
            assert_eq!(v.object(own, true).map(|o| o.id), Some(own));
            assert_eq!(v.message(own, &mut walk_layer()), Some(0));
            assert!(v.matches(own, &f, 0));
            assert_eq!(v.count_in_table(0, &f), 1);
        }
        state.fog.seen.or(14, 10, 1 << 0);
        let mut sim = Sim::new(&mut state, &rules);
        let mut v = View::new(&mut sim, 0, Sight::Fog);
        assert_eq!(v.object(foreign, false).map(|o| o.id), Some(foreign));
        assert_eq!(v.message(foreign, &mut walk_layer()), Some(0));
        assert!(v.attribute(foreign, 14).is_some());
        assert!(v.matches(foreign, &f, 0));
        assert_eq!(v.find_nearest(pos, &f, -1, 0), Some(foreign));
        assert_eq!(v.collect_in_rect([0, 0, 63, 63], &f, 0).len(), 2);
        assert_eq!(v.objects_in_table(1, &f), vec![foreign]);
        assert_eq!(v.count_in_table(1, &f), 1);
        // Foreign filter branch: counts the seen ones like count_in_table with from_id; GUID stays hidden.
        let by_id = crate::filter::from_id(&rules, 5);
        assert_eq!(
            v.count_guid_or_filter(1, 5, true),
            v.count_in_table(1, &by_id)
        );
        assert_eq!(v.count_guid_or_filter(1, 5, true), 1);
        assert_eq!(v.count_guid_or_filter(1, WARRIOR as i32, false), 0);
    }

    #[test]
    #[cfg(debug_assertions)]
    #[should_panic(expected = "not a query message")]
    fn command_messages_are_refused_in_debug_builds() {
        let (rules, mut state, own, _) = world();
        let mut sim = Sim::new(&mut state, &rules);
        let mut v = View::new(&mut sim, 0, Sight::Omniscient);
        // 0x500: path message to the unit itself (command, writes).
        let _ = v.message(own, &mut Message::new(0x500, &[0, 0, 1, 0, -1, 0, -1]));
    }

    #[test]
    fn tile_path_length_counts_cells_on_a_free_line() {
        let (rules, mut state, _, _) = world();
        let mut sim = Sim::new(&mut state, &rules);
        let v = View::new(&mut sim, 0, Sight::Fog);
        assert_eq!(v.tile_path_len(0, (10, 10), (20, 10)), Some(10));
        assert_eq!(v.tile_path_len(0, (10, 10), (10, 10)), Some(0));
        assert_eq!(
            v.tile_path_len(0, (10, 10), (64, 10)),
            None,
            "target outside"
        );
        assert_eq!(v.tile_path_len(9, (10, 10), (20, 10)), None);
    }

    #[test]
    fn fog_view_hides_foreign_accounts_and_fog() {
        let (rules, mut state, _, _) = world();
        state.fog.seen.or(14, 10, 1 << 1);
        state.fog.seen.or(10, 10, 1 << 0);
        let mut sim = Sim::new(&mut state, &rules);
        let mut v = View::new(&mut sim, 0, Sight::Fog);
        assert_eq!(v.resource(1, 0), None);
        assert_eq!(v.resource(0, 0), Some(100));
        assert_eq!(v.guid_count(1, WARRIOR), None);
        assert_eq!(v.guid_count(0, WARRIOR), Some(1));
        assert_eq!(v.pending_count(1, WARRIOR), None);
        assert_eq!(v.pending_count(0, WARRIOR), Some(0));
        assert_eq!(v.can_build_or_make(1, WARRIOR), None);
        assert!(v.can_build_or_make(0, WARRIOR).is_some());
        assert_eq!(v.count_guid_or_filter(1, WARRIOR as i32, false), 0);
        assert_eq!(v.count_guid_or_filter(0, WARRIOR as i32, false), 1);
        assert_eq!(v.fog_state(14, 10, 1), 0, "foreign fog");
        assert_eq!(v.fog_state(10, 10, 0), 3);
        assert!(v.attack_reports(1).is_none());
        assert!(v.attack_reports(0).is_some());
        assert_eq!(v.counter_upgrades(1), 0);
        // Public: slots and diplomacy.
        assert!(v.players(1).is_some());
        assert_eq!(v.relation(0, 1), (false, true, false));
    }

    #[test]
    fn views_leave_the_checksum_unchanged() {
        let (rules, mut state, own, foreign) = world();
        state.fog.seen.or(14, 10, 1 << 0);
        state.attack_reports[0].insert(foreign.as_i32(), ReportTile { x: 14, z: 10 });
        let (before, events) = (checksum(&state), state.events.clone());
        let f = own_or_enemy();
        for sight in [Sight::Omniscient, Sight::Fog] {
            let mut sim = Sim::new(&mut state, &rules);
            let mut v = View::new(&mut sim, 0, sight);
            let _ = (v.player_slot(), v.sight(), v.rules().tile_count);
            for id in [own, foreign] {
                let _ = v.object(id, false).map(|o| o.tile);
                let _ = v.object(id, true).map(|o| o.tile);
                // Real queries: walk layer (`0x1400`) and attack target (`0x1511`).
                assert!(v.message(id, &mut walk_layer()).is_some());
                assert!(v.message(id, &mut Message::new(0x1511, &[])).is_some());
                let _ = v.attribute(id, 14);
                let _ = v.fraction(id, 14);
                let _ = v.matches(id, &f, 0);
            }
            let _ = v.find_nearest(centre((12, 10)), &f, -1, 0);
            let _ = v.find_nearest(centre((12, 10)), &f, 1 << 14, 1);
            let _ = v.collect_in_rect([0, 0, 63, 63], &f, 0);
            for slot in [0, 1, 8, -1] {
                let _ = v.objects_in_table(slot, &f);
                let _ = v.count_in_table(slot, &f);
                let _ = v.count_guid_or_filter(slot, WARRIOR as i32, true);
                let _ = v.count_guid_or_filter(slot, 5, false);
                let _ = v.players(slot);
                let _ = v.resource(slot, 4);
                let _ = v.guid_count(slot, WARRIOR);
                let _ = v.pending_count(slot, WARRIOR);
                let _ = v.can_build_or_make(slot, WARRIOR);
                let _ = v.relation(0, slot);
                let _ = v.fog_state(14, 10, slot);
                let _ = v.counter_upgrades(slot);
                let _ = v.attack_reports(slot).map(|m| m.len());
                let _ = v.headhunter(slot);
            }
            let _ = (v.map_size(), v.mode(), v.max_living_space());
            let _ = (v.campaign_difficulty(), v.water_share());
            let _ = v.counter_running(WARRIOR);
            assert_eq!(v.reachable_cell(0, (10, 10), (14, 10)), Some((14, 10)));
            assert_eq!(v.reachable_cell(-1, (10, 10), (14, 10)), None);
            assert!(v.free_cell(0, (10, 10)).is_some());
            assert_eq!(v.free_cell(9, (10, 10)), None);
            assert!(v.tile_path_len(0, (10, 10), (20, 20)).is_some());
            assert_eq!(v.tile_path_len(0, (10, 10), (10, 10)), Some(0));
            assert_eq!(v.tile_path_len(-1, (10, 10), (20, 20)), None);
            assert_eq!(checksum(&state), before, "{sight:?}");
            assert_eq!(state.events, events, "{sight:?}");
        }
    }
}
