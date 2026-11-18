//! Checksum, savegame and replay: fixed serialization with `postcard`; checksum = FNV-1a 64 bit over
//! these bytes (always 0 in the original). Savegame = start package + state + commands from that state on;
//! replay = start package + all commands with their tick.

use anyhow::{Context, bail};
use postcard::ser_flavors::Flavor;
use serde::{Deserialize, Serialize};

use crate::command::Command;
use crate::id::SLOT_CAPACITY;
use crate::path::grid::DirtyRect;
use crate::path::region::SECTOR;
use crate::rules::{Fnv64, Rules};
use crate::setup::{FORMAT_VERSION, StartPackage, new_match};
use crate::state::{SimEvent, SimState};
use crate::tick::step;

/// File identifiers.
pub const SAVE_MAGIC: &[u8; 8] = b"NMLSAVE\0";
pub const REPLAY_MAGIC: &[u8; 8] = b"NMLREPL\0";

/// Serializes directly into the checksum (without an intermediate buffer).
struct HashFlavor(Fnv64);

impl Flavor for HashFlavor {
    type Output = u64;

    fn try_extend(&mut self, data: &[u8]) -> postcard::Result<()> {
        self.0.write(data);
        Ok(())
    }

    fn try_push(&mut self, data: u8) -> postcard::Result<()> {
        self.0.write(&[data]);
        Ok(())
    }

    fn finalize(self) -> postcard::Result<u64> {
        Ok(self.0.0)
    }
}

/// Checksum of the state (without the events of the tick).
pub fn checksum(state: &SimState) -> u64 {
    checksum_of(state)
}

/// FNV-1a 64 over the `postcard` bytes of a value, without an intermediate buffer (also for partial states
/// such as the AI state).
pub fn checksum_of<T: Serialize>(value: &T) -> u64 {
    postcard::serialize_with_flavor(value, HashFlavor(Fnv64::default()))
        .expect("state is serializable")
}

/// Checksum per part of the state: one entry per field of [`SimState`] in declaration order (without `events`),
/// value [`checksum_of`] of the field. The comparison on a desync thereby shows which parts differ. The
/// destructuring names every field without `..`: a new field in `SimState` breaks the build here and has to go
/// into the list.
pub fn part_checksums(state: &SimState) -> Vec<(&'static str, u64)> {
    let SimState {
        tick,
        phase,
        steps,
        mode,
        rng,
        crt,
        tables,
        tick_groups,
        destroy_queue,
        owner_changes,
        players,
        diplomacy,
        applied_upgrades,
        guid_counts,
        flag_counts,
        flag_leader,
        time_limit,
        path,
        occupancy,
        collision,
        planners,
        collision_records,
        object_config,
        economy,
        pending_counts,
        extra_tick,
        fog,
        combat,
        tile_map,
        projectiles,
        groups,
        object_event_bits,
        kill_counts,
        button_locks,
        slot_running,
        inactive_lists,
        type_upgrades,
        upgrade_queue,
        counter_upgrades,
        daytime,
        area_queries,
        headhunters,
        ki,
        attack_reports,
        campaign_difficulty,
        names,
        script,
        countdowns,
        accounting,
        idle_workers,
        network,
        events: _,
    } = state;
    vec![
        ("tick", checksum_of(tick)),
        ("phase", checksum_of(phase)),
        ("steps", checksum_of(steps)),
        ("mode", checksum_of(mode)),
        ("rng", checksum_of(rng)),
        ("crt", checksum_of(crt)),
        ("tables", checksum_of(tables)),
        ("tick_groups", checksum_of(tick_groups)),
        ("destroy_queue", checksum_of(destroy_queue)),
        ("owner_changes", checksum_of(owner_changes)),
        ("players", checksum_of(players)),
        ("diplomacy", checksum_of(diplomacy)),
        ("applied_upgrades", checksum_of(applied_upgrades)),
        ("guid_counts", checksum_of(guid_counts)),
        ("flag_counts", checksum_of(flag_counts)),
        ("flag_leader", checksum_of(flag_leader)),
        ("time_limit", checksum_of(time_limit)),
        ("path", checksum_of(path)),
        ("occupancy", checksum_of(occupancy)),
        ("collision", checksum_of(collision)),
        ("planners", checksum_of(planners)),
        ("collision_records", checksum_of(collision_records)),
        ("object_config", checksum_of(object_config)),
        ("economy", checksum_of(economy)),
        ("pending_counts", checksum_of(pending_counts)),
        ("extra_tick", checksum_of(extra_tick)),
        ("fog", checksum_of(fog)),
        ("combat", checksum_of(combat)),
        ("tile_map", checksum_of(tile_map)),
        ("projectiles", checksum_of(projectiles)),
        ("groups", checksum_of(groups)),
        ("object_event_bits", checksum_of(object_event_bits)),
        ("kill_counts", checksum_of(kill_counts)),
        ("button_locks", checksum_of(button_locks)),
        ("slot_running", checksum_of(slot_running)),
        ("inactive_lists", checksum_of(inactive_lists)),
        ("type_upgrades", checksum_of(type_upgrades)),
        ("upgrade_queue", checksum_of(upgrade_queue)),
        ("counter_upgrades", checksum_of(counter_upgrades)),
        ("daytime", checksum_of(daytime)),
        ("area_queries", checksum_of(area_queries)),
        ("headhunters", checksum_of(headhunters)),
        ("ki", checksum_of(ki)),
        ("attack_reports", checksum_of(attack_reports)),
        ("campaign_difficulty", checksum_of(campaign_difficulty)),
        ("names", checksum_of(names)),
        ("script", checksum_of(script)),
        ("countdowns", checksum_of(countdowns)),
        ("accounting", checksum_of(accounting)),
        ("idle_workers", checksum_of(idle_workers)),
        ("network", checksum_of(network)),
    ]
}

