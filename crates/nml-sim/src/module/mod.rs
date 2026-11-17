//! Behavior modules (`CProperty...`, re/property-modules): one module per module index of the
//! original (factory `GameObject_CreateModule` `0x004670C0`). Modules that are not implemented yet are
//! **idle**: they exist with their flags but do not have any behavior yet.

#[cfg(test)]
pub(crate) mod ability_testkit;
pub mod animal;
pub mod arming;
pub mod assassin;
pub mod attack;
pub mod base;
pub mod boarding;
pub mod broadside;
pub mod building;
pub mod construct;
pub mod diving;
pub mod dying;
pub mod edible;
pub mod events;
pub mod evoke_animal;
#[cfg(test)]
mod evoke_tests;
pub mod exec_temp;
pub mod fight_far;
pub mod fight_near;
pub mod gather;
pub mod ghosts;
pub mod global_upgrade;
pub mod headhunter;
#[cfg(test)]
mod headhunter_tests;
pub mod healing;
pub mod influence;
pub mod masking;
pub mod multipart;
pub mod natur;
pub mod quarter;
pub mod resource;
pub mod riding;
pub mod robbing;
pub mod selfheal;
pub mod shoot;
pub mod special;
pub mod spell;
#[cfg(test)]
mod spell_tests;
pub mod stand;
#[cfg(test)]
mod stealth_tests;
pub mod swim;
pub mod tent;
pub mod trading;
pub mod train;
pub mod tree;
pub mod walk;

use nml_formats::property::ModuleDesc;
use serde::{Deserialize, Serialize};

use crate::id::ObjectId;
use crate::message::{Envelope, Message};
use crate::sim::Sim;

use self::base::ModuleBase;

/// Behavior of a module.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ModuleKind {
    /// Module not implemented yet: base fields only.
    Inert,
    /// Module 0.
    Walk(Box<walk::Walk>),
    /// Module 2.
    Stand(stand::Stand),
    /// Module 3.
    Building(Box<building::Building>),
    /// Module 4.
    Tree(tree::Tree),
    /// Module 5.
    Natur(natur::Natur),
    /// Module 6.
    Resource(resource::Resource),
    /// Module 8 (gatherer base embedded).
    Lumber(Box<gather::lumber::Lumber>),
    /// Module 7 (gatherer base embedded).
    Mining(Box<gather::mining::Mining>),
    /// Module 12 (gatherer base embedded).
    Harvest(Box<gather::harvest::Harvest>),
    /// Module 55 (gatherer base embedded).
    Fishing(Box<gather::fishing::Fishing>),
    /// Module 23.
    Quarter(Box<quarter::Quarter>),
    /// Module 40.
    SelfHealing(selfheal::SelfHealing),
    /// Module 1.
    Construct(Box<construct::Construct>),
    /// Module 17 (new kinds only at the end: the savegame stores the variant index).
    Attack(Box<attack::Attack>),
    /// Module 18.
    FightNear(Box<fight_near::FightNear>),
    /// Module 16.
    Dying(Box<dying::Dying>),
    /// Module 19.
    FightFar(Box<fight_far::FightFar>),
    /// Module 10.
    Shoot(Box<shoot::Shoot>),
    /// Module 20.
    Animal(Box<animal::Animal>),
    /// Module 21.
    Edible(Box<edible::Edible>),
    /// Module 22 (gatherer base embedded).
    Hunting(Box<gather::hunting::Hunting>),
    /// Module 13 (rider) and 14 (boat occupant): the same class, `Riding::boat` distinguishes them.
    Riding(Box<riding::Riding>),
    /// Module 15.
    Swim(swim::Swim),
    /// Module 24.
    Diving(diving::Diving),
    /// Module 33 (locomotive, tender, wagon).
    Train(Box<train::Train>),
    /// Module 11 (front part of a multipart object).
    MultiHead(multipart::MultiHead),
    /// Module 51 (rear part of a multipart object).
    MultiPart(multipart::MultiPart),
    /// Module 30.
    GlobalUpgrade(global_upgrade::GlobalUpgrade),
    /// Module 38 (kind 4).
    Spell(Box<spell::Spell>),
    /// Module 44 (execute).
    ExecTempUpgrade(exec_temp::ExecTempUpgrade),
    /// Module 35 (travois).
    TransportTent(tent::transport::TransportTent),
    /// Module 42 (tent).
    Tent(tent::Tent),
    /// Module 43 (trade cart and taxes).
    Trading(trading::Trading),
    /// Module 25 (camouflage).
    Masking(masking::Masking),
    /// Module 28 (dig in and assassinate).
    Assassin(assassin::Assassin),
    /// Module 29 (healing).
    Healing(healing::Healing),
    /// Module 26 (evoke ghost).
    EvokeGhost(ghosts::evoke::EvokeGhost),
    /// Module 27 (ghost).
    Ghost(ghosts::ghost::Ghost),
    /// Module 31 (banish).
    Banish(ghosts::banish::Banish),
    /// Module 32 (evoke animal).
    EvokeAnimal(evoke_animal::EvokeAnimal),
    /// Module 36 (demolition expert).
    Bomberman(special::bomberman::Bomberman),
    /// Module 37 (killer).
    Killer(special::killer::Killer),
    /// Module 49 (whirlwind and damage building).
    SpecialMove(special::special_move::SpecialMove),
    /// Module 34 (headhunter).
    HeadHunter(headhunter::HeadHunter),
    /// Module 48 (robbing).
    Robbing(robbing::Robbing),
}

/// A module in its place.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Module {
    pub base: ModuleBase,
    pub kind: ModuleKind,
}

