//! Actions, layer E (`ScriptBlock_ExecuteActions` `0x00567740`, re/script-actions A.3-A.8) and the entry
//! into layer D (`Script_DispatchAction` `0x00422B60`).
//!
//! Layer E reads the parameters for every number with a case (`atol`, `Mid(5)`, text, word count),
//! sets `m_bDeleteMe` as in A.3, waits as in A.4 and handles the script-internal actions on the container
//! itself. All other numbers send a typed message [`ScriptMsg`] to [`dispatch`] (layer D). Layer D
//! implements the speech length (D case `0x89`), the dialog start (39, D case `0x20`, [`start_dialog`]), dialog
//! line (150), tip (183) and info window (213, speech state in [`speech`]) and recognizes the empty D cases (E-133);
//! commands (`command`), world (`world`) and presentation (`present`) are handled in their own modules. Every number of
//! the catalog with a case has an effect; [`Dispatched::Pending`] and [`ScriptTrace::Pending`] ("no effect yet") remain
//! as a safety net for a number without a case in layer D.

use opennml_formats::abl::catalog::{self, Effect, WaitKind};
use opennml_formats::abl::{RtAction, RtBlock, mid5};
use opennml_formats::property::atol;

use super::container::{Container, ScriptSource, suspend};
use super::{DIALOG_CONTAINER_ID, ScriptTrace, conditions, speech, trace};
use crate::sim::Sim;

/// Divisor of the random actions 33 and 180 (`0x7FFF`).
const RAND_DIVISOR: i32 = 0x7FFF;
/// Message "speech length" to layer D (`{500, 1, TextID}` -> D case `0x89`, re/script-actions A.7).
pub const SPEECH_LENGTH: i32 = 500;
/// Action or message "start dialog" (case `0x24` -> D case `0x20`).
pub const START_DIALOG: i32 = 39;
/// Speak a dialog line (layer D -> `Dialog_PlaySpeech`).
pub const PLAY_LINE: i32 = 150;
/// Show a tip (layer D -> `FUN_0043E250`).
pub const SHOW_TIP: i32 = 183;
/// Info window with a line (layer D -> `FUN_00443A20`).
pub const INFO_WINDOW: i32 = 213;
/// Fixed wait time of 149 and 156 in ms (`LAB_0056994B`).
const PORTRAIT_WAIT_MS: i32 = 600;
/// Event bit "train stops" (163, 164) and "idle/arrived" (other event waits).
const TRAIN_BIT: u8 = 0;
const IDLE_BIT: u8 = 1;

/// A value slot of the message: `Z` (`atol`) as a number, `T`/`M`/`K` as text (`M` already without the
/// first five characters). The original writes texts as `{length, characters…}` into the word buffer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MsgValue {
    Int(i32),
    Text(String),
}

/// Message from layer E to layer D: number (`msg[0]`) and the described value slots from `msg[2]` on
/// according to re/script-actions A.8. `argc` (`msg[1]`) is omitted (D almost never evaluates it, A.13.8); undescribed
/// slots (15, 16, 18, 19, AI start with two parameters) are missing. Replies from D are in `values[0]` (`msg[2]`) as in
/// the original.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScriptMsg {
    pub number: i32,
    pub values: Vec<MsgValue>,
}

impl ScriptMsg {
    /// Text slot `i`; a number or a missing slot counts as empty text.
    pub fn text(&self, i: usize) -> &str {
        match self.values.get(i) {
            Some(MsgValue::Text(t)) => t,
            _ => "",
        }
    }

    /// Value slot `i` as a number; a text or a missing slot counts as 0.
    pub fn int(&self, i: usize) -> i32 {
        match self.values.get(i) {
            Some(MsgValue::Int(v)) => *v,
            _ => 0,
        }
    }
}

/// A command that a script action sent via the script path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Issued {
    /// Command type of the original without the high part `0x5000_0000` (e.g. `0x3C`).
    pub code: u32,
    pub outcome: crate::command::Outcome,
}

/// Result of layer D.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Dispatched {
    /// Executed (also an empty D case without effect).
    Done,
    /// Executed via the command path ([`super::command`]): the sent commands with their verdict in send
    /// order. An empty list (unknown name, n = 0) is silent, as in the original; the caller treats it like
    /// `Done` and reports [`ScriptTrace::Command`] for each entry.
    Issued(Vec<Issued>),
    /// Effect not yet implemented ("no effect yet").
    Pending,
    /// Runtime error from a value (A7): no effect; the caller reports `Guarded` and ends the block as on an
    /// error from `execute`.
    Guarded(String),
}