/// A command with the engine tick before which it takes effect (`SimState::steps`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecordedCommand {
    pub step: u64,
    pub command: Command,
}

/// Savegame.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SaveGame {
    pub start: StartPackage,
    pub state: SimState,
    /// Commands from the state on (for continuing a replay).
    pub commands: Vec<RecordedCommand>,
}

/// Replay.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Replay {
    pub start: StartPackage,
    pub commands: Vec<RecordedCommand>,
}

/// Longest playback in engine ticks: 576,000 ticks at 50 ms = eight hours of game time. Limit for
/// the target tick of [`Replay::play_observed`] (hang protection) and for `steps` of the control
/// (`opennml_control::MAX_REPLAY_STEPS`, same size); the command log of the host ends there.
pub const MAX_REPLAY_STEPS: u64 = 576_000;

/// Header: identifier (8 bytes), format version (u32 LE), checksum of the body (u64 LE).
pub const HEADER_LEN: usize = 20;
/// Margin by which a dirty rectangle of the path grid may extend beyond the map: the track lock
/// of the train (K-207, re/train-multipart 4.1, 13.1) widens it unclamped and does not clear it after the init.
/// The locomotive tile is clamped to the map, the lock window extends at most 11 tiles
/// further (`T' − 11r` or `T' + 3r` while driving, `+2 … −9` at the init); on top comes the derivation
/// with +-1, which does not read until the clearing.
const DIRTY_RECT_MARGIN: i32 = 11;

/// Largest collision radius in the savegame (64 tiles). Only protects against the cost of the
/// spiral search, not against unusual game data: the shipped data (all 55 maps) has at most 4200
/// hard and 9216 soft, changed data (mod, other `property.ini`) has plenty of headroom.
const MAX_COLLISION_RADIUS: i32 = 64 << 12;
/// Smallest hard radius != 0 of a unit with a walk module in the savegame (1/32 tile; shipped data
/// 0 for WID_Geist 1413, otherwise at least 256 for PAT_Kundschafter 2306). With both limits
/// the spiral search (step width = double the hard radius, abort radius = one tile plus the
/// own hard plus soft radius of the carrier) has at most 4,330,560 points (hard radius 128,
/// soft radius of the carrier 64 tiles) instead of ~1.7*10^9 at hard radius 1.
const MIN_WALK_HARD_RADIUS: i32 = 128;

/// FNV-1a 64 over the body: detects corruption; deliberate forgery is not ruled out by it
/// (the structure check catches the known crash points).
fn body_checksum(body: &[u8]) -> u64 {
    let mut h = Fnv64::default();
    h.write(body);
    h.0
}

/// Container with a header (savegame, replay, in the client the savegame of the match and the network state):
/// identifier `magic` (8 bytes), format version `version` (u32 LE), [`body_checksum`] of the body (u64 LE), then
/// the body.
pub fn write_container(magic: &[u8; 8], version: u32, body: &[u8]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(HEADER_LEN + body.len());
    bytes.extend(magic);
    bytes.extend(version.to_le_bytes());
    bytes.extend(body_checksum(body).to_le_bytes());
    bytes.extend(body);
    bytes
}

