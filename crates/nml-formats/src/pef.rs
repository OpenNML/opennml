//! Effect file `.pef` (particle, billboard and model effects), see `docs/formats/pef.md`.
//! Read like `EffectLibrary_LoadPef` (`0x0057CD50`): line by line, case-sensitive substring search in
//! the check order of the original, values like
//! `sscanf("%s %f ...")` (the first word is the key). `FILE`, `RENDERPARTICLESFIRST` and
//! unknown lines are ignored.

/// Sample points of a curve (`*_MIN_MAX`).
pub const CURVE_POINTS: usize = 100;

/// A `.pef`: components in file order (= drawing order).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PefFile {
    /// `MOVEMENTTIME`: seconds per 128 units for particles with a target (default 0).
    pub movement_time: f32,
    pub components: Vec<PefComponent>,
}

impl PefFile {
    /// Number of components per kind (particle, billboard, model).
    pub fn counts(&self) -> [usize; 3] {
        let mut counts = [0; 3];
        for component in &self.components {
            counts[match component {
                PefComponent::Particles(_) => 0,
                PefComponent::Billboard(_) => 1,
                PefComponent::Object(_) => 2,
            }] += 1;
        }
        counts
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum PefComponent {
    Particles(ParticleDesc),
    Billboard(BillboardDesc),
    Object(ObjectEffectDesc),
}

/// `PARTICLEEFFECT` (description, `1000` bytes). Missing keys are undefined in the original
/// (memory without defaults); here 0 applies and for `EMTTYPE`/`EMTRADIUS` the
/// editor defaults 1/0 (`0x00597748`, `0x00597752`).
#[derive(Debug, Clone, PartialEq)]
pub struct ParticleDesc {
    pub name: String,
    /// `BLENDALPHA` != 0: alpha blending (`eff_billboard_alpha`), otherwise additive.
    pub blend_alpha: bool,
    /// `TEXTURE`, lowercase (path as in the file).
    pub texture: String,
    pub looping: bool,
    pub start: f32,
    pub duration: f32,
    pub burst: bool,
    pub burst_delay: f32,
    pub pos: [f32; 3],
    pub emit: i32,
    pub spread: f32,
    pub gravity: [f32; 3],
    pub yaw: f32,
    pub yaw_var: f32,
    pub pitch: f32,
    pub pitch_var: f32,
    pub speed: f32,
    /// `ACCEL`: time factor of the movement.
    pub accel: f32,
    pub attract: f32,
    pub attract_axes: [i32; 3],
    /// `COLORi`: age (s), R, G, B.
    pub colors: [[f32; 4]; 4],
    /// `ALPHAi`: age (s), alpha.
    pub alphas: [[f32; 2]; 4],
    /// `SIZEi`: age (s), half edge length.
    pub sizes: [[f32; 2]; 4],
    pub spin: f32,
    pub life: f32,
    pub rot_y: f32,
    pub emitter_type: i32,
    pub emitter_radius: f32,
    pub move_with_object: bool,
}

impl Default for ParticleDesc {
    fn default() -> Self {
        Self {
            name: String::new(),
            blend_alpha: false,
            texture: String::new(),
            looping: false,
            start: 0.0,
            duration: 0.0,
            burst: false,
            burst_delay: 0.0,
            pos: [0.0; 3],
            emit: 0,
            spread: 0.0,
            gravity: [0.0; 3],
            yaw: 0.0,
            yaw_var: 0.0,
            pitch: 0.0,
            pitch_var: 0.0,
            speed: 0.0,
            accel: 0.0,
            attract: 0.0,
            attract_axes: [0; 3],
            colors: [[0.0; 4]; 4],
            alphas: [[0.0; 2]; 4],
            sizes: [[0.0; 2]; 4],
            spin: 0.0,
            life: 0.0,
            rot_y: 0.0,
            emitter_type: 1,
            emitter_radius: 0.0,
            move_with_object: false,
        }
    }
}

/// `BILLBOARDEFFECT`: origin and layers.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct BillboardDesc {
    pub name: String,
    pub position: [f32; 3],
    pub layers: Vec<BillboardLayerDesc>,
}

/// Curve with [`CURVE_POINTS`] values, already converted by `Curve_MapRawToRange` (`0x00597920`):
/// `a + raw * (b - a) / 64`.
#[derive(Debug, Clone, PartialEq)]
pub struct Curve {
    pub values: [f32; CURVE_POINTS],
}

impl Default for Curve {
    fn default() -> Self {
        Self {
            values: [0.0; CURVE_POINTS],
        }
    }
}

/// `BILLBOARDLAYER` (layer, `0xC80` bytes).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct BillboardLayerDesc {
    pub name: String,
    /// Textures of the image sequence (`NUMTEX` lines after `NUMTEX`, index = line); `None` if the
    /// line contains no `TEXTURE`. Paths lowercase.
    pub textures: Vec<Option<String>>,
    /// `DIRECTIONAL` != 0: lying quad; tilt angles about x and z (rad).
    pub directional: bool,
    pub tilt_x: f32,
    pub tilt_z: f32,
    pub looping: bool,
    pub blend_alpha: bool,
    pub offset: [f32; 3],
    pub start: f32,
    pub duration: f32,
    pub pause: f32,
    pub color: [f32; 3],
    pub rot_off: bool,
    /// `SIZE` x/y; `None` if the key is missing (then pixel size of the texture 0, without a loadable
    /// texture 64 x 64, `BillboardLayerDesc_SetTextureCount` `0x00598CC0`).
    pub size: Option<[i32; 2]>,
    pub x_scale: Curve,
    pub y_scale: Curve,
    pub x_move: Curve,
    pub y_move: Curve,
    /// In radians (the file has degrees, `* 0.017453292` as at `0x0059B33D`).
    pub rotation: Curve,
    pub fading: Curve,
    pub move_ud: Curve,
}