/// Names of the module indices (`PROPID` names of the `property.ini`, re/property-modules).
pub const MODULE_NAMES: [&str; 56] = [
    "PTTOPT",
    "CONSTRUCT",
    "STAND",
    "BUILDING",
    "TREE",
    "NATURE",
    "RESOURCE",
    "MINING",
    "LUMBERING",
    "EFFECT",
    "SHOOTING",
    "MULTIHEAD",
    "HARVESTING",
    "RIDING",
    "RIDINGBOAT",
    "SWIM",
    "DIE",
    "ATTACK",
    "FIGHTNEAR",
    "FIGHTFAR",
    "ANIMAL",
    "EDIBLEANIMAL",
    "HUNTING",
    "QUARTER",
    "DIVING",
    "MASKING",
    "EVOKEGHOST",
    "GHOST",
    "ASSASSIN",
    "HEALING",
    "GLOBALUPGRADE",
    "BANISH",
    "EVOKEANIMAL",
    "TRAIN",
    "HEADHUNTER",
    "TRANSPORTTENT",
    "BOMBERMAN",
    "KILLER",
    "SPELL",
    "ARMING",
    "SELFHEALING",
    "PATTERN",
    "TENT",
    "TRADING",
    "EXECTEMPUPGRADE",
    "BONDSLAVE",
    "SIEGEENGINE",
    "PATTERNWALK",
    "ROBBING",
    "SPECIALMOVE",
    "TOWER",
    "MULTIPART",
    "SAPPING",
    "FIREGUARDING",
    "ARMORUPGRADE",
    "FISHING",
];

/// Module indices without a class (the factory returns 0): 9 `EFFECT`, 41 `PATTERN`, 50 `TOWER`; and
/// everything from 56.
pub fn has_class(index: usize) -> bool {
    !matches!(index, 9 | 41 | 50) && index <= 55
}

/// `VALUEID` of the module flags that `Init` loads after `m_MatchInt`, and fixed bits
/// (catalog in re/property-modules 2): tree `| 0x104`, quarters `0x100`,
/// edible animal `0x200`; fishing reads its flags from 80001 (`PropertyFishing_Init` `0x00514F30`),
/// mining from 80004.
pub fn init_flags(index: usize, desc: &ModuleDesc) -> u32 {
    let value = |id: u32| desc.value_int(id).unwrap_or(0) as u32;
    match index {
        0 => value(10003),
        1 => value(20002),
        2 => value(30003),
        3 => value(40003),
        4 => value(50003) | 0x104,
        5 => value(60002),
        6 => value(70002),
        7 => value(80004),
        55 => value(80001),
        10 => value(110001),
        13 => value(140001),
        16 => value(170001),
        17 => value(180001),
        18 => value(190001),
        19 => value(200001),
        20 => value(210001),
        21 => 0x200,
        23 => 0x100,
        25 => value(260002),
        29 => value(300002),
        31 => value(320003),
        33 => value(340001),
        35 => value(360001),
        36 => value(370001),
        37 => value(380001),
        38 => value(390001),
        39 => value(400001),
        48 => value(490001),
        49 => value(500001),
        _ => 0,
    }
}

impl Module {
    /// New module (`Module_ConstructBase`).
    pub fn new(index: u8) -> Self {
        let kind = match index {
            1 => ModuleKind::Construct(Box::default()),
            2 => ModuleKind::Stand(stand::Stand::default()),
            3 => ModuleKind::Building(Box::default()),
            4 => ModuleKind::Tree(tree::Tree::default()),
            5 => ModuleKind::Natur(natur::Natur::default()),
            6 => ModuleKind::Resource(resource::Resource::default()),
            8 => ModuleKind::Lumber(Box::default()),
            7 => ModuleKind::Mining(Box::default()),
            12 => ModuleKind::Harvest(Box::default()),
            55 => ModuleKind::Fishing(Box::default()),
            23 => ModuleKind::Quarter(Box::default()),
            40 => ModuleKind::SelfHealing(selfheal::SelfHealing::default()),
            17 => ModuleKind::Attack(Box::default()),
            18 => ModuleKind::FightNear(Box::default()),
            19 => ModuleKind::FightFar(Box::default()),
            10 => ModuleKind::Shoot(Box::default()),
            20 => ModuleKind::Animal(Box::default()),
            21 => ModuleKind::Edible(Box::default()),
            22 => ModuleKind::Hunting(Box::default()),
            16 => ModuleKind::Dying(Box::default()),
            13 => ModuleKind::Riding(Box::new(riding::Riding::new(false))),
            14 => ModuleKind::Riding(Box::new(riding::Riding::new(true))),
            15 => ModuleKind::Swim(swim::Swim::default()),
            24 => ModuleKind::Diving(diving::Diving::default()),
            // `train::INDEX`, `multipart::HEAD`, `multipart::PART` (the indices are `usize`).
            0x21 => ModuleKind::Train(Box::default()),
            0x0B => ModuleKind::MultiHead(multipart::MultiHead::default()),
            0x33 => ModuleKind::MultiPart(multipart::MultiPart::default()),
            // `global_upgrade::INDEX`, `spell::INDEX`, `exec_temp::INDEX`.
            0x1E => ModuleKind::GlobalUpgrade(global_upgrade::GlobalUpgrade::default()),
            0x26 => ModuleKind::Spell(Box::default()),
            0x2C => ModuleKind::ExecTempUpgrade(exec_temp::ExecTempUpgrade::default()),
            // `tent::transport::INDEX`, `tent::INDEX`, `trading::INDEX`.
            0x23 => ModuleKind::TransportTent(tent::transport::TransportTent::default()),
            0x2A => ModuleKind::Tent(tent::Tent::default()),
            0x2B => ModuleKind::Trading(trading::Trading::default()),
            // `masking::INDEX`, `assassin::INDEX`, `healing::INDEX`.
            0x19 => ModuleKind::Masking(masking::Masking::default()),
            0x1C => ModuleKind::Assassin(assassin::Assassin::default()),
            0x1D => ModuleKind::Healing(healing::Healing::default()),
            // `ghosts::{evoke, ghost, banish}::INDEX`, `evoke_animal::INDEX`.
            0x1A => ModuleKind::EvokeGhost(ghosts::evoke::EvokeGhost::default()),
            0x1B => ModuleKind::Ghost(ghosts::ghost::Ghost::default()),
            0x1F => ModuleKind::Banish(ghosts::banish::Banish::default()),
            0x20 => ModuleKind::EvokeAnimal(evoke_animal::EvokeAnimal::default()),
            // `special::{bomberman, killer, special_move}::INDEX`.
            0x24 => ModuleKind::Bomberman(special::bomberman::Bomberman::default()),
            0x25 => ModuleKind::Killer(special::killer::Killer::default()),
            0x31 => ModuleKind::SpecialMove(special::special_move::SpecialMove::default()),
            // `headhunter::INDEX`, `robbing::INDEX`.
            0x22 => ModuleKind::HeadHunter(headhunter::HeadHunter::default()),
            0x30 => ModuleKind::Robbing(robbing::Robbing::default()),
            _ => ModuleKind::Inert,
        };
        Self {
            base: ModuleBase::new(index),
            kind,
        }
    }
}