/// Actions of the block `c.blocks[bi]` (file block `block`) from `cur_action`. Return value: action 39 ran
/// (dialog start) and the block loop of the container ends (`local_b1`, re/script-actions A.3).
///
/// Continue while `(i < count && !running) || (running && cur_action == i)` (A.3); a halted block therefore
/// stops after the halting action. Numbers without a case (`0x72`, > 512, negative) and those unreachable
/// from `.abl` (E-133) are only skipped, without `m_bDeleteMe`. An index outside the actions (only from a
/// corrupted state) and a missing parameter are exceptions in the original: here `Guarded` and end of the
/// block.
pub fn execute_block(sim: &mut Sim, c: &mut Container, bi: usize, block: &RtBlock) -> bool {
    let count = block.actions.len() as i64;
    let mut i = i64::from(c.blocks[bi].cur_action);
    let mut dialog_started = false;
    loop {
        let b = c.blocks[bi];
        let go = (i < count && !b.running) || (b.running && i64::from(b.cur_action) == i);
        if !go {
            return dialog_started;
        }
        if i < 0 || i >= count {
            guard(
                sim,
                c,
                bi,
                -1,
                format!("action index {i} outside the block"),
            );
            return dialog_started;
        }
        let index = i as usize;
        let action = &block.actions[index];
        let skip = catalog::action(action.number)
            .is_none_or(|spec| matches!(spec.effect, Effect::NoCase | Effect::Unreachable));
        if !skip {
            match execute(sim, c, bi, index, action) {
                Ok(started) => dialog_started |= started,
                Err(why) => {
                    guard(sim, c, bi, action.number, why);
                    return dialog_started;
                }
            }
        }
        i += 1;
    }
}

/// Runtime error (A7): no effect, message.
fn guard(sim: &mut Sim, c: &Container, bi: usize, number: i32, why: String) {
    trace(
        sim,
        ScriptTrace::Guarded {
            container: c.id,
            block: c.blocks[bi].index,
            number,
            why,
        },
    );
}

/// Parameter `i` as text.
fn text(a: &RtAction, i: usize) -> Result<&str, String> {
    a.params
        .get(i)
        .map(String::as_str)
        .ok_or_else(|| format!("action {}: parameter {} missing", a.number, i + 1))
}

/// Parameter `i` via `atol`, truncated to 32 bits as in the original.
fn int(a: &RtAction, i: usize) -> Result<i32, String> {
    text(a, i).map(|p| atol(p) as i32)
}