/// `OBJECTEFFECT` (description, `0x254` bytes); missing keys with the editor defaults
/// (`ObjectEffectDesc_Construct` `0x0059BAD0`).
#[derive(Debug, Clone, PartialEq)]
pub struct ObjectEffectDesc {
    pub name: String,
    /// `OBJECT`, lowercase.
    pub object: String,
    pub anim_guid: i32,
    pub position: [f32; 3],
    /// Read, not used when drawing.
    pub rotation: [f32; 3],
    pub culling: bool,
    /// `D3DBLEND` values.
    pub src_blend: i32,
    pub dest_blend: i32,
    pub fade_in: bool,
    pub fade_out: bool,
    pub loop_fading: bool,
    /// Milliseconds.
    pub fade_in_ms: i32,
    pub fade_out_ms: i32,
    pub duration_ms: i32,
    /// Playback speed, 128 = normal.
    pub movement: i32,
}

impl Default for ObjectEffectDesc {
    fn default() -> Self {
        Self {
            name: String::new(),
            object: String::new(),
            anim_guid: -1,
            position: [0.0; 3],
            rotation: [0.0; 3],
            culling: true,
            src_blend: 5,
            dest_blend: 6,
            fade_in: false,
            fade_out: false,
            loop_fading: false,
            fade_in_ms: 0,
            fade_out_ms: 0,
            duration_ms: 0,
            movement: 128,
        }
    }
}

/// Longest numeric prefix like `strtod` or `sscanf("%f")` of the MSVC runtime: `-1.#IND00` yields
/// `-1`, `1.#INF00` yields `1`. `None` if no digit follows.
pub fn c_float(text: &str) -> Option<f32> {
    let text = text.trim_start();
    let bytes = text.as_bytes();
    let mut end = 0;
    if matches!(bytes.first(), Some(b'+' | b'-')) {
        end = 1;
    }
    let digits = |from: usize| {
        bytes[from..]
            .iter()
            .take_while(|b| b.is_ascii_digit())
            .count()
    };
    let int = digits(end);
    end += int;
    let mut frac = 0;
    if bytes.get(end) == Some(&b'.') {
        frac = digits(end + 1);
        end += 1 + frac;
    }
    if int + frac == 0 {
        return None;
    }
    if matches!(bytes.get(end), Some(b'e' | b'E')) {
        let mut exp = end + 1;
        if matches!(bytes.get(exp), Some(b'+' | b'-')) {
            exp += 1;
        }
        let n = digits(exp);
        if n > 0 {
            end = exp + n;
        }
    }
    text[..end].parse().ok()
}

