//! Recherche sémantique embarquée 100 % locale (#38) — v1 sans modèle ONNX :
//! tokenisation, expansion par synonymes/humeurs (FR/EN), similarité par
//! trigrammes et score BM25-lite pondéré par champ (titre > artiste/album > genre).

use std::collections::BTreeSet;

/// Mots vides FR/EN retirés de la requête avant scoring.
const STOPWORDS: &[&str] = &[
    // FR
    "le", "la", "les", "un", "une", "des", "du", "de", "d", "l", "et", "ou", "the", "a", "an",
    "and", "of", "in", "on", "my", "me", "i", "je", "tu", "il", "elle", "nous", "vous", "son",
    "sa", "ses", "mon", "ma", "mes", "pour", "par", "avec", "sur", "au", "aux", "en", "dans",
    "que", "qui", "quoi", "est", "pas", "plus", "très", "une", "this", "that", "with", "for", "to",
    "is", "it", "de la", "ce", "cette",
];

/// Expansion humeur/mot-clé → termes de genre/style associés (FR + EN).
/// Une requête contenant la clé booste les pistes dont le genre/titre
/// contient un des termes d'expansion.
const MOOD_SYNONYMS: &[(&str, &[&str])] = &[
    (
        "calme",
        &[
            "acoustic",
            "ambient",
            "chill",
            "lounge",
            "douce",
            "slow",
            "piano",
            "instrumental",
        ],
    ),
    (
        "chill",
        &[
            "acoustic",
            "ambient",
            "lounge",
            "chillout",
            "downtempo",
            "trip hop",
        ],
    ),
    ("relax", &["acoustic", "ambient", "lounge", "chill", "jazz"]),
    (
        "détente",
        &["acoustic", "ambient", "lounge", "chill", "bossa"],
    ),
    (
        "énergique",
        &[
            "rock",
            "electro",
            "dance",
            "techno",
            "house",
            "drum",
            "rapide",
            "energetic",
        ],
    ),
    (
        "energetic",
        &[
            "rock",
            "electro",
            "dance",
            "techno",
            "house",
            "drum and bass",
            "upbeat",
        ],
    ),
    ("dynamique", &["rock", "pop", "electro", "dance", "funk"]),
    (
        "triste",
        &[
            "melancholic",
            "sad",
            "blues",
            "melancholy",
            "ballad",
            "piano",
            "acoustic",
        ],
    ),
    (
        "mélancolique",
        &[
            "melancholic",
            "sad",
            "blues",
            "ballad",
            "ambient",
            "post rock",
        ],
    ),
    (
        "sad",
        &[
            "melancholic",
            "blues",
            "ballad",
            "acoustic",
            "piano",
            "indie",
        ],
    ),
    (
        "joyeux",
        &[
            "happy", "pop", "funk", "disco", "ska", "reggae", "sunny", "upbeat",
        ],
    ),
    (
        "happy",
        &["pop", "funk", "disco", "soul", "reggae", "upbeat"],
    ),
    (
        "fête",
        &[
            "dance", "house", "techno", "disco", "party", "electro", "funk",
        ],
    ),
    (
        "party",
        &["dance", "house", "techno", "disco", "electro", "pop"],
    ),
    (
        "amour",
        &[
            "ballad", "love", "soul", "rnb", "chanson", "acoustic", "romantic",
        ],
    ),
    (
        "love",
        &["ballad", "soul", "rnb", "acoustic", "romantic", "pop"],
    ),
    (
        "sport",
        &[
            "rock",
            "electro",
            "hip hop",
            "rap",
            "drum and bass",
            "workout",
            "energetic",
        ],
    ),
    (
        "workout",
        &[
            "electro",
            "hip hop",
            "rock",
            "drum and bass",
            "techno",
            "energetic",
        ],
    ),
    (
        "concentration",
        &[
            "ambient",
            "classical",
            "instrumental",
            "lounge",
            "piano",
            "study",
        ],
    ),
    (
        "étude",
        &[
            "ambient",
            "classical",
            "instrumental",
            "lounge",
            "piano",
            "study",
            "focus",
        ],
    ),
    (
        "focus",
        &[
            "ambient",
            "instrumental",
            "lounge",
            "classical",
            "study",
            "concentration",
        ],
    ),
    (
        "sommeil",
        &[
            "ambient",
            "chillout",
            "downtempo",
            "lounge",
            "meditation",
            "sleep",
        ],
    ),
    (
        "sleep",
        &["ambient", "chillout", "downtempo", "meditation", "lounge"],
    ),
    (
        "pluie",
        &["ambient", "jazz", "lounge", "chillout", "rain", "acoustic"],
    ),
    (
        "rain",
        &["ambient", "jazz", "lounge", "chillout", "acoustic"],
    ),
    (
        "jazz",
        &["jazz", "swing", "bebop", "bossa", "lounge", "soul"],
    ),
    (
        "classique",
        &[
            "classical",
            "baroque",
            "romantic",
            "symphony",
            "opera",
            "concerto",
            "piano",
        ],
    ),
    (
        "classical",
        &[
            "classical",
            "baroque",
            "symphony",
            "opera",
            "concerto",
            "chamber",
        ],
    ),
    (
        "rock",
        &[
            "rock",
            "alternative",
            "indie rock",
            "hard rock",
            "punk",
            "garage",
        ],
    ),
    (
        "électro",
        &[
            "electro",
            "techno",
            "house",
            "trance",
            "edm",
            "synthwave",
            "electronic",
        ],
    ),
    (
        "electro",
        &[
            "electro",
            "techno",
            "house",
            "trance",
            "edm",
            "electronic",
            "synthwave",
        ],
    ),
    ("rap", &["hip hop", "rap", "trap", "conscious", "drill"]),
    ("hiphop", &["hip hop", "rap", "trap", "rnb"]),
    ("soul", &["soul", "funk", "rnb", "motown", "neo soul"]),
    ("blues", &["blues", "delta blues", "chicago blues", "soul"]),
    (
        "reggae",
        &["reggae", "dub", "ska", "rocksteady", "dancehall"],
    ),
    (
        "métal",
        &[
            "metal",
            "heavy metal",
            "thrash",
            "death metal",
            "black metal",
            "doom",
        ],
    ),
    (
        "metal",
        &["metal", "heavy metal", "thrash", "doom", "power metal"],
    ),
    ("pop", &["pop", "synth pop", "indie pop", "kpop", "jpop"]),
    (
        "rapide",
        &[
            "rock",
            "electro",
            "dance",
            "drum and bass",
            "punk",
            "energetic",
        ],
    ),
    (
        "lent",
        &[
            "ambient",
            "ballad",
            "chillout",
            "downtempo",
            "acoustic",
            "slow",
        ],
    ),
    (
        "nostalgie",
        &[
            "retro",
            "synthwave",
            "80s",
            "90s",
            "old school",
            "vintage",
            "chanson",
        ],
    ),
    (
        "nostalgic",
        &["retro", "synthwave", "80s", "90s", "old school", "vintage"],
    ),
    (
        "exotique",
        &["world", "afrobeat", "latin", "bossa", "reggae", "ethnic"],
    ),
    (
        "world",
        &["world", "afrobeat", "latin", "bossa", "ethnic", "folk"],
    ),
    (
        "enfant",
        &["comptine", "children", "kids", "disney", "animation"],
    ),
    (
        "kids",
        &["comptine", "children", "kids", "disney", "animation"],
    ),
    ("noël", &["christmas", "noel", "holiday", "carol", "hymn"]),
    ("christmas", &["christmas", "noel", "holiday", "carol"]),
];