/// An action with a case (re/script-actions A.6, A.8). Return value: action 39 ran.
fn execute(
    sim: &mut Sim,
    c: &mut Container,
    bi: usize,
    index: usize,
    a: &RtAction,
) -> Result<bool, String> {
    match a.number {
        // Block repeatable: `m_bDeleteable = m_bDeleteMe = 0`.
        10 => {
            let b = &mut c.blocks[bi];
            b.deleteable = false;
            b.delete_me = false;
            return Ok(false);
        }
        // Countdown in seconds (without overflow check).
        11 => c.counter = int(a, 0)?.wrapping_mul(1000),
        12 | 137 => {
            c.switches.insert(text(a, 0)?.to_owned(), 1);
        }
        23 | 138 => {
            c.switches.remove(text(a, 0)?);
        }
        25 | 139 => {
            let value = int(a, 1)?;
            c.timers.insert(text(a, 0)?.to_owned(), value);
        }
        26 | 140 => {
            c.timers.remove(text(a, 0)?);
        }
        33 => {
            let max = int(a, 1)?;
            let name = text(a, 0)?.to_owned();
            let r = sim.state.script.rand.rand();
            c.random_switches
                .insert(name, r.wrapping_mul(max) / RAND_DIVISOR);
        }
        39 => return start_dialog_action(sim, c, bi, index, a),
        // Wait `Z0` ms; `m_bDeleteMe` already before reading the parameter.
        40 => {
            c.blocks[bi].delete_me = true;
            let ms = int(a, 0)?;
            wait(sim, c, bi, index, a.wait, ms);
            return Ok(false);
        }
        // Timer to a random value: parameters name; min; max.
        180 => {
            let min = int(a, 1)?;
            let max = int(a, 2)?;
            let name = text(a, 0)?.to_owned();
            let r = sim.state.script.rand.rand();
            let value = (r.wrapping_mul(max.wrapping_sub(min)) / RAND_DIVISOR).wrapping_add(min);
            c.timers.insert(name, value);
        }
        // Timer from the speech length (case `0x49`): `Timer[p0] = percent_len(len(Z2), Z1)`.
        230 => {
            let len = speech_length(sim, c, int(a, 2)?);
            let pct = int(a, 1)?;
            c.timers
                .insert(text(a, 0)?.to_owned(), percent_len(len, pct));
        }
        // Increase the timer by the speech length (case `0x4A`); if it is missing, the length.
        232 => {
            let len = speech_length(sim, c, int(a, 1)?);
            let name = text(a, 0)?;
            let value = c.timers.get(name).map_or(len, |&old| old.wrapping_add(len));
            c.timers.insert(name.to_owned(), value);
        }
        // Timer from the remaining value of another timer (if it is missing, 0) and percent.
        233 => {
            let source = timer(c, text(a, 2)?);
            let pct = int(a, 1)?;
            c.timers
                .insert(text(a, 0)?.to_owned(), percent_time(pct, source));
        }
        333 => {}
        334..=337 | 402 | 403 => return event_pair(sim, c, bi, index, a),
        // Query 400 `{400, 2, Z0, Z2}` (GUID count without orders) -> `Var[p1]`.
        359 => {
            let slot = int(a, 0)?;
            let guid = int(a, 2)?;
            let name = text(a, 1)?.to_owned();
            let count = crate::economy::count_guid_or_filter(sim, slot, guid, false);
            c.variables.insert(name, count);
        }
        360 => {
            let value = int(a, 1)?;
            c.variables.insert(text(a, 0)?.to_owned(), value);
        }
        // E-132: a missing variable becomes the operand, also for - and *.
        361..=363 => {
            let operand = int(a, 1)?;
            let name = text(a, 0)?;
            let value = match c.variables.get(name) {
                None => operand,
                Some(&old) => match a.number {
                    361 => old.wrapping_add(operand),
                    362 => old.wrapping_sub(operand),
                    _ => old.wrapping_mul(operand),
                },
            };
            c.variables.insert(name.to_owned(), value);
        }
        // Write profile (case `100`): `operator[]` creates `Var[p0]` with 0 **before** the check of the name;
        // empty name: neither message nor `m_bDeleteMe`.
        382 => {
            let name = text(a, 0)?;
            let value = *c.variables.entry(name.to_owned()).or_insert(0);
            if name.is_empty() {
                return Ok(false);
            }
            send(sim, c, bi, a, value)?;
        }
        // Read profile (case `0x65`): empty name like 382. Layer D answers in `msg[2]` with the value of the
        // profile mirror (0 if it is missing), then `Var[p0] = msg[2]` applies.
        383 => {
            let name = text(a, 0)?;
            if name.is_empty() {
                return Ok(false);
            }
            let reply = send(sim, c, bi, a, 0)?;
            c.variables.insert(name.to_owned(), reply.int(0));
        }
        // Attribute (case `0x66`): reads `Z0`, `p1`, `Z2`; with an empty name `p1` only `i++`, without message
        // and without `m_bDeleteMe` (A.3, `0x0056CA99`-`0x0056CACB`).
        384 | 385 => {
            int(a, 0)?;
            let name = text(a, 1)?;
            int(a, 2)?;
            if name.is_empty() {
                return Ok(false);
            }
            send(sim, c, bi, a, 0)?;
        }
        // Only if both variables exist; without `m_bDeleteMe` (E-126).
        510..=512 => {
            let target = text(a, 0)?;
            // The second name is not read until the first exists (case `0x71`).
            if let Some(&old) = c.variables.get(target)
                && let Some(&operand) = c.variables.get(text(a, 1)?)
            {
                let value = match a.number {
                    510 => old.wrapping_add(operand),
                    511 => old.wrapping_sub(operand),
                    _ => old.wrapping_mul(operand),
                };
                c.variables.insert(target.to_owned(), value);
            }
            return Ok(false);
        }
        _ => return through_dispatch(sim, c, bi, index, a),
    }
    c.blocks[bi].delete_me = true;
    Ok(false)
}