impl ModuleKind {
    /// Load check per module (called from `save::check_state`): only values that the tick takes as a divisor or
    /// index, over which it loops without end or on which it hangs; plausibility is not checked. `map` is the
    /// map size in tiles (width, height). Each check has a row in the combined test `save/tests/crash.rs`.
    pub(crate) fn check(&self, map: (i32, i32)) -> Result<(), &'static str> {
        match self {
            ModuleKind::Walk(w) => {
                // Layer: index of the layer masks (`goto.rs`, combat target).
                if w.layer >= crate::path::LAYER_COUNT {
                    return Err("walk layer outside 0..8");
                }
                // Step multiplier: divisor of the planner (`bstar.rs`).
                if w.request.mult < 1 {
                    return Err("walk request with step multiplier below 1");
                }
                // Step size: `0x800 / cur` would otherwise give the multiplier 0 (`plan.rs`).
                if !(0..=walk::MAX_STEP).contains(&w.speed.cur) {
                    return Err("walk step size outside 0..0x800");
                }
                // Turn 0: the turning radius divides by `fixed_mul(TWO_PI, 0)` (`motion/speed.rs`).
                if w.speed.turn < 1 {
                    return Err("walk turn speed below 1");
                }
                // Planning state 4: the planner divides by the step size.
                if w.state == 4 && w.speed.cur <= 0 {
                    return Err("walk plans with step size 0");
                }
                // Pair tables: `buckets[b]` with `b < 17` (`collide.rs`).
                if [&w.hard_pairs, &w.soft_pairs]
                    .iter()
                    .any(|m| !m.buckets.is_empty() && m.buckets.len() != 17)
                {
                    return Err("walk pair table neither empty nor with 17 buckets");
                }
            }
            ModuleKind::Building(b) => {
                // Construction time: divisor of the construction step (`construction.rs`).
                if b.build_time < 1 {
                    return Err("construction time below 1");
                }
                // Farm pointer: `request_pos + 1` indexes the five farm slots (`farm.rs`).
                if !(-1..building::FARM_SLOTS as i32).contains(&b.request_pos) {
                    return Err("farm pointer outside -1..4");
                }
                // Footprint and occupancy: otherwise the double loops would run over arbitrarily many tiles.
                let inside = |r: &[i32; 4]| building::rect_within_map(r, map.0, map.1);
                if !inside(&b.rect) || !inside(&b.occupancy_rect) {
                    return Err("building rectangle outside the map");
                }
            }
            ModuleKind::Train(t) => {
                // Grid direction: otherwise the dirty rectangle would grow without limit after every tile change.
                let (rx, rz) = t.raster_dir;
                if !(-1..=1).contains(&rx) || !(-1..=1).contains(&rz) {
                    return Err("train grid direction outside -1..1");
                }
            }
            _ => {}
        }
        Ok(())
    }
}

/// `Init(values, player slot, ready)` (slot 9). Idle modules load only their flags.
pub fn init(sim: &mut Sim, id: ObjectId, index: usize, desc: &ModuleDesc, slot: u8, ready: bool) {
    match index {
        0 => walk::init(sim, id, desc),
        construct::INDEX => construct::init(sim, id, desc),
        2 => stand::init(sim, id, desc),
        3 => building::init(sim, id, desc, slot, ready),
        4 => tree::init(sim, id, desc),
        5 => natur::init(sim, id, desc),
        6 => resource::init(sim, id, desc),
        gather::lumber::INDEX => gather::lumber::init(sim, id, desc),
        gather::mining::INDEX => gather::mining::init(sim, id, desc),
        gather::harvest::INDEX => gather::harvest::init(sim, id, desc),
        gather::fishing::INDEX => gather::fishing::init(sim, id, desc),
        gather::hunting::INDEX => gather::hunting::init(sim, id, desc),
        quarter::INDEX => quarter::init(sim, id, desc),
        selfheal::INDEX => selfheal::init(sim, id, desc),
        attack::INDEX => attack::init(sim, id, desc),
        fight_near::INDEX => fight_near::init(sim, id, desc),
        fight_far::INDEX => fight_far::init(sim, id, desc),
        shoot::INDEX => shoot::init(sim, id, desc),
        animal::INDEX => animal::init(sim, id, desc, slot),
        edible::INDEX => edible::init(sim, id, desc),
        dying::INDEX => dying::init(sim, id, desc),
        riding::RIDING | riding::RIDING_BOAT => riding::init(sim, id, index, desc),
        swim::INDEX => swim::init(sim, id, desc),
        diving::INDEX => diving::init(sim, id, desc),
        // Render bits (re/object-classes): train ORs, rear part sets.
        train::INDEX => train::init(sim, id, desc, ready),
        multipart::HEAD => multipart::init_head(sim, id, desc),
        multipart::PART => multipart::init_part(sim, id, desc),
        global_upgrade::INDEX => global_upgrade::init(sim, id, desc, slot),
        spell::INDEX => spell::init(sim, id, desc),
        exec_temp::INDEX => exec_temp::init(sim, id, desc),
        tent::transport::INDEX => tent::transport::init(sim, id, desc),
        tent::INDEX => tent::init(sim, id, desc),
        trading::INDEX => trading::init(sim, id, desc, ready),
        // The ability modules set their flags themselves (otherwise the fallback below would do it).
        masking::INDEX => masking::init(sim, id, desc),
        assassin::INDEX => assassin::init(sim, id, desc),
        healing::INDEX => healing::init(sim, id, desc, ready),
        ghosts::evoke::INDEX => ghosts::evoke::init(sim, id, desc),
        ghosts::ghost::INDEX => ghosts::ghost::init(sim, id, desc),
        ghosts::banish::INDEX => ghosts::banish::init(sim, id, desc),
        evoke_animal::INDEX => evoke_animal::init(sim, id, desc),
        special::bomberman::INDEX => special::bomberman::init(sim, id, desc),
        special::killer::INDEX => special::killer::init(sim, id, desc),
        special::special_move::INDEX => special::special_move::init(sim, id, desc),
        headhunter::INDEX => headhunter::init(sim, id, desc, slot),
        robbing::INDEX => robbing::init(sim, id, desc),
        _ => {
            let flags = init_flags(index, desc);
            if let Some(m) = sim.module_mut(id, index) {
                m.base.flags = flags;
            }
        }
    }
}

