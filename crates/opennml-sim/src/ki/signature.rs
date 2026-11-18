//! Stack signatures of the world builtins used in the corpus (re/ki-vm 12.3 P1): what a builtin without a VM-internal
//! implementation takes from the stack, whether it pushes a result, which arguments are addresses of
//! output variables, whether it sleeps afterwards and which request it makes to the game.
//!
//! Analysis result from the "Stack" and "Route/request" columns of re/ki-builtins A 2, B 3 and C 3; the
//! output parameters and pauses were additionally checked against the handler decompilations (all handlers
//! with `FUN_004137f0` or `FUN_004b4180`). Part A lists the arguments in source order, B and C in pop order
//! (`p1` = topmost = last argument); here the source order `a1 ... aN` is always used. The signature describes
//! the normal case: that some handlers push nothing for invalid arguments (re/ki-builtins A 3 no. 8, B-8,
//! C 5 A7) is known only to the real world.

/// Stack behavior of a world builtin.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Signature {
    /// Number of arguments `a1 … aN` (`a1` pushed first).
    pub pops: u8,
    /// `true` if the builtin pushes a result.
    pub push: bool,
    /// Positions in `a1 … aN` (0-based) that are addresses of output variables.
    pub outs: &'static [u8],
    /// `m_iPauseCounter` after the call: 500 for the searches and `0x12C`, 5000 for `0x2AA`,
    /// otherwise 0.
    pub pause_ms: i32,
    /// Request code to `ScriptQuery_Dispatch` according to the catalog, 0 without a request.
    pub code: u32,
}

/// Query with a result.
const fn ask(pops: u8, code: u32) -> Signature {
    Signature {
        pops,
        push: true,
        outs: &[],
        pause_ms: 0,
        code,
    }
}

/// Command without a result.
const fn cmd(pops: u8, code: u32) -> Signature {
    Signature {
        pops,
        push: false,
        outs: &[],
        pause_ms: 0,
        code,
    }
}

/// Object search with a result and a 500 ms sleep afterwards.
const fn find(pops: u8, code: u32) -> Signature {
    Signature {
        pops,
        push: true,
        outs: &[],
        pause_ms: 500,
        code,
    }
}

/// Query with a result and output variables.
const fn out(pops: u8, outs: &'static [u8], code: u32) -> Signature {
    Signature {
        pops,
        push: true,
        outs,
        pause_ms: 0,
        code,
    }
}

/// Request `20000`: nation of a player (`FUN_0049e5d0`).
const PEOPLE: u32 = 20000;

