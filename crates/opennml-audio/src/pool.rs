//! Channel allocation of the game (`SoundManager_AllocateChannel` `0x00551AC0`, re/audio,
//! section "Channels, categories, limits"): 32 slots, slot index = FMOD channel. Hardware channels
//! (`0x400`) are dropped in the reimplementation; all calls take the software slots.

/// Channels (`FSOUND_Init(44100, 32, 0)`, no hardware channels).
pub const CHANNELS: usize = 32;

/// Flag word of a playback call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Flags(pub u32);

impl Flags {
    /// Bits 0-3: rank when displacing; 8 = exclusive, 9 = aborts 8.
    pub fn category(self) -> u32 {
        self.0 & 0xF
    }

    /// Bits 4-7: at most this many simultaneous instances of the same sample (0 = unlimited).
    pub fn instance_limit(self) -> u32 {
        (self.0 >> 4) & 0xF
    }

    /// `0x100`: at the limit, move the most distant instance to the new position.
    pub fn moves(self) -> bool {
        self.0 & 0x100 != 0
    }

    /// `0x200`: at the limit, displace the most distant instance.
    pub fn displaces(self) -> bool {
        self.0 & 0x200 != 0
    }
}

/// Occupied slot from the allocation's point of view.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SlotInfo {
    pub category: u32,
    /// 3D: distance to the listener at the start (or after moving), 2D: 0.
    pub priority: f32,
    /// ID of the sample/stream (instance limit).
    pub sample: u64,
}

/// Result of the allocation.
#[derive(Debug, Clone, PartialEq)]
pub enum Allocation {
    /// Slot for the new sound; `stopped` = slots stopped beforehand (category 9 rule,
    /// displacing), in this order.
    Slot { slot: usize, stopped: Vec<usize> },
    /// Instance limit reached, slot `slot` is moved to the new position (message
    /// `0x20001`); no new sound.
    Moved { slot: usize, stopped: Vec<usize> },
    /// No sound.
    Rejected { stopped: Vec<usize> },
}

/// The slots.
#[derive(Debug, Clone, PartialEq)]
pub struct ChannelPool {
    slots: Vec<Option<SlotInfo>>,
}

impl Default for ChannelPool {
    fn default() -> Self {
        Self::new()
    }
}

impl ChannelPool {
    pub fn new() -> Self {
        Self {
            slots: vec![None; CHANNELS],
        }
    }

    pub fn get(&self, slot: usize) -> Option<&SlotInfo> {
        self.slots[slot].as_ref()
    }

    /// Occupies a slot (after [`Allocation::Slot`]).
    pub fn occupy(&mut self, slot: usize, info: SlotInfo) {
        self.slots[slot] = Some(info);
    }

    /// Releases a slot (sound finished or stopped).
    pub fn free(&mut self, slot: usize) {
        self.slots[slot] = None;
    }

    /// Priority after moving (message `0x20001` sets the new distance).
    pub fn set_priority(&mut self, slot: usize, priority: f32) {
        if let Some(s) = &mut self.slots[slot] {
            s.priority = priority;
        }
    }