/// Destructor (VTable slot 1) when the object is removed: buildings and natur release their locks,
/// trees nothing (quirk); module 44 removes its influences from the set
/// (`0x00512C00`, re/evoke-aura 3.9); the headhunter empties its world slot and removes its
/// sight source (`0x00517B80`, re/headhunter-robbing 1.2).
pub fn destruct(sim: &mut Sim, id: ObjectId, index: usize) {
    match sim.module(id, index).map(|m| &m.kind) {
        Some(ModuleKind::Building(_)) => building::destruct(sim, id),
        Some(ModuleKind::Natur(_)) => natur::destruct(sim, id),
        Some(ModuleKind::ExecTempUpgrade(_)) => influence::destruct(sim, id),
        Some(ModuleKind::HeadHunter(_)) => headhunter::destruct(sim, id),
        _ => {}
    }
}

/// `ReadParams(values)` (slot 4), on a variant switch. Natur and idle modules read nothing.
pub fn read_params(sim: &mut Sim, id: ObjectId, index: usize, desc: &ModuleDesc) {
    match index {
        0 => walk::read_params(sim, id, desc),
        2 => stand::read_params(sim, id, desc),
        3 => building::read_params(sim, id, desc),
        quarter::INDEX => quarter::read_params(sim, id, desc),
        selfheal::INDEX => selfheal::read_params(sim, id, desc),
        attack::INDEX => attack::read_params(sim, id, desc),
        dying::INDEX => dying::read_params(sim, id, desc),
        fight_far::INDEX => fight_far::read_params(sim, id, desc),
        shoot::INDEX => shoot::read_params(sim, id, desc),
        riding::RIDING | riding::RIDING_BOAT => riding::read_params(sim, id, index, desc),
        multipart::HEAD => multipart::read_params_head(sim, id, desc),
        multipart::PART => multipart::read_params_part(sim, id, desc),
        // Camouflage and dig in read nothing anew (slot 4 empty, re/masking 1.2/1.3).
        healing::INDEX => healing::read_params(sim, id, desc),
        _ => {}
    }
}