/// Remaining value of the timer `name` (`Lookup`, 0 if it is missing).
fn timer(c: &Container, name: &str) -> i32 {
    c.timers.get(name).copied().unwrap_or(0)
}

/// Speech length via layer D (`{500, 1, TextID}` -> D case `0x89`, reply in `msg[2]`).
fn speech_length(sim: &mut Sim, c: &mut Container, text: i32) -> i32 {
    let mut msg = ScriptMsg {
        number: SPEECH_LENGTH,
        values: vec![MsgValue::Int(text)],
    };
    dispatch(sim, c, &mut msg);
    msg.int(0)
}

/// Message to layer D and `m_bDeleteMe`; "no effect yet" is reported. `derived` is the value slot that
/// depends on the state (see [`message`]). Return value: the message after D (for wait times that the
/// original reads from it after the call).
pub(super) fn send(
    sim: &mut Sim,
    c: &mut Container,
    bi: usize,
    a: &RtAction,
    derived: i32,
) -> Result<ScriptMsg, String> {
    let mut msg = message(sim, a, derived);
    match dispatch(sim, c, &mut msg) {
        Dispatched::Pending => trace(
            sim,
            ScriptTrace::Pending {
                container: c.id,
                block: c.blocks[bi].index,
                action: a.number,
            },
        ),
        // Like an error from `execute`: `Guarded` with the real block, the block ends (without `m_bDeleteMe`).
        Dispatched::Guarded(why) => return Err(why),
        // Like `Done`, with a message per sent command (the script path also reports rejections).
        Dispatched::Issued(issued) => {
            for Issued { code, outcome } in issued {
                trace(
                    sim,
                    ScriptTrace::Command {
                        container: c.id,
                        block: c.blocks[bi].index,
                        action: a.number,
                        code,
                        outcome,
                    },
                );
            }
        }
        Dispatched::Done => {}
    }
    c.blocks[bi].delete_me = true;
    Ok(msg)
}

/// Start dialog (case `0x24`): on the first run message `{39, T(name)}` (layer D forms the path from folder,
/// name and `.abl` via `ScriptRules::dialogs`), `m_bDeleteMe`, waiting with a **forced** flag and -1, return
/// value 1 (even if the start fails, re/script-actions A.6). While waiting: continue without a check.
fn start_dialog_action(
    sim: &mut Sim,
    c: &mut Container,
    bi: usize,
    index: usize,
    a: &RtAction,
) -> Result<bool, String> {
    if c.blocks[bi].running {
        c.blocks[bi].running = false;
        return Ok(false);
    }
    text(a, 0)?;
    send(sim, c, bi, a, 0)?;
    wait(sim, c, bi, index, true, -1);
    Ok(true)
}

/// Case `0x51` (334-337, 402, 403): message `{number, T0, T1}` only if the texts together are not empty
/// (`L > 0`); otherwise neither message nor `m_bDeleteMe` nor `SuspendAtAction`. While waiting: query 2500
/// with bit 1 on `p0`.
fn event_pair(
    sim: &mut Sim,
    c: &mut Container,
    bi: usize,
    index: usize,
    a: &RtAction,
) -> Result<bool, String> {
    if c.blocks[bi].running {
        if conditions::query_2500(sim, text(a, 0)?, IDLE_BIT, true) {
            c.blocks[bi].running = false;
        }
        return Ok(false);
    }
    let empty = text(a, 0)?.is_empty() && text(a, 1)?.is_empty();
    if empty {
        return Ok(false);
    }
    send(sim, c, bi, a, 0)?;
    wait(sim, c, bi, index, a.wait, -1);
    Ok(false)
}