/// Tokenize : minuscules, découpage sur tout caractère non alphanumérique,
/// suppression des stopwords et des tokens de moins de 2 caractères.
pub fn tokenize(s: &str) -> Vec<String> {
    s.split(|c: char| !c.is_alphanumeric())
        .filter(|t| !t.is_empty())
        .map(|t| t.to_lowercase())
        .filter(|t| t.len() >= 2 && !STOPWORDS.contains(&t.as_str()))
        .collect()
}

/// Termes de la requête avec poids : tokens originaux (1.0) + expansions
/// synonymes (0.7). Dédupliqués.
pub fn query_terms(q: &str) -> Vec<(String, f64)> {
    let tokens = tokenize(q);
    let mut out: Vec<(String, f64)> = tokens.iter().map(|t| (t.clone(), 1.0)).collect();
    for tok in &tokens {
        for (key, expansions) in MOOD_SYNONYMS {
            if tok == key
                || (key.len() >= 4 && tok.contains(key))
                || (tok.len() >= 4 && key.contains(tok))
            {
                for e in *expansions {
                    out.push((e.to_string(), 0.7));
                }
            }
        }
    }
    // dédup en gardant le meilleur poids
    let mut best: std::collections::HashMap<String, f64> = std::collections::HashMap::new();
    for (t, w) in out {
        let e = best.entry(t).or_insert(0.0);
        if w > *e {
            *e = w;
        }
    }
    let mut v: Vec<(String, f64)> = best.into_iter().collect();
    v.sort_by(|a, b| {
        b.1.partial_cmp(&a.1)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(a.0.cmp(&b.0))
    });
    v
}