/// `Query(msg)` (slot 7): synchronous query; -1 = not responsible.
pub fn query(sim: &mut Sim, id: ObjectId, index: usize, msg: &mut Message) -> i32 {
    #[cfg(test)]
    if let Some(observer) = test_hook::OBSERVE.with(|q| *q.borrow()) {
        observer(sim, id, index, msg);
    }
    match sim.module(id, index).map(|m| &m.kind) {
        Some(ModuleKind::Walk(_)) => walk::query(sim, id, msg),
        Some(ModuleKind::Stand(_)) => stand::query(sim, id, msg),
        Some(ModuleKind::Building(_)) => building::query(sim, id, msg),
        Some(ModuleKind::Natur(_)) => natur::query(sim, id, msg),
        Some(ModuleKind::Tree(_)) => tree::query(sim, id, msg),
        Some(ModuleKind::Resource(_)) => resource::query(sim, id, msg),
        Some(ModuleKind::Lumber(_)) => gather::lumber::query(sim, id, msg),
        Some(ModuleKind::Mining(_)) => gather::mining::query(sim, id, msg),
        Some(ModuleKind::Harvest(_)) => gather::harvest::query(sim, id, msg),
        Some(ModuleKind::Fishing(_)) => gather::fishing::query(sim, id, msg),
        Some(ModuleKind::Quarter(_)) => quarter::query(sim, id, msg),
        Some(ModuleKind::Construct(_)) => construct::query(sim, id, msg),
        Some(ModuleKind::Attack(_)) => attack::query(sim, id, msg),
        Some(ModuleKind::FightNear(_)) => fight_near::query(sim, id, msg),
        Some(ModuleKind::FightFar(_)) => fight_far::query(sim, id, msg),
        Some(ModuleKind::Shoot(_)) => shoot::query(sim, id, msg),
        Some(ModuleKind::Animal(_)) => animal::query(sim, id, msg),
        Some(ModuleKind::Edible(_)) => edible::query(sim, id, msg),
        Some(ModuleKind::Hunting(_)) => gather::hunting::query(sim, id, msg),
        Some(ModuleKind::Dying(_)) => dying::query(sim, id, msg),
        Some(ModuleKind::Riding(_)) => riding::query(sim, id, index, msg),
        Some(ModuleKind::Swim(_)) => swim::query(sim, id, msg),
        Some(ModuleKind::Diving(_)) => diving::query(sim, id, msg),
        Some(ModuleKind::Train(_)) => train::query(sim, id, msg),
        Some(ModuleKind::MultiHead(_)) => multipart::query_head(sim, id, msg),
        Some(ModuleKind::MultiPart(_)) => multipart::query_part(sim, id, msg),
        Some(ModuleKind::Spell(_)) => spell::query(sim, id, msg),
        Some(ModuleKind::ExecTempUpgrade(_)) => exec_temp::query(sim, id, msg),
        Some(ModuleKind::TransportTent(_)) => tent::transport::query(sim, id, msg),
        Some(ModuleKind::Tent(_)) => tent::query(sim, id, msg),
        Some(ModuleKind::Trading(_)) => trading::query(sim, id, msg),
        // Module 39 stays idle (no tick, no state of its own) but answers.
        Some(ModuleKind::Inert) if index == arming::INDEX => arming::query(sim, id, msg),
        Some(ModuleKind::Inert) => inert_query(sim, id, index, msg),
        // Module 30 does not answer.
        Some(ModuleKind::SelfHealing(_) | ModuleKind::GlobalUpgrade(_)) | None => -1,
        Some(ModuleKind::Masking(_)) => masking::query(sim, id, msg),
        Some(ModuleKind::Assassin(_)) => assassin::query(sim, id, msg),
        Some(ModuleKind::Healing(_)) => healing::query(sim, id, msg),
        Some(ModuleKind::EvokeGhost(_)) => ghosts::evoke::query(sim, id, msg),
        Some(ModuleKind::Ghost(_)) => ghosts::ghost::query(sim, id, msg),
        Some(ModuleKind::Banish(_)) => ghosts::banish::query(sim, id, msg),
        Some(ModuleKind::EvokeAnimal(_)) => evoke_animal::query(sim, id, msg),
        // Module 36 also answers unknown codes with 0 (`0x00503FD0`, K-321).
        Some(ModuleKind::Bomberman(_)) => special::bomberman::query(sim, id, msg),
        Some(ModuleKind::Killer(_)) => special::killer::query(sim, id, msg),
        Some(ModuleKind::SpecialMove(_)) => special::special_move::query(sim, id, msg),
        Some(ModuleKind::HeadHunter(_)) => headhunter::query(sim, id, msg),
        // Module 48 answers unknown codes with 0 (`0x00523BC0`, re/headhunter-robbing 2.2).
        Some(ModuleKind::Robbing(_)) => robbing::query(sim, id, msg),
    }
}

/// Idle modules do not answer (-1); tests can hook in an answer ([`test_hook`]).
fn inert_query(sim: &mut Sim, id: ObjectId, index: usize, msg: &mut Message) -> i32 {
    #[cfg(test)]
    if let Some(hook) = test_hook::QUERY.with(|q| *q.borrow()) {
        return hook(sim, id, index, msg);
    }
    let _ = (sim, id, index, msg);
    -1
}

/// Test answers of idle modules (per test thread), e.g. the gatherer queries before their implementation.
#[cfg(test)]
pub(crate) mod test_hook {
    use std::cell::RefCell;

    use super::{Message, ObjectId, Sim};

    /// `(simulation, object, module index, message) -> answer`.
    pub type Hook = fn(&mut Sim, ObjectId, usize, &mut Message) -> i32;

    /// `(simulation, object, module index, message)`: sees every query before delivery.
    pub type Observer = fn(&mut Sim, ObjectId, usize, &Message);

    thread_local! {
        pub static QUERY: RefCell<Option<Hook>> = const { RefCell::new(None) };
        pub static OBSERVE: RefCell<Option<Observer>> = const { RefCell::new(None) };
    }

    /// Hooks in the observer `observer` for the duration of `f`.
    pub fn observe<R>(observer: Observer, f: impl FnOnce() -> R) -> R {
        OBSERVE.with(|q| *q.borrow_mut() = Some(observer));
        let r = f();
        OBSERVE.with(|q| *q.borrow_mut() = None);
        r
    }

    /// Hooks in `hook` for the duration of `f`.
    pub fn with<R>(hook: Hook, f: impl FnOnce() -> R) -> R {
        QUERY.with(|q| *q.borrow_mut() = Some(hook));
        let r = f();
        QUERY.with(|q| *q.borrow_mut() = None);
        r
    }
}

/// `OnMessage(envelope)` (slot 12); the base reports `true` and does nothing.
pub fn on_message(sim: &mut Sim, id: ObjectId, index: usize, env: &Envelope) -> bool {
    match sim.module(id, index).map(|m| &m.kind) {
        Some(ModuleKind::Walk(_)) => walk::on_message(sim, id, env),
        Some(ModuleKind::Lumber(_)) => gather::lumber::on_message(sim, id, env),
        Some(ModuleKind::Mining(_)) => gather::mining::on_message(sim, id, env),
        Some(ModuleKind::Harvest(_)) => gather::harvest::on_message(sim, id, env),
        Some(ModuleKind::Fishing(_)) => gather::fishing::on_message(sim, id, env),
        Some(ModuleKind::Hunting(_)) => gather::on_command(sim, id, gather::hunting::INDEX, env),
        Some(ModuleKind::Construct(_)) => construct::on_message(sim, id, env),
        Some(ModuleKind::Attack(_)) => attack::on_message(sim, id, env),
        Some(ModuleKind::FightNear(_)) => fight_near::on_message(sim, id, env),
        Some(ModuleKind::FightFar(_)) => fight_far::on_message(sim, id, env),
        Some(ModuleKind::Shoot(_)) => shoot::on_message(sim, id, env),
        Some(ModuleKind::Dying(_)) => dying::on_message(sim, id, env),
        Some(ModuleKind::Riding(_)) => riding::on_message(sim, id, index, env),
        Some(ModuleKind::Ghost(_)) => ghosts::ghost::on_message(sim, id, env),
        Some(ModuleKind::EvokeAnimal(_)) => evoke_animal::on_message(sim, id, env),
        Some(ModuleKind::Spell(_)) => spell::on_message(sim, id, env),
        _ => true,
    }
}