/// Integer like `sscanf("%d")`: optional sign and digits; `None` without a digit.
pub fn c_int(text: &str) -> Option<i32> {
    let text = text.trim_start();
    let bytes = text.as_bytes();
    let sign = usize::from(matches!(bytes.first(), Some(b'+' | b'-')));
    let n = bytes[sign..]
        .iter()
        .take_while(|b| b.is_ascii_digit())
        .count();
    if n == 0 {
        return None;
    }
    let value = text[sign..sign + n]
        .bytes()
        .fold(0i64, |acc, d| {
            acc.saturating_mul(10).saturating_add(i64::from(d - b'0'))
        })
        .min(i64::from(i32::MAX) + 1);
    let value = if bytes[0] == b'-' { -value } else { value };
    Some(value.clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32)
}

/// Fields after the keyword (`sscanf("%s ...")`).
fn fields(line: &str) -> Vec<&str> {
    line.split_whitespace().skip(1).collect()
}

/// `%f` values after the key into `out` (only the fields read, like `sscanf`, which stops at an
/// unreadable field).
fn read_floats(line: &str, out: &mut [f32]) {
    for (slot, field) in out.iter_mut().zip(fields(line)) {
        match c_float(field) {
            Some(v) => *slot = v,
            None => return,
        }
    }
}

fn read_float(line: &str, out: &mut f32) {
    read_floats(line, std::slice::from_mut(out));
}

fn read_ints(line: &str, out: &mut [i32]) {
    for (slot, field) in out.iter_mut().zip(fields(line)) {
        match c_int(field) {
            Some(v) => *slot = v,
            None => return,
        }
    }
}

fn read_int(line: &str, out: &mut i32) {
    read_ints(line, std::slice::from_mut(out));
}

fn read_bool(line: &str, out: &mut bool) {
    let mut v = i32::from(*out);
    read_int(line, &mut v);
    *out = v != 0;
}

/// Text between the first and the last `"` (empty without two `"`).
fn quoted(line: &str) -> String {
    match (line.find('"'), line.rfind('"')) {
        (Some(a), Some(b)) if b > a => line[a + 1..b].to_string(),
        _ => String::new(),
    }
}

/// Lines like `GameFile_ReadLine` (`0x00574C00`): only `\r` is removed.
struct Lines<'a> {
    lines: std::str::Split<'a, char>,
}

impl<'a> Lines<'a> {
    fn next(&mut self) -> Option<&'a str> {
        self.lines.next().map(|l| l.trim_end_matches('\r'))
    }
}

/// Reads a `.pef`. Without a line with `EFFECT` the effect has no components (the loader creates
/// the instance beforehand).
pub fn parse_pef(text: &str) -> PefFile {
    let mut pef = PefFile::default();
    let mut lines = Lines {
        lines: text.split('\n'),
    };
    while let Some(line) = lines.next() {
        if line.contains("EFFECT") {
            parse_effect(&mut lines, &mut pef);
        }
    }
    pef
}

fn parse_effect(lines: &mut Lines, pef: &mut PefFile) {
    while let Some(line) = lines.next() {
        if line.contains('}') {
            return;
        } else if line.contains("MOVEMENTTIME") {
            read_float(line, &mut pef.movement_time);
        } else if line.contains("PARTICLEEFFECT") {
            pef.components
                .push(PefComponent::Particles(parse_particles(lines)));
        } else if line.contains("BILLBOARDEFFECT") {
            pef.components
                .push(PefComponent::Billboard(parse_billboard(lines)));
        } else if line.contains("OBJECTEFFECT") {
            pef.components
                .push(PefComponent::Object(parse_object(lines)));
        }
    }
}