/// All numbers with a D call without a special path: continuation branch (waiting), otherwise message
/// according to re/script-actions A.8, D, `m_bDeleteMe` and, for numbers with a wait case, `SuspendAtAction` with
/// the time according to A.4.
fn through_dispatch(
    sim: &mut Sim,
    c: &mut Container,
    bi: usize,
    index: usize,
    a: &RtAction,
) -> Result<bool, String> {
    let Some(spec) = catalog::action(a.number) else {
        return Ok(false);
    };
    if c.blocks[bi].running
        && let Some(ready) = continuation(sim, a)?
    {
        if ready {
            c.blocks[bi].running = false;
        }
        return Ok(false);
    }
    if a.params.len() < usize::from(spec.min_params) {
        return Err(format!(
            "action {}: parameter {} missing",
            a.number,
            a.params.len() + 1
        ));
    }
    let derived = match a.number {
        220 => percent_len(speech_length(sim, c, int(a, 3)?), int(a, 2)?),
        221 => percent_len(speech_length(sim, c, int(a, 2)?), int(a, 1)?),
        222 | 223 => percent_len(speech_length(sim, c, int(a, 1)?), int(a, 0)?),
        240 => percent_time(int(a, 2)?, timer(c, text(a, 3)?)),
        241 => percent_time(int(a, 1)?, timer(c, text(a, 2)?)),
        242 | 243 => percent_time(int(a, 0)?, timer(c, text(a, 1)?)),
        _ => 0,
    };
    let msg = send(sim, c, bi, a, derived)?;
    if spec.wait != WaitKind::None {
        wait(sim, c, bi, index, a.wait, wait_ms(a.number, &msg));
    }
    Ok(false)
}

/// Continuation branch of the cases with event waiting (re/script-actions A.4), chosen solely by `+0x34` as in the
/// original: query 600 (case `0x13`, `0x2B`, `0x43`) or 2500 (case `0x2E`, `0x35`, `0x36`). `None`: the case has no
/// continuation branch (the action runs again, e.g. time wait with -1).
fn continuation(sim: &mut Sim, a: &RtAction) -> Result<Option<bool>, String> {
    let ready = match a.number {
        22 | 183 | 150 | 213 => speech::take_event(sim.state, int(a, 0)?),
        153 | 188 | 374 | 376 | 181 | 375 => {
            conditions::query_2500(sim, text(a, 0)?, IDLE_BIT, true)
        }
        163 | 164 => conditions::query_2500(sim, text(a, 0)?, TRAIN_BIT, true),
        _ => return Ok(None),
    };
    Ok(Some(ready))
}

/// Wait time in ms according to re/script-actions A.4, read from the message **after** layer D. 240-242 calculate with
/// the wrong fields of the original (E-131): 240 `(t · player) / 100`, 241 `(angle · t) / 100`, 242 reads behind the
/// buffer - set to 0. Event and text waits: -1.
fn wait_ms(number: i32, msg: &ScriptMsg) -> i32 {
    match number {
        142 | 212 => msg.int(1),
        145 | 146 | 172 | 216 => msg.int(0),
        149 | 156 => PORTRAIT_WAIT_MS,
        240 | 241 => msg.int(0).wrapping_mul(msg.int(1)) / 100,
        242 => 0,
        _ => -1,
    }
}

/// `ScriptBlock_SuspendAtAction` with a message when the block really stops (`w` flag).
fn wait(sim: &mut Sim, c: &mut Container, bi: usize, index: usize, flag: bool, ms: i32) {
    suspend(&mut c.blocks[bi], index, flag, ms);
    if flag {
        trace(
            sim,
            ScriptTrace::Suspended {
                container: c.id,
                block: c.blocks[bi].index,
                action: index as u32,
            },
        );
    }
}

/// Message of an action according to re/script-actions A.8 (column "message", value slots from `msg[2]`). Internal
/// numbers without a D call yield an empty message with their number. Value slots that depend on the state are 0 here:
/// the time of 220-223 (speech length) and 240-243 (timer) as well as `Var[p0]` of 382;
/// [`execute_block`] inserts the computed value there. Missing parameters count as empty text (the load
/// check demands the minimum count, layer E checks it before the call).
pub fn build_msg(sim: &Sim, action: &RtAction) -> ScriptMsg {
    message(sim, action, 0)
}