/// `Tick(dt)` (Slot 13).
pub fn tick(sim: &mut Sim, id: ObjectId, index: usize, dt: i32) {
    match sim.module(id, index).map(|m| &m.kind) {
        Some(ModuleKind::Walk(_)) => walk::tick(sim, id),
        Some(ModuleKind::Stand(_)) => stand::tick(sim, id, dt),
        Some(ModuleKind::Natur(_)) => natur::tick(sim, id, dt),
        Some(ModuleKind::Building(_)) => building::tick(sim, id, dt),
        Some(ModuleKind::Tree(_)) => tree::tick(sim, id),
        Some(ModuleKind::Lumber(_)) => gather::lumber::tick(sim, id, dt),
        Some(ModuleKind::Mining(_)) => gather::mining::tick(sim, id, dt),
        Some(ModuleKind::Harvest(_)) => gather::harvest::tick(sim, id, dt),
        Some(ModuleKind::Fishing(_)) => gather::fishing::tick(sim, id, dt),
        Some(ModuleKind::Quarter(_)) => quarter::tick(sim, id, dt),
        Some(ModuleKind::SelfHealing(_)) => selfheal::tick(sim, id),
        Some(ModuleKind::Construct(_)) => construct::tick(sim, id, dt),
        Some(ModuleKind::Attack(_)) => attack::tick(sim, id, dt),
        Some(ModuleKind::FightNear(_)) => fight_near::tick(sim, id, dt),
        Some(ModuleKind::Dying(_)) => dying::tick(sim, id, dt),
        Some(ModuleKind::Shoot(_)) => shoot::tick(sim, id, dt),
        Some(ModuleKind::Animal(_)) => animal::tick(sim, id, dt),
        Some(ModuleKind::Edible(_)) => edible::tick(sim, id, dt),
        Some(ModuleKind::Hunting(_)) => gather::hunting::tick(sim, id, dt),
        Some(ModuleKind::Riding(_)) => riding::tick(sim, id, index, dt),
        Some(ModuleKind::Swim(_)) => swim::tick(sim, id, dt),
        Some(ModuleKind::Diving(_)) => diving::tick(sim, id, dt),
        Some(ModuleKind::Train(_)) => train::tick(sim, id, dt),
        Some(ModuleKind::MultiHead(_)) => multipart::tick_head(sim, id, dt),
        Some(ModuleKind::Spell(_)) => spell::tick(sim, id, dt),
        Some(ModuleKind::ExecTempUpgrade(_)) => exec_temp::tick(sim, id, dt),
        Some(ModuleKind::Tent(_)) => tent::tick(sim, id, dt),
        Some(ModuleKind::Trading(_)) => trading::tick(sim, id, dt),
        Some(ModuleKind::Masking(_)) => masking::tick(sim, id, dt),
        Some(ModuleKind::Assassin(_)) => assassin::tick(sim, id, dt),
        Some(ModuleKind::Healing(_)) => healing::tick(sim, id, dt),
        Some(ModuleKind::Ghost(_)) => ghosts::ghost::tick(sim, id, dt),
        // Modules 26 and 31 only run down the timers in the original (slot 13 `0x005119A0`).
        Some(ModuleKind::EvokeGhost(_) | ModuleKind::Banish(_)) => {
            events::update_events(sim, id, index, dt)
        }
        Some(ModuleKind::EvokeAnimal(_)) => evoke_animal::tick(sim, id, dt),
        // Tick of the special attacks: only `Module_UpdateEvents` (slot 13 `0x005119A0`).
        Some(ModuleKind::Bomberman(_) | ModuleKind::Killer(_) | ModuleKind::SpecialMove(_)) => {
            events::update_events(sim, id, index, dt)
        }
        Some(ModuleKind::HeadHunter(_)) => headhunter::tick(sim, id, dt),
        Some(ModuleKind::Robbing(_)) => robbing::tick(sim, id, dt),
        // Without tick: the rear part (`0x004FF840`), module 30 and module 35 (slot 13 empty).
        Some(
            ModuleKind::Resource(_)
            | ModuleKind::Inert
            | ModuleKind::FightFar(_)
            | ModuleKind::MultiPart(_)
            | ModuleKind::GlobalUpgrade(_)
            | ModuleKind::TransportTent(_),
        )
        | None => {}
    }
}

/// Frame update (slot 14 `FrameUpdate`) in the object tick instead of per drawn frame of visible
/// objects. `GameObject_FrameUpdate` (`0x00470BD0`) calls `vf+0x38` of the modules of the
/// tick list `+0x758` in their order; only walk (walking and turning animations) and stand
/// (idle animation) have a frame update here, other entries stay without effect.
pub fn frame_update(sim: &mut Sim, id: ObjectId) {
    let list = sim
        .object(id)
        .map_or_else(Vec::new, |o| o.tick_list.clone());
    for n in list {
        let index = usize::from(n);
        match (index, sim.module(id, index).map(|m| &m.kind)) {
            (walk::INDEX, Some(ModuleKind::Walk(_))) => walk::frame_animation(sim, id),
            (stand::INDEX, Some(ModuleKind::Stand(_))) => stand::frame_update(sim, id),
            _ => {}
        }
    }
}

