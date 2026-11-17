//! Map description from the menu and everything that is fixed before parsing: seed hash
//! (`Game_StartRandomMapFromSeedText` `0x004BF3E0`), script path and defines (`RandomMap_Generate`
//! `0x00546ACB … 0x00546EB0`), map factor (`FUN_0053FAC0`). See re/random-map Z.2, Z.5.2.

use anyhow::bail;
use nml_formats::mp_levels::RandomMapTypes;
use nml_sim::fx::Fx;

/// The menu values of a random map; they live in `nml_sim::random_map` (savegame and start package).
pub use nml_sim::random_map::RandomMapSpec;

/// Seed: sum of `(i+1) · wchar[i]` over `wcslen` of the seed text, 32 bits with overflow
/// (`0x004BF637 … 0x004BF656`). `wchar_t` is one UTF-16 unit on Windows; `wcslen` ends at the
/// first NUL.
pub fn seed_hash(text: &str) -> u32 {
    text.encode_utf16()
        .take_while(|&unit| unit != 0)
        .enumerate()
        .fold(0u32, |hash, (i, unit)| {
            hash.wrapping_add((i as u32).wrapping_add(1).wrapping_mul(u32::from(unit)))
        })
}

/// Edge length of the map in tiles per size index (64/128/256). Other indexes are rejected by
/// [`script_and_defines`]; here they yield 64.
pub fn map_dim(size: i32) -> usize {
    match size {
        1 => 128,
        2 => 256,
        _ => 64,
    }
}

/// Size define and size index `G+0x11C` (`0x00546ACB … 0x00546C4A`): 64 -> `MAP_SIZE_SMALL`/0,
/// 128 -> `MAP_SIZE_MEDIUM`/1, 256 -> `MAP_SIZE_LARGE`/2, **any other size** `MAP_SIZE_SMALL`/0
/// (D-326).
pub fn size_defines(dim: usize) -> (&'static str, i32) {
    match dim {
        128 => ("MAP_SIZE_MEDIUM", 1),
        256 => ("MAP_SIZE_LARGE", 2),
        _ => ("MAP_SIZE_SMALL", 0),
    }
}

/// Map factor `G+0x268` (`FUN_0053FAC0`): `Fixed_FromFloat(1.0/2.0/4.0)` per size index 0/1/2,
/// otherwise 1.0.
pub fn scale_factor(index: i32) -> Fx {
    Fx::from_f32(match index {
        1 => 2.0,
        2 => 4.0,
        _ => 1.0,
    })
}

/// Splits the define string at `,` (`_mbschr`, `0x00546C88`), removes only spaces `0x20` in each
/// part (`FUN_00541C70`, tabs stay) and drops empty parts (D-326).
pub fn split_defines(text: &str) -> Vec<String> {
    text.split(',')
        .map(|part| part.replace(' ', ""))
        .filter(|part| !part.is_empty())
        .collect()
}

