//! Paroles synchronisees (format LRC) : parsing pur, sans I/O.

/// Vrai si le texte ressemble a du LRC (`[mm:ss.xx] ...` en tete de ligne).
pub fn looks_like_lrc(s: &str) -> bool {
    s.lines().any(|l| {
        let r = l.trim_start();
        r.starts_with('[') && r.find(']').and_then(|i| parse_ts(&r[1..i])).is_some()
    })
}

/// Parse le contenu d'une balise temps `mm:ss.xx` (ou `mm:ss`, `hh:mm:ss.xx`).
/// Les metadonnees (`ti:...`, `ar:...`) ne sont pas des temps -> None.
fn parse_ts(inner: &str) -> Option<f64> {
    let parts: Vec<&str> = inner.split(':').collect();
    let (h, m, s) = match parts.as_slice() {
        [a, b] => (0.0, a.parse::<f64>().ok()?, b),
        [a, b, c] => (a.parse::<f64>().ok()?, b.parse::<f64>().ok()?, c),
        _ => return None,
    };
    let sec = s.parse::<f64>().ok()?;
    if !(0.0..=59.0).contains(&m) {
        return None;
    }
    Some(h * 3600.0 + m * 60.0 + sec)
}

/// Supprime les balises temps en tete de `rest`, renvoie `(stamps, texte)`.
fn split_line(line: &str) -> (Vec<f64>, &str) {
    let mut rest = line.trim_start();
    let mut stamps = Vec::new();
    while rest.starts_with('[') {
        match rest.find(']') {
            Some(i) if parse_ts(&rest[1..i]).is_some() => {
                stamps.push(parse_ts(&rest[1..i]).unwrap());
                rest = rest[i + 1..].trim_start();
            }
            _ => break,
        }
    }
    (stamps, rest)
}

/// Parse un texte LRC en lignes `(secondes, texte)`, triees par temps croissant.
/// Une ligne peut porter plusieurs balises (`[00:01.00][00:10.00]refrain`).
/// Les balises de metadonnees (`[ti:...]`, `[ar:...]`...) sont ignorees.
pub fn parse_lrc(text: &str) -> Vec<(f64, String)> {
    let mut out = Vec::new();
    for line in text.lines() {
        let (stamps, rest) = split_line(line);
        let text_part = rest.trim();
        if stamps.is_empty() || text_part.is_empty() {
            continue; // metadonnee, texte libre non horodate ou ligne instrumentale
        }
        for ts in stamps {
            out.push((ts, text_part.to_string()));
        }
    }
    out.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
    out
}

/// Texte brut des paroles : LRC depouille de ses balises, sinon le texte tel quel.
pub fn plain_text(raw: &str) -> String {
    if looks_like_lrc(raw) {
        let mut lines: Vec<String> = Vec::new();
        for line in raw.lines() {
            let (_, rest) = split_line(line);
            let t = rest.trim();
            if !t.is_empty() {
                lines.push(t.to_string());
            }
        }
        lines.join("\n")
    } else {
        raw.trim().to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_lrc_simple_et_trie() {
        let lrc = "[00:05.50]World\n[00:01.00]Hello\n";
        let v = parse_lrc(lrc);
        assert_eq!(
            v,
            vec![(1.0, "Hello".to_string()), (5.5, "World".to_string())]
        );
    }

    #[test]
    fn parse_lrc_balises_multiples_et_meta_ignorees() {
        let lrc = "[ti:Night Prowler]\n[ar:AC/DC]\n[00:01.00][00:09.25]Somewhere a clock strikes midnight\n[00:12.00]\n";
        let v = parse_lrc(lrc);
        assert_eq!(
            v,
            vec![
                (1.0, "Somewhere a clock strikes midnight".to_string()),
                (9.25, "Somewhere a clock strikes midnight".to_string()),
            ]
        );
        assert!(looks_like_lrc(lrc));
    }

    #[test]
    fn parse_lrc_heures_et_centiemes() {
        let v = parse_lrc("[01:02:03.75]fin\n[00:00.10]debut\n");
        assert_eq!(v[0], (0.1, "debut".to_string()));
        assert_eq!(v[1], (3723.75, "fin".to_string()));
    }

    #[test]
    fn plain_text_depouille_les_balises() {
        assert_eq!(
            plain_text("[00:01.00]Hello\n[00:05.50]World\n"),
            "Hello\nWorld"
        );
        assert_eq!(plain_text("parole simple\n"), "parole simple");
        assert!(!looks_like_lrc("parole simple"));
    }
}