/// `Abort()` (slot 11); the base reports `true` (also the dying module).
pub fn abort(sim: &mut Sim, id: ObjectId, index: usize) -> bool {
    match sim.module(id, index).map(|m| &m.kind) {
        Some(ModuleKind::Walk(_)) => walk::abort(sim, id),
        Some(ModuleKind::Lumber(_)) => gather::lumber::abort(sim, id),
        Some(ModuleKind::Mining(_)) => gather::mining::abort(sim, id),
        Some(ModuleKind::Harvest(_)) => gather::harvest::abort(sim, id),
        Some(ModuleKind::Fishing(_)) => gather::fishing::abort(sim, id),
        Some(ModuleKind::Hunting(_)) => gather::hunting::abort(sim, id),
        Some(ModuleKind::Quarter(_)) => quarter::abort(sim, id),
        Some(ModuleKind::Construct(_)) => construct::abort(sim, id),
        Some(ModuleKind::Attack(_)) => attack::abort(sim, id),
        Some(ModuleKind::FightNear(_)) => fight_near::abort(sim, id),
        Some(ModuleKind::FightFar(_)) => fight_far::abort(sim, id),
        Some(ModuleKind::Shoot(_)) => shoot::abort(sim, id),
        Some(ModuleKind::Riding(_)) => riding::abort(sim, id, index),
        Some(ModuleKind::Spell(_)) => spell::abort(sim, id),
        Some(ModuleKind::EvokeGhost(_)) => ghosts::evoke::abort(sim, id),
        Some(ModuleKind::Banish(_)) => ghosts::banish::abort(sim, id),
        Some(ModuleKind::EvokeAnimal(_)) => evoke_animal::abort(sim, id),
        Some(ModuleKind::Bomberman(_)) => special::bomberman::abort(sim, id),
        Some(ModuleKind::Killer(_)) => special::killer::abort(sim, id),
        Some(ModuleKind::SpecialMove(_)) => special::special_move::abort(sim, id),
        Some(ModuleKind::Robbing(_)) => robbing::abort(sim, id),
        _ => true,
    }
}

/// "Busy?" (slot 10): the shoot module reports `m_IsBusy` (`0x005248A0`), the riding module
/// its own (`0x005219A0`), the camouflage module its own (`0x0051D7E0`, re/masking 1.2: the FIFO waits in the
/// camouflage and uncamouflage animation), all others the base `false` (also module 28, dig in, whose
/// `m_IsBusy` is never set, and module 29, healing).
pub fn is_busy(sim: &Sim, id: ObjectId, index: usize) -> bool {
    match sim.module(id, index).map(|m| &m.kind) {
        Some(ModuleKind::Shoot(s)) => s.is_busy,
        Some(ModuleKind::Riding(r)) => r.busy,
        Some(ModuleKind::Masking(m)) => m.is_busy,
        _ => false,
    }
}

/// `GameObject_SetMatchBits`/`ClearMatchBits` (`0x00467FC0`/`0x00468000`): low byte `0xFF` ->
/// category bits (`+0x330`), otherwise the flags of module `mid & 0xFF`; the bits
/// `mid & 0xFFFF_FF00` are changed.
pub fn set_match_bits(sim: &mut Sim, id: ObjectId, mid: u32, on: bool) {
    let bits = mid & 0xFFFF_FF00;
    let index = (mid & 0xFF) as usize;
    if index == 0xFF {
        if let Some(o) = sim.object_mut(id) {
            if on {
                o.category |= bits;
            } else {
                o.category &= !bits;
            }
        }
    } else if let Some(m) = sim.module_mut(id, index) {
        if on {
            m.base.flags |= bits;
        } else {
            m.base.flags &= !bits;
        }
    }
}

/// Change of owner (slot 5) with old and new ID (`id` is the new one): harvest rewrites
/// its farm slot, walk leaves its group with the old ID and a quartered occupant reports the new ID to
/// its carrier (`PropertyWalk_OnOwnerChanged`), quarters pass it on to their occupants, a building to the
/// wall group and farm workers, a carcass keeps the color of the old owner, module 30 queues the removal of its
/// upgrade at the old slot, module 44 deletes its timer group `0x80` and removes its influences
/// with the filter of the old slot, the travois (module 35)
/// queues its tent for destruction on the change to slot 8.
pub fn on_owner_changed(sim: &mut Sim, id: ObjectId, index: usize, old: ObjectId) {
    match sim.module(id, index).map(|m| &m.kind) {
        Some(ModuleKind::Harvest(_)) => gather::harvest::on_owner_changed(sim, id, old),
        Some(ModuleKind::Walk(_)) => walk::formation::on_owner_changed(sim, id, old),
        Some(ModuleKind::Quarter(_)) => quarter::on_owner_changed(sim, id),
        Some(ModuleKind::Building(_)) => building::on_owner_changed(sim, id),
        Some(ModuleKind::Dying(_)) => dying::on_owner_changed(sim, id, old),
        Some(ModuleKind::MultiHead(_)) => multipart::on_owner_changed_head(sim, id, old),
        Some(ModuleKind::MultiPart(_)) => multipart::on_owner_changed_part(sim, id, old),
        Some(ModuleKind::GlobalUpgrade(_)) => global_upgrade::on_owner_changed(sim, id, old),
        Some(ModuleKind::ExecTempUpgrade(_)) => exec_temp::on_owner_changed(sim, id, old),
        Some(ModuleKind::TransportTent(_)) => tent::transport::on_owner_changed(sim, id),
        _ => {}
    }
}