/// `ParticleEmitter_ParsePef` (`0x00596680`).
fn parse_particles(lines: &mut Lines) -> ParticleDesc {
    let mut d = ParticleDesc::default();
    while let Some(line) = lines.next() {
        if line.contains('}') {
            break;
        } else if line.contains("BLENDALPHA") {
            read_bool(line, &mut d.blend_alpha);
        } else if line.contains("NAME") {
            d.name = quoted(line);
        } else if line.contains("TEXTURE") {
            d.texture = quoted(line).to_lowercase();
        } else if line.contains("LOOP") {
            read_bool(line, &mut d.looping);
        } else if line.contains("START") {
            read_float(line, &mut d.start);
        } else if line.contains("DURATION") {
            read_float(line, &mut d.duration);
        } else if line.contains("BURST") {
            read_bool(line, &mut d.burst);
        } else if line.contains("BDELAY") {
            read_float(line, &mut d.burst_delay);
        } else if line.contains("POS") {
            read_floats(line, &mut d.pos);
        } else if line.contains("EMIT") {
            read_int(line, &mut d.emit);
        } else if line.contains("SPREAD") {
            read_float(line, &mut d.spread);
        } else if line.contains("GRAVITY") {
            read_floats(line, &mut d.gravity);
        } else if line.contains("YAW") {
            read_float(line, &mut d.yaw);
        } else if line.contains("YVAR") {
            read_float(line, &mut d.yaw_var);
        } else if line.contains("PITCH") {
            read_float(line, &mut d.pitch);
        } else if line.contains("PVAR") {
            read_float(line, &mut d.pitch_var);
        } else if line.contains("SPEED") {
            read_float(line, &mut d.speed);
        } else if line.contains("ACCEL") {
            read_float(line, &mut d.accel);
        } else if line.contains("ATTRACT") {
            read_float(line, &mut d.attract);
        } else if line.contains("AAXIS") {
            read_ints(line, &mut d.attract_axes);
        } else if let Some(i) = indexed(line, "COLOR") {
            read_floats(line, &mut d.colors[i]);
        } else if let Some(i) = indexed(line, "ALPHA") {
            read_floats(line, &mut d.alphas[i]);
        } else if let Some(i) = indexed(line, "SIZE") {
            read_floats(line, &mut d.sizes[i]);
        } else if line.contains("SPIN") {
            read_float(line, &mut d.spin);
        } else if line.contains("LIFE") {
            read_float(line, &mut d.life);
        } else if line.contains("ROTY") {
            read_float(line, &mut d.rot_y);
        } else if line.contains("EMTTYPE") {
            read_int(line, &mut d.emitter_type);
        } else if line.contains("EMTRADIUS") {
            read_float(line, &mut d.emitter_radius);
        } else if line.contains("MOVEPARTICLESWITHOBJECT") {
            read_bool(line, &mut d.move_with_object);
        }
    }
    d
}

/// `KEY0` ... `KEY3` (checked in this order).
fn indexed(line: &str, key: &str) -> Option<usize> {
    (0..4).find(|i| line.contains(&format!("{key}{i}")))
}

/// `BillboardEffect_ParsePef` (`0x0059A830`), part `BILLBOARDEFFECT`.
fn parse_billboard(lines: &mut Lines) -> BillboardDesc {
    let mut d = BillboardDesc::default();
    while let Some(line) = lines.next() {
        if line.contains('}') {
            break;
        } else if line.contains("BILLBOARDLAYER") {
            d.layers.push(parse_layer(lines));
        } else if line.contains("NAME") {
            d.name = quoted(line);
        } else if line.contains("POSITION") {
            read_floats(line, &mut d.position);
        }
    }
    d
}

/// Curve line after `NAME_MIN_MAX a b` (`BillboardLayer_ParseCurve` `0x005999E0`): 100 times
/// `%f`, each after the next space; unreadable values stay 0.
fn parse_curve(key_line: &str, lines: &mut Lines) -> Curve {
    let mut range = [0.0f32; 2];
    read_floats(key_line, &mut range);
    let [a, b] = range;
    let raw_line = lines.next().unwrap_or("");
    let mut raws = raw_line.split(' ').filter(|s| !s.trim().is_empty());
    let mut curve = Curve::default();
    for value in &mut curve.values {
        let raw = raws.next().and_then(c_float).unwrap_or(0.0);
        *value = a + raw * (b - a) / 64.0;
    }
    curve
}

