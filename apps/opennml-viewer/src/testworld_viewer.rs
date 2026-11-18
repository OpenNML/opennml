//! Part of the test world with the viewer's settings: `sim_setup` as in the control channel, the AI host with its
//! settings, and the script host. The world itself (types, rules, hosts) lives in
//! `opennml_client::game::testworld` (feature `testkit`).

use opennml_client::game::options::DEFAULT_GAME_SPEED;
use opennml_client::game::sim_host::SimHost;
use opennml_client::game::testworld::{ai_slot, human_slot, ki_rules};

use crate::app::SimSetupArgs;
use crate::config::{Config, apply_config};

/// `sim_setup` as in the control channel: values into the settings, check, restart.
pub fn sim_setup(
    host: &mut SimHost,
    config: &mut Config,
    args: SimSetupArgs,
) -> Result<(), String> {
    let mut next = config.clone();
    let restart = args.apply(&mut next);
    next.check().map_err(|e| format!("{e:#}"))?;
    apply_config(host, &next, restart)?;
    *config = next;
    Ok(())
}

/// Host of an AI match: slot 0 human, slot 1 AI kind 3 (nation 1), AI on.
pub fn ki_host() -> (SimHost, Config) {
    let rules = ki_rules();
    let setup = opennml_sim::setup::MatchSetup::default_for(&rules);
    let mut host = SimHost::new(rules, setup, DEFAULT_GAME_SPEED).unwrap();
    let mut config = Config::default();
    let args = SimSetupArgs {
        slots: Some(vec![human_slot(), ai_slot(1, 3)]),
        ki_enabled: Some(true),
        ..Default::default()
    };
    sim_setup(&mut host, &mut config, args).unwrap();
    (host, config)
}

/// Host with [`script_rules`] and the script switched on (via `sim_setup` as in the control channel).
pub fn script_host(level: &str) -> (SimHost, crate::config::Config) {
    script_host_with_areas(level, &[])
}

/// Like [`script_host`], with named areas of the map (`[x0, z0, x1, z1]`).
pub fn script_host_with_areas(
    level: &str,
    areas: &[(&str, [i32; 4])],
) -> (SimHost, crate::config::Config) {
    script_host_with(level, &[], areas)
}

/// Like [`script_host`], with dialogs `(name, script text)` that the level script (action 39) can start.
pub fn script_host_with_dialogs(
    level: &str,
    dialogs: &[(&str, &str)],
) -> (SimHost, crate::config::Config) {
    script_host_with(level, dialogs, &[])
}

fn script_host_with(
    level: &str,
    dialogs: &[(&str, &str)],
    areas: &[(&str, [i32; 4])],
) -> (SimHost, crate::config::Config) {
    let mut rules = opennml_client::game::testworld::rules();
    rules.scripts = opennml_sim::script::testing::rules_with(level, dialogs, areas);
    assert!(
        rules.scripts.errors.is_empty(),
        "{:?}",
        rules.scripts.errors
    );
    let setup = opennml_sim::setup::MatchSetup::default_for(&rules);
    let mut host = SimHost::new(rules, setup, DEFAULT_GAME_SPEED).unwrap();
    let mut config = crate::config::Config::default();
    let args = crate::app::SimSetupArgs {
        script_enabled: Some(true),
        ..Default::default()
    };
    sim_setup(&mut host, &mut config, args).unwrap();
    (host, config)
}