/// Reads the header of a container ([`write_container`]) and returns the body: identifier, header length, format
/// version and checksum, in this order, with an error message for `name`. No access before the length check.
pub fn read_container<'a>(
    magic: &[u8; 8],
    version: u32,
    bytes: &'a [u8],
    name: &str,
) -> anyhow::Result<&'a [u8]> {
    if bytes.get(..8) != Some(magic.as_slice()) {
        bail!("not a {name} file (identifier missing)");
    }
    let Some((head, body)) = bytes.split_first_chunk::<HEADER_LEN>() else {
        bail!("{name} corrupted (header incomplete)");
    };
    let stored = u32::from_le_bytes(head[8..12].try_into().expect("4 Byte"));
    if stored != version {
        bail!("{name} has format version {stored}, expected {version}");
    }
    let checksum = u64::from_le_bytes(head[12..].try_into().expect("8 Byte"));
    if body_checksum(body) != checksum {
        bail!("{name} corrupted (checksum does not match)");
    }
    Ok(body)
}

fn encode<T: Serialize>(magic: &[u8; 8], value: &T) -> Vec<u8> {
    let body = postcard::to_stdvec(value).expect("serializable");
    write_container(magic, FORMAT_VERSION, &body)
}

fn decode<T: for<'de> Deserialize<'de>>(
    magic: &[u8; 8],
    bytes: &[u8],
    what: &str,
) -> anyhow::Result<T> {
    let body = read_container(magic, FORMAT_VERSION, bytes, what).map_err(|e| {
        // Another format version of the simulation computes differently.
        let other_version = bytes.get(..8) == Some(magic.as_slice())
            && bytes.len() >= HEADER_LEN
            && bytes[8..12] != FORMAT_VERSION.to_le_bytes();
        if other_version {
            anyhow::anyhow!("{e} (different simulation behavior)")
        } else {
            e
        }
    })?;
    postcard::from_bytes(body).with_context(|| format!("{what} corrupted"))
}

