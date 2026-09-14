//! How a person types a colour, and what `color.colorRGB` needs: `0xRRGGBB`.
//! Nothing here knows about Google's wire layout (`traits.rs` does).

/// Named colours a person would type; anything else is `#rrggbb`,
/// `rgb(r,g,b)` or `hsv(h,s[,v])`.
const NAMED_COLORS: &[(&str, u32)] = &[
    ("red", 0xff0000),
    ("green", 0x00ff00),
    ("blue", 0x0000ff),
    ("white", 0xffffff),
    ("warm white", 0xffd9a0),
    ("cool white", 0xd6ecff),
    ("yellow", 0xffff00),
    ("orange", 0xff8000),
    ("purple", 0x8000ff),
    ("violet", 0x8f00ff),
    ("pink", 0xff69b4),
    ("magenta", 0xff00ff),
    ("cyan", 0x00ffff),
    ("teal", 0x008080),
    ("lime", 0x80ff00),
    ("gold", 0xffd700),
    ("amber", 0xffbf00),
    ("lavender", 0xb57edc),
    ("turquoise", 0x40e0d0),
    ("sky blue", 0x87ceeb),
];

/// Parse a colour as `0xRRGGBB`: a name from the table, `#rrggbb` /
/// `rrggbb`, `rgb(r,g,b)` (0–255 each) or `hsv(h,s[,v])` (hue 0–360,
/// saturation and value 0–100, value defaulting to 100).
pub fn parse_color(s: &str) -> Result<u32, String> {
    let t = s.trim().to_lowercase();
    if let Some((_, c)) = NAMED_COLORS.iter().find(|(n, _)| *n == t) {
        return Ok(*c);
    }
    let hex = t.strip_prefix('#').unwrap_or(&t);
    if hex.len() == 6 && hex.chars().all(|c| c.is_ascii_hexdigit()) {
        return u32::from_str_radix(hex, 16).map_err(|e| e.to_string());
    }
    let inner = |prefix: &str| -> Option<Vec<f64>> {
        let body = t
            .strip_prefix(prefix)?
            .strip_prefix('(')?
            .strip_suffix(')')?;
        body.split(',')
            .map(|x| x.trim().parse::<f64>().ok())
            .collect::<Option<Vec<_>>>()
    };
    if let Some(v) = inner("rgb") {
        if v.len() == 3 && v.iter().all(|x| (0.0..=255.0).contains(x)) {
            return Ok(((v[0] as u32) << 16) | ((v[1] as u32) << 8) | v[2] as u32);
        }
        return Err("rgb() takes three values 0–255".into());
    }
    if let Some(v) = inner("hsv") {
        if (v.len() == 2 || v.len() == 3)
            && (0.0..=360.0).contains(&v[0])
            && (0.0..=100.0).contains(&v[1])
            && v.get(2).is_none_or(|x| (0.0..=100.0).contains(x))
        {
            let val = v.get(2).copied().unwrap_or(100.0) / 100.0;
            return Ok(hsv_to_rgb(v[0] % 360.0, v[1] / 100.0, val));
        }
        return Err("hsv() takes hue 0–360, saturation 0–100 and an optional value 0–100".into());
    }
    let names: Vec<&str> = NAMED_COLORS.iter().map(|(n, _)| *n).collect();
    Err(format!(
        "unknown colour `{s}`; use a name ({}), #rrggbb, rgb(r,g,b) or hsv(h,s)",
        names.join(", ")
    ))
}

fn hsv_to_rgb(h: f64, s: f64, v: f64) -> u32 {
    let c = v * s;
    let x = c * (1.0 - ((h / 60.0) % 2.0 - 1.0).abs());
    let m = v - c;
    let (r, g, b) = match h {
        h if h < 60.0 => (c, x, 0.0),
        h if h < 120.0 => (x, c, 0.0),
        h if h < 180.0 => (0.0, c, x),
        h if h < 240.0 => (0.0, x, c),
        h if h < 300.0 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    let ch = |f: f64| ((f + m) * 255.0).round().clamp(0.0, 255.0) as u32;
    (ch(r) << 16) | (ch(g) << 8) | ch(b)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colours_parse_from_names_hex_rgb_and_hsv() {
        assert_eq!(parse_color("Teal").unwrap(), 0x008080);
        assert_eq!(parse_color(" warm white ").unwrap(), 0xffd9a0);
        assert_eq!(parse_color("#FF8800").unwrap(), 0xff8800);
        assert_eq!(parse_color("ff8800").unwrap(), 0xff8800);
        assert_eq!(parse_color("rgb(0, 255, 128)").unwrap(), 0x00ff80);
        assert_eq!(parse_color("hsv(0,100)").unwrap(), 0xff0000);
        assert_eq!(parse_color("hsv(120,100,50)").unwrap(), 0x008000);
        assert_eq!(parse_color("hsv(0,0)").unwrap(), 0xffffff);
        assert!(parse_color("rgb(300,0,0)").is_err());
        assert!(parse_color("hsv(400,10)").is_err());
        assert!(parse_color("#12345").is_err());
        assert!(parse_color("mauve-ish").unwrap_err().contains("teal"));
    }
}