/// [`build_msg`] with the state-dependent value `derived`.
fn message(sim: &Sim, a: &RtAction, derived: i32) -> ScriptMsg {
    use MsgValue::{Int, Text};
    let p = |i: usize| a.params.get(i).map_or("", String::as_str);
    let z = |i: usize| Int(atol(p(i)) as i32);
    let t = |i: usize| Text(p(i).to_owned());
    let m = |i: usize| Text(mid5(p(i)).to_owned());
    // `Replace("CAMERA ", "")` after the parser lowercases: has no effect (re/script-actions A.13.11).
    let k = t;
    let all_z = || (0..a.params.len()).map(z).collect::<Vec<_>>();
    let flag = |b: bool| Int(i32::from(b));
    let n = a.number;
    let (number, values) = match n {
        0 | 1 => (n, ki_words(sim, a)),
        2 | 7 | 20 | 24 | 347 | 372 => (n, vec![z(0), m(1)]),
        3 => (n, vec![z(0), z(1), z(2), m(3)]),
        4 | 9 | 13 | 17 => (n, vec![z(0)]),
        5 | 14 | 21 | 22 | 27..=32 | 36 | 37 | 182 | 183 | 200 | 203 | 205 => (n, all_z()),
        6 | 34 | 195 | 196 => (n, vec![z(0), z(1), m(2)]),
        8 | 373 => (n, vec![z(0), z(1), m(2), m(3)]),
        // Value slots reserved but not described (case `0x0E`-`0x10`, `0x28`, `0x63`,
        // `0x6C`, `0x70`).
        15 | 16 | 18 | 19 | 143 | 144 | 147 | 148 | 176 | 381 | 501..=504 | 508 => (n, Vec::new()),
        35 => (n, vec![z(0), z(2), m(1)]),
        38 | 212 | 341 | 365 | 366 => (n, vec![z(0), z(1)]),
        39 => (n, vec![t(0)]),
        141 => (n, vec![z(0), k(1)]),
        142 => (n, vec![z(0), z(2), k(1)]),
        145
        | 146
        | 156
        | 159
        | 160
        | 169
        | 172
        | 173
        | 184..=187
        | 198
        | 199
        | 207..=210
        | 216
        | 342
        | 343
        | 370
        | 371
        | 507 => (n, vec![z(0)]),
        149 | 340 | 505 => (n, vec![z(0), z(1), z(2)]),
        150 => (n, vec![z(0), z(1)]),
        151 => (n, vec![z(0), z(1), t(2), m(3)]),
        152 | 155 | 158 | 161 | 162 | 165 | 166 | 170 | 171 | 175 | 177 | 178 | 191 | 193 | 194
        | 215 | 338 | 354 | 383 | 395 => (n, vec![t(0)]),
        153 | 188 | 374 | 376 => (n, vec![t(0), m(1), m(2)]),
        154 | 163 | 164 => (n, vec![t(0), m(1)]),
        157 => (n, vec![z(1), z(2), t(0)]),
        167 | 181 | 334..=337 | 375 | 390 | 393 | 394 | 402 | 403 => (n, vec![t(0), t(1)]),
        168 | 213 | 506 => (n, vec![z(0), z(1), z(2), z(3)]),
        174 => (n, vec![m(0)]),
        179 | 189 | 192 | 201 | 202 | 231 | 350..=353 => (n, vec![z(1), t(0)]),
        190 => (n, vec![z(1), t(0), t(2)]),
        197 => (n, vec![z(0), z(2), m(1)]),
        204 => (n, vec![z(0), z(3), z(1), m(2)]),
        211 | 214 => (n, vec![z(0), z(1), m(2)]),
        217 => (n, vec![z(0), z(1), z(4), t(2), m(3)]),
        220 | 240 => (142, vec![z(0), Int(derived), k(1)]),
        221 | 241 => (212, vec![z(0), Int(derived)]),
        222 | 242 => (216, vec![Int(derived)]),
        223 | 243 => (172, vec![Int(derived)]),
        344..=346 => (n, vec![z(0), z(1), t(2)]),
        380 => (n, vec![z(0), z(2), z(3), z(1)]),
        382 => (n, vec![Int(derived), t(0)]),
        384 | 385 => (n, vec![z(0), z(2), flag(n == 384), t(1)]),
        391 | 392 => (n, vec![flag(n == 392), t(0)]),
        396 => (n, vec![z(0), z(1), z(2), m(3), t(4)]),
        397 => (n, vec![z(0), m(1), t(2)]),
        // Internal numbers (re/script-actions A.10) and numbers without a case: no message.
        _ => (n, Vec::new()),
    };
    ScriptMsg { number, values }
}