/// Structure check of a loaded state: only what would otherwise crash, hang or take unbounded
/// memory (index, divisor, cycles, lengths); it does not check plausibility. Every check has a row
/// in the combined test `save/tests/crash.rs`. Fixed lengths (object tables, players, upgrade lists, module slots,
/// attribute lists, building mask, layers, planners, type tables, MFC buckets) are fixed by the type
/// [`crate::fixed_vec::FixedVec`].
pub(crate) fn check_state(state: &SimState) -> anyhow::Result<()> {
    for (slot, table) in state.tables.iter().enumerate() {
        if let Err(why) = table.check(SLOT_CAPACITY[slot]) {
            bail!("savegame corrupted (object table {slot}: {why})");
        }
    }
    // Extra tick list: without duplicates (the pass looks for the remembered next entry and would
    // otherwise run in circles) and not longer than all object slots together.
    let capacity: usize = SLOT_CAPACITY.iter().sum();
    let mut seen = std::collections::BTreeSet::new();
    if state.extra_tick.len() > capacity {
        bail!(
            "savegame corrupted (extra tick list with {} entries, at most {capacity})",
            state.extra_tick.len()
        );
    }
    if !state.extra_tick.iter().all(|id| seen.insert(id.0)) {
        bail!("savegame corrupted (extra tick list with duplicate entries)");
    }
    if !state.economy.is_consistent() {
        bail!(
            "savegame corrupted (market price 0 for wood {}/food {}: the market divides by it)",
            state.economy.wood_price,
            state.economy.food_price
        );
    }
    // Per object: collision radii and the modules.
    let map = (state.path.width, state.path.height);
    for o in state.objects() {
        // Collision radii (exception A7): the spiral search (`FindFreeSpotSpiral`, step width
        // = double the hard radius of the unit, abort after at most 10 tiles or tile +
        // radii) grows with (radius / step width)^2. The limits are generous (0 ... 64 tiles,
        // hard on a unit with walk module 0 - the spiral aborts - or from 1/32 tile);
        // shipped data: hard 0 or 256 ... 4200, soft 512 ... 9216.
        let walk = o.modules.first().is_some_and(Option::is_some);
        let in_range = |r: i32| (0..=MAX_COLLISION_RADIUS).contains(&r);
        if !in_range(o.hard_radius)
            || !in_range(o.soft_radius)
            || (walk && (1..MIN_WALK_HARD_RADIUS).contains(&o.hard_radius))
        {
            bail!(
                "savegame corrupted (object {} with radius hard {}, soft {})",
                o.id.0,
                o.hard_radius,
                o.soft_radius
            );
        }
        for (i, m) in o.modules.iter().enumerate() {
            if let Some(Err(why)) = m.as_ref().map(|m| m.kind.check(map)) {
                bail!(
                    "savegame corrupted (object {}, module slot {i}: {why})",
                    o.id.0
                );
            }
        }
    }
    // Wall group: master and members without a cycle (build step and demolition follow them recursively).
    if let Some((id, why)) = crate::module::building::construction::wall_group_fault(state) {
        bail!("savegame corrupted: building {} {why}", id.0);
    }
    // Path and occupancy grid: the layers (nine via the type) in map size, sides multiples of the sector size 16
    // (the regions run sector by sector over the cells).
    let p = &state.path;
    let cells = (p.width.max(0) as usize) * (p.height.max(0) as usize);
    if p.width < 0
        || p.height < 0
        || p.width % SECTOR != 0
        || p.height % SECTOR != 0
        || p.layers
            .iter()
            .any(|l| l.width != p.width || l.height != p.height || l.cells.len() != cells)
        || (state.occupancy.width, state.occupancy.height) != (p.width, p.height)
        || state.occupancy.cells.len() != cells
    {
        bail!("savegame corrupted (path or occupancy grid does not match the map size)");
    }
    if let Err(why) = state.fog.check() {
        bail!("savegame corrupted (fog: {why})");
    }
    // Dirty rectangle: empty (the state after clearing) or a non-empty rectangle in the range
    // -[`DIRTY_RECT_MARGIN`] ... map size + margin - 1 (+-1 is read when deriving). A rectangle
    // beyond that would let the next sync run over any number of rows. Also without a mask:
    // a rectangle with mask 0 is kept at the next entry and is then synced as well.
    let d = &p.dirty;
    let empty = *d == DirtyRect::default();
    let inside = d.min_x <= d.max_x
        && d.min_z <= d.max_z
        && d.min_x >= -DIRTY_RECT_MARGIN
        && d.min_z >= -DIRTY_RECT_MARGIN
        && d.max_x < p.width.saturating_add(DIRTY_RECT_MARGIN)
        && d.max_z < p.height.saturating_add(DIRTY_RECT_MARGIN);
    if !(empty || inside) {
        bail!("savegame corrupted: dirty rectangle of the path grid outside the map");
    }
    // Collision manager: bucket count as the map (the queries index `cols * bz + bx`).
    if !state.collision.fits(p.width, p.height) {
        bail!("savegame corrupted (bucket grid of the collision manager does not match the map)");
    }
    if !crate::world::carry_chains_are_acyclic(state) {
        bail!("savegame corrupted (carrier chain of an object runs in a cycle)");
    }
    if !crate::module::riding::rider_chains_are_acyclic(state) {
        bail!("savegame corrupted (rider chain of an object runs in a cycle)");
    }
    if !state.collision_records.is_consistent() {
        bail!("savegame corrupted (collision records do not match their lists)");
    }
    if !state
        .planners
        .iter()
        .all(crate::bstar::Planner::is_consistent)
    {
        bail!("savegame corrupted (B* planner with references outside its nodes)");
    }
    // Rear group of a formation (re/groups-formations 3.2): it has no formation itself. Otherwise
    // `member_count`/`is_member_of` would recurse over chains and cycles (A -> B -> A, A -> B -> C -> A) until the
    // stack overflows; this way every chain ends after one level (this also covers the two-cycle and the self-reference
    // of a formation). A missing rear group is harmless (`find` -> `None`).
    for (_, g) in state.groups.map.iter() {
        if let Some(f) = &g.formation
            && f.rear != crate::group::NO_GROUP
            && let Some(rear) = state.groups.map.get(crate::group::key(f.rear))
            && rear.formation.is_some()
        {
            bail!(
                "savegame corrupted: group {}: rear group {} has a formation itself",
                g.id,
                f.rear
            );
        }
    }
    if let Err(why) = crate::ki::save::check_state(&state.ki) {
        bail!("savegame corrupted (AI: {why})");
    }
    // Attack reports `GameWorld+0x2340` (re/ki-queries 5.11.6): one MFC table per slot.
    for (p, map) in state.attack_reports.iter().enumerate() {
        if let Err(why) = map.check(crate::state::ATTACK_REPORT_BUCKETS) {
            bail!("savegame corrupted: attack reports slot {p}: {why}");
        }
    }
    // Fade-out lists: no object twice in a list. `tick_inactive_lists` jumps back to the first
    // occurrence with `position(next)` and would otherwise run backwards.
    for (slot, list) in state.inactive_lists.iter().enumerate() {
        let mut seen = std::collections::BTreeSet::new();
        if let Some(id) = list.iter().find(|id| !seen.insert(id.0)) {
            bail!(
                "savegame corrupted (object {} twice in the list of slot {slot})",
                id.0
            );
        }
    }
    Ok(())
}

