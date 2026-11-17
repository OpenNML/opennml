# `targets.ini`, `priorities.ini`, `flowcontroltragets.ini`, `targetdefines.ini` - match filters ("MatchStructs")

Three files in `data/gifcon/` describe object filters in the same block format; `targetdefines.ini` supplies the
symbolic names for them. The filters are evaluated by the game's object filter test, which the game logic itself also
uses with built-in filters (target search, projectiles, healing).

Lines beginning with `#` match no key and are therefore ignored. Unknown names for `MM_PROPID`/`PROPERTY`/
`MATCHBITS` and unknown values for `MM_MODE`/`MM_MODE2` **abort the whole file**. `DISABLERBUTTON` has no effect in
the shipped data. `priorities.ini` is read by the same filter reader.

| File | Purpose |
|---|---|
| `targetdefines.ini` | Names -> number (loaded before the others) |
| `targets.ini` | Right-click commands: which own objects (`MY_MM`) execute which command (`BUTTONMSG`) on which target (`TARGET_MM`); mouse pointer `MAUSICONID` |
| `flowcontroltragets.ini` (sic) | Second list of the same reader: filters for scripts ("FlowControl") and the AI API, number in `BUTTONMSG` |
| `priorities.ini` | Target priorities of the automatic target choice, order = priority |

## `targetdefines.ini`

Lines `NAME VALUE` (split at the first space, value `sscanf("%d")`), 212 lines, no comments. Unknown names in the
filter files give "not found"; the value is then **not** set (return 0).

The values follow the scheme of the module flags:

- `PROP_xxx n` - module index `n` (= `PROPID` in `property.ini`), e.g. `PROP_ATTACK 17`, `PROP_FIGHTNEAR 18`.
- `MID_PROP_xxx_yyy w` - flag bit **plus module index in the low byte**: `w = bit | n`. Example
  `MID_PROP_ATTACK_SWORD 16401 = 0x4011` (bit `0x4000`, module `0x11`). Low byte `0xFF` = category bit, e.g.
  `MID_PROP_GLOBAL_HURTBY_SWORD 8447 = 0x20FF`.

Entries that matter for combat (the complete list is in the file):

| Name | Value | Meaning |
|---|---|---|
| `MID_PROP_GLOBAL_CANCLEARUP` | `0x10FF` | Category: "scout" -> reconnaissance sight (fog state 7) |
| `MID_PROP_GLOBAL_HURTBY_SWORD/MOUNTY/PEEK/CANNON` | `0x20FF`/`0x40FF`/`0x80FF`/`0x100FF` | Category: accepts an attack bonus |
| `MID_PROP_PTTOPT_UNITCLEARINGUP` | `0x80000` (module 0) | Reconnaissance sight |
| `MID_PROP_PTTOPT_SADDLE` | `0x8000` | Horse/mount (excluded from the target choice) |
| `MID_PROP_ATTACK_FIGHTNEAR/FIGHTFAR` | `0x111`/`0x811` | Can fight near / far (on foot; `RIDING...`, `RIDINGBOAT...` mounted/from a boat) |
| `MID_PROP_ATTACK_SWORD/MOUNTY/PEEK/CANNON/FIRE` | `0x4011`/`0x8011`/`0x10011`/`0x20011`/`0x40011` | Attack kind for `bonus.ini` |
| `MID_PROP_ATTACK_HASSHOOTTOLERANCE` | `0x80011` | Aiming angle tolerance (`m_ShootTolerance`) |
| `MID_PROP_ATTACK_CHECKLINEOFSIGHT` | `0x100011` | Check the line of sight |
| `MID_PROP_SHOOTING_ARROW/CANON/GUN/SHIP` | `0x10A`/`0x20A`/`0x40A`/`0x2000A` | Kind of shot |
| `MID_PROP_SHOOTING_RELOAD` | `0x1000A` | Magazine, reloading |
| `MID_PROP_SHOOTING_FIREARROW`, `..._CANSHOOTFIREARROW` | `0x4000A`, `0x20000A` | Fire arrow |
| `MID_PROP_FIGHTFAR_ISBOUNTY` | `0x113` | Experience /10; in the shipped data only transport and warships 1108, 1109, 2010, no headhunter; FightFar uses the constant as a mask for `m_IsShip` |
| `MID_PROP_ANIMAL_IS_AGGRESSIVE` | `0x114` | Neutral animal attacks / is attacked automatically |
| `MID_PROP_BUILDING_CONSTRUCTED_FINISHED` | `0x403` | Finished building (sight, healing) |

