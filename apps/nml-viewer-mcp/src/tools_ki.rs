//! Tool `viewer_ki` (protocol 26): parameters and execution of the AI commands `ki_state`,
//! `ki_query` and `ki_command` (`docs/viewer-control.md`, section "AI match"). The tool method is in `tools.rs`
//! right after `viewer_ui`. Also the slots of `viewer_sim setup` ([`SlotParam`]).

use nml_control::{Command, KiAction, SlotSetup, SlotSetupKind};
use schemars::JsonSchema;
use serde::Deserialize;

use crate::session::Session;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum KiToolAction {
    /// AI state (ki_state): per engine threads in execution order (module, name, pc, status, sleep,
    /// semaphore, stack depth, top 8 stack values, child), semaphores, with values also m_iValues; counters
    /// of rejected AI commands per slot, code and reason as well as the command statistics; only slot, if given
    State,
    /// AI query (like viewer_sim ki_query): slot (required), code, words, sight; read-only
    Query,
    /// start module module with args on the engine of slot, sync = immediately to the end, otherwise as a thread
    /// (ki_command start, takes effect at the next tick)
    Start,
    /// stop the whole AI (paused, default true) or with paused false let it continue (ki_command pause)
    Pause,
    /// with the AI stopped passes passes (default 1), per thread of slot exactly one step with dt = 20
    /// (ki_command step, takes effect at the next tick)
    Step,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct KiParams {
    pub action: KiToolAction,
    /// state: only this slot 0-7; query: slot whose sight the query uses (required); start, step: slot
    /// of the engine (required)
    pub slot: Option<u8>,
    /// state: also return m_iValues (default false)
    pub values: Option<bool>,
    /// query: query code 0x4E20...0x5130 as a decimal number (required)
    pub code: Option<u32>,
    /// query: request words from [2] (default [slot], at most 128)
    pub words: Option<Vec<i32>>,
    /// query: omniscient (default) or fog
    pub sight: Option<String>,
    /// start: module ID from nml.db (required)
    pub module: Option<u32>,
    /// start: arguments in order onto the stack (at most 64)
    pub args: Option<Vec<i32>>,
    /// start: synchronously to the end instead of as a thread (default false)
    pub sync: Option<bool>,
    /// pause: stop (default true) or continue (false)
    pub paused: Option<bool>,
    /// step: passes 1-1000 (default 1)
    pub passes: Option<u32>,
}

impl KiParams {
    /// Command of the control channel; missing required fields and limits (`Command::validate`) are errors.
    pub fn command(&self) -> Result<Command, String> {
        let ki = |action| Command::KiCommand {
            action,
            slot: self.slot,
            module: self.module,
            args: self.args.clone(),
            sync: self.sync,
            paused: self.paused,
            passes: self.passes,
        };
        let command = match self.action {
            KiToolAction::State => Command::KiState {
                slot: self.slot,
                values: self.values,
            },
            KiToolAction::Query => Command::KiQuery {
                player: self.slot.ok_or("query needs slot")?,
                code: self.code.ok_or("query needs code")?,
                words: self.words.clone(),
                sight: self.sight.clone(),
            },
            KiToolAction::Start => ki(KiAction::Start),
            KiToolAction::Pause => ki(KiAction::Pause),
            KiToolAction::Step => ki(KiAction::Step),
        };
        command.validate()?;
        Ok(command)
    }
}

/// Kind of a lobby slot for `viewer_sim setup` `slots`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SlotKindParam {
    Closed,
    Human,
    Ai,
}

/// A lobby slot for `viewer_sim setup` (protocol 26); missing values as the default of the map.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, JsonSchema)]
pub struct SlotParam {
    /// closed, human or ai
    pub kind: SlotKindParam,
    /// Nation 0-5 (0 WID, 1 PRI, 2 SPA, 3 SID, 4 ENG, 5 PAT)
    pub nation: Option<i32>,
    pub team: Option<i32>,
    /// Color 0-8 (8 = white)
    pub color: Option<i32>,
    /// free handicap 0-5 (not together with ki_kind)
    pub handicap: Option<i32>,
    /// only with ai: AI kind 1-4 (handicap 5/2/0/0 as in the original)
    pub ki_kind: Option<u8>,
}

impl From<&SlotParam> for SlotSetup {
    fn from(p: &SlotParam) -> Self {
        SlotSetup {
            kind: match p.kind {
                SlotKindParam::Closed => SlotSetupKind::Closed,
                SlotKindParam::Human => SlotSetupKind::Human,
                SlotKindParam::Ai => SlotSetupKind::Ai,
            },
            nation: p.nation,
            team: p.team,
            color: p.color,
            handicap: p.handicap,
            ki_kind: p.ki_kind,
        }
    }
}

