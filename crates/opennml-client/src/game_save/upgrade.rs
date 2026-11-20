//! Structure steps of the game savegame: an older body is read as an untyped value and brought step by step to the
//! current savegame number before it is typed (rename, move or retype a field, turn a tuple into a struct).

use anyhow::{Context, bail};
use rmpv::Value;

use super::FIRST_SAVE_FORMAT;

/// One structure step on the value of the body.
pub(crate) type Step = fn(&mut Value) -> anyhow::Result<()>;

/// The structure steps: `STEPS[i]` brings savegame number `FIRST_SAVE_FORMAT + i` to `FIRST_SAVE_FORMAT + i + 1`.
/// Every number has its step: a number raised only for a recompute step (`opennml_sim::save::upgrade::RECOMPUTE`)
/// gets the identity `|_| Ok(())` here.
pub(crate) const STEPS: &[Step] = &[];

/// Applies the steps from number `from` on: `steps[from - FIRST_SAVE_FORMAT..]` in order.
pub(crate) fn upgrade(value: &mut Value, from: u32, steps: &[Step]) -> anyhow::Result<()> {
    let first = from
        .checked_sub(FIRST_SAVE_FORMAT)
        .and_then(|i| usize::try_from(i).ok())
        .filter(|&i| i <= steps.len())
        .with_context(|| format!("no structure steps from savegame number {from}"))?;
    for (number, step) in (from..).zip(&steps[first..]) {
        step(value).with_context(|| format!("structure step {number} -> {}", number + 1))?;
    }
    Ok(())
}

/// Removes the field at `path` (keys of nested maps) and returns its value.
#[cfg_attr(
    not(test),
    expect(dead_code, reason = "for the structure steps; none exists yet")
)]
pub(crate) fn take(value: &mut Value, path: &[&str]) -> anyhow::Result<Value> {
    let Some((last, parents)) = path.split_last() else {
        bail!("empty path");
    };
    let map = map_at(value, parents, false)?;
    let index = map
        .iter()
        .position(|(k, _)| k.as_str() == Some(last))
        .with_context(|| format!("field {} missing", path.join(".")))?;
    Ok(map.remove(index).1)
}

/// Sets the field at `path` to `new`; missing maps on the way are created, an existing field is replaced.
#[cfg_attr(
    not(test),
    expect(dead_code, reason = "for the structure steps; none exists yet")
)]
pub(crate) fn put(value: &mut Value, path: &[&str], new: Value) -> anyhow::Result<()> {
    let Some((last, parents)) = path.split_last() else {
        bail!("empty path");
    };
    let map = map_at(value, parents, true)?;
    match map.iter_mut().find(|(k, _)| k.as_str() == Some(last)) {
        Some((_, v)) => *v = new,
        None => map.push((Value::from(*last), new)),
    }
    Ok(())
}

/// The map at `path`; with `create` missing fields on the way become empty maps.
fn map_at<'a>(
    value: &'a mut Value,
    path: &[&str],
    create: bool,
) -> anyhow::Result<&'a mut Vec<(Value, Value)>> {
    let mut current = value;
    for key in path {
        let Value::Map(map) = current else {
            bail!("{key}: parent is not a map");
        };
        let index = match map.iter().position(|(k, _)| k.as_str() == Some(key)) {
            Some(index) => index,
            None if create => {
                map.push((Value::from(*key), Value::Map(Vec::new())));
                map.len() - 1
            }
            None => bail!("field {key} missing"),
        };
        current = &mut map[index].1;
    }
    match current {
        Value::Map(map) => Ok(map),
        _ => bail!("{} is not a map", path.join(".")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game_save::SAVE_FORMAT;

    /// Raises counter `i` of `host.test_steps`.
    fn bump(v: &mut Value, i: usize) -> anyhow::Result<()> {
        let mut counters = take(v, &["host", "test_steps"])?;
        let Value::Array(list) = &mut counters else {
            anyhow::bail!("counters are not a list");
        };
        let counter = list.get_mut(i).context("counter missing")?;
        *counter = Value::from(counter.as_u64().context("not a number")? + 1);
        put(v, &["host", "test_steps"], counters)
    }

    #[test]
    fn steps_run_from_loaded_number() {
        let fake: [Step; 3] = [|v| bump(v, 0), |v| bump(v, 1), |v| bump(v, 2)];
        let mut value = Value::Map(vec![(
            "host".into(),
            Value::Map(vec![("test_steps".into(), Value::Array(vec![0.into(); 3]))]),
        )]);
        upgrade(&mut value, FIRST_SAVE_FORMAT + 1, &fake).unwrap();
        assert_eq!(
            take(&mut value, &["host", "test_steps"]).unwrap(),
            Value::Array(vec![0.into(), 1.into(), 1.into()])
        );
    }

    #[test]
    fn steps_match_save_format() {
        assert_eq!(STEPS.len() as u32, SAVE_FORMAT - FIRST_SAVE_FORMAT);
    }
}