/// AI start 0/1 (case 0, re/ki-lifecycle 1.2): a parameter with `area ` (case-insensitive) counts as two
/// words and yields the area center (query 500, `FUN_004374A0`, unknown -> (0, 0)) from the runtime table
/// ([`super::area`], E-190), any other counts as one (`atol`). Before the third parameter comes `W − 2`
/// (`W` = word count); with at most two parameters this slot stays undescribed.
fn ki_words(sim: &Sim, a: &RtAction) -> Vec<MsgValue> {
    let is_area = |p: &str| {
        p.chars()
            .take(5)
            .collect::<String>()
            .eq_ignore_ascii_case("area ")
    };
    let words = a
        .params
        .iter()
        .fold(0i32, |w, p| w.wrapping_add(if is_area(p) { 2 } else { 1 }));
    let mut values = Vec::new();
    for (i, p) in a.params.iter().enumerate() {
        if i == 2 {
            values.push(MsgValue::Int(words.wrapping_sub(2)));
        }
        if is_area(p) {
            let (x, z) = super::query::area_center(sim, mid5(p));
            values.push(MsgValue::Int(x));
            values.push(MsgValue::Int(z));
        } else {
            values.push(MsgValue::Int(atol(p) as i32));
        }
    }
    values
}

/// Layer D (`Script_DispatchAction` `0x00422B60`). Implemented here: the speech length (`{500, TextID}` ->
/// D case `0x89` `Dialog_GetSpeechLengthMs`, reply in `values[0]`), the dialog start (`{39, T(name)}` ->
/// D case `0x20`, [`start_dialog`]), line, tip and info window (`{150|183|213, text, pos, …}` ->
/// [`speech::play_line`], [`speech::show_tip`], [`speech::info_window`]; a missing value slot counts 0) and
/// the empty D cases `0x0A`/`0x90` (17, 344-346, 350-352, 403; E-133) and 402 (E-219). The world actions are
/// handled by [`super::world::dispatch_world`], the presentation by [`super::present::dispatch_present`]. A
/// number without a case is "no effect yet" (safety net; the catalog has none left); the caller reports
/// [`ScriptTrace::Pending`]. `c` is the running container (a dialog start from the dialog container is
/// rejected).
pub fn dispatch(sim: &mut Sim, c: &mut Container, msg: &mut ScriptMsg) -> Dispatched {
    match msg.number {
        SPEECH_LENGTH => {
            let len = sim
                .rules
                .scripts
                .speech_ms(msg.int(0), sim.state.script.time_per_char);
            match msg.values.first_mut() {
                Some(v) => *v = MsgValue::Int(len),
                None => msg.values.push(MsgValue::Int(len)),
            }
            Dispatched::Done
        }
        // 402 (crew siege engine, command `0x4A`): no effect in the reimplementation (deviation from the original,
        // E-219); waiting for bit 1 is handled by `event_pair` as for 334 to 337.
        17 | 344..=346 | 350..=352 | 402 | 403 => Dispatched::Done,
        // D case `0x20`: start dialog. Never from a dialog container (deviates from E-174: the load check
        // rejects 39 in dialog scripts; the running container would otherwise be replaced during its own
        // tick).
        START_DIALOG => {
            let name = match msg.values.first() {
                Some(MsgValue::Text(name)) => name.clone(),
                _ => String::new(),
            };
            if c.id == DIALOG_CONTAINER_ID {
                trace(
                    sim,
                    ScriptTrace::Guarded {
                        container: c.id,
                        block: 0,
                        number: START_DIALOG,
                        why: format!("dialog start \"{name}\" from a dialog script"),
                    },
                );
            } else {
                start_dialog(sim, &name);
            }
            Dispatched::Done
        }
        // `{150, text, pos}` -> `Dialog_PlaySpeech`.
        PLAY_LINE => {
            speech::play_line(sim, msg.int(0), msg.int(1));
            Dispatched::Done
        }
        // `{183, text, pos}` -> `FUN_0043E250` (tip, info history).
        SHOW_TIP => {
            speech::show_tip(sim, msg.int(0), msg.int(1));
            Dispatched::Done
        }
        // `{213, text, pos, portrait, frame}` -> `FUN_00443A20` (close window, info history, line at `pos + 4`,
        // speech window with portrait and frame).
        INFO_WINDOW => {
            speech::info_window(sim, msg.int(0), msg.int(1), msg.int(2), msg.int(3));
            Dispatched::Done
        }
        // D cases `0x0D`/`0x0E` (`FUN_0042D910`): bind the countdown display to the own container or release
        // it (`m_CountDownTriggerEngineID`, E-205). 16 sets 0; 0 matches no container (the level container
        // has ID 1, E-185).
        15 => {
            sim.state.script.mission.countdown_container = c.id;
            Dispatched::Done
        }
        16 => {
            sim.state.script.mission.countdown_container = 0;
            Dispatched::Done
        }
        // Command actions ([`super::command::dispatch_command`]), presentation and world actions
        // ([`super::world::dispatch_world`]); a number without a case stays `Pending`.
        _ => {
            if let Some(issued) = super::command::dispatch_command(sim, msg) {
                return Dispatched::Issued(issued);
            }
            // Presentation actions ([`super::present::dispatch_present`]) before the world actions.
            if let Some(done) = super::present::dispatch_present(sim, c, msg) {
                return done;
            }
            match super::world::dispatch_world(sim, msg) {
                None => Dispatched::Pending,
                Some(Ok(())) => Dispatched::Done,
                Some(Err(why)) => Dispatched::Guarded(why),
            }
        }
    }
}

