//! Base fields of all modules (`Module_ConstructBase` `0x004FD400`): variant mask, switch, flags, replies and the
//! timers/watchers (`Module_PostDelayedMessage` `0x004FD690`, `Module_UpdateEvents` `0x004FD7E0`), see
//! re/property-modules 1.

use std::collections::VecDeque;

use serde::{Deserialize, Serialize};

use crate::id::ObjectId;
use crate::message::Message;

/// Event flags (`ModuleEvent.flags`): active, repeat, timer, watcher, comparison `>`/`<`/`==`; group in the top byte.
pub mod event_flags {
    pub const ACTIVE: u32 = 0x1;
    pub const REPEAT: u32 = 0x2;
    pub const TIMER: u32 = 0x4;
    pub const WATCHER: u32 = 0x8;
    pub const GREATER: u32 = 0x10;
    pub const LESS: u32 = 0x20;
    pub const EQUAL: u32 = 0x40;
}

/// Timer or watcher (16 bytes in the original).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModuleEvent {
    pub flags: u32,
    /// Timer: accumulated time (ms); watcher: key of the range query.
    pub a: i32,
    /// Timer: delay (ms); watcher: threshold.
    pub b: i32,
    pub msg: Message,
}

impl ModuleEvent {
    /// Group (top byte).
    pub fn group(&self) -> u32 {
        self.flags >> 24
    }
}

/// Reply with a target object (`ObjectId::NONE` = own object).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Reply {
    pub msg: Message,
    pub target: ObjectId,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModuleBase {
    /// Module index (`+0x6C`).
    pub index: u8,
    /// Variant mask of the active descriptor (`m_PropBits`, without argument -1).
    pub prop_bits: u32,
    /// `m_Enabled`.
    pub enabled: bool,
    /// Module flags (`m_MatchInt`).
    pub flags: u32,
    /// Success reply (`m_SuccessMsg`, target `m_SuccessMsgTargetHID`).
    pub success: Option<Message>,
    pub success_target: ObjectId,
    pub fault: Option<Message>,
    pub fault_target: ObjectId,
    /// Time at the last firing of a timer (`m_CurPropTime`).
    pub cur_prop_time: i32,
    /// Idle: not in the tick list when switched on (`m_IsIdle`).
    pub idle: bool,
    /// Newly created timers (`m_TimersWS`), taken over only at the end of `update_events`.
    pub timers_ws: VecDeque<ModuleEvent>,
    /// Running timers and watchers (`m_Events`).
    pub events: Vec<ModuleEvent>,
    /// Pending group clearing (`m_ClearEvents`), 0 = none.
    pub clear_events: i32,
}

impl ModuleBase {
    pub fn new(index: u8) -> Self {
        Self {
            index,
            prop_bits: u32::MAX,
            enabled: true,
            flags: 0,
            success: None,
            success_target: ObjectId::NONE,
            fault: None,
            fault_target: ObjectId::NONE,
            cur_prop_time: 0,
            idle: false,
            timers_ws: VecDeque::new(),
            events: Vec::new(),
            clear_events: 0,
        }
    }

    /// `Module_HasFlags` (`0x004FD280`): `flags & x & 0xFFFFFF00 ≠ 0` (the low byte is only a reading aid).
    pub fn has_flags(&self, x: u32) -> bool {
        self.flags & x & 0xFFFF_FF00 != 0
    }

    /// `Module_PostDelayedMessage`: new timer **at the front** of `m_TimersWS` (several between two `update_events`
    /// therefore end up in reverse order in `m_Events`).
    pub fn post_delayed_message(&mut self, delay_ms: i32, msg: Message, repeat: bool, group: u8) {
        let mut flags = u32::from(group) << 24 | event_flags::ACTIVE | event_flags::TIMER;
        if repeat {
            flags |= event_flags::REPEAT;
        }
        self.timers_ws.push_front(ModuleEvent {
            flags,
            a: 0,
            b: delay_ms,
            msg,
        });
    }