/// Player data changed (slot 8, after a variant switch): quarters read their capacity
/// again (`PropertyQuarter_OnPlayerDataChanged` `0x0051FC90`), walk the base speed
/// (`PropertyWalk_OnPlayerTypeDataChanged` `0x00532810`).
pub fn on_player_data_changed(sim: &mut Sim, id: ObjectId, index: usize) {
    if index == quarter::INDEX {
        quarter::on_player_data_changed(sim, id);
    }
    if index == walk::INDEX {
        let v = crate::fx::fixed_div(sim.attribute(id, 16), crate::motion::speed::TIME_UNIT);
        if let Some(w) = walk::walk_mut(sim, id) {
            w.speed.set_default_speed(v);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    #[test]
    fn inert_modules_load_their_flags_from_the_catalogue() {
        let desc = |id: u32, v: &str| ModuleDesc {
            index: 0,
            mask: u32::MAX,
            values: BTreeMap::from([(id, v.to_string())]),
        };
        assert_eq!(init_flags(0, &desc(10003, "2048")), 2048);
        assert_eq!(init_flags(4, &desc(50003, "0")), 0x104);
        assert_eq!(init_flags(7, &desc(80004, "256")), 256);
        assert_eq!(init_flags(55, &desc(80004, "256")), 0);
        assert_eq!(init_flags(55, &desc(80001, "256")), 256);
        assert_eq!(init_flags(23, &desc(1, "7")), 0x100);
        assert_eq!(init_flags(40, &desc(410001, "50")), 0);
        assert!(has_class(0) && has_class(55) && !has_class(9) && !has_class(41));
        assert!(!has_class(50) && !has_class(56));
    }

    #[test]
    fn tent_and_trade_modules_are_created_with_their_data() {
        use self::tent::testkit::{DWELLING_TENT, TRAVOIS, TYPES, rules_from, spawn, state};
        // Additional type with module 43 and tax data (data values of the analysis re/trading 1.4).
        let types = format!(
            "{TYPES}{}",
            concat!(
                "GUID 1111\nNAME Steuerkarren\nVALUEID 100 6 1 0\nPROPID 43\n",
                "VALUEID 440001 1 1 2\nVALUEID 440002 1 1 1100 1102\nVALUEID 440003 1 1 5\n",
            )
        );
        let rules = rules_from(&types);
        let mut state = state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        // Dwelling tent: travois GUID from 430001, resting (packing: `tent::unit_tests`).
        let tent_id = spawn(&mut sim, DWELLING_TENT, 0, (10, 10));
        let o = sim.object(tent_id).unwrap();
        assert_eq!(
            o.module(tent::INDEX).unwrap().kind,
            ModuleKind::Tent(tent::Tent {
                travois_guid: TRAVOIS,
                status: tent::status::REST,
                ..Default::default()
            })
        );
        assert_eq!(o.module(tent::INDEX).unwrap().base.flags, 0);
        // Travois: module 35 without tent, flags from 360001 (shipped data 0).
        let travois = spawn(&mut sim, TRAVOIS, 0, (14, 10));
        let m35 = sim.module(travois, tent::transport::INDEX).unwrap();
        assert_eq!(
            m35.kind,
            ModuleKind::TransportTent(tent::transport::TransportTent {
                tent: ObjectId::NONE
            })
        );
        assert_eq!(m35.base.flags, 0);
        // Cart with tax data: interval in ms, payer list, maximum tax, tax level 50.
        let cart = spawn(&mut sim, 1111, 0, (18, 10));
        assert_eq!(
            sim.module(cart, trading::INDEX).unwrap().kind,
            ModuleKind::Trading(trading::Trading {
                tax_interval: 2000,
                payer_guids: vec![1100, 1102],
                max_tax: 5,
                tax_level: 50,
                ..Default::default()
            })
        );
        // Cart 1110 of the shipped data: all data values 0, payer list `[0]` (440002 = "0").
        let cart = spawn(&mut sim, tent::testkit::CART, 1, (22, 10));
        let ModuleKind::Trading(t) = &sim.module(cart, trading::INDEX).unwrap().kind else {
            panic!("module 43 is not trading");
        };
        assert_eq!(
            (t.max_tax, t.tax_interval, t.tax_level, &t.payer_guids[..]),
            (0, 0, 50, &[0][..])
        );
    }

    #[test]
    fn upgrade_modules_are_created_with_their_data() {
        use crate::rules::Rules;
        use crate::state::SimState;
        use crate::world::{ObjectDesc, create_object};
        // Type with the modules 30, 38 and 44 (values from property.ini: missionary 2840, governor).
        let types = concat!(
            "GUID 1112\nNAME Gouverneur\nVALUEID 100 6 1 0\nPROPID 0\nPROPID 2\n",
            "VALUEID 30001 1 1 0\nPROPID 30\nVALUEID 310001 1 1 2840\nPROPID 38\n",
            "VALUEID 390001 6 1 0\nVALUEID 390002 1 1 7 7 7\nVALUEID 390003 1 1 5 6 11\n",
            "PROPID 44\nVALUEID 450001 1 1 4452 4450 4451\n",
        );
        let rules = Rules::synthetic(types, 16);
        let mut state = SimState::new(0, 0);
        let mut sim = Sim::new(&mut state, &rules);
        let id =
            create_object(&mut sim, &mut ObjectDesc::new(1112, 0, [100.0, 0.0, 100.0])).unwrap();
        let o = sim.object(id).unwrap();
        let kind = |index: usize| &o.module(index).unwrap().kind;
        assert_eq!(
            kind(global_upgrade::INDEX),
            &ModuleKind::GlobalUpgrade(global_upgrade::GlobalUpgrade { upgrade_guid: 2840 })
        );
        let ModuleKind::Spell(s) = kind(spell::INDEX) else {
            panic!("module 38 is not a spell");
        };
        assert_eq!(
            **s,
            spell::Spell {
                anims: vec![7, 7, 7],
                effects: vec![5, 6, 11],
                ..Default::default()
            }
        );
        assert_eq!(
            kind(exec_temp::INDEX),
            &ModuleKind::ExecTempUpgrade(exec_temp::ExecTempUpgrade {
                exec_list: vec![4452, 4450, 4451],
                influence_list: Vec::new(),
                ..Default::default()
            })
        );
        // Module 30 answers like an idle module (spell and execute: `spell`, `exec_temp`).
        let index = global_upgrade::INDEX;
        for code in [0x126, 0x12C, 0x226] {
            assert_eq!(
                query(&mut sim, id, index, &mut Message::new(code, &[1, 0])),
                -1
            );
        }
        // "Busy?" reports the base for all three modules (spell slot 10 `0x00532A90`).
        for index in [global_upgrade::INDEX, spell::INDEX, exec_temp::INDEX] {
            assert!(!is_busy(&sim, id, index));
        }
        assert!(abort(&mut sim, id, index));
    }
}
