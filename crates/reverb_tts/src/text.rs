//! Pure-Rust English text normalization, frame estimation, sentence splitting,
//! and audio silence trimming for Reverb-TTS.

/// Supported languages matching Reverb-TTS training tags.
pub const SUPPORTED_LANGUAGES: &[&str] = &["de", "en", "es", "fr", "it", "pl", "pt"];

pub fn check_language(lang: &str) -> Result<&str, String> {
    let l = lang.trim();
    if SUPPORTED_LANGUAGES.contains(&l) {
        Ok(l)
    } else {
        Err(format!(
            "Unsupported ReverbTTS language {lang:?}; expected one of: {}",
            SUPPORTED_LANGUAGES.join(", ")
        ))
    }
}

/// Format the exact Reverb-TTS prompt string consumed by `tokenizer.json`.
pub fn format_prompt(text: &str, reference_text: &str, language: &str) -> String {
    format!(
        "<|denoise|><|lang_start|>{language}<|lang_end|><|instruct_start|>None<|instruct_end|><|text_start|>{reference_text} {text}<|text_end|>"
    )
}

/// Lightweight pure-Rust English verbalizer for news/event summaries.
/// Expands numbers, decimals, magnitudes (`M5.4`), currencies, percentages,
/// and common units/abbreviations, and strips brackets `()[]{}`.
pub fn prepare_text(text: &str, language: &str) -> String {
    let cleaned: String = text
        .chars()
        .map(|c| match c {
            '(' | ')' | '[' | ']' | '{' | '}' => ' ',
            '\u{2018}' | '\u{2019}' => '\'',
            '\u{201C}' | '\u{201D}' => '"',
            '\u{2013}' | '\u{2014}' => '-',
            _ => c,
        })
        .collect();
    if language != "en" {
        return cleaned.split_whitespace().collect::<Vec<_>>().join(" ");
    }

    let mut out = Vec::new();
    for raw_tok in cleaned.split_whitespace() {
        let lead_punct_len = raw_tok
            .bytes()
            .take_while(|b| matches!(b, b'"' | b'\''))
            .count();
        let lead = &raw_tok[..lead_punct_len];
        let rest = &raw_tok[lead_punct_len..];
        let trail_punct_len = rest
            .bytes()
            .rev()
            .take_while(|b| matches!(b, b'.' | b',' | b'!' | b'?' | b';' | b':' | b'"' | b'\''))
            .count();
        let core = &rest[..rest.len().saturating_sub(trail_punct_len)];
        let trail = &rest[rest.len().saturating_sub(trail_punct_len)..];

        if core.is_empty() {
            out.push(raw_tok.to_owned());
            continue;
        }
        let expanded = expand_english_token(core);
        out.push(format!("{lead}{expanded}{trail}"));
    }
    out.join(" ").split_whitespace().collect::<Vec<_>>().join(" ")
}

fn expand_english_token(tok: &str) -> String {
    // Common abbreviations & units in event feeds
    match tok {
        "km" | "KM" => return "kilometers".into(),
        "km/h" | "kph" | "KPH" => return "kilometers per hour".into(),
        "mph" | "MPH" => return "miles per hour".into(),
        "m/s" => return "meters per second".into(),
        "ft" => return "feet".into(),
        "mi" => return "miles".into(),
        "UTC" | "GMT" => return "U T C".into(),
        "US" | "U.S." | "U.S" => return "U S".into(),
        "UK" | "U.K." => return "U K".into(),
        "UN" | "U.N." => return "U N".into(),
        "EU" => return "E U".into(),
        "USGS" => return "U S G S".into(),
        "NASA" => return "NASA".into(),
        "NOAA" => return "NOAA".into(),
        "NATO" => return "NATO".into(),
        "EMS" => return "E M S".into(),
        "FAA" => return "F A A".into(),
        "FEMA" => return "FEMA".into(),
        "NWS" => return "National Weather Service".into(),
        "&" => return "and".into(),
        _ => {}
    }

    // Earthquake magnitude: M5.4 or M4.8
    if (tok.starts_with('M') || tok.starts_with('m'))
        && tok.len() >= 2
        && tok.as_bytes()[1].is_ascii_digit()
        && let Some(num) = verbalize_number(&tok[1..])
    {
        return format!("magnitude {num}");
    }

    // Percentage: 45% or 12.5%
    if let Some(prefix) = tok.strip_suffix('%')
        && let Some(num) = verbalize_number(prefix)
    {
        return format!("{num} percent");
    }

    // Currency: $100, $2.5M
    if let Some(rest) = tok.strip_prefix('$') {
        if let Some(n) = rest.strip_suffix('M').and_then(verbalize_number) {
            return format!("{n} million dollars");
        }
        if let Some(n) = rest.strip_suffix('B').and_then(verbalize_number) {
            return format!("{n} billion dollars");
        }
        if let Some(n) = verbalize_number(rest) {
            return format!("{n} dollars");
        }
    }

    // Number with unit suffix (e.g. "24km", "150m", "30mph", "10km")
    for (suffix, spoken) in [
        ("km/h", "kilometers per hour"),
        ("mph", "miles per hour"),
        ("km", "kilometers"),
        ("mi", "miles"),
        ("ft", "feet"),
        ("°C", "degrees Celsius"),
        ("°F", "degrees Fahrenheit"),
        ("C", "degrees Celsius"),
        ("F", "degrees Fahrenheit"),
        ("m", "meters"),
    ] {
        if let Some(prefix) = tok.strip_suffix(suffix)
            && !prefix.is_empty()
            && prefix.as_bytes()[0].is_ascii_digit()
            && let Some(num) = verbalize_number(prefix)
        {
            return format!("{num} {spoken}");
        }
    }

    // Plain integer or decimal number (including comma-grouped numbers like 1,200)
    if tok.as_bytes()[0].is_ascii_digit()
        && let Some(num) = verbalize_number(tok)
    {
        return num;
    }

    tok.to_owned()
}