/// Executes `viewer_ki`; result: the response of the viewer as indented JSON text (`state`: AI state,
/// `query`: request field and direct changes, `start`/`pause`/`step`: `{"step"}`).
pub async fn execute(session: &mut Session, params: &KiParams) -> Result<String, String> {
    let command = params.command()?;
    let value = session.command(command).await?;
    serde_json::to_string_pretty(&value).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn params(value: serde_json::Value) -> KiParams {
        serde_json::from_value(value).unwrap()
    }

    fn ki(
        action: KiAction,
        slot: Option<u8>,
        module: Option<u32>,
        args: Option<Vec<i32>>,
        sync: Option<bool>,
        paused: Option<bool>,
        passes: Option<u32>,
    ) -> Command {
        Command::KiCommand {
            action,
            slot,
            module,
            args,
            sync,
            paused,
            passes,
        }
    }

    #[test]
    fn ki_actions_map_to_commands() {
        assert_eq!(
            params(json!({"action": "state"})).command(),
            Ok(Command::KiState {
                slot: None,
                values: None
            })
        );
        assert_eq!(
            params(json!({"action": "state", "slot": 1, "values": true})).command(),
            Ok(Command::KiState {
                slot: Some(1),
                values: Some(true)
            })
        );
        assert_eq!(
            params(json!({"action": "query", "slot": 1, "code": 20_004, "words": [1, 1], "sight": "fog"}))
                .command(),
            Ok(Command::KiQuery {
                player: 1,
                code: 20_004,
                words: Some(vec![1, 1]),
                sight: Some("fog".into()),
            })
        );
        assert_eq!(
            params(json!({"action": "start", "slot": 1, "module": 195_235, "args": [1, 4, 3]}))
                .command(),
            Ok(ki(
                KiAction::Start,
                Some(1),
                Some(195_235),
                Some(vec![1, 4, 3]),
                None,
                None,
                None
            ))
        );
        assert_eq!(
            params(json!({"action": "start", "slot": 0, "module": 9826, "sync": true})).command(),
            Ok(ki(
                KiAction::Start,
                Some(0),
                Some(9826),
                None,
                Some(true),
                None,
                None
            ))
        );
        assert_eq!(
            params(json!({"action": "pause"})).command(),
            Ok(ki(KiAction::Pause, None, None, None, None, None, None))
        );
        assert_eq!(
            params(json!({"action": "pause", "paused": false})).command(),
            Ok(ki(
                KiAction::Pause,
                None,
                None,
                None,
                None,
                Some(false),
                None
            ))
        );
        assert_eq!(
            params(json!({"action": "step", "slot": 3, "passes": 5})).command(),
            Ok(ki(KiAction::Step, Some(3), None, None, None, None, Some(5)))
        );
        // Missing required fields and limits: error with the field name.
        for (bad, field) in [
            (json!({"action": "query", "code": 1}), "slot"),
            (json!({"action": "query", "slot": 1}), "code"),
            (json!({"action": "start", "module": 1}), "slot"),
            (json!({"action": "start", "slot": 1}), "module"),
            (json!({"action": "step"}), "slot"),
            (json!({"action": "step", "slot": 1, "passes": 0}), "passes"),
            (json!({"action": "step", "slot": 8}), "slot"),
            (json!({"action": "pause", "slot": 1}), "slot"),
            (
                json!({"action": "start", "slot": 1, "module": 1, "args": vec![0; 65]}),
                "args",
            ),
            (json!({"action": "state", "slot": 9}), "slot"),
        ] {
            let error = params(bad.clone()).command().unwrap_err();
            assert!(error.contains(field), "{bad}: {error}");
        }
        assert!(serde_json::from_value::<KiParams>(json!({"action": "run"})).is_err());
    }

    #[test]
    fn slot_params_become_slot_setups() {
        let p: SlotParam =
            serde_json::from_value(json!({"kind": "ai", "nation": 1, "ki_kind": 1})).unwrap();
        assert_eq!(
            SlotSetup::from(&p),
            SlotSetup {
                kind: SlotSetupKind::Ai,
                nation: Some(1),
                team: None,
                color: None,
                handicap: None,
                ki_kind: Some(1),
            }
        );
        assert!(serde_json::from_value::<SlotParam>(json!({"kind": "passive"})).is_err());
    }
}