impl SaveGame {
    pub fn to_bytes(&self) -> Vec<u8> {
        encode(SAVE_MAGIC, self)
    }

    pub fn from_bytes(bytes: &[u8]) -> anyhow::Result<Self> {
        let save: SaveGame = decode(SAVE_MAGIC, bytes, "savegame")?;
        check_state(&save.state)?;
        Ok(save)
    }

    /// Checks whether the savegame matches the loaded rules: start package (map, map description,
    /// game mode, rules checksum), network flag as in the start package and map size (path and occupancy grid
    /// against the map of the rules). The AI catalog and scripts are part of the rules checksum
    /// (`KiCatalog::load`, `ScriptRules::load`); nobody checks threads and containers against it, the run time
    /// tolerates every state.
    pub fn check(&self, rules: &Rules) -> anyhow::Result<()> {
        check_start(&self.start, rules)?;
        // `new_match` sets the network flag from the start package, no step changes it.
        if self.state.network != self.start.setup.network {
            bail!(
                "savegame corrupted (network flag {}, in the start package {})",
                self.state.network,
                self.start.setup.network
            );
        }
        let (w, h) = (rules.map.dim_x, rules.map.dim_z);
        let (p, o) = (&self.state.path, &self.state.occupancy);
        if (p.width, p.height) != (w, h) || (o.width, o.height) != (w, h) {
            bail!(
                "savegame does not match the map: grid {} x {}, map {w} x {h}",
                p.width,
                p.height
            );
        }
        Ok(())
    }
}

impl Replay {
    pub fn to_bytes(&self) -> Vec<u8> {
        encode(REPLAY_MAGIC, self)
    }

    /// Reads a replay. Loading does not check the command sequence, an unordered sequence runs without a
    /// crash, but not the same everywhere: [`Self::play_observed`] executes a command only at exactly its tick
    /// (`== now`); a command with a smaller tick than its predecessor therefore holds it and all following commands
    /// up, they never run. The playback in the client (`SimHost::take_playback_step`, `<=`), in contrast, catches
    /// it up in the running tick. [`Self::play_observed`] limits the length.
    pub fn from_bytes(bytes: &[u8]) -> anyhow::Result<Self> {
        decode(REPLAY_MAGIC, bytes, "replay")
    }

    /// The end of the playback when none is given: one tick after the last command (0 without commands); an error if
    /// that would overflow `u64` (last command at `u64::MAX`). [`Self::play_observed`] rejects anything above
    /// [`MAX_REPLAY_STEPS`].
    pub fn implied_end(&self) -> anyhow::Result<u64> {
        let Some(last) = self.commands.last() else {
            return Ok(0);
        };
        match last.step.checked_add(1) {
            Some(end) => Ok(end),
            None => bail!(
                "replay corrupted: last command at tick {}, the end one tick after it overflows u64",
                last.step
            ),
        }
    }

    /// Plays the replay up to and including tick `steps` (at most
    /// [`MAX_REPLAY_STEPS`]).
    pub fn play(&self, rules: &Rules, steps: u64) -> anyhow::Result<SimState> {
        self.play_observed(rules, steps, &mut |_| {})
    }

