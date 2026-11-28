//! Instant Mix (#105) : génère une file d'attente « titres similaires » à
//! partir d'un morceau, d'un album ou d'un artiste, jouable immédiatement.
//!
//! Sources de similarité, par ordre de poids :
//! - artistes similaires MusicBrainz (même tag dominant, via `artistinfo`),
//!   quand MusicBrainz est activé ;
//! - même artiste que la graine ;
//! - même album que la graine ;
//! - genres/tags communs (tags locaux, séparés par « ; »).
//!
//! Le tout depuis la **bibliothèque locale** ; tri déterministe (hash id+graine)
//! pour rester reproductible entre deux appels.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use axum::{extract::State, Json};
use serde::Deserialize;
use serde_json::{json, Value};

use auris_core::{AurisError, Track};

use crate::{artistinfo, lastfm, ApiResult, AppState};

const TTL_SECS: i64 = 600;
const DEFAULT_LIMIT: usize = 50;

#[derive(Debug, Deserialize)]
pub struct MixBody {
    /// « track » | « album » | « artist »
    pub kind: String,
    pub id: String,
    pub limit: Option<usize>,
}

/// Contexte de la graine pour le scoring.
pub struct MixSeed {
    pub name: String,
    pub artist_key: String,
    pub album_key: Option<String>,
    pub genres: Vec<String>,
    pub exclude_track_id: Option<String>,
    /// artistes similaires (clés minuscules), ex. via MusicBrainz
    pub similar_artists: Vec<String>,
}

type SimilarCache = Mutex<HashMap<String, (i64, Vec<String>)>>;

static SIMILAR_CACHE: OnceLock<SimilarCache> = OnceLock::new();

fn similar_cache() -> &'static SimilarCache {
    SIMILAR_CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Noms (clés minuscules) des artistes similaires à `artist_name` via
/// MusicBrainz (recherche artiste -> tag dominant -> artistes tagués).
/// Cache mémoire 10 min ; vide si MusicBrainz est désactivé ou en échec.
pub async fn similar_artist_names(s: &AppState, artist_name: &str) -> Vec<String> {
    if !lastfm::mb_enabled(s) {
        return Vec::new();
    }
    let key = artist_name.trim().to_lowercase();
    let now = chrono::Utc::now().timestamp();
    if let Some((ts, v)) = similar_cache().lock().unwrap().get(&key) {
        if now - *ts < TTL_SECS {
            return v.clone();
        }
    }
    let out = fetch_similar(s, artist_name).await;
    if let Ok(mut m) = similar_cache().lock() {
        m.insert(key, (now, out.clone()));
    }
    out
}

async fn fetch_similar(s: &AppState, artist_name: &str) -> Vec<String> {
    let mb = lastfm::mb_cache(s);
    let search = match mb.client.artist_search(artist_name).await {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!("mix: artist_search {artist_name}: {e}");
            return Vec::new();
        }
    };
    let Some((mbid, _)) = artistinfo::pick_artist(&search, artist_name) else {
        return Vec::new();
    };
    let Ok(fiche) = mb.client.artist(&mbid).await else {
        return Vec::new();
    };
    let sum = artistinfo::summarize_artist(&fiche);
    let Some(tag) = sum
        .get("tags")
        .and_then(|t| t.get(0))
        .and_then(|t| t.get("name"))
        .and_then(|n| n.as_str())
        .map(str::to_string)
    else {
        return Vec::new();
    };
    match mb.client.artists_by_tag(&tag).await {
        Ok(v) => artistinfo::summarize_similar(&v, artist_name)
            .iter()
            .filter_map(|a| a.get("name").and_then(|n| n.as_str()))
            .map(|n| n.trim().to_lowercase())
            .filter(|n| !n.is_empty() && n != &artist_name.trim().to_lowercase())
            .collect(),
        Err(e) => {
            tracing::warn!("mix: artists_by_tag {tag}: {e}");
            Vec::new()
        }
    }
}