    /// `Module_AddValueWatcher` (`0x004FD750`) via `Module_AddActiveEvent` (`0x004FD710`) and `EventList_AddTail`
    /// (`0x004FD570`): a watcher (re/arming-gates 10.1) is appended **immediately at the back of `m_Events`**, not to
    /// `m_TimersWS` like a timer; a pass of `update_events` that is running right now therefore still sees it (K-49).
    /// `key` is the key of the range query, `threshold` the threshold; `flags` names the comparison
    /// (`GREATER`/`LESS`/`EQUAL`) and `WATCHER`, `ACTIVE` is added, `group` goes into the top byte.
    pub fn add_value_watcher(
        &mut self,
        flags: u32,
        key: i32,
        threshold: i32,
        msg: Message,
        group: u8,
    ) {
        self.events.push(ModuleEvent {
            flags: u32::from(group) << 24 | flags | event_flags::ACTIVE,
            a: key,
            b: threshold,
            msg,
        });
    }

    /// `Module_ClearEventGroup` (`0x004FD670`): all groups <= `g`; in `m_TimersWS` immediately (unsigned), in
    /// `m_Events` not until the next `update_events` (signed). With `g = 0`, `m_Events` stays untouched; several calls:
    /// only the last `g` counts.
    pub fn clear_event_group(&mut self, g: u8) {
        self.timers_ws.retain(|e| e.group() > u32::from(g));
        self.clear_events = i32::from(g);
    }

    /// A timer with the message `code` is pending (query for tests and viewer, not an original function): an entry in
    /// `m_TimersWS` always counts (there [`ModuleBase::clear_event_group`] clears immediately, anything created later
    /// survives), one in `m_Events` only if active and not covered by a pending group clearing (`m_ClearEvents ≠ 0` and
    /// group <= `m_ClearEvents`; the clearing takes effect not until the next `update_events`).
    pub fn has_pending_timer(&self, code: u32) -> bool {
        self.timers_ws.iter().any(|e| e.msg.code == code)
            || self.events.iter().any(|e| {
                e.msg.code == code
                    && e.flags & event_flags::ACTIVE != 0
                    && !(self.clear_events != 0 && e.group() as i32 <= self.clear_events)
            })
    }

    /// Run the pending group clearing in `m_Events`.
    pub fn apply_clear_events(&mut self) {
        if self.clear_events != 0 {
            let g = self.clear_events;
            self.events.retain(|e| (e.group() as i32) > g);
            self.clear_events = 0;
        }
    }

    /// `Module_SetSuccessMessage` (`0x004FD1C0`): replace with a reply; without one it only clears (the target then
    /// stays).
    pub fn set_success(&mut self, msg: Option<&Message>, target: ObjectId) {
        self.success = msg.cloned();
        if msg.is_some() {
            self.success_target = target;
        }
    }

