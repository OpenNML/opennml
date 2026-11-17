//! Fog memories under the fog (re/vision 9): the snapshot (`FogMemory_TakeSnapshot` `0x00415920`) and the
//! copy of the hit filter on it (`FogMemory_MatchesFilter` `0x00415AC0`). The memories themselves are host state
//! per viewer; only data and pure functions live here.

use nml_formats::targets::MatchFilter;

use crate::id::ObjectId;
use crate::object::{GameObject, MODULE_SLOTS, status};

use super::FogView;

/// Snapshot of an object with module 3, 4, 6, `0x29` or `0x32`.
#[derive(Debug, Clone, PartialEq)]
pub struct MemorySnapshot {
    pub id: ObjectId,
    pub guid: u32,
    /// Tile of the memory (`+8/+0xC`), set on creation.
    pub tile: (i32, i32),
    pub world_pos: [f32; 3],
    pub rotation: [f32; 3],
    pub scale: [f32; 3],
    /// Render bits (`+0x70`) and progress (`+0x74`).
    pub render_bits: u32,
    pub progress: f32,
    /// State (`+0x78`).
    pub status: u32,
    /// Selectable (`+0x80`).
    pub selectable: bool,
    /// "Life not full" (`+0x88`) and "extra energy not full" (`+0x89`).
    pub hp_lost: bool,
    pub extra_lost: bool,
    /// Load: amount, kind (`+0x8C/+0x90`).
    pub load: (i32, i32),
    /// `+0xD0 + 4n`: entry 0 the category, otherwise the flags of the present modules.
    pub flags: [u32; MODULE_SLOTS],
    /// `+0x94 + n`: module n present; the original never sets entry 0.
    pub present: [bool; MODULE_SLOTS],
}

impl MemorySnapshot {
    /// `0x00415920` over the previous snapshot: flags of modules no longer present stay as they are, only
    /// their present flag drops.
    pub fn take(o: &GameObject, tile: (i32, i32), previous: Option<&MemorySnapshot>) -> Self {
        let mut flags = previous.map_or([0; MODULE_SLOTS], |p| p.flags);
        let mut present = [false; MODULE_SLOTS];
        flags[0] = o.category;
        for n in 1..MODULE_SLOTS {
            if o.has_module(n, false) {
                present[n] = true;
                flags[n] = o.module(n).map_or(0, |m| m.base.flags);
            }
        }
        let a = &o.attributes;
        Self {
            id: o.id,
            guid: o.guid,
            tile,
            world_pos: o.world_pos,
            rotation: o.rotation,
            scale: o.scale,
            render_bits: o.render_bits,
            progress: o.progress_scale,
            status: o.status,
            selectable: o.selectable,
            hp_lost: a.is_present(14) && !a.is_full(14),
            extra_lost: a.is_present(15) && !a.is_full(15),
            load: (o.load_amount, o.load_kind),
            flags,
            present,
        }
    }
}

/// `FogMemory_MatchesFilter` (`0x00415AC0`) for the TARGET callers (`0x0047ACF0`,
/// `0x0047D770`): `f[6]` = `fog_player` on the head filter, follow-up filters with their parser value. Same
/// chain as [`crate::filter::matches_target`], but only on the data of the snapshot.
pub fn memory_matches(
    snap: &MemorySnapshot,
    view: &FogView,
    filter: &MatchFilter,
    player: i32,
    fog_player: i32,
) -> bool {
    let mut f = filter;
    let mut player = player;
    let mut fog = fog_player;
    loop {
        let b = evaluate(snap, view, f, &mut player, fog);
        let m = f.mode;
        let next = f.next.as_deref();
        if m & 0x5000_0000 != 0 {
            if !b {
                return false;
            }
            let Some(next) = next else { return false };
            if m & 0x1000_0000 != 0 {
                f = next;
                fog = next.fog_player;
                continue;
            }
            return !memory_matches(snap, view, next, player, next.fog_player);
        }
        if m & 0xA000_0000 != 0 {
            if b {
                return true;
            }
            let Some(next) = next else { return false };
            if m & 0x2000_0000 != 0 {
                f = next;
                fog = next.fog_player;
                continue;
            }
            return !memory_matches(snap, view, next, player, next.fog_player);
        }
        return b;
    }
}