fn verbalize_number(s: &str) -> Option<String> {
    let clean = s.replace(',', "");
    if clean.is_empty() {
        return None;
    }
    if let Some((whole, frac)) = clean.split_once('.') {
        if whole.is_empty()
            || frac.is_empty()
            || !whole.bytes().all(|b| b.is_ascii_digit())
            || !frac.bytes().all(|b| b.is_ascii_digit())
        {
            return None;
        }
        let w = integer_to_words(whole.parse::<u64>().ok()?);
        let digits: Vec<&str> = frac
            .bytes()
            .map(|b| digit_word((b - b'0') as usize))
            .collect();
        return Some(format!("{w} point {}", digits.join(" ")));
    }
    if !clean.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let val = clean.parse::<u64>().ok()?;
    // 4-digit years 1900..=2099 read naturally ("twenty twenty six" / "nineteen ninety nine")
    if (1900..=2099).contains(&val) && !s.contains(',') {
        let hi = val / 100;
        let lo = val % 100;
        if lo == 0 {
            return Some(integer_to_words(val));
        } else if lo < 10 {
            return Some(format!("{} oh {}", integer_to_words(hi), integer_to_words(lo)));
        } else {
            return Some(format!("{} {}", integer_to_words(hi), integer_to_words(lo)));
        }
    }
    Some(integer_to_words(val))
}

fn digit_word(d: usize) -> &'static str {
    const DIGITS: [&str; 10] = [
        "zero", "one", "two", "three", "four", "five", "six", "seven", "eight", "nine",
    ];
    DIGITS[d.min(9)]
}

fn integer_to_words(n: u64) -> String {
    if n == 0 {
        return "zero".into();
    }
    const ONES: [&str; 20] = [
        "", "one", "two", "three", "four", "five", "six", "seven", "eight", "nine", "ten",
        "eleven", "twelve", "thirteen", "fourteen", "fifteen", "sixteen", "seventeen", "eighteen",
        "nineteen",
    ];
    const TENS: [&str; 10] = [
        "", "", "twenty", "thirty", "forty", "fifty", "sixty", "seventy", "eighty", "ninety",
    ];
    fn under_thousand(n: u64) -> String {
        let mut parts = Vec::new();
        let h = n / 100;
        let rem = n % 100;
        if h > 0 {
            parts.push(format!("{} hundred", ONES[h as usize]));
        }
        if rem >= 20 {
            let t = TENS[(rem / 10) as usize];
            let o = ONES[(rem % 10) as usize];
            if o.is_empty() {
                parts.push(t.to_owned());
            } else {
                parts.push(format!("{t} {o}"));
            }
        } else if rem > 0 {
            parts.push(ONES[rem as usize].to_owned());
        }
        parts.join(" ")
    }

    let scales = [
        (1_000_000_000_000u64, "trillion"),
        (1_000_000_000u64, "billion"),
        (1_000_000u64, "million"),
        (1_000u64, "thousand"),
    ];
    let mut rem = n;
    let mut parts = Vec::new();
    for (div, name) in scales {
        if rem >= div {
            let q = rem / div;
            rem %= div;
            parts.push(format!("{} {name}", under_thousand(q)));
        }
    }
    if rem > 0 {
        parts.push(under_thousand(rem));
    }
    parts.join(" ")
}

