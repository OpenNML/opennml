//! Messages to objects and modules (re/sim-core 3): code
//! `(number << 8) | module index`, `0x7F` = the object itself; arguments as `i32`.

use serde::{Deserialize, Serialize};

use crate::id::ObjectId;

/// Module index of the object's own messages (`GameObject_HandleOwnMessage`).
pub const OBJECT_MODULE: u8 = 0x7F;

/// A message `{id, argc, args...}`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Message {
    pub code: u32,
    pub args: Vec<i32>,
}

impl Message {
    pub fn new(code: u32, args: &[i32]) -> Self {
        Self {
            code,
            args: args.to_vec(),
        }
    }

    /// Target module (`code & 0xFF`).
    pub fn module(&self) -> u8 {
        (self.code & 0xFF) as u8
    }

    /// Argument `i`, 0 if missing (the original then reads memory behind the message; the modules check `argc`
    /// beforehand).
    pub fn arg(&self, i: usize) -> i32 {
        self.args.get(i).copied().unwrap_or(0)
    }

    /// Write a reply into argument `i` (`msg[2 + i]`, as the original's queries write into the sender's buffer);
    /// missing arguments before it are created with 0, and `argc` grows along with it.
    pub fn set_arg(&mut self, i: usize, value: i32) {
        if self.args.len() <= i {
            self.args.resize(i + 1, 0);
        }
        self.args[i] = value;
    }
}

/// Command envelope for `OnMessage` (`GameObject_PostCommandMessage` `0x0046B7E0`): message with success and failure
/// feedback and its target (`ObjectId::NONE` = own object).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Envelope {
    pub msg: Message,
    pub success: Option<Message>,
    pub fault: Option<Message>,
    pub success_target: ObjectId,
    pub fault_target: ObjectId,
}

impl Envelope {
    /// Envelope without feedback.
    pub fn bare(msg: Message) -> Self {
        Self {
            msg,
            success: None,
            fault: None,
            success_target: ObjectId::NONE,
            fault_target: ObjectId::NONE,
        }
    }
}

/// Entry of an object's message FIFO (`MessageQueue_PushBack` `0x0046A690`): flags 1/5 (message, with abort) or 2/6
/// (command envelope, with abort).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Posted {
    Message { msg: Message, abort: bool },
    Command { env: Envelope, abort: bool },
}

impl Posted {
    /// Abort all modules beforehand (flag 4).
    pub fn aborts(&self) -> bool {
        match self {
            Self::Message { abort, .. } | Self::Command { abort, .. } => *abort,
        }
    }

    /// Code of the message.
    pub fn code(&self) -> u32 {
        match self {
            Self::Message { msg, .. } => msg.code,
            Self::Command { env, .. } => env.msg.code,
        }
    }
}

/// Message filter per object (`GameObject+0x730`, `MessageFilter_IsAllowed` `0x004E3400`): mode 0 = only the list is
/// allowed, mode 1 = list forbidden (default, empty = everything allowed).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MessageFilter {
    pub mode: u8,
    pub codes: Vec<u32>,
}

impl Default for MessageFilter {
    fn default() -> Self {
        Self {
            mode: 1,
            codes: Vec::new(),
        }
    }
}

impl MessageFilter {
    pub fn is_allowed(&self, code: u32) -> bool {
        match self.mode {
            0 => self.codes.contains(&code),
            1 => !self.codes.contains(&code),
            _ => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes_route_to_modules_and_the_filter_allows_by_default() {
        let m = Message::new(0x106, &[2, 50]);
        assert_eq!((m.module(), m.arg(1), m.arg(5)), (6, 50, 0));
        assert_eq!(Message::new(0x37F, &[]).module(), OBJECT_MODULE);
        // Replies in the arguments: missing ones before it become 0.
        let mut q = Message::new(0xA08, &[-1]);
        q.set_arg(1, 77);
        q.set_arg(0, 5);
        assert_eq!(q.args, [5, 77]);
        q.set_arg(3, 9);
        assert_eq!(q.args, [5, 77, 0, 9]);
        let mut filter = MessageFilter::default();
        assert!(filter.is_allowed(0x200));
        filter.codes.push(0x200);
        assert!(!filter.is_allowed(0x200) && filter.is_allowed(0x300));
        filter.mode = 0;
        assert!(filter.is_allowed(0x200) && !filter.is_allowed(0x300));
        filter.codes.clear();
        // End of the healing trance (mode 0, empty list): everything forbidden.
        assert!(!filter.is_allowed(0x200));
        let p = Posted::Message {
            msg: m,
            abort: true,
        };
        assert!(p.aborts() && p.code() == 0x106);
    }
}