/// Ensemble de trigrammes d'une chaîne (bordures d'espace ajoutées).
pub fn trigrams(s: &str) -> BTreeSet<String> {
    let t = format!(" {s} ");
    let chars: Vec<char> = t.chars().collect();
    if chars.len() < 3 {
        return BTreeSet::from([t.trim().to_string()]);
    }
    (0..chars.len() - 2)
        .map(|i| chars[i..i + 3].iter().collect())
        .collect()
}

/// Distance de Damerau-Levenshtein (transpositions incluses) entre deux chaînes.
fn damerau_distance(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let (n, m) = (a.len(), b.len());
    if n == 0 {
        return m;
    }
    if m == 0 {
        return n;
    }
    let mut d = vec![vec![0usize; m + 1]; n + 1];
    for (i, row) in d.iter_mut().enumerate().take(n + 1) {
        row[0] = i;
    }
    for (j, cell) in d[0].iter_mut().enumerate().take(m + 1) {
        *cell = j;
    }
    for i in 1..=n {
        for j in 1..=m {
            let cost = usize::from(a[i - 1] != b[j - 1]);
            let mut v = (d[i - 1][j] + 1)
                .min(d[i][j - 1] + 1)
                .min(d[i - 1][j - 1] + cost);
            if i > 1 && j > 1 && a[i - 1] == b[j - 2] && a[i - 2] == b[j - 1] {
                v = v.min(d[i - 2][j - 2] + 1);
            }
            d[i][j] = v;
        }
    }
    d[n][m]
}

/// Similarité entre deux tokens dans [0, 1] : 1 - distance normalisée de
/// Damerau-Levenshtein (tolère fautes de frappe et suppressions de lettres).
pub fn token_sim(a: &str, b: &str) -> f64 {
    if a == b {
        return 1.0;
    }
    let a = a.to_lowercase();
    let b = b.to_lowercase();
    let max = a.chars().count().max(b.chars().count()).max(1);
    1.0 - damerau_distance(&a, &b) as f64 / max as f64
}

/// Correspondance floue entre un token de requête et un token de champ :
/// égalité, inclusion (longueur >= 4 pour éviter les faux positifs), ou
/// similarité trigramme >= 0.6 (tolère les fautes d'orthographe).
pub fn token_matches(query_tok: &str, field_tok: &str) -> bool {
    if query_tok == field_tok {
        return true;
    }
    if query_tok.len() >= 4 && field_tok.contains(query_tok) {
        return true;
    }
    if field_tok.len() >= 4 && query_tok.contains(field_tok) {
        return true;
    }
    // Flou uniquement sur les tokens d'au moins 5 lettres : en dessous, la
    // distance normalisée génère des faux positifs (ex. "lounge" ~ "long").
    if query_tok.len() >= 5 && field_tok.len() >= 5 {
        return token_sim(query_tok, field_tok) >= 0.55;
    }
    false
}

/// Score d'un champ texte pour une requête : somme des poids des tokens de
/// requête qui trouvent un match dans le champ, avec bonus préfixe (x1.25)
/// et bonus sous-chaîne exacte de la requête entière (x2).
pub fn score_field(terms: &[(String, f64)], field: &str, weight: f64) -> f64 {
    if terms.is_empty() || field.trim().is_empty() {
        return 0.0;
    }
    let f = field.to_lowercase();
    let f_tokens = tokenize(&f);
    let mut s = 0.0;
    for (qt, qw) in terms {
        let mut best = 0.0f64;
        for ft in &f_tokens {
            if token_matches(qt, ft) {
                let mut m = *qw;
                if ft.starts_with(qt) {
                    m *= 1.25;
                }
                if m > best {
                    best = m;
                }
            }
        }
        // le champ contient la requête complète telle quelle
        let full = terms
            .iter()
            .filter(|(_, w)| *w >= 1.0)
            .map(|(t, _)| t.as_str())
            .collect::<Vec<_>>()
            .join(" ");
        if !full.is_empty() && f.contains(&full) {
            best = best.max(2.0);
        }
        s += best;
    }
    s * weight
}

