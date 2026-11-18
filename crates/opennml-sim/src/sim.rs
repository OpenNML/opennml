//! Context of the simulation functions: mutable state and the rules. Modules work with
//! `(object, module index)` instead of `&mut self`, so that synchronous messages (also to the sending module
//! itself, as in the original) are possible at any time.

use std::cell::RefCell;

use crate::command::{Origin, Verdict};
use crate::id::ObjectId;
use crate::module::Module;
use crate::object::GameObject;
use crate::rules::Rules;
use crate::state::SimState;

pub struct Sim<'a> {
    pub state: &'a mut SimState,
    pub rules: &'a Rules,
    /// Origin of the commands that are currently running (transient, not in the state; set by
    /// [`crate::player::submit`]).
    pub(crate) origin: Origin,
    /// Verdict on the running command ([`crate::command::execute`]; transient).
    pub(crate) verdict: RefCell<Verdict>,
    /// The simulation is currently in the AI step (transient; set by `ki::run::with_ki`): `state.ki`
    /// is then only a placeholder.
    pub(crate) in_ki_step: bool,
    /// Test switch "all conditions true" of the scripts (dry run; transient, not in the
    /// state; set by [`crate::tick::step_with`]).
    #[doc(hidden)]
    pub all_conditions_true: bool,
}

impl<'a> Sim<'a> {
    pub fn new(state: &'a mut SimState, rules: &'a Rules) -> Self {
        Self {
            state,
            rules,
            origin: Origin::External,
            verdict: RefCell::new(Verdict::default()),
            in_ki_step: false,
            all_conditions_true: false,
        }
    }

    /// Object including dying ones (`GetObjectById(id, 0)`).
    pub fn object(&self, id: ObjectId) -> Option<&GameObject> {
        self.state.object(id, false)
    }

    pub fn object_mut(&mut self, id: ObjectId) -> Option<&mut GameObject> {
        self.state.object_mut(id, false)
    }

    pub fn module(&self, id: ObjectId, index: usize) -> Option<&Module> {
        self.object(id)?.module(index)
    }

    pub fn module_mut(&mut self, id: ObjectId, index: usize) -> Option<&mut Module> {
        self.object_mut(id)?.module_mut(index)
    }

    /// Upgrade percentages of the owner for a type (`GameWorld_GetTypeUpgradePercent`
    /// `0x0047DC90`): sum of the type table of the slot, players 0-7 only
    /// ([`crate::upgrades::type_upgrade_percent`]).
    pub fn type_upgrade_percent(&self, slot: u8, guid: u32, attribute: u32) -> i32 {
        crate::upgrades::type_upgrade_percent(self.state, slot, guid, attribute)
    }

    /// Current value of an attribute (`AttributeSet_GetCurrentValue`).
    pub fn attribute(&self, id: ObjectId, attribute: u32) -> i32 {
        self.object(id).map_or(0, |o| {
            let p = self.type_upgrade_percent(o.owner(), o.guid, attribute);
            o.attributes.current_value(attribute, p)
        })
    }
}