/// `GameEngine_StartDialog` (`0x00430B60`, D case `0x20`): creates the dialog container (ID -1) with all
/// blocks of the dialog script `name`; the dialog is thereby running (`+0x601`, [`super::dialog_running`]).
/// A running dialog ends first (`FUN_0042DC40` clears only the flag `+0x601`, the old container is freed;
/// the speech line remains) and is replaced that way. Not reachable from level scripts because they rest
/// during the dialog.
///
/// The original builds the path from the folder of the container (`m_strFileDir`), `p0` and `.abl` (re/script-actions
/// A.8) and parses the file. Here the already loaded script is in `ScriptRules::dialogs` under the normalized name `p0`
/// (loaded from the folder of the level script, the only container that may contain 39). If it
/// is missing (impossible after the load check), no container is created (`Guarded`); a running dialog is
/// then ended as in the original (E-143, `DialogEnded`). Return value: the dialog was started.
pub fn start_dialog(sim: &mut Sim, name: &str) -> bool {
    let rules = sim.rules;
    let replaced = sim.state.script.dialog.take().is_some();
    let Some(script) = rules.scripts.dialogs.get(name) else {
        trace(
            sim,
            ScriptTrace::Guarded {
                container: DIALOG_CONTAINER_ID,
                block: 0,
                number: START_DIALOG,
                why: format!("dialog script \"{name}\" missing in the rules"),
            },
        );
        if replaced {
            trace(sim, ScriptTrace::DialogEnded);
        }
        return false;
    };
    let source = ScriptSource {
        path: script.path.clone(),
        checksum: script.checksum,
    };
    sim.state.script.dialog = Some(Container::new(DIALOG_CONTAINER_ID, source, &script.file));
    trace(
        sim,
        ScriptTrace::DialogStarted {
            path: script.path.clone(),
        },
    );
    true
}

/// `ftol` (`CRT_ftol2`): truncate toward zero; outside `i32` (and NaN) `i32::MIN`.
fn ftol(v: f32) -> i32 {
    if v.is_nan() || !(-2_147_483_648.0..2_147_483_648.0).contains(&v) {
        i32::MIN
    } else {
        v as i32
    }
}

/// Percent of a speech length (220-223, 230): x87 in single precision `FILD len; FMUL 0.01; FIMUL pct;
/// ftol` - each product first in `f64`, then rounded to `f32`, truncated at the end.
/// `percent_len(29, 100)` = 29 (in `f64` it would be 28).
pub fn percent_len(len: i32, pct: i32) -> i32 {
    let scaled = (f64::from(len) * 0.01) as f32;
    ftol((f64::from(scaled) * f64::from(pct)) as f32)
}

/// Percent of a time (233, 240-243): x87 in single precision `FILD p; FILD t; FMUL 0.01; FMULP; ftol` -
/// each product first in `f64`, then rounded to `f32`, truncated at the end.
/// `percent_time(50, 58)` = 29 (in `f64` it would be 28).
pub fn percent_time(pct: i32, t: i32) -> i32 {
    let scaled = (f64::from(t) * 0.01) as f32;
    ftol((f64::from(pct) * f64::from(scaled)) as f32)
}