/// `BillboardEffect_ParsePef`, part `BILLBOARDLAYER`.
fn parse_layer(lines: &mut Lines) -> BillboardLayerDesc {
    let mut d = BillboardLayerDesc::default();
    while let Some(line) = lines.next() {
        if line.contains('}') {
            break;
        } else if line.contains("NAME") {
            d.name = quoted(line);
        } else if line.contains("NUMTEX") {
            let mut count = 0;
            read_int(line, &mut count);
            // Reading stops at the end of the input: a nonsensical `NUMTEX` (corrupted file) thus
            // allocates no memory for `count` entries in advance and does not loop on nothing.
            d.textures.clear();
            for _ in 0..count.max(0) {
                let Some(l) = lines.next() else { break };
                d.textures
                    .push(l.contains("TEXTURE").then(|| quoted(l).to_lowercase()));
            }
        } else if line.contains("DIRECTIONAL") {
            let fields = fields(line);
            d.directional = fields.first().and_then(|f| c_int(f)).unwrap_or(0) != 0;
            let mut tilt = [0.0f32; 2];
            for (slot, field) in tilt.iter_mut().zip(fields.iter().skip(1)) {
                *slot = c_float(field).unwrap_or(0.0);
            }
            [d.tilt_x, d.tilt_z] = tilt;
        } else if line.contains("LOOP") {
            read_bool(line, &mut d.looping);
        } else if line.contains("BLENDALPHA") {
            read_bool(line, &mut d.blend_alpha);
        } else if line.contains("OFFSET") {
            read_floats(line, &mut d.offset);
        } else if line.contains("START") {
            read_float(line, &mut d.start);
        }
        // The original additionally checks the remaining keys on every line.
        if line.contains("DURATION") {
            read_float(line, &mut d.duration);
        }
        if line.contains("PAUSE") {
            read_float(line, &mut d.pause);
        }
        if line.contains("COLOR") {
            read_floats(line, &mut d.color);
        }
        if line.contains("ROTOFF") {
            read_bool(line, &mut d.rot_off);
        }
        if line.contains("SIZE") {
            let mut size = d.size.unwrap_or([64, 64]);
            read_ints(line, &mut size);
            d.size = Some(size);
        }
        if line.contains("XSCALE_MIN_MAX") {
            d.x_scale = parse_curve(line, lines);
        }
        if line.contains("YSCALE_MIN_MAX") {
            d.y_scale = parse_curve(line, lines);
        }
        if line.contains("XMOVE_MIN_MAX") {
            d.x_move = parse_curve(line, lines);
        }
        if line.contains("YMOVE_MIN_MAX") {
            d.y_move = parse_curve(line, lines);
        }
        if line.contains("ROTATION_MIN_MAX") {
            d.rotation = parse_curve(line, lines);
            for v in &mut d.rotation.values {
                *v *= 0.017_453_292;
            }
        }
        if line.contains("FADING_MIN_MAX") {
            d.fading = parse_curve(line, lines);
        }
        if line.contains("MOVEUD_MIN_MAX") {
            d.move_ud = parse_curve(line, lines);
        }
    }
    d
}