/// Exact port of `reverb_tts/text.py:estimate_frames`.
pub fn estimate_frames(text: &str, reference_text: &str, reference_frames: usize) -> usize {
    fn char_weight(c: char) -> f64 {
        if c.is_whitespace() {
            0.2
        } else if c.is_numeric() {
            3.5
        } else if c.is_ascii_punctuation() || (!c.is_alphanumeric() && !c.is_whitespace()) {
            0.5
        } else {
            1.0
        }
    }
    let w_text: f64 = text.chars().map(char_weight).sum();
    let w_ref: f64 = reference_text.chars().map(char_weight).sum::<f64>().max(1.0);
    let mut frames = w_text / (w_ref / reference_frames.max(1) as f64);
    if frames < 50.0 {
        frames = 50.0 * (frames / 50.0).cbrt();
    }
    (frames as usize).max(1)
}

/// Exact port of `reverb_tts/text.py:split_text`.
pub fn split_text(
    text: &str,
    reference_text: &str,
    reference_frames: usize,
    longest: usize,
    piece: usize,
) -> Vec<String> {
    if estimate_frames(text, reference_text, reference_frames) <= longest {
        return vec![text.to_owned()];
    }
    let mut sentences = Vec::new();
    let mut start = 0;
    let bytes = text.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if matches!(bytes[i], b'.' | b'!' | b'?') && i + 1 < bytes.len() && bytes[i + 1].is_ascii_whitespace() {
            let end = i + 1;
            let seg = text[start..end].trim();
            if !seg.is_empty() {
                sentences.push(seg.to_owned());
            }
            i = end;
            while i < bytes.len() && bytes[i].is_ascii_whitespace() {
                i += 1;
            }
            start = i;
        } else {
            i += 1;
        }
    }
    let tail = text[start..].trim();
    if !tail.is_empty() {
        sentences.push(tail.to_owned());
    }
    if sentences.is_empty() {
        return vec![text.to_owned()];
    }

    let mut pieces: Vec<String> = Vec::new();
    for sentence in sentences {
        if let Some(last) = pieces.last_mut() {
            let joined = format!("{last} {sentence}");
            if estimate_frames(&joined, reference_text, reference_frames) <= piece {
                *last = joined;
                continue;
            }
        }
        pieces.push(sentence);
    }
    pieces
}

/// Exact port of `reverb_tts/audio.py:trim_silence`.
pub fn trim_silence(audio: &[f32], sample_rate: usize, keep_secs: f32) -> Vec<f32> {
    const SILENCE_DB: f32 = -50.0;
    let window = (sample_rate / 100).max(1);
    let count = audio.len() / window;
    if count == 0 {
        return audio.to_vec();
    }
    let mut loud = Vec::new();
    for w in 0..count {
        let slice = &audio[w * window..(w + 1) * window];
        let mean_sq = slice.iter().map(|&s| s * s).sum::<f32>() / window as f32;
        let db = 20.0 * (mean_sq.sqrt() + 1e-10).log10();
        if db >= SILENCE_DB {
            loud.push(w);
        }
    }
    if loud.is_empty() {
        return audio.to_vec();
    }
    // Split contiguous sound islands separated by >200ms (20 windows) of silence.
    let mut sounds: Vec<(usize, usize, usize)> = Vec::new(); // (first, last, count)
    let mut seg_start = loud[0];
    let mut seg_prev = loud[0];
    let mut seg_len = 1usize;
    for &idx in &loud[1..] {
        if idx - seg_prev > 20 {
            sounds.push((seg_start, seg_prev, seg_len));
            seg_start = idx;
            seg_len = 1;
        } else {
            seg_len += 1;
        }
        seg_prev = idx;
    }
    sounds.push((seg_start, seg_prev, seg_len));

    if sounds.len() > 1 && sounds[0].2 <= 12 {
        sounds.remove(0);
    }
    if sounds.len() > 1 && sounds.last().unwrap().2 <= 12 {
        sounds.pop();
    }
    let margin = (keep_secs * 100.0).round() as usize;
    let start_w = sounds.first().unwrap().0.saturating_sub(margin);
    let end_w = (sounds.last().unwrap().1 + 1 + margin).min(count);
    audio[start_w * window..(end_w * window).min(audio.len())].to_vec()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_event_numbers_units_and_brackets() {
        let out = prepare_text("M5.4 quake 12km near Tokyo (USGS) in 2026.", "en");
        assert_eq!(
            out,
            "magnitude five point four quake twelve kilometers near Tokyo U S G S in twenty twenty six."
        );
    }

    #[test]
    fn frame_estimator_handles_short_and_normal_text() {
        let f_short = estimate_frames("Hi.", "Hi Max. It is Tris.", 100);
        assert!(f_short >= 15 && f_short <= 55, "f_short={f_short}");
        let f_same = estimate_frames("Hi Max. It is Tris.", "Hi Max. It is Tris.", 100);
        assert_eq!(f_same, 100);
    }
}