    /// Allocation as in the original; stopped and displaced slots are free afterwards.
    pub fn allocate(
        &mut self,
        flags: Flags,
        priority: f32,
        sample: u64,
        positioned: bool,
    ) -> Allocation {
        let category = flags.category();
        let mut stopped = Vec::new();
        let occupied = |s: &Self| -> Vec<(usize, SlotInfo)> {
            s.slots
                .iter()
                .enumerate()
                .filter_map(|(i, x)| x.map(|x| (i, x)))
                .collect()
        };
        if category == 8
            && occupied(self)
                .iter()
                .any(|(_, s)| matches!(s.category, 8 | 9))
        {
            return Allocation::Rejected { stopped };
        }
        if category == 9 {
            for (i, s) in occupied(self) {
                if s.category == 8 {
                    self.slots[i] = None;
                    stopped.push(i);
                }
            }
        }
        let limit = flags.instance_limit();
        if limit != 0 {
            // Count instances of the same sample and remember the most distant one that is farther away
            // than the new one.
            let mut left = limit as i32;
            let mut farthest: Option<(usize, f32)> = None;
            for (i, s) in occupied(self) {
                if s.sample == sample {
                    left -= 1;
                    let bound = farthest.map_or(priority, |(_, p)| p);
                    if bound < s.priority {
                        farthest = Some((i, s.priority));
                    }
                }
            }
            if left < 1 {
                if flags.moves() && positioned {
                    return match farthest {
                        Some((slot, _)) => {
                            self.set_priority(slot, priority);
                            Allocation::Moved { slot, stopped }
                        }
                        None => Allocation::Rejected { stopped },
                    };
                }
                return match farthest.filter(|_| flags.displaces()) {
                    Some((slot, _)) => {
                        self.slots[slot] = None;
                        stopped.push(slot);
                        Allocation::Slot { slot, stopped }
                    }
                    None => Allocation::Rejected { stopped },
                };
            }
        }
        // First free slot, otherwise the one with the lowest category below the new one.
        let mut lowest: Option<(usize, u32)> = None;
        for (i, s) in self.slots.iter().enumerate() {
            let Some(s) = s else {
                return Allocation::Slot { slot: i, stopped };
            };
            if s.category < lowest.map_or(category, |(_, c)| c) {
                lowest = Some((i, s.category));
            }
        }
        if let Some((slot, _)) = lowest {
            self.slots[slot] = None;
            stopped.push(slot);
            return Allocation::Slot { slot, stopped };
        }
        // Same category with the greatest distance, if farther away than the new sound.
        let mut farthest: Option<(usize, f32)> = None;
        for (i, s) in occupied(self) {
            if s.category == category && farthest.map_or(priority, |(_, p)| p) < s.priority {
                farthest = Some((i, s.priority));
            }
        }
        match farthest {
            Some((slot, _)) => {
                self.slots[slot] = None;
                stopped.push(slot);
                Allocation::Slot { slot, stopped }
            }
            None => Allocation::Rejected { stopped },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn info(category: u32, priority: f32, sample: u64) -> SlotInfo {
        SlotInfo {
            category,
            priority,
            sample,
        }
    }

    /// Occupies all 32 slots with category 3, distance = slot, sample 100 + slot.
    fn full() -> ChannelPool {
        let mut pool = ChannelPool::new();
        for i in 0..CHANNELS {
            pool.occupy(i, info(3, i as f32, 100 + i as u64));
        }
        pool
    }

    #[test]
    fn allocation_follows_the_original_order() {
        // (initial state, flags, priority, sample, positioned) -> result.
        let mut cases: Vec<(ChannelPool, u32, f32, u64, bool, Allocation)> = Vec::new();
        let mut one = ChannelPool::new();
        one.occupy(0, info(1, 0.0, 1));
        cases.push((
            one.clone(),
            0x12,
            0.0,
            2,
            false,
            Allocation::Slot {
                slot: 1,
                stopped: vec![],
            },
        ));
        // Full: the lowest category below the new one is displaced (the first on a tie).
        let mut f = full();
        f.occupy(7, info(1, 0.0, 9));
        f.occupy(9, info(1, 0.0, 9));
        cases.push((
            f,
            0x13,
            5.0,
            2,
            true,
            Allocation::Slot {
                slot: 7,
                stopped: vec![7],
            },
        ));
        // Full, same category: the most distant one, if farther away than the new one.
        cases.push((
            full(),
            0x03,
            30.5,
            2,
            true,
            Allocation::Slot {
                slot: 31,
                stopped: vec![31],
            },
        ));
        cases.push((
            full(),
            0x03,
            31.0,
            2,
            true,
            Allocation::Rejected { stopped: vec![] },
        ));
        // 2D (priority 0) displaces equal-ranked ones only if they are farther away.
        cases.push((
            full(),
            0x02,
            0.0,
            2,
            false,
            Allocation::Rejected { stopped: vec![] },
        ));
        // Instance limit 1 with 0x100: the more distant instance is moved.
        let mut surf = ChannelPool::new();
        surf.occupy(4, info(3, 9.0, 55));
        cases.push((
            surf.clone(),
            0x113,
            3.0,
            55,
            true,
            Allocation::Moved {
                slot: 4,
                stopped: vec![],
            },
        ));
        cases.push((
            surf.clone(),
            0x113,
            12.0,
            55,
            true,
            Allocation::Rejected { stopped: vec![] },
        ));
        cases.push((
            surf.clone(),
            0x113,
            3.0,
            56,
            true,
            Allocation::Slot {
                slot: 0,
                stopped: vec![],
            },
        ));
        // Without a position 0x100 has no effect; then 0x200 applies, or rejection.
        cases.push((
            surf.clone(),
            0x313,
            3.0,
            55,
            false,
            Allocation::Slot {
                slot: 4,
                stopped: vec![4],
            },
        ));
        cases.push((
            surf,
            0x13,
            3.0,
            55,
            true,
            Allocation::Rejected { stopped: vec![] },
        ));
        // Category 8 exclusive, 9 aborts 8.
        let mut voice = ChannelPool::new();
        voice.occupy(2, info(8, 0.0, 70));
        cases.push((
            voice.clone(),
            0x18,
            0.0,
            71,
            false,
            Allocation::Rejected { stopped: vec![] },
        ));
        cases.push((
            voice.clone(),
            0x29,
            0.0,
            72,
            false,
            Allocation::Slot {
                slot: 0,
                stopped: vec![2],
            },
        ));
        let mut dialog = ChannelPool::new();
        dialog.occupy(5, info(9, 0.0, 80));
        cases.push((
            dialog,
            0x18,
            0.0,
            71,
            false,
            Allocation::Rejected { stopped: vec![] },
        ));
        for (i, (mut pool, flags, priority, sample, positioned, expected)) in
            cases.into_iter().enumerate()
        {
            assert_eq!(
                pool.allocate(Flags(flags), priority, sample, positioned),
                expected,
                "case {i}"
            );
        }
    }

    #[test]
    fn stopped_slots_are_free_and_moves_take_the_new_priority() {
        let mut pool = ChannelPool::new();
        pool.occupy(4, info(3, 9.0, 55));
        assert_eq!(
            pool.allocate(Flags(0x113), 3.0, 55, true),
            Allocation::Moved {
                slot: 4,
                stopped: vec![]
            }
        );
        assert_eq!(pool.get(4).unwrap().priority, 3.0);
        let mut voice = ChannelPool::new();
        voice.occupy(2, info(8, 0.0, 70));
        voice.allocate(Flags(0x29), 0.0, 72, false);
        assert!(voice.get(2).is_none());
        assert_eq!(Flags(0x113).instance_limit(), 1);
        assert_eq!(Flags(0x29).category(), 9);
    }
}