/// `ObjectEffect_ParsePef` (`0x0059BFA0`).
fn parse_object(lines: &mut Lines) -> ObjectEffectDesc {
    let mut d = ObjectEffectDesc::default();
    while let Some(line) = lines.next() {
        if line.contains('}') {
            break;
        } else if line.contains("NAME") {
            d.name = quoted(line);
        } else if line.contains("OBJECT") {
            d.object = quoted(line).to_lowercase();
        } else if line.contains("ANIMGUID") {
            read_int(line, &mut d.anim_guid);
        } else if line.contains("POSITION") {
            read_floats(line, &mut d.position);
        } else if line.contains("ROTATION") {
            read_floats(line, &mut d.rotation);
        } else if line.contains("CULLING") {
            read_bool(line, &mut d.culling);
        } else if line.contains("SRCBLEND") {
            read_int(line, &mut d.src_blend);
        } else if line.contains("DESTBLEND") {
            read_int(line, &mut d.dest_blend);
        } else if line.contains("FADE_IN") {
            read_bool(line, &mut d.fade_in);
        } else if line.contains("FADE_OUT") {
            read_bool(line, &mut d.fade_out);
        } else if line.contains("LOOP_FADING") {
            read_bool(line, &mut d.loop_fading);
        } else if line.contains("FADEIN_DUR") {
            read_int(line, &mut d.fade_in_ms);
        } else if line.contains("FADEOUT_DUR") {
            read_int(line, &mut d.fade_out_ms);
        } else if line.contains("DURATION") {
            read_int(line, &mut d.duration_ms);
        } else if line.contains("MOVEMENT") {
            read_int(line, &mut d.movement);
        }
    }
    d
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "EFFECT\r\n{\r\n\tFILE \"data\\effects\\x.pef\"\r\n\tMOVEMENTTIME 1.000000\r\n\r\n\tPARTICLEEFFECT\r\n\t{\r\n\t\tBLENDALPHA 1\r\n\t\tNAME \"fire\"\r\n\t\tTEXTURE \"Data\\Effects\\Neu\\Rauch01\\rauch01.TGA\"\r\n\t\tLOOP 1\r\n\t\tSTART 0.5\r\n\t\tDURATION 1.000000\r\n\t\tBURST 1\r\n\t\tBDELAY 0.25\r\n\t\tPOS 1 2 3\r\n\t\tEMIT 4\r\n\t\tSPREAD 4.0\r\n\t\tGRAVITY 0 40 0\r\n\t\tYAW 0.1\r\n\t\tYVAR 0.4\r\n\t\tPITCH 1.5\r\n\t\tPVAR 0.5\r\n\t\tSPEED 12\r\n\t\tACCEL 2\r\n\t\tATTRACT -3\r\n\t\tAAXIS 1 0 1\r\n\t\tCOLOR0 0 254 216 126\r\n\t\tCOLOR1 1 255 159 53\r\n\t\tCOLOR2 2 242 79 0\r\n\t\tCOLOR3 3 255 128 64\r\n\t\tALPHA0 0 0\r\n\t\tALPHA1 1 255\r\n\t\tALPHA2 2 255\r\n\t\tALPHA3 3 0\r\n\t\tSIZE0 0 35\r\n\t\tSIZE1 1 9\r\n\t\tSIZE2 2 9\r\n\t\tSIZE3 3 0\r\n\t\tSPIN -3\r\n\t\tLIFE 3\r\n\t\tROTY 0.5\r\n\t\tEMTTYPE 2\r\n\t\tEMTRADIUS 7\r\n\t\tMOVEPARTICLESWITHOBJECT 1\r\n\t}\r\n";

    #[test]
    fn particle_block_reads_every_key() {
        let text = format!("{SAMPLE}}}\r\n");
        let pef = parse_pef(&text);
        assert_eq!(pef.movement_time, 1.0);
        assert_eq!(pef.counts(), [1, 0, 0]);
        let PefComponent::Particles(p) = &pef.components[0] else {
            panic!("particles expected");
        };
        assert!(p.blend_alpha && p.looping && p.burst && p.move_with_object);
        assert_eq!(p.name, "fire");
        assert_eq!(p.texture, "data\\effects\\neu\\rauch01\\rauch01.tga");
        assert_eq!((p.start, p.duration, p.burst_delay), (0.5, 1.0, 0.25));
        assert_eq!((p.pos, p.emit, p.spread), ([1.0, 2.0, 3.0], 4, 4.0));
        assert_eq!(p.gravity, [0.0, 40.0, 0.0]);
        assert_eq!(
            (p.yaw, p.yaw_var, p.pitch, p.pitch_var),
            (0.1, 0.4, 1.5, 0.5)
        );
        assert_eq!((p.speed, p.accel, p.attract), (12.0, 2.0, -3.0));
        assert_eq!(p.attract_axes, [1, 0, 1]);
        assert_eq!(p.colors[2], [2.0, 242.0, 79.0, 0.0]);
        assert_eq!(p.alphas[1], [1.0, 255.0]);
        assert_eq!(p.sizes[0], [0.0, 35.0]);
        assert_eq!((p.spin, p.life, p.rot_y), (-3.0, 3.0, 0.5));
        assert_eq!((p.emitter_type, p.emitter_radius), (2, 7.0));
    }

    #[test]
    fn missing_particle_keys_take_zero_and_the_editor_emitter_defaults() {
        let pef = parse_pef("EFFECT\n{\nPARTICLEEFFECT\n{\nEMIT 3\n}\n}\n");
        let PefComponent::Particles(p) = &pef.components[0] else {
            panic!("particles expected");
        };
        assert_eq!((p.emit, p.life, p.speed), (3, 0.0, 0.0));
        assert_eq!((p.emitter_type, p.emitter_radius), (1, 0.0));
        assert!(!p.move_with_object);
        // Without an `EFFECT` line: no components.
        assert_eq!(parse_pef("PARTICLEEFFECT\n{\nEMIT 3\n}\n").components, []);
    }

    /// Curve line with 100 raw values: `first` at index 0, otherwise 32.
    fn curve_line(first: &str) -> String {
        let mut values = vec!["32"; CURVE_POINTS];
        values[0] = first;
        values.join(" ") + " "
    }

    #[test]
    fn billboard_layers_curves_and_textures() {
        let curve = curve_line("64");
        let text = format!(
            "EFFECT\r\n{{\r\n\tBILLBOARDEFFECT\r\n\t{{\r\n\t\tNAME \"bb\"\r\n\t\tPOSITION 1 2 3\r\n\r\n\t\tBILLBOARDLAYER\r\n\t\t{{\r\n\t\t\tNAME \"layer 1\"\r\n\t\t\tNUMTEX 3\r\n\t\t\tTEXTURE0 \"A.tga\"\r\n\t\t\tempty\r\n\t\t\tTEXTURE7 \"c.tga\"\r\n\t\t\tDIRECTIONAL 1 0.5 -0.25\r\n\t\t\tLOOP 1\r\n\t\t\tBLENDALPHA 1\r\n\t\t\tOFFSET 4 5 6\r\n\t\t\tSTART 0.5\r\n\t\t\tDURATION 2\r\n\t\t\tPAUSE 0.25\r\n\t\t\tCOLOR 255 159 64\r\n\t\t\tROTOFF 1\r\n\t\t\tSIZE 32 48\r\n\t\t\tXSCALE_MIN_MAX 0 1.75\r\n{curve}\r\n\t\t\tROTATION_MIN_MAX -360 360\r\n{curve}\r\n\t\t\tFADING_MIN_MAX 7 7\r\n{nan}\r\n\t\t}}\r\n\t\tBILLBOARDLAYER\r\n\t\t{{\r\n\t\t\tNUMTEX 1\r\n\t\t\tTEXTURE0 \"d.tga\"\r\n\t\t}}\r\n\t}}\r\n}}\r\n",
            nan = curve_line("-1.#IND00")
        );
        let pef = parse_pef(&text);
        assert_eq!(pef.counts(), [0, 1, 0]);
        let PefComponent::Billboard(b) = &pef.components[0] else {
            panic!("billboard expected");
        };
        assert_eq!((b.name.as_str(), b.position), ("bb", [1.0, 2.0, 3.0]));
        assert_eq!(b.layers.len(), 2);
        let l = &b.layers[0];
        assert_eq!(l.name, "layer 1");
        assert_eq!(
            l.textures,
            [Some("a.tga".to_string()), None, Some("c.tga".to_string())]
        );
        assert!(l.directional && l.looping && l.blend_alpha && l.rot_off);
        assert_eq!((l.tilt_x, l.tilt_z), (0.5, -0.25));
        assert_eq!(l.offset, [4.0, 5.0, 6.0]);
        assert_eq!((l.start, l.duration, l.pause), (0.5, 2.0, 0.25));
        assert_eq!(l.color, [255.0, 159.0, 64.0]);
        assert_eq!(l.size, Some([32, 48]));
        // a + raw * (b - a) / 64: 64 -> b, 32 -> middle.
        assert_eq!(l.x_scale.values[0], 1.75);
        assert_eq!(l.x_scale.values[1], 0.875);
        // Degrees -> radians.
        assert!((l.rotation.values[0] - std::f32::consts::TAU).abs() < 1e-5);
        assert_eq!(l.rotation.values[1], 0.0);
        // `-1.#IND00` with a == b: value a.
        assert_eq!(l.fading.values[0], 7.0);
        // Missing curves stay 0, a missing SIZE stays open.
        assert_eq!(l.y_move.values, [0.0; CURVE_POINTS]);
        assert_eq!(b.layers[1].size, None);
        assert!(!b.layers[1].directional);
    }

    #[test]
    fn an_absurd_numtex_stops_at_the_end_of_the_input() {
        let text = "EFFECT\r\n{\r\n\tBILLBOARDEFFECT\r\n\t{\r\n\t\tBILLBOARDLAYER\r\n\t\t{\r\n\t\t\tNUMTEX 2000000000\r\n\t\t\tTEXTURE0 \"a.tga\"\r\n\t\t}\r\n\t}\r\n}\r\n";
        let pef = parse_pef(text);
        let PefComponent::Billboard(b) = &pef.components[0] else {
            panic!("billboard expected");
        };
        // All remaining lines (three braces and the empty one after the last line ending) have become
        // texture lines, there are no more.
        assert_eq!(
            b.layers[0].textures,
            [Some("a.tga".to_string()), None, None, None, None]
        );
        // Negative count: no textures.
        let pef = parse_pef(&text.replace("2000000000", "-5"));
        let PefComponent::Billboard(b) = &pef.components[0] else {
            panic!("billboard expected");
        };
        assert!(b.layers[0].textures.is_empty());
    }

    #[test]
    fn object_effect_keys_and_defaults() {
        let text = "EFFECT\n{\n\tOBJECTEFFECT\n\t{\n\t\tNAME \"object-effect\"\n\t\tOBJECT \"data\\R3D\\strahl01.rdo\"\n\t\tANIMGUID 0\n\t\tCULLING 0\n\t\tSRCBLEND 6\n\t\tDESTBLEND 7\n\t\tFADE_OUT 1\n\t\tLOOP_FADING 1\n\t\tFADEOUT_DUR 1000\n\t\tDURATION 500\n\t\tMOVEMENT 64\n\t}\n}\n";
        let pef = parse_pef(text);
        let PefComponent::Object(o) = &pef.components[0] else {
            panic!("model effect expected");
        };
        assert_eq!(o.object, "data\\r3d\\strahl01.rdo");
        assert_eq!((o.anim_guid, o.src_blend, o.dest_blend), (0, 6, 7));
        assert!(!o.culling && o.fade_out && o.loop_fading && !o.fade_in);
        assert_eq!((o.fade_out_ms, o.duration_ms, o.movement), (1000, 500, 64));
        let defaults = ObjectEffectDesc::default();
        assert_eq!((defaults.src_blend, defaults.dest_blend), (5, 6));
        assert!(defaults.culling);
    }

    #[test]
    fn numbers_are_read_like_the_msvc_runtime() {
        assert_eq!(c_float("-1.#IND00"), Some(-1.0));
        assert_eq!(c_float("1.#INF00"), Some(1.0));
        assert_eq!(c_float("  2.5e2x"), Some(250.0));
        assert_eq!(c_float(".5"), Some(0.5));
        assert_eq!(c_float("e5"), None);
        assert_eq!(c_float("-"), None);
        assert_eq!(c_int("-12abc"), Some(-12));
        assert_eq!(c_int("x"), None);
        assert_eq!(c_int("99999999999"), Some(i32::MAX));
        // Very long digit sequences saturate instead of overflowing.
        assert_eq!(c_int("9999999999999999999999999"), Some(i32::MAX));
        assert_eq!(c_int("-9999999999999999999999999"), Some(i32::MIN));
    }
}