    /// Like [`Self::play`]; `observe` sees the events of every tick (the host uses them to update its own state,
    /// for example the fog switch of `rainbowring`, which lives in the host (deviation from the original)).
    pub fn play_observed(
        &self,
        rules: &Rules,
        steps: u64,
        observe: &mut dyn FnMut(&[SimEvent]),
    ) -> anyhow::Result<SimState> {
        if steps > MAX_REPLAY_STEPS {
            bail!("tick {steps} invalid: at most {MAX_REPLAY_STEPS} ticks allowed");
        }
        check_start(&self.start, rules)?;
        let mut state = new_match(rules, &self.start.setup).map_err(anyhow::Error::msg)?;
        let mut next = 0;
        while state.steps < steps {
            let now = state.steps;
            let start = next;
            while next < self.commands.len() && self.commands[next].step == now {
                next += 1;
            }
            let commands: Vec<Command> = self.commands[start..next]
                .iter()
                .map(|c| c.command.clone())
                .collect();
            observe(&step(&mut state, rules, &commands));
        }
        Ok(state)
    }
}

/// Start package against the loaded data (map, map description, game mode, rules checksum). Nobody checks the setup
/// itself: `new_match` tolerates every value.
fn check_start(start: &StartPackage, rules: &Rules) -> anyhow::Result<()> {
    if start.level != rules.level_path {
        bail!(
            "belongs to map {}, loaded is {}",
            start.level,
            rules.level_path
        );
    }
    // Random map: the same description as the loaded rules, the game mode of the match
    // is that of the description (deviating from D-348), and the newly generated map has the same checksum. On the real
    // load path the rules arise from the description of the savegame, so the comparison of the description is always
    // satisfied there; the rules checksum protects there (it contains the checksum of the generator including the
    // description).
    let loaded = rules.random_map.as_ref().map(|r| &r.spec);
    if start.random_map.as_ref() != loaded {
        bail!(
            "savegame: map description differs (savegame {}, loaded {})",
            describe_random_map(start.random_map.as_ref()),
            describe_random_map(loaded)
        );
    }
    if let Some(spec) = &start.random_map
        && usize::try_from(start.setup.mode).ok() != Some(spec.game_type)
    {
        bail!(
            "savegame: game mode {} does not match the map description (game mode {})",
            start.setup.mode,
            spec.game_type
        );
    }
    if start.rules_checksum != rules.checksum {
        if start.random_map.is_some() {
            bail!(
                "savegame: map differs (other game data or mods): checksum {:016x} instead of {:016x}",
                start.rules_checksum,
                rules.checksum
            );
        }
        bail!(
            "rules differ (checksum {:016x} instead of {:016x}: other game data or mods)",
            start.rules_checksum,
            rules.checksum
        );
    }
    Ok(())
}

/// Map description for messages: "fixed map" or the menu values.
fn describe_random_map(spec: Option<&crate::random_map::RandomMapSpec>) -> String {
    match spec {
        None => "fixed map".to_string(),
        Some(s) => format!(
            "seed \"{}\", size {}, {} players, map type {}, landscape {}, time of day {}, game mode {}",
            s.seed_text, s.size, s.players, s.terrain, s.landscape, s.daytime, s.game_type
        ),
    }
}

/// Test helpers for saving (reused by the group and riding tests).
#[cfg(test)]
pub(crate) mod testkit {
    use super::*;
    use crate::setup::{MatchSetup, start_package};

    /// Fixed match setup for savegames that a test builds by hand.
    pub(crate) fn setup() -> MatchSetup {
        MatchSetup {
            mode: -1,
            seed: 0,
            lobby: Vec::new(),
            resource_setting: 0,
            living_space: 2,
            map_explored: false,
            ki: crate::ki::KiSetup::default(),
            script: crate::script::ScriptSetup::default(),
            ..MatchSetup::default()
        }
    }

    /// Savegame bytes of the state (start package from [`setup`], no commands).
    pub(crate) fn save_bytes(state: &SimState, rules: &Rules) -> Vec<u8> {
        SaveGame {
            start: start_package(rules, &setup()),
            state: state.clone(),
            commands: Vec::new(),
        }
        .to_bytes()
    }

    /// Savegame round trip: save and load again immediately like a restart, including the check against the rules
    /// (`from_bytes`, then `check(rules)`; every state that arises from a loadable one through ticks loads
    /// again).
    pub(crate) fn save_and_load(state: &SimState, rules: &Rules) -> SimState {
        let save = SaveGame::from_bytes(&save_bytes(state, rules)).expect("loading");
        save.check(rules).expect("matches the rules");
        save.state
    }
}

#[cfg(test)]
mod tests;