/// Signature of the world builtin `id`; `None` for VM-internal, unused and unassigned IDs.
pub fn signature(id: u16) -> Option<Signature> {
    Some(match id {
        // A 2: main house of the own nation and next main house (`0x4E21`).
        0x45 | 0x46 => ask(0, 0x4E21),
        0x47 => ask(1, 0x4E21),
        // A 2: next object of a type for owner `p`, `(p, type, x, z, f)` (`0x4E23`).
        0x51 => ask(5, 0x4E23),
        // A 2: searches with marking, then pause 500 (re/ki-builtins A 3 no. 13); `(x, z, f1, f2)` or
        // `(type, x, z, f)` own, with a leading `p` for an owner.
        0x55 | 0x56 => find(4, 0x4E24),
        0x58..=0x5A => find(5, 0x4E24),
        0x5B | 0x5C => find(4, 0x4E25),
        0x5E..=0x60 => find(5, 0x4E25),
        0x65 | 0x66 => find(5, 0x4E26),
        // A 2: object matches filter, `(obj, f)`.
        0x6E..=0x70 => ask(2, 0x4E28),
        0x72 => ask(2, 0x4E29),
        // A 2: object exists, attribute `0x0E`, `(obj)`.
        0x74..=0x76 => ask(1, 0x4E2A),
        0x78..=0x7A => ask(1, 0x4E2B),
        // A 2: world position, `(obj, &x, &z) → found` (handler `FUN_0049f3c0`).
        0x7B..=0x7D => out(3, &[1, 2], 0x4E2C),
        // A 2: match and player queries.
        0x7F => ask(0, 0x4E2D),
        0x80 => ask(0, 0x4E2E),
        0x83 => ask(0, PEOPLE),
        0x84 => ask(1, PEOPLE),
        0x85 => ask(1, 0x4E32),
        0x86 => ask(0, 0x4EF7),
        // A 2: next reachable cell, `(x, z, &x', &z')` without a result (`FUN_0049e1b0`).
        0x87 => Signature {
            pops: 4,
            push: false,
            outs: &[2, 3],
            pause_ms: 0,
            code: 0x4E34,
        },
        // A 2: the same for an object, `(obj, x, z, &x', &z') → 0/1` (`FUN_004a0400`).
        0x88 | 0xB2 => out(5, &[3, 4], 0x4E35),
        // A 2: map width, height and `GameEngine+0x5F8` (meaning open; `setschwierigkeitsgrad`
        // reads it as a level).
        0x8A => ask(0, 0x4E37),
        0x8B => ask(0, 0x4E38),
        0x8C => ask(0, 0x4E39),
        // A 2: resource, stock, relation, object type with module 3.
        0x92 => ask(1, 0x4E3E),
        0x94 => ask(1, 0x4E3F),
        0x96 => ask(2, 0x4E40),
        0x9A => ask(1, 0x4F6F),
        // A 2: set / clear marking, remove object, `(obj)` without a result.
        0x9C | 0x9D => cmd(1, 0x4E44),
        0x9E..=0xA0 => cmd(1, 0x4E45),
        0xA1 | 0xA3 => cmd(1, 0x4E46),
        // A 2: type mapping per nation via the own nation (request 20000).
        0xA4 => ask(1, PEOPLE),
        // A 2: counts for the own player or for `p`.
        0xA5 => ask(1, 0x4E47),
        0xA6 => ask(2, 0x4E47),
        0xA8 => ask(1, 0x4E48),
        0xA9 => ask(0, 0x4E49),
        0xAA => ask(1, 0x4E49),
        0xAD => ask(4, 0x4E4B),
        0xAE => ask(5, 0x4E4B),
        0xAF => ask(1, 0x4EFD),
        0xB1 => ask(4, 0x4F56),
        // A 2: build/repair `(u, b)`.
        0xB4 => cmd(2, 0x4E4C),
        // A 2: count in a rectangle, own or for `p`.
        0xB7 => ask(5, 0x4F8D),
        0xB8 => ask(6, 0x4F8D),
        // A 2: attributes `0x1A`, `0x18`.
        0xBF => ask(1, 0x50D7),
        0xC0 => ask(1, 0x50DC),
        // A 2: mount `(u, horse) → 0`.
        0xC8 => ask(2, 0x4E4E),
        // A 2: headhunter `(kind, &a, &b, &price) → x` (`FUN_0049e780`).
        0xCD => out(4, &[1, 2, 3], 0x4E51),
        // A 2: hire headhunter `(p, guid, kind)`.
        0xCE => cmd(3, 0x4E50),
        // A 2: player still owns an object, living space, resource 4.
        0xCF => ask(1, 0x510E),
        0xD2 => ask(0, 0x4E53),
        0xD3 => ask(0, 0x4E54),
        0xD5 => ask(0, 0x4E55),
        // A 2: fog in a square for `me`, `(x, z, r)`.
        0xD8 => ask(3, 0x500A),
        0xDA => ask(3, 0x4FF6),
        // B 3: value of another AI `(player, key)`, without a request (`KIManager vf[6]`).
        0xE6 => ask(2, 0),
        // B 3: single movement `(unit, x, y)`; groups `(obj, start, n, x, y, formation)`.
        0xF5 | 0xF9 => cmd(3, 0x4E5C),
        0xF6 | 0xFA => cmd(3, 0x5019),
        0xF7 | 0xFB => cmd(6, 0x4EFE),
        0xF8 => cmd(6, 0x501E),
        0xFD | 0x101 => cmd(3, 0x4F4C),
        0xFE | 0x102 => cmd(3, 0x5023),
        0xFF | 0x103 => cmd(6, 0x4F51),
        0x100 => cmd(6, 0x5028),
        // B 3: cancel the AI reservation `(obj, start, n)`.
        0x106 => cmd(3, 0x4FA6),
        // B 3: mounted GUID, inline without a request.
        0x108 => ask(1, 0),
        // B 3: list checks `(obj, start, n, x, y, R)` or `(obj, start, n, filter)`.
        0x112 | 0x117 => ask(6, 0x4E2C),
        0x113 | 0x11E | 0x115 | 0x121 => ask(4, 0x4E28),
        // B 3: scout GUID of the own nation.
        0x116 => ask(0, PEOPLE),
        // B 3: last attacker exists.
        0x122 => ask(1, 0x50BE),
        // B 3: requests without effect `(a, b)` and `(a)` (re/ki-builtins B-10).
        0x12A => cmd(2, 0x50EB),
        0x12B => cmd(1, 0x4F47),
        // B 3: build a building `(worker, guid, x, y, &outX, &outY, border)`, then sleep 500
        // (`FUN_004a04b0`).
        0x12C => Signature {
            pops: 7,
            push: true,
            outs: &[4, 5],
            pause_ms: 500,
            code: 0x4E84,
        },
        // B 3: production `(building, guid) → 100`, rally point `(building, x, y)`.
        0x12D => ask(2, 0x4E85),
        0x12E => cmd(3, 0x4E86),
        // B 3: object and master data queries `(id)` or `(guid)`.
        0x131 => ask(1, 0x4E89),
        0x132 => ask(1, 0x4E8A),
        0x133..=0x135 => ask(0, PEOPLE),
        0x136 => ask(2, 0x4F33),
        0x137 => ask(1, 0x4F38),
        0x138 => ask(1, 0x4F42),
        0x139 => ask(1, 0x4F6A),
        0x13A => ask(1, 0x50D2),
        // B 3: occupancy border `(building, value)`.
        0x13B => cmd(2, 0x5104),
        // C 3: queries `(id)` or `()`.
        0x141 => ask(1, 0x4F94),
        0x143 => ask(1, 0x5113),
        0x144 => ask(0, 0x5118),
        0x147 => ask(1, 0x511D),
        // C 3: attack mode of an object `(id, a, b)`.
        0x158 => cmd(3, 0x4FE7),
        0x15C => ask(1, 0x5122),
        // C 3: train forward / backward `(train)`.
        0x167 => cmd(1, 0x512C),
        0x168 => cmd(1, 0x512D),
        0x16A | 0x16B => ask(1, 0x4FAB),
        0x16D => ask(1, 0x502D),
        // C 3: take a point from the player list `(x, y, &a, &b) → 0/1` (`FUN_004a45f0`).
        0x16F => out(4, &[2, 3], 0x512E),
        0x170 | 0x171 => ask(1, 0x4FB5),
        // C 3: quarter, board boat `(id, container)`.
        0x172 => cmd(2, 0x4F7E),
        0x173 => cmd(2, 0x4F83),
        0x174 => cmd(2, 0x4F88),
        0x175 | 0x179 | 0x17A => ask(1, 0x4FC9),
        0x178 => cmd(1, 0x4FD8),
        // C 3: credit resource `(slot, resource, amount)`.
        0x18B => cmd(3, 0x5005),
        0x18E | 0x18F | 0x197 => ask(1, 0x4F2E),
        // C 3: unit commands.
        0x191 => cmd(1, 0x4EE9),
        0x192 => ask(2, 0x4EEA),
        // C 3: chop down tree `(id, x, z, &kx, &kz) → 0/1` (`FUN_0049fd80`).
        0x193 => out(5, &[3, 4], 0x4EEB),
        0x194 => ask(2, 0x4EEC),
        0x195 => ask(2, 0x4EED),
        0x198 | 0x199 => cmd(2, 0x4F3D),
        // C 3: behavior for list units `(obj, start, n, mode)`; `obj` is read.
        0x19B => cmd(4, 0x4F5B),
        0x19D | 0x19E => ask(1, 0x4F79),
        0x19F | 0x1AB => ask(1, 0x4FA1),
        0x1A0 => ask(1, 0x4F9C),
        // C 3: path length `(id, sx, sy, tx, ty)`.
        0x1A1 => ask(5, 0x4FB0),
        0x1A2 => cmd(1, 0x4FBA),
        0x1A3 | 0x1AA => ask(1, 0x4FF1),
        0x1A4..=0x1A6 => cmd(2, 0x4F60),
        0x1AE => ask(1, 0x4F97),
        0x1B8 => cmd(1, 0x4FFB),
        // C 3: object searches with a 500 ms sleep (re/ki-builtins C 5 A12); 4 or 5 arguments without radius, 6 with
        // radius, each with a leading `p` for a foreign player.
        0x1E5 | 0x1E6 => find(4, 0x4F24),
        0x1E8..=0x1EA => find(5, 0x4F24),
        0x1EB | 0x1EC => find(4, 0x4F25),
        0x1EE..=0x1F0 => find(5, 0x4F25),
        0x1F4..=0x1F6 => find(5, 0x4F26),
        0x1FB => find(5, 0x4F28),
        0x20E => find(6, 0x4E26),
        0x219 | 0x21A => find(6, 0x4F24),
        0x21F..=0x221 => find(6, 0x4F25),
        0x225 | 0x226 => find(6, 0x4F26),
        0x22C => find(6, 0x4F28),
        0x230 => ask(1, 0x5032),
        // C 3: counter-upgrade `(building, guid)`.
        0x231 => cmd(2, 0x5037),
        // C 3: searches with three filters `(x, y, fA, fB, fC)`, without sleep.
        0x235 => ask(5, 0x50C3),
        0x236 => ask(5, 0x50C8),
        0x237 => ask(5, 0x50CD),
        0x23A => cmd(1, 0x512F),
        0x23B => ask(0, 0x5130),
        // C 3: abilities and spells `(id)` or `(id, target)`; dynamite `0x2AA` sleeps 5000.
        0x258 => cmd(1, 0x503C),
        0x259 => cmd(1, 0x5041),
        0x26C => cmd(1, 0x5046),
        0x26D => cmd(1, 0x504B),
        0x26E => cmd(1, 0x5050),
        0x26F => cmd(1, 0x5055),
        0x280 => cmd(2, 0x505A),
        0x281 => cmd(2, 0x505F),
        0x282 => cmd(1, 0x50F0),
        0x283 => cmd(2, 0x50F5),
        0x284 => cmd(1, 0x50FA),
        0x294 => cmd(1, 0x5064),
        0x295 => cmd(2, 0x5069),
        0x296 => cmd(1, 0x506E),
        0x297 => cmd(1, 0x5073),
        0x2A8 => cmd(1, 0x5078),
        0x2A9 => cmd(1, 0x507D),
        0x2AA => Signature {
            pops: 2,
            push: false,
            outs: &[],
            pause_ms: 5000,
            code: 0x5082,
        },
        0x2AB => cmd(2, 0x5087),
        0x2AC => cmd(1, 0x5109),
        0x2BC => cmd(1, 0x508C),
        0x2BD => cmd(1, 0x5091),
        0x2BE => cmd(1, 0x5096),
        0x2EE => cmd(1, 0x509B),
        0x2EF => cmd(2, 0x50A0),
        0x2F0 => cmd(1, 0x50A5),
        // C 3: scout `(id, kx, kz)`, heal/camouflage/dig in `(id, on)`, kill `(id, target)`.
        0x2F1 => cmd(3, 0x50AA),
        0x2F2 => cmd(2, 0x50AF),
        0x2F3 => cmd(2, 0x50B4),
        0x2F4 => cmd(2, 0x50B9),
        0x2F5 => cmd(2, 0x50E1),
        _ => return None,
    })
}