/// One filter without chaining (order of `0x00415AC0`).
fn evaluate(
    snap: &MemorySnapshot,
    view: &FogView,
    f: &MatchFilter,
    player: &mut i32,
    fog_player: i32,
) -> bool {
    let st = snap.status;
    if f.mode & 0x400_0000 == 0 && st & status::DIED != 0 {
        return false;
    }
    if f.mode2 & 0x4 == 0 && st & status::QUARTERED != 0 {
        return false;
    }
    if *player < 0 {
        *player = i32::from(snap.id.slot());
    }
    if f.mode & 0x800_0000 != 0 {
        return false;
    }
    if f.mode2 & 0x80 != 0 {
        return true;
    }
    let and = f.mode & 0x10 != 0;
    macro_rules! gate {
        ($b:expr) => {{
            let b = $b;
            if and != b {
                return b;
            }
        }};
    }
    // 1. State: ridden/carried (0x2000/0x8000) never (no carrier in the memory).
    if f.mode & 0x2001 != 0 {
        let b = st & 0xA000 == 0 && f.status & st != 0;
        gate!(b != (f.mode & 0x2000 != 0));
    }
    // 2. Type bits: 0xFF = category (entry 0), otherwise flags of a present module.
    if f.mode & 0x4042 != 0 {
        let m = (f.match_bits & 0xFF) as usize;
        let bits = f.match_bits & !0xFF;
        let have = if m == 0xFF {
            Some(snap.flags[0])
        } else if m < MODULE_SLOTS && snap.present[m] {
            Some(snap.flags[m])
        } else {
            None
        };
        let b = have.is_some_and(|h| {
            let h = h & !0xFF;
            if f.mode & 0x40 != 0 {
                h & bits == bits
            } else if f.mode & 0x80 != 0 {
                bits == h
            } else {
                bits & h != 0
            }
        });
        gate!(b != (f.mode & 0x4000 != 0));
    }
    // 3. Module present.
    if f.mode & 0x8004 != 0 {
        let p = f.prop_id as usize;
        let b = p < MODULE_SLOTS && snap.present[p];
        gate!(b != (f.mode & 0x8000 != 0));
    }
    // 4. Type GUID.
    if f.mode & 0x10008 != 0 {
        gate!((f.guid as u32 == snap.guid) != (f.mode & 0x10000 != 0));
    }
    // 5. Not too many opponents: never in AND mode, neutral in OR mode.
    if f.mode2 & 0x40 != 0 && and {
        return false;
    }
    // 6. Load.
    if f.mode & 0x70_0000 != 0 {
        let (amount, kind) = snap.load;
        let b = amount > 0
            && ((f.mode & 0x40_0000 != 0 && kind == 2)
                || (f.mode & 0x10_0000 != 0 && kind == 0)
                || (f.mode & 0x20_0000 != 0 && kind == 1));
        gate!(b != (f.mode & 0x2_0000 != 0));
    }
    // 7. Player lists (without the special case "without building/walk" of the object filter).
    {
        let owner = i32::from(snap.id.slot());
        let id_bits = snap.id.0 & 0xF800;
        let d = view.diplomacy;
        let (mut any, mut b) = (false, false);
        if f.mode & 0x100 != 0 {
            any = true;
            b = owner == *player;
        }
        if f.mode & 0x200 != 0 {
            any = true;
            b |= id_bits == 0x4000;
        }
        if f.mode & 0x1000 != 0 {
            any = true;
            b |= id_bits == 0x4800;
        }
        if f.mode & 0x800 != 0 {
            any = true;
            b |= d.is_friend(*player, owner);
        }
        if f.mode & 0x400 != 0 {
            any = true;
            b |= d.is_enemy(*player, owner);
        }
        if f.mode & 0x4_0000 != 0 {
            b |= id_bits < 0x4000;
        } else if !any {
            b = and;
        }
        gate!(b);
    }
    // 8. Hit points not full (flag).
    if f.mode & 0x80_0000 != 0 {
        gate!(snap.hp_lost);
    }
    // 9. Fog: not camouflaged is neutral; camouflaged needs "explored" at the tile of the memory, with
    // WAITING4KILLING never (quirk). No check `f[6] < 8`.
    if f.mode & 0x200_0000 != 0 && st & status::MASKED != 0 {
        let fog = view.state_at(snap.tile.0, snap.tile.1, fog_player);
        gate!(st & status::WAITING4KILLING == 0 && fog & 1 != 0);
    }
    // 10. Built-in `mode & 0x80000`: never. This holds only for stored memories; living objects check
    // the bit as "marked by the AI" (`filter::evaluate`, re/ki-queries 5.6, reads `f[6]` of the filter
    // being evaluated).
    if f.mode & 0x8_0000 != 0 {
        gate!(false);
    }
    // 11. Extra energy not full (flag).
    if f.mode2 & 0x1 != 0 {
        gate!(snap.extra_lost);
    }
    // 12.-15. CALLMETHOD, is attacking, built-in distance check, `mode2 & 8`: never.
    for never in [
        f.mode2 & 0x2 != 0,
        f.mode2 & 0x400 != 0,
        f.mode & 0x20 != 0,
        f.mode2 & 0x8 != 0,
    ] {
        if never {
            gate!(false);
        }
    }
    // 16. Shootable: true in OR mode.
    if f.mode2 & 0x20 != 0 {
        gate!(true);
    }
    // 17. Can fight at range: false (without `gate`).
    if f.mode2 & 0x100 != 0 {
        return false;
    }
    and
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::command::testkit as kit;
    use crate::object::status;
    use crate::sim::Sim;
    use nml_formats::targets::{parse_target_defines, parse_targets};

    fn filter(lines: &str) -> MatchFilter {
        let d = parse_target_defines(kit::DEFINES);
        let text = format!("BEGIN\nMY_MM\n{lines}\nMM_END\nEND\n");
        parse_targets(&text, &d).entries.remove(0).my
    }

    #[test]
    fn snapshots_keep_category_module_flags_and_markers() {
        let rules = kit::rules();
        let mut state = kit::fogged_state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let store = kit::spawn(&mut sim, kit::STORE, 0, (20, 20));
        let o = sim.object(store).unwrap().clone();
        let snap = MemorySnapshot::take(&o, (20, 20), None);
        assert_eq!(
            (snap.id, snap.guid, snap.tile, snap.status),
            (store, kit::STORE, (20, 20), o.status)
        );
        assert!(snap.present[3] && !snap.present[0] && !snap.present[4]);
        assert_eq!(snap.flags[0], o.category);
        assert_eq!(snap.flags[3], o.module(3).unwrap().base.flags);
        // A vanished module keeps its old flags (only the present byte drops).
        let mut o2 = o.clone();
        o2.modules[3] = None;
        let again = MemorySnapshot::take(&o2, (20, 20), Some(&snap));
        assert!(!again.present[3] && again.flags[3] == snap.flags[3]);
    }

    #[test]
    fn the_filter_copy_judges_the_snapshot_like_0x00415ac0() {
        let rules = kit::rules();
        let mut state = kit::fogged_state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let tree = kit::spawn(&mut sim, kit::TREE, 9, (30, 30));
        let mut snap = MemorySnapshot::take(sim.object(tree).unwrap(), (30, 30), None);
        let view = FogView::new(&*sim.state);
        let m = |f: &str, s: &MemorySnapshot| memory_matches(s, &view, &filter(f), 0, 0);
        // Fog: neutral for non-camouflaged memories (AND and OR).
        assert!(m("MM_MODE MM_ANDALL\nMM_MODE MM_CHECKFOG", &snap));
        assert!(!m("MM_MODE MM_CHECKFOG", &snap));
        // Module present from the present bytes, module 0 never.
        assert!(m("MM_MODE MM_CHECKPROPERTY\nMM_PROPID PROP_TREE", &snap));
        assert!(!m("MM_MODE MM_CHECKPROPERTY\nMM_PROPID PROP_PTTOPT", &snap));
        // Camouflaged: explored required, never with WAITING4KILLING.
        snap.status |= status::MASKED;
        let fog_and = "MM_MODE MM_ANDALL\nMM_MODE MM_CHECKFOG";
        assert!(!m(fog_and, &snap));
        let mut fog = crate::fog::Fog::new(64, 64);
        fog.explored.or(30, 30, 1);
        let d = crate::state::Diplomacy::default();
        let explored = FogView {
            fog: &fog,
            diplomacy: &d,
            show_mode: 0,
        };
        assert!(memory_matches(&snap, &explored, &filter(fog_and), 0, 0));
        snap.status |= status::WAITING4KILLING;
        assert!(!memory_matches(&snap, &explored, &filter(fog_and), 0, 0));
        // CALLMETHOD, IS_ATTACKING and built-in distance check: never; SHOOTABLE true in OR mode.
        assert!(!m(
            "MM_MODE MM_ANDALL\nMM_MODE2 MM2_CALLMETHOD\nMM_METHODINDEX 2",
            &snap
        ));
        assert!(m("MM_MODE2 MM2_SHOOTABLE", &snap));
        // The dead drop out, except with MM_RETURN_DIED.
        snap.status = status::DIED;
        assert!(!m("MM_MODE2 MM2_RETURNTRUE", &snap));
        assert!(m("MM_MODE MM_RETURN_DIED\nMM_MODE2 MM2_RETURNTRUE", &snap));
    }
}