    /// `Module_SetFaultMessage` (`0x004FD140`), like [`ModuleBase::set_success`].
    pub fn set_fault(&mut self, msg: Option<&Message>, target: ObjectId) {
        self.fault = msg.cloned();
        if msg.is_some() {
            self.fault_target = target;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timers_go_to_the_front_and_groups_clear_up_to_g() {
        let mut base = ModuleBase::new(2);
        assert!(base.enabled && base.prop_bits == u32::MAX);
        base.post_delayed_message(100, Message::new(0x702, &[]), false, 0x80);
        base.post_delayed_message(200, Message::new(0x602, &[]), true, 0x10);
        assert_eq!(base.timers_ws[0].b, 200);
        assert_eq!(base.timers_ws[0].flags, 0x1000_0007);
        assert_eq!(base.timers_ws[1].flags, 0x8000_0005);
        // Group <= 0x10 immediately from m_TimersWS, m_Events only later.
        base.events.push(base.timers_ws[0].clone());
        base.clear_event_group(0x10);
        assert_eq!(base.timers_ws.len(), 1);
        assert_eq!(base.events.len(), 1);
        base.apply_clear_events();
        assert!(base.events.is_empty() && base.clear_events == 0);
        // g = 0: m_Events stays.
        base.events.push(base.timers_ws[0].clone());
        base.clear_event_group(0);
        base.apply_clear_events();
        assert_eq!(base.events.len(), 1);
        assert!(base.has_flags(0x100) == (base.flags & 0x100 != 0));
    }

    #[test]
    fn value_watchers_are_appended_to_the_running_events() {
        let mut base = ModuleBase::new(3);
        base.post_delayed_message(1000, Message::new(0x2B03, &[]), false, 0x80);
        base.add_value_watcher(
            event_flags::WATCHER | event_flags::GREATER,
            7,
            0,
            Message::new(0x2703, &[]),
            0x80,
        );
        // The watcher is in `events` immediately (timers wait in `timers_ws`).
        assert_eq!(base.events.len(), 1);
        let w = &base.events[0];
        assert_eq!((w.flags, w.a, w.b), (0x8000_0019, 7, 0));
        assert_eq!(w.msg, Message::new(0x2703, &[]));
        assert_eq!(base.timers_ws.len(), 1);
        assert_eq!(base.timers_ws[0].flags, 0x8000_0005);
        // A further watcher comes after the first.
        base.add_value_watcher(
            event_flags::WATCHER | event_flags::EQUAL,
            9,
            2,
            Message::new(0x2903, &[]),
            0x01,
        );
        assert_eq!(base.events[1].flags, 0x0100_0049);
        assert_eq!(base.events[1].a, 9);
    }

    #[test]
    fn pending_timers_ignore_cleared_groups() {
        let msg = |code| Message::new(code, &[]);
        // Group 0 without clearing: in `timers_ws` and, after the takeover, in `events`.
        let mut base = ModuleBase::new(31);
        assert!(!base.has_pending_timer(0x41F));
        base.post_delayed_message(4332, msg(0x41F), false, 0);
        assert!(base.has_pending_timer(0x41F));
        let e = base.timers_ws.pop_front().unwrap();
        base.events.push(e);
        assert!(base.has_pending_timer(0x41F));
        assert!(!base.has_pending_timer(0x31F));
        // An inactive entry does not count.
        base.events[0].flags &= !event_flags::ACTIVE;
        assert!(!base.has_pending_timer(0x41F));

        // Group 0x80 in `events` (0x21F) and in `timers_ws` (0x31F), plus 0x90 in `events`: `clear_event_group(0x80)`
        // before the next `update_events` -> cleared, although 0x21F is still in `events`; 0x90 stays.
        let mut base = ModuleBase::new(31);
        base.post_delayed_message(1500, msg(0x21F), false, 0x80);
        base.post_delayed_message(1500, msg(0x51F), false, 0x90);
        base.events.extend(base.timers_ws.drain(..));
        base.post_delayed_message(5332, msg(0x31F), false, 0x80);
        assert!(base.has_pending_timer(0x21F) && base.has_pending_timer(0x31F));
        base.clear_event_group(0x80);
        assert_eq!(base.events.len(), 2);
        assert!(!base.has_pending_timer(0x21F));
        assert!(!base.has_pending_timer(0x31F));
        assert!(base.has_pending_timer(0x51F));
        // Created afterwards (new cast after an abort): in `timers_ws`, counts.
        base.post_delayed_message(1500, msg(0x21F), false, 0x80);
        assert!(base.has_pending_timer(0x21F));
        base.apply_clear_events();
        assert!(base.has_pending_timer(0x21F) && base.has_pending_timer(0x51F));
        assert!(!base.has_pending_timer(0x31F));
    }

    #[test]
    fn replies_keep_their_target_when_cleared() {
        let mut base = ModuleBase::new(0);
        let target = ObjectId::new(3, 1, 1);
        base.set_success(Some(&Message::new(0x207, &[])), target);
        base.set_success(None, ObjectId::NONE);
        assert_eq!((base.success.clone(), base.success_target), (None, target));
    }
}