## Block format

Line by line; `#` at the start of a line = comment; empty and unknown lines are skipped (`priorities.ini` contains a
line `Alle Gebäude, für Nichtgeister` without `#` - without effect).

```
BEGIN                         new entry
DISABLERBUTTON                (targets.ini only) disabled flag of the entry = 0
BUTTONMSG n                   command/filter number (sscanf %d)
MY_MM                         filter for the acting object  -> entry field 1
  ... filter lines ...
MM_END
TARGET_MM                     filter for the target         -> entry field 2
  ... filter lines ...
MM_END
MAUSICONID n                  mouse pointer
END                           append the entry to the list
```

`priorities.ini` uses only `BEGIN`, `MY_MM`, `TARGET_MM`, `END` (without `BUTTONMSG`).

### Filter lines

Keys are recognized by prefix comparison; order of the checks: `DISABLERBUTTON`, `MM_END`, `MM_NEXT`, `MM_MODE2`,
`MM_MODE`, `MM_PROPID`, `MM_PROPERTY`, `MM_MATCHBITS`, `MM_GUID`, `MM_METHODINDEX`, `MM_IS_ATTACKING_WHO`,
`MM_STATUS`. Values of `MM_MODE`/`MM_MODE2`/`MM_STATUS` are **ORed** into the respective word, and so is
`MM_MATCHBITS` (so it is better not to mix bits of different modules in one filter - the low byte is ORed too).

Filter fields:

| Field | Content |
|---|---|
| mode | `MM_MODE` bits |
| mode2 | `MM_MODE2` bits |
| status | `MM_STATUS` bits (status word of the object) |
| match bits | `MM_MATCHBITS` |
| property | `MM_PROPID` / `MM_PROPERTY` |
| GUID | `MM_GUID` |
| player | Player for `MM_CHECKFOG` (set at run time) |
| method index | `MM_METHODINDEX` |
| reference object | Reference object (HID), at run time |
| attacking who | `MM_IS_ATTACKING_WHO`: `ME` 1, `MY_FRIEND` 2, `MY_FORMATION` 4 |
| range/position | Minimum/maximum distance, target tile (run time) |
| angle tolerance, line of sight | Angle tolerance, check the line of sight |
| next | Follow-up filter (`MM_NEXT`) |

`MM_MODE`:

| Name | Bit | Check |
|---|---|---|
| `MM_CHECKSTATUS` / `MM_CHECKNOTSTATUS` | `0x1` / `0x2000` | `status & status bits != 0` (for riders/boats additionally the `MOVE` bit of the carrier) |
| `MM_CHECKHANDLETYPE` / `..._ANDMODE` / `MM_CHECKNOTHANDLETYPE` | `0x2` / `0x40` / `0x4000` | Match bits: low byte `0xFF` -> category bits, otherwise flags of the module in the low byte; at least one bit (`ANDMODE`: all bits) |
| `MM_CHECKPROPERTY` / `MM_CHECKNOTPROPERTY` | `0x4` / `0x8000` | Module `property` present |
| `MM_CHECKGUID` / `MM_CHECKNOTGUID` | `0x8` / `0x10000` | Type GUID = `GUID` |
| `MM_ANDALL` | `0x10` | Combination AND instead of OR (see below) |
| `MM_HL_MYLIST` | `0x100` | Owner = reference player |
| `MM_HL_NOMANSLIST` | `0x200` | Owner = 8 (neutral, animals) |
| `MM_HL_ENEMYLIST` | `0x400` | Enemy relation between the reference player and the owner **and** module 0 (Walk) or 3 (Building) |
| `MM_HL_FRIENDLIST` | `0x800` | Friend relation between the reference player and the owner |
| `MM_HL_TREELIST` | `0x1000` | Owner = 9 (trees) |
| `MM_HL_PLAYERLIST` | `0x40000` | Owner < 8 |
| `MM_CHECKNOTLOAD`, `MM_CHECKLOADGOLD/WOOD/FOOD` | `0x20000`, `0x100000`/`0x200000`/`0x400000` | Carried load |
| `MM_ENERGYLOST` | `0x800000` | Hit points not full |
| `MM_CHECKFOG` | `0x2000000` | Fog state of the object's tile for player `player` has bit 2 (3 or 7; camouflaged objects see the combat documentation) |
| `MM_RETURN_DIED` | `0x4000000` | Dead objects too (otherwise `false` up front) |
| `MM_RETURNFALSE` | `0x8000000` | Result `false` |
| `MM_ANDNEXTMATCH` / `MM_ORNEXTMATCH` / `MM_ANDNOTNEXTMATCH` / `MM_ORNOTNEXTMATCH` | `0x10000000` / `0x20000000` / `0x40000000` / `0x80000000` | Chaining with `MM_NEXT` |

