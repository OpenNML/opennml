# Resource presets `resourcenconfig.ini`

`data/gifcon/resourcenconfig.ini`, line-oriented text (cp1252, CRLF, **no** bracket grammar). For the game setting
"Resources" (four levels) it defines the starting resources of the players and the amount of certain deposits.

Application: deposits are replaced only on **random maps** (a placement flag set only when creating a random map
level; `.wld` files place with flag 0); before that the whole `[CONFIG]` block of the object is cleared (a quirk: also
for GUIDs without a record). Fixed maps take the amounts from `[CONFIG]` of the `.wld`; starting resources come from
this file at levels 1 ... 3 there, at level 0 from `[RESOURCES]` of the `.lvl`. The campaign takes over `Engine.ini`
`<Resourcen>` without effect; `CONFIG_VALUE0` acts on deposits only on random maps.

## Purpose and locations

One file, read when the game engine is constructed, directly after `translate.ini`. Comment of the file: the levels
"von Karte übernehmen" (take from map), "niedrig" (low), "normal", "hoch" (high).

## Layout

```
#Gold                      Comment (not evaluated, falls through the comparisons)
CONFIG_GUID 0              Create a record (config_guid, v[4]), config_guid = atol(Mid(12))
CONFIG_VALUE0 2000         Index = atol(Mid(12)) (0...3), value = atol(Mid(14))
CONFIG_VALUE1 500
CONFIG_VALUE2 1500
CONFIG_VALUE3 3000
/CONFIG_GUID               Append the record to the starting-resource list

GUID 3500                  Object GUID = atol(Mid(5)) (applies until /GUID)
CONFIG_GUID 26             Record for config value 26 ("yield gold")
CONFIG_VALUE0 2000 ...
/GUID                      Map[GUID] = record
```

Each line is recognized by comparing fixed prefixes: `Left(5) == "GUID "`, `Left(12) == "CONFIG_GUID "`,
`Left(12) == "CONFIG_VALUE"`, `Left(5) == "/GUID"`, whole line `== "/CONFIG_GUID"`. The line is **not** trimmed.

| Key | Meaning |
|---|---|
| `CONFIG_GUID n` without an enclosing `GUID` | Starting resource; `n` is the **internal resource index** 0 gold, 1 wood, 2 food (unlike in the `.lvl`, where 0 is wood) |
| `GUID g` ... `/GUID` | Deposit type `g`; the contained `CONFIG_GUID` is a config number from [config-guids.md](config-guids.md) (24 food, 26 gold) |
| `CONFIG_VALUEi v` | Value for level `i` (0 ... 3) |

Shipped data: starting resources gold/wood/food 2000/500/1500/3000 each; gold deposit 3500 (config 26) and fertile
soil 3503 (config 24) 2000/3000/5000/8000 each; fishing baskets 3505 (config 24) 250/250/500/750.

## Application

The level comes from the game settings.

- Starting resources: only for a random map or at a level > 0, and at levels 0 ... 3: for all 9 players the resource
  `n` is set to `v[level]`.
- Deposits: at levels 0 ... 3 the configuration of the placed object is replaced by `{config_guid: v[level]}` (cleared
  beforehand).

## Coordinate system and units

Whole resource units.

## Ignored sections

Comment lines (`#`) and empty lines fall through all comparisons. A `CONFIG_VALUE` with an index outside 0 ... 3 is
discarded.

## Validation

The file has 6 records without deviations from the schema above.

## Open questions

- Source of error: the index is exactly one digit at position 12 and the value starts at position 14; two-digit
  indexes or double spaces would be read wrongly (not the case in the shipped data).
