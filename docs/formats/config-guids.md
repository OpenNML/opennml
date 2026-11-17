# Configuration values (`CONFIG_GUID`)

Meaning of the property GUIDs that appear in `data/projects/default.dat`, in the `.lvl` (`[RESOURCES]`,
`[CONFIGDATA]`, `[UPGRADES]`) and in the `[CONFIG]` block of a `.wld` object. The block grammar
(`CONFIG_GUID n { CONFIG_LABEL ... CONFIG_ENABLE ... CONFIG_VALUE "..." }`) and the reading order are described in
[lvl.md](lvl.md); this document assigns the numbers and says where the game reads them.

Corrections that apply throughout:

- The second set that is consulted for a missing value is `default.dat`. If a text is missing from a choice list, the
  lookup returns the pointer value of its input (not 0, and not deterministic), see [setup-readers.md](setup-readers.md)
  section 1.
- Objects, GUID 17: the current value is the load, clamped to the maximum; the load amount can be above it (trade
  cart: 800 wood).
- Upgrades, GUID 1: the check reads the whole value set of the type for the slot; 0 hides the button.

## Purpose and locations

| File | Role |
|---|---|
| `data/config/*.cfg` (`gebäude`, `einheiten`, `tiere`, `rohstoffe`, `ding`, `spells`, `upgrades`, `levelresources`) | Editor description: `PROPERTY { GUID n LABEL "..." CHECKBOX SLIDER {MIN MAX} | DROPDOWN {...} }`. **Not read by the game** (no file name in the executable); only a source of the labels |
| `data/projects/dropdown.ini` | Text to number for choice values (`VALUEGUID n` = `CONFIG_GUID`, `DROPDOWN number text`) |
| `data/projects/default.dat` | Default values per object type (512) and upgrade (232) |
| `.lvl`, `.wld` | Overrides per player or per placed object |

## Reading rules

- A value is lowercased first. If a choice list exists for the GUID, its number is the value; otherwise the text is
  parsed as a decimal integer.
- The value for (player slot, object or upgrade GUID, config GUID) falls back to a second set if it is missing or 0.
  The result is 0 if it is set nowhere.
- The same lookup exists for the owner of an object.
- The cost vector of a type uses the config values 12, 11, 10, 22 and 0.

## Layout

### Object types

| GUID | Label (editor) | Values | Read as / effect |
|---|---|---|---|
| 1 | "... is in the level" | `aktiviert`=1 / `deaktiviert`=0 | Availability (UI/AI, not followed further) |
| 10, 11, 12 | Food, wood, gold | 0 ... 10000 | Cost; resource index 2, 1, 0 |
| 13 | Production time (sec.) | 0 ... 600 | Construction time (`*100` construction step units) or training/research time (`*1000` ms) |
| 14 | Hit points | 0 ... 10000 | Maximum HP |
| 15 | Extra energy | 0 ... 1000 | Mana (abilities) |
| 16 | Speed | choice: 128, 145, 183, 200, 227, 254 | Movement |
| 17 | Carrying capacity | 0 ... 15 (editor), shipped data 20/200/300/500 | Load capacity; the current value is the load, clamped to the maximum; the load amount can be above it (trade cart: 800 wood) |
| 18 | Sight range | choice: 16, 20, 32, 40, 52, 64, 5 (`keine`); 4 units = 1 tile | Sight |
| 19, 20 | Attack value melee/ranged | | Combat |
| 21 | Weapon range | choice: 8, 12, 28, 36, 48, 56 | Combat |
| 22 | Living space | 0 ... 100 | Population that a unit occupies (resource index 3) |
| 23 | Capacity | 0 ... 15 | Quartering (quarter module) |
| 24, 25, 26 | Yield food/wood/gold | 0 ... 10000 | Amount of a deposit (fertile soil, fishing baskets, game animal / tree / gold deposit) |
| 27 | Living space for | 0 ... 100 | Living space that a finished building provides (resource index 4) |
| 28, 29, 30 | Stamina, morale, experience | | Combat |
| 31, 32, 33 | Food/wood/gold yield per minute | | Gathering tick `60000/value` ms per unit |
| 34 | Minimum combat distance | choice: 0, 4, 8, 16 | Combat |

### Upgrades (`[UPGRADES]`, `UPGRADE_GUID n`)

| GUID | Meaning |
|---|---|
| 1 | Upgrade available in the level; the check reads the whole value set of the type for the slot, 0 hides the button |
| 10, 11, 12, 13 | Cost and research time as for objects |
| 114 ... 133 | Percentage change of the value `GUID - 100` (114 -> HP, 116 -> speed, 117 -> load capacity, 118 -> sight, 119/120 -> attack, 121 -> range, 122 -> living space, 123 -> capacity, 128 -> stamina, 129 -> morale, 130 -> experience, 131/132/133 -> yield food/wood/gold per minute) |
| 15 (`spells.cfg`) | Extra energy that an ability costs |

Applying an upgrade reads the values 113 ... 134 of the upgrade and enters them as a percentage modifier on the value
`GUID - 100` of all types whose `OBJ_UPGRADES` contains the upgrade.

### Level resources (`[RESOURCES]` of the `.lvl`)

| GUID | Meaning | Target |
|---|---|---|
| 0 | Wood | Resource index 1 |
| 1 | Food | Resource index 2 |
| 2 | Gold | Resource index 0 |
| 40 | Season/time of day (choice 0 ... 3) | Upgrade set |

## Coordinate system and units

Times in seconds, amounts in whole resource units, sight/range in quarter tiles, yields per minute.

## Ignored areas

`CONFIG_LABEL`; the range data (`MIN`/`MAX`) of the editor `.cfg` files are not checked (load capacity 200 lies
outside the editor range).

## Validation

A check script resolves all `CONFIG_GUID` of the levels against `data/config/*.cfg`
([lvl.md](lvl.md)). The assignment of the numbers to effects comes from the places where the game uses them.

## Open questions

- Behavior of the lookup when a text is missing from the choice list (does it return the input pointer?); the shipped
  data mixes case ("Sehr Niedrig"), which the lowercasing absorbs.
- The second set in the value lookup (presumably neutral or `default.dat`).