/// Score de similarité d'une piste avec la graine (plus c'est haut, mieux c'est).
pub fn score_track(t: &Track, seed: &MixSeed) -> i64 {
    let mut score = 0i64;
    if seed.similar_artists.iter().any(|a| a == &t.artist_key) {
        score += 80;
    }
    if !seed.similar_artists.contains(&seed.artist_key) && t.artist_key == seed.artist_key {
        score += 50;
    }
    if let Some(ak) = &seed.album_key {
        if &t.album_key == ak {
            score += 40;
        }
    }
    let t_genres = split_genres(t.genre.as_deref());
    let overlap = t_genres.iter().filter(|g| seed.genres.contains(g)).count() as i64;
    score += (overlap * 10).min(30);
    score
}

fn split_genres(g: Option<&str>) -> Vec<String> {
    g.unwrap_or("")
        .split(';')
        .map(|s| s.trim().to_lowercase())
        .filter(|s| !s.is_empty())
        .collect()
}

/// Hash déterministe (FNV) pour départager à score égal.
fn hash(id: &str, seed: &str) -> u64 {
    let mut h = 0xcbf29ce484222325u64;
    for b in id.bytes().chain(seed.bytes()) {
        h ^= b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}

/// Classe les pistes de la bibliothèque par pertinence décroissante.
pub fn rank_tracks(tracks: Vec<Track>, seed: &MixSeed, limit: usize) -> Vec<Track> {
    let mut scored: Vec<(i64, u64, Track)> = tracks
        .into_iter()
        .filter(|t| Some(&t.id) != seed.exclude_track_id.as_ref())
        .map(|t| {
            let h = hash(&t.id, &seed.name);
            (score_track(&t, seed), h, t)
        })
        .collect();
    scored.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    // peut renvoyer moins que `limit` (voire rien si tout est exclu) ;
    // le repli « bibliothèque entière » est géré par l'appelant (build_mix)
    scored
        .into_iter()
        .take(limit.max(1))
        .map(|(_, _, t)| t)
        .collect()
}

/// Construit le mix : (pistes classées, artistes similaires, nom de la graine).
pub async fn build_mix(
    s: &AppState,
    kind: &str,
    id: &str,
    limit: usize,
) -> ApiResult<(Vec<Track>, Vec<String>, String)> {
    let lib = &s.library;
    // (nom affiché, artiste affiché, clé artiste, album graine, genres, piste à exclure)
    let (seed_name, artist_display, artist_key, album_key, genres, exclude): (
        String,
        String,
        String,
        Option<String>,
        Vec<String>,
        Option<String>,
    ) = match kind {
        "track" => {
            let t = lib
                .track(id)
                .ok_or_else(|| AurisError::TrackNotFound(id.to_string()))?;
            (
                format!("{} — {}", t.artist, t.title),
                t.artist.clone(),
                t.artist_key.clone(),
                Some(t.album_key.clone()),
                split_genres(t.genre.as_deref()),
                Some(t.id.clone()),
            )
        }
        "album" => {
            let (a, ts) = lib
                .album(id)
                .ok_or_else(|| AurisError::AlbumNotFound(id.to_string()))?;
            let mut genres: Vec<String> = ts
                .iter()
                .flat_map(|t| split_genres(t.genre.as_deref()))
                .collect();
            genres.sort();
            genres.dedup();
            (
                format!("{} — {}", a.artist, a.title),
                a.artist.clone(),
                a.artist.to_lowercase(),
                Some(a.id.clone()),
                genres,
                None,
            )
        }
        "artist" => {
            let (a, _) = lib
                .artist(id)
                .ok_or_else(|| AurisError::ArtistNotFound(id.to_string()))?;
            (
                a.name.clone(),
                a.name.clone(),
                a.name.to_lowercase(),
                None,
                Vec::new(),
                None,
            )
        }
        other => {
            return Err(AurisError::other(format!(
                "kind invalide: {other} (attendu: track|album|artist)"
            ))
            .into())
        }
    };
    let similar = similar_artist_names(s, &artist_display).await;
    let seed = MixSeed {
        name: seed_name.clone(),
        artist_key,
        album_key,
        genres,
        exclude_track_id: exclude,
        similar_artists: similar.clone(),
    };
    let all = lib.tracks();
    let mut tracks = rank_tracks(all.clone(), &seed, limit);
    if tracks.is_empty() {
        // repli : toute la bibliothèque, triée par hash (deterministe)
        let mut rest: Vec<(u64, Track)> = all
            .into_iter()
            .filter(|t| Some(&t.id) != seed.exclude_track_id.as_ref())
            .map(|t| (hash(&t.id, &seed.name), t))
            .collect();
        rest.sort_by_key(|(h, _)| *h);
        tracks = rest
            .into_iter()
            .take(limit.max(1))
            .map(|(_, t)| t)
            .collect();
    }
    Ok((tracks, similar, seed_name))
}

/// POST /api/mix — { kind: "track"|"album"|"artist", id, limit? }
/// -> { tracks, similar_artists, seed, kind }
pub async fn post_mix(
    State(s): State<AppState>,
    Json(body): Json<MixBody>,
) -> ApiResult<Json<Value>> {
    let limit = body.limit.unwrap_or(DEFAULT_LIMIT).clamp(1, 500);
    let (tracks, similar, seed_name) = build_mix(&s, &body.kind, &body.id, limit).await?;
    Ok(Json(json!({
        "kind": body.kind,
        "seed": seed_name,
        "similar_artists": similar,
        "tracks": tracks,
    })))
}

#[cfg(test)]
mod tests {
    use super::*;
    use auris_core::Track;

    fn tr(id: &str, artist: &str, album: &str, genre: Option<&str>) -> Track {
        Track {
            id: id.into(),
            title: format!("titre {id}"),
            artist: artist.into(),
            album: album.into(),
            album_artist: artist.into(),
            duration_secs: Some(200.0),
            track_number: Some(1),
            disc_number: Some(1),
            year: Some(2000),
            genre: genre.map(str::to_string),
            uri: format!("src://nas/{id}.flac"),
            source_id: "nas".into(),
            format: None,
            size_bytes: Some(1000),
            album_key: format!("{}|{}", album.to_lowercase(), artist.to_lowercase()),
            artist_key: artist.to_lowercase(),
            has_cover: false,
            composer: None,
            producer: None,
            label: None,
            publisher: None,
            isrc: None,
            copyright: None,
            lyrics: None,
            mb_track_id: None,
            added_at: 0,
        }
    }

    fn seed() -> MixSeed {
        MixSeed {
            name: "Daft Punk — One More Time".into(),
            artist_key: "daft punk".into(),
            album_key: Some("discovery|daft punk".into()),
            genres: vec!["electronic".into(), "house".into()],
            exclude_track_id: Some("t1".into()),
            similar_artists: vec!["justice".into(), "airy".into()],
        }
    }

    #[test]
    fn score_pondere_artiste_similaire_album_genre() {
        let s = seed();
        let similar = tr("t3", "Justice", "Cross", None); // artiste similaire MB : 80
        let same_artist = tr("t2", "Daft Punk", "Autre album", None); // même artiste : 50
        let same_album = tr("t4", "Daft Punk", "Discovery", None); // + album graine : 50 + 40
        let genre_only = tr("t6", "Quelqu'un", "X", Some("Electronic")); // genre commun : 10
        let unrelated = tr("t5", "Metallica", "Black", Some("Metal")); // 0
        assert!(score_track(&similar, &s) > score_track(&same_artist, &s));
        assert!(score_track(&same_artist, &s) > score_track(&unrelated, &s));
        assert!(score_track(&same_album, &s) > score_track(&same_artist, &s));
        assert!(score_track(&genre_only, &s) > score_track(&unrelated, &s));
    }

    #[test]
    fn rank_exclut_la_graine_et_trie() {
        let s = seed();
        let tracks = vec![
            tr("t1", "Daft Punk", "Discovery", Some("Electronic")), // la graine : exclue
            tr("t5", "Metallica", "Black", Some("Metal")),
            tr("t3", "Justice", "Cross", Some("Electronic")), // similaire + genre : 90
            tr("t2", "Daft Punk", "Discovery", Some("Electronic; House")), // artiste+album+genres : 110
        ];
        let out = rank_tracks(tracks, &s, 10);
        assert_eq!(out.len(), 3);
        assert!(!out.iter().any(|t| t.id == "t1"));
        assert_eq!(out[0].id, "t2"); // même artiste + album + genres en tête
        assert_eq!(out[1].id, "t3"); // artiste similaire ensuite
        assert_eq!(out[2].id, "t5");
    }

    #[test]
    fn split_genres_separe_point_virgule() {
        assert_eq!(
            split_genres(Some(" Electronic ; House; ")),
            vec!["electronic".to_string(), "house".to_string()]
        );
        assert!(split_genres(None).is_empty());
    }
}