/// Score global d'un item multi-champs : `fields` = (texte, poids du champ).
pub fn score_fields(terms: &[(String, f64)], fields: &[(&str, f64)]) -> f64 {
    fields
        .iter()
        .map(|(txt, w)| score_field(terms, txt, *w))
        .sum()
}

/// Poids des champs d'une piste pour le ranking (#38).
pub fn track_fields<'a>(
    title: &'a str,
    artist: &'a str,
    album_artist: &'a str,
    album: &'a str,
    genre: Option<&'a str>,
) -> Vec<(&'a str, f64)> {
    let mut v = vec![
        (title, 3.0),
        (artist, 2.0),
        (album_artist, 2.0),
        (album, 2.0),
    ];
    if let Some(g) = genre {
        v.push((g, 1.5));
    }
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokenize_drops_stopwords_and_short() {
        assert_eq!(
            tokenize("Le Chat de Schrödinger"),
            vec!["chat", "schrödinger"]
        );
        assert_eq!(tokenize("la la la"), Vec::<String>::new());
        assert!(tokenize("a b c").is_empty());
    }

    #[test]
    fn query_terms_expands_moods() {
        let terms = query_terms("musique calme");
        let keys: Vec<&str> = terms.iter().map(|(t, _)| t.as_str()).collect();
        assert!(keys.contains(&"calme"));
        assert!(keys.contains(&"ambient"));
        assert!(keys.contains(&"acoustic"));
        // poids original > poids expansion
        let w_calme = terms.iter().find(|(t, _)| t == "calme").unwrap().1;
        let w_ambient = terms.iter().find(|(t, _)| t == "ambient").unwrap().1;
        assert!(w_calme > w_ambient);
    }

    #[test]
    fn token_sim_handles_typos() {
        assert!(token_sim("musique", "musik") >= 0.55);
        assert!(token_sim("beetles", "beatles") >= 0.55);
        assert!(token_sim("jazz", "rock") < 0.55);
        assert_eq!(token_sim("abc", "abc"), 1.0);
    }

    #[test]
    fn token_matches_fuzzy() {
        assert!(token_matches("beatles", "beatles"));
        assert!(token_matches("beat", "beatles"));
        assert!(token_matches("beetles", "beatles")); // faute de frappe
        assert!(token_matches("musique", "musik")); // lettres supprimées
        assert!(!token_matches("xylo", "piano"));
        assert!(!token_matches("lounge", "long")); // faux positif court
        assert!(!token_matches("calme", "long"));
    }

    #[test]
    fn score_field_ranks_prefix_and_exact() {
        let terms = query_terms("radio");
        let exact = score_field(&terms, "Radio Nova", 3.0);
        let partial = score_field(&terms, "Radioactive", 3.0);
        let none = score_field(&terms, "Classique FM", 3.0);
        assert!(exact > 0.0 && partial > 0.0 && none == 0.0);
    }

    #[test]
    fn score_fields_weights_title_over_genre() {
        let terms = query_terms("jazz");
        let in_title = score_fields(&terms, &[("Jazz Me Up", 3.0), ("", 1.5)]);
        let in_genre = score_fields(&terms, &[("Autre titre", 3.0), ("Jazz", 1.5)]);
        assert!(in_title > in_genre);
    }

    #[test]
    fn mood_query_matches_genre_via_expansion() {
        // "calme" doit matcher une piste de genre "ambient" même sans le mot "calme"
        let terms = query_terms("calme");
        let ambient = score_fields(&terms, &[("Nuit Blanche", 3.0), ("Ambient", 1.5)]);
        let rock = score_fields(&terms, &[("Nuit Blanche", 3.0), ("Hard Rock", 1.5)]);
        assert!(ambient > 0.0);
        assert!(rock == 0.0);
    }
}