The lists `MM_HL_*` are **ORed** with one another and together form one check.

`MM_MODE2`:

| Name | Bit | Check |
|---|---|---|
| `MM2_EXTRAENERGYLOST` | `0x1` | Extra energy (attribute 15) present and not full |
| `MM2_CALLMETHOD` | `0x2` | Special check `MM_METHODINDEX` 0...5 (0/1: message `0x329`; 2: `0x1C17`; 3: distance to the reference object <= sight range/3; 4: `0x603` >= 0; 5: not in a camouflaged carrier) |
| `MM2_DONOTIGNOREQUARTEREDUNITS` | `0x4` | Do not discard quartered objects up front |
| `MM2_NOHITPOINTS` | `0x10` | (read, effect not traced) |
| `MM2_SHOOTABLE` | `0x20` | Always true |
| `MM2_NOT_TOO_MANY_OPPONENTS` | `0x40` | Message `0x3100` != 1 |
| `MM2_RETURNTRUE` | `0x80` | Result `true` |
| `MM2_CAN_FIGHT_FAR` | `0x100` | Message `0x1311` (Attack: can fight far) |
| `MM2_IS_IN_WEAPON_RANGE` | `0x200` | Distance to the reference object < weapon range (attribute 21 * 1024) |
| `MM2_IS_ATTACKING` | `0x400` | The target is attacking right now (message `0x1511`), namely `attacking who`: me / a friend (in sight range) / my formation |
| `MM2_MORETHAN70` | `0x800` | Hit points > 70 % |
| `MM2_POSITIONALLOWED` | `0x1000` | The target may also be a ground position (UI) |

`MM_STATUS` (status word of the object): `MOVE` `0x1`, `FIGHTING` `0x2`, `CONSTRUCT` `0x20`, `MINING` `0x40`,
`LUMBERING` `0x80`, `QUARTERED` `0x100`, `HARVESTING` `0x400`, `ISMOUNTED` `0x2000`, `CARRIING` `0x4000`,
`ISMOUNTEDONBOAT` `0x8000`, `SWIMMING` `0x10000`, `DIVING` `0x20000`, `DIED` `0x40000`, `HEAL` `0x80000`, `MASKED`
`0x100000`, `EVOKING` `0x200000`, `WAITING4KILLING` `0x400000`, `ONWALL` `0x2000000`; collective values `NOTWAITING`
`0xF8EC0FFF`, `NOTWAITINGORLUMBERING` `0xFFEC0F7F`, `NOTWAITINGORMINING` `0xFFEC0FBF`, `NOTWAITINGORHARVESTING`
`0xFFEC0BFF`.

## Evaluation

```
match(obj, f, player):
  if obj.status & DIED and not f.mode & RETURN_DIED      -> false
  if obj.status & QUARTERED and not f.mode2 & 4          -> false
  player < 0 -> player = owner of obj
  RETURNFALSE -> b = false ; RETURNTRUE -> b = true ; otherwise:
  for every active check in the order of the tables above:
      b = check (NOT variants negated)
      without ANDALL: b -> stop with true      (OR)
      with    ANDALL: !b -> stop with false    (AND)
  no list check active -> neutral element (ANDALL: true, otherwise false)
  chaining: ANDNEXT: b ? match(next) : false     ORNEXT: b ? true : match(next)
            ANDNOTNEXT: b ? !match(next) : false ORNOTNEXT: b ? true : !match(next)
```

For `targets.ini` the reference player is that of the acting object; `MM_CHECKFOG` refers to the local player.

## Shipped data and oddities

- `targets.ini`: 77 entries (among others `BUTTONMSG 411` attack in four variants, `410` building attacks, `427` attack
  target/rally point, `457` pistoleer "snipern", `398`-`402` resources).
- `priorities.ini`: 12 entries (ships, cannons, ranged fighters, "greift mich an", warriors, assassins, non-fighters,
  quartered ranged fighters, finished buildings, all buildings, neutral animals).
- Typos: `MM_MDOE` (skipped), `MM_MODE_MM_CHECKNOTSTATUS` (still takes effect, prefix comparison + `mid(8)`).
- `flowcontroltragets.ini`: a comment warns to use chained filters only after agreement.
