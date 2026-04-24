//! Shared parsing logic. Consumed by both the host binary and the WASM skill.

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Mission<'a> {
    pub date: &'a str,
    pub id: &'a str,
    pub destination: &'a str,
    pub status: &'a str,
    pub crew: i64,
    pub duration: i64,
    pub security_code: &'a str,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Best {
    pub code: String,
    pub id: String,
    pub date: String,
    pub duration: i64,
    pub crew: i64,
}

impl Best {
    pub fn encode(&self) -> String {
        format!("{}|{}|{}|{}|{}", self.code, self.id, self.date, self.duration, self.crew)
    }

    pub fn decode(s: &str) -> Option<Best> {
        let mut it = s.split('|');
        let code = it.next()?.to_string();
        let id = it.next()?.to_string();
        let date = it.next()?.to_string();
        let duration = it.next()?.parse::<i64>().ok()?;
        let crew = it.next()?.parse::<i64>().ok()?;
        if it.next().is_some() { return None; }
        Some(Best { code, id, date, duration, crew })
    }
}

fn is_noise(line: &str) -> bool {
    let t = line.trim_start();
    if t.is_empty() { return true; }
    if t.starts_with('#') { return true; }
    // the real file contains these — they have no pipes anyway,
    // but short-circuiting them avoids a pointless split.
    for prefix in ["SYSTEM", "CONFIG", "CHECKSUM", "CHECKPOINT"] {
        if t.starts_with(prefix) { return true; }
    }
    false
}

pub fn parse_line(line: &str) -> Option<Mission<'_>> {
    if is_noise(line) {
        return None;
    }
    let t = line.trim();

    let mut it = t.split('|');
    let date = it.next()?.trim();
    let id = it.next()?.trim();
    let destination = it.next()?.trim();
    let status = it.next()?.trim();
    let crew_s = it.next()?.trim();
    let dur_s = it.next()?.trim();
    let _rate = it.next()?.trim();
    let security_code = it.next()?.trim();
    // reject lines that have trailing fields beyond the expected 8
    if it.next().is_some() {
        return None;
    }

    let duration = dur_s.parse::<i64>().ok()?;
    let crew = crew_s.parse::<i64>().unwrap_or(0);

    if date.is_empty() || id.is_empty() || destination.is_empty() || status.is_empty() || security_code.is_empty() {
        return None;
    }

    Some(Mission { date, id, destination, status, crew, duration, security_code })
}

/// Scan `log` and return the longest match. Returns None if no row matches.
pub fn find_longest(log: &str, destination: &str, status: &str) -> Option<Best> {
    let mut best: Option<(i64, &str, &str, &str, i64)> = None;
    for line in log.lines() {
        let Some(m) = parse_line(line) else { continue };
        if m.destination != destination || m.status != status {
            continue;
        }
        match best {
            Some((d, _, _, _, _)) if m.duration <= d => {}
            _ => {
                best = Some((m.duration, m.security_code, m.id, m.date, m.crew));
            }
        }
    }
    best.map(|(duration, code, id, date, crew)| Best {
        code: code.to_string(),
        id: id.to_string(),
        date: date.to_string(),
        duration,
        crew,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_clean_line() {
        let m = parse_line("2045-07-12 | KLM-1234 | Mars | Completed | 5 | 387 | 98.7 | TRX-842-YHG").unwrap();
        assert_eq!(m.date, "2045-07-12");
        assert_eq!(m.id, "KLM-1234");
        assert_eq!(m.destination, "Mars");
        assert_eq!(m.status, "Completed");
        assert_eq!(m.crew, 5);
        assert_eq!(m.duration, 387);
        assert_eq!(m.security_code, "TRX-842-YHG");
    }

    #[test]
    fn tolerates_messy_whitespace() {
        let m = parse_line("  2045-07-12   |  KLM-1234|  Mars|   Completed  | 5|387  |98.7   |   TRX-842-YHG   ").unwrap();
        assert_eq!(m.destination, "Mars");
        assert_eq!(m.status, "Completed");
        assert_eq!(m.duration, 387);
        assert_eq!(m.security_code, "TRX-842-YHG");
    }

    #[test]
    fn rejects_comments_and_noise() {
        assert!(parse_line("# comment").is_none());
        assert!(parse_line("   # indented comment").is_none());
        assert!(parse_line("SYSTEM: stuff").is_none());
        assert!(parse_line("CONFIG: foo=bar").is_none());
        assert!(parse_line("CHECKSUM: abc").is_none());
        assert!(parse_line("CHECKPOINT: Record batch 213").is_none());
        assert!(parse_line("").is_none());
        assert!(parse_line("   ").is_none());
    }

    #[test]
    fn rejects_wrong_field_count() {
        assert!(parse_line("a|b|c").is_none());
        assert!(parse_line("a|b|c|d|e|f|g").is_none());
        assert!(parse_line("a|b|c|d|e|f|g|h|i").is_none());
    }

    #[test]
    fn rejects_unparsable_duration() {
        assert!(parse_line("2045-07-12 | X | Mars | Completed | 5 | NaN | 98.7 | T-1-Y").is_none());
        assert!(parse_line("2045-07-12 | X | Mars | Completed | 5 | 3.14 | 98.7 | T-1-Y").is_none());
    }

    #[test]
    fn accepts_zero_and_large_crew() {
        let m = parse_line("2045 | X | Mars | Completed | 0 | 100 | 98 | T-1-Y").unwrap();
        assert_eq!(m.crew, 0);
        let m = parse_line("2045 | X | Mars | Completed | 999999 | 100 | 98 | T-1-Y").unwrap();
        assert_eq!(m.crew, 999999);
    }

    #[test]
    fn find_longest_finds_max_and_ignores_other_filters() {
        let log = "\
# header
2045 | A | Mars | Completed | 3 | 100 | 98 | AAA-111-AAA
2045 | B | Mars | Completed | 3 | 999 | 98 | BBB-222-BBB
2045 | C | Mars | Failed    | 3 | 2000 | 98 | CCC-333-CCC
2045 | D | Venus| Completed | 3 | 3000 | 98 | DDD-444-DDD
2045 | E | Mars | Completed | 3 | 500  | 98 | EEE-555-EEE
";
        let best = find_longest(log, "Mars", "Completed").unwrap();
        assert_eq!(best.code, "BBB-222-BBB");
        assert_eq!(best.duration, 999);
    }

    #[test]
    fn find_longest_returns_none_when_nothing_matches() {
        let log = "\
# only non-mars
2045 | A | Venus | Completed | 3 | 100 | 98 | AAA-111-AAA
";
        assert!(find_longest(log, "Mars", "Completed").is_none());
    }

    #[test]
    fn find_longest_takes_first_at_max_on_ties() {
        // current policy: first occurrence at max wins (strict >). Document it.
        let log = "\
2045 | A | Mars | Completed | 3 | 100 | 98 | FIRST-111-AAA
2045 | B | Mars | Completed | 3 | 100 | 98 | SECND-222-BBB
";
        let best = find_longest(log, "Mars", "Completed").unwrap();
        assert_eq!(best.code, "FIRST-111-AAA");
    }

    #[test]
    fn encode_decode_roundtrip() {
        let b = Best {
            code: "XRT-421-ZQP".into(),
            id: "WGU-0200".into(),
            date: "2065-06-05".into(),
            duration: 1629,
            crew: 4,
        };
        let s = b.encode();
        let b2 = Best::decode(&s).unwrap();
        assert_eq!(b, b2);
    }
}
