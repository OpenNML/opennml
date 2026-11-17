# `profiles.ini` and `campaign.ini`

The archive format itself is described in [engine-ini.md](engine-ini.md), section "Format". The reimplementation
reads and writes `profiles.ini` with `nml-formats/src/profiles.rs` and reads `campaign.ini` with
`nml-formats/src/campaign.rs`. It never writes the file of the installation; the reimplementation keeps `profiles.ini`
in the folder of the settings file (deviation from the original).

## `profiles.ini`

The same archive format as `Engine.ini` (one line, `<key>value</>`), but **without a root block**. Original of the
installation (no profile created):

```
<m_CurProfileID>-1</><m_NextProfileGUID>1</><m_Profiles><arraysize>0</></><m_CurLevelID>1</><m_CurCampaignID>0</>
```

Write order:

| # | Key | Content |
|---|---|---|
| 1 | `m_CurProfileID` | int, **index** into `m_Profiles`, -1 = none |
| 2 | `m_NextProfileGUID` | int, GUID of the next new profile |
| 3 | `m_Profiles` | `<arraysize>n</>`, per profile `<i%04d>...</>` (block without class identifier) |
| 4 | `m_CurLevelID` | int, current entry of the current campaign (global) |
| 5 | `m_CurCampaignID` | int, current campaign (global) |

Profile block:

| # | Key | Content |
|---|---|---|
| 1 | `m_Name` | `<fixedsize>40</>`, `<i0000>`...`<i0039>`: UTF-16 units as unsigned decimals, the rest 0 |
| 2 | `m_ProfileGUID` | int |
| 3 | `m_LevelList` | `<listsize>n</>`, per level record `<i%04d>` with `m_CampaignID`, `m_LevelID`, `m_TrialCount` (int) and `m_LevelComplete` (`0`/`1`) |
| 4 | `m_VarMap` | `<mapsize>n</>`, per pair `<k%04d>text</><v%04d>int</>` |

Reading as in the original: keys are searched by name, missing ones keep the default (current -1, GUID counter 0,
campaign 0, entry 0, no profiles); unknown keys are skipped. A level record is preset when reading with
`{campaign -1, entry -1, trials 0, not completed}`. `m_LevelComplete` is false only for the text `0`. A name with
`fixedsize != 40` is **not read**, and the profile is then called `Jon Doe` (default of the constructor). The
reimplementation does not reject this: it does not crash the original, and the name is only a display. `m_VarMap` is
read and written back unchanged (who fills it is open).

Writing produces exactly the bytes of the original form (one line, no line ending); a line ending that an editor
appended at the end of the file does not disturb reading.

### Rejection (deviation, O-126)

The original loads a corrupted file without checking and then crashes or deletes savegames of other profiles. The
reimplementation rejects it with `FormatError::Invalid`; the message always begins with "corrupted profile file: ". The
host then starts without profiles and renames the file to `profiles.ini.broken` (an older backup of this name is
replaced, only the last one is kept; a warning in the log names both paths). So the next writing of the profiles
(creating, deleting, choosing) does not overwrite it; if the renaming fails, the warning says so. An unreadable file
(read error) stays where it is (`nml-client/src/settings.rs`, `load_profiles`). Rejected are:

| Case | Consequence in the original |
|---|---|
| Profile block without `m_ProfileGUID` | GUID uninitialized (heap content); the savegame suffix and deleting hit arbitrary files |
| `m_CurProfileID` not in -1 ... count-1 | taken over unchecked; access outside the list |
| duplicate `m_ProfileGUID` | deleting one profile deletes the savegames of the other |
| `m_NextProfileGUID` <= highest GUID | a new profile would get a GUID that is already in use |
| archive unreadable, number unreadable, too few elements | parse error: the original ignores the return value and loads defaults (hypothesis) |

The file without profiles (`arraysize 0`) is valid, also with `m_NextProfileGUID` 0 or 1 and any
`m_CurLevelID`/`m_CurCampaignID`. The reader does not check these two numbers (neither does the original when reading).

## `campaign.ini`

Only `data/gifcon/campaign.ini` is loaded (from the VFS, line ending CR LF). The parser keeps a **working entry**
{type, name, text, `STOP`, path} and the current campaign. It compares the start of the line including the space;
it reads numbers with `atol` (whitespace, sign, digits, the rest is skipped, 0 without digits) and `CAMPAIGN` with
`sscanf("%d")`.

| Line | Effect on the working entry | Append |
|---|---|---|
| `CAMPAIGN n` | current campaign n (no append) | - |
| `HEADLINE id` | type 2, name = text = id, `STOP` 0 | **immediately**, path empty |
| `LEVELNAME id` | type 0, name = text = id, `STOP` 0 | - |
| `VIDEONAME id` | type 1, name = text = id; `STOP` **stays** | - |
| `LEVELTEXT id`, `AUTOSAVENAME id`, `VIDEOTEXT id` | only the text field = id (shared field) | - |
| `STOP` (whole line) | `STOP` 1 | - |
| `LEVELPATH p` | type **0**, path p | yes, then path empty |
| `VIDEOPATH p` | type **1**, path p | yes, then path empty |

Quirks that the reimplementation takes over (O-182 ... O-184):

- The working entry persists between the entries: a second video without its own `STOP` inherits it from the first;
  two path lines in a row produce two entries with the same names.
- A text field that is set after the path line (e.g. `VIDEOTEXT`) no longer reaches the finished entry; it applies
  only to the next appended one.
- The path lines set the type themselves (`LEVELPATH` 0, `VIDEOPATH` 1). For the shipped file this is equivalent to
  keeping the type as set, because name and path lines always match there, but not for arbitrary input.
- The name of a level stays `LEVELNAME`; `AUTOSAVENAME` overwrites only the text field, not the name.

Deviation: the original does not check `CAMPAIGN n` (n outside 0 ... 3 writes behind the entry lists). The
reimplementation discards the entries of such a campaign until a valid number follows again (O-183). Unknown lines
and a `STOP` with a suffix (`STOP `, `STOPP`) are skipped by the parser as in the original.