/// Script path (`MSCRIPT` of the map type, `FUN_004CC8E0`) and defines in list order:
/// size define, then the parts of `GTYPE + "," + LTYPE + "," + DTYPE` (`FUN_004CC9D0`,
/// `FUN_004CC930`, `FUN_004CC980`). Values that the menu does not offer are an error.
pub fn script_and_defines(
    spec: &RandomMapSpec,
    types: &RandomMapTypes,
) -> anyhow::Result<(String, Vec<String>)> {
    // Crash guard, not a duplicate check: when loading, the client builds the random map from the still
    // unchecked start package (`GameSession::resume_from`, `read_net_save`), that is, before `check_start`.
    // The number of players determines the number of player areas (`lib.rs`, `repeat_n(…, players as
    // usize)`): -1 would yield almost 2^64 areas and, without this check, occupy unlimited memory.
    if !(2..=8).contains(&spec.players) {
        bail!(
            "random map: number of players {} outside 2 to 8",
            spec.players
        );
    }
    if !(0..=2).contains(&spec.size) {
        bail!("random map: map size {} outside 0 to 2", spec.size);
    }
    let Some(script) = types.map_scripts.get(spec.terrain) else {
        bail!(
            "random map: map type {} unknown ({} map types in randommaptypes.ini)",
            spec.terrain,
            types.map_scripts.len()
        );
    };
    let Some(landscape) = types.landscape_types.get(spec.landscape) else {
        bail!(
            "random map: landscape {} unknown ({} landscapes in randommaptypes.ini)",
            spec.landscape,
            types.landscape_types.len()
        );
    };
    let Some(daytime) = types.daytime_types.get(spec.daytime) else {
        bail!(
            "random map: time of day {} unknown ({} times of day in randommaptypes.ini)",
            spec.daytime,
            types.daytime_types.len()
        );
    };
    let Some(game_type) = types.game_types.get(spec.game_type) else {
        bail!(
            "random map: game mode {} unknown ({} game modes in randommaptypes.ini)",
            spec.game_type,
            types.game_types.len()
        );
    };
    let (size_define, _) = size_defines(map_dim(spec.size));
    let mut defines = vec![size_define.to_string()];
    defines.extend(split_defines(&format!(
        "{},{},{}",
        game_type.gtype, landscape, daytime
    )));
    Ok((script.clone(), defines))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use nml_formats::mp_levels::GameTypeDesc;

    /// `randommaptypes.ini` in the layout of the original (names freely chosen, no game data).
    pub(crate) fn test_types() -> RandomMapTypes {
        let words = |list: &[&str]| list.iter().map(|w| w.to_string()).collect::<Vec<_>>();
        RandomMapTypes {
            game_types: [
                "NORMAL",
                "NORMAL",
                "NORMAL",
                "KING_OF_THE_HILL",
                "RAILROAD1",
            ]
            .iter()
            .enumerate()
            .map(|(i, g)| GameTypeDesc {
                name_id: 100 + i as i32,
                gtype: g.to_string(),
            })
            .collect(),
            maps: (0..6).collect(),
            map_scripts: words(&[
                "maps/a.rnd",
                "maps/b.rnd",
                "maps/c.rnd",
                "maps/d.rnd",
                "maps/e.rnd",
                "maps/f.rnd",
            ]),
            landscapes: (0..5).collect(),
            landscape_types: words(&["CARIBBEAN", "PRAIRIE", "DESERT", "WINTER", "MEADOW"]),
            daytimes: (0..2).collect(),
            daytime_types: words(&["DAY", "NIGHT"]),
            ..RandomMapTypes::default()
        }
    }

    pub(crate) fn test_spec() -> RandomMapSpec {
        RandomMapSpec {
            seed_text: "ZKA".to_string(),
            size: 0,
            players: 2,
            terrain: 0,
            landscape: 4,
            daytime: 0,
            game_type: 0,
        }
    }

    /// The description lives in `nml_sim::random_map`; JSON (control channel) with umlauts in the seed text.
    #[test]
    fn random_map_spec_round_trips_through_json() {
        let spec = RandomMapSpec {
            seed_text: "ÄÖ ü".to_string(),
            size: 1,
            players: 4,
            terrain: 2,
            landscape: 0,
            daytime: 1,
            game_type: 3,
        };
        let json = serde_json::to_string(&spec).unwrap();
        assert!(json.contains("ÄÖ ü"));
        assert_eq!(serde_json::from_str::<RandomMapSpec>(&json).unwrap(), spec);
    }

    // QUIRK O-430: kept
    #[test]
    fn seed_hash_runs_over_utf16_units() {
        assert_eq!(seed_hash("AC"), 199);
        assert_eq!(seed_hash("EA"), 199);
        assert_eq!(seed_hash(""), 0);
        assert_eq!(seed_hash("ä"), 228);
        // Characters outside the BMP count as two units like `wchar_t`.
        assert_eq!(seed_hash("\u{1F600}"), 0xD83D + 2 * 0xDE00);
        let long = "\u{FFFF}".repeat(40);
        let expected = (1..=40u32).fold(0u32, |h, i| h.wrapping_add(i.wrapping_mul(0xFFFF)));
        assert_eq!(seed_hash(&long), expected);
        // `wcslen` ends at the first NUL.
        assert_eq!(seed_hash("AC\0EA"), 199);
    }

    // QUIRK D-326: kept
    #[test]
    fn defines_follow_size_and_types() {
        let spec = RandomMapSpec {
            size: 1,
            game_type: 3,
            landscape: 3,
            daytime: 1,
            terrain: 2,
            ..test_spec()
        };
        let (script, defines) = script_and_defines(&spec, &test_types()).unwrap();
        assert_eq!(script, "maps/c.rnd");
        assert_eq!(
            defines,
            ["MAP_SIZE_MEDIUM", "KING_OF_THE_HILL", "WINTER", "NIGHT"]
        );
        assert_eq!(split_defines(" A ,\tB,,"), ["A", "\tB"]);
        assert!(split_defines("").is_empty());
        assert_eq!(map_dim(0), 64);
        assert_eq!(map_dim(2), 256);
    }

    // QUIRK D-326: kept
    #[test]
    fn scale_factor_per_size() {
        assert_eq!(scale_factor(0), Fx(4096));
        assert_eq!(scale_factor(1), Fx(8192));
        assert_eq!(scale_factor(2), Fx(16384));
        assert_eq!(scale_factor(7), Fx(4096));
        assert_eq!(size_defines(100), ("MAP_SIZE_SMALL", 0));
        assert_eq!(size_defines(64), ("MAP_SIZE_SMALL", 0));
        assert_eq!(size_defines(128), ("MAP_SIZE_MEDIUM", 1));
        assert_eq!(size_defines(256), ("MAP_SIZE_LARGE", 2));
    }

    #[test]
    fn invalid_spec_is_rejected() {
        let types = test_types();
        let cases = [
            (
                RandomMapSpec {
                    players: 1,
                    ..test_spec()
                },
                "number of players 1",
            ),
            (
                RandomMapSpec {
                    players: 9,
                    ..test_spec()
                },
                "number of players 9",
            ),
            (
                RandomMapSpec {
                    players: -1,
                    ..test_spec()
                },
                "number of players -1",
            ),
            (
                RandomMapSpec {
                    players: 1_000_000,
                    ..test_spec()
                },
                "number of players 1000000",
            ),
            (
                RandomMapSpec {
                    size: 3,
                    ..test_spec()
                },
                "map size 3",
            ),
            (
                RandomMapSpec {
                    terrain: 6,
                    ..test_spec()
                },
                "map type 6",
            ),
            (
                RandomMapSpec {
                    landscape: 5,
                    ..test_spec()
                },
                "landscape 5",
            ),
            (
                RandomMapSpec {
                    daytime: 2,
                    ..test_spec()
                },
                "time of day 2",
            ),
            (
                RandomMapSpec {
                    game_type: 5,
                    ..test_spec()
                },
                "game mode 5",
            ),
        ];
        for (spec, text) in cases {
            let err = script_and_defines(&spec, &types).unwrap_err().to_string();
            assert!(err.contains(text), "{err}");
        }
        assert!(script_and_defines(&test_spec(), &types).is_ok());
    }
}
