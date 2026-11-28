//! Intégrations externes — #26 : scrobble Last.fm + fiche MusicBrainz.
//!
//! - Last.fm : API signée (md5 de `key+value` triés + secret). Flux
//!   `auth.getToken` → autorisation web → `auth.getSession`, puis
//!   `track.updateNowPlaying` au début de chaque piste et `track.scrobble`
//!   quand la piste a été jouée plus de la moitié de sa durée (ou 4 min,
//!   le plus court des deux ; durée minimale 30 s, règles Last.fm).
//! - MusicBrainz : WS2 publique (sans clé, User-Agent obligatoire, 1 req/s).
//!   La fiche recording enrichit `GET /api/infos/{id}` (cache 10 min).

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use auris_core::{LastfmSettings, MusicBrainzSettings, PlayStatus, Track};
use serde_json::{json, Value};

use crate::AppState;

const LASTFM_API_DEFAULT: &str = "https://ws.audioscrobbler.com/2.0/";
const MB_API_DEFAULT: &str = "https://musicbrainz.org/ws/2";
/// MusicBrainz demande 1 req/s max ; on marge à 1,1 s.
const MB_MIN_INTERVAL: Duration = Duration::from_millis(1100);
/// TTL du cache MusicBrainz par track_id.
const MB_CACHE_TTL_SECS: i64 = 600;

// ---------- signature Last.fm ----------

/// `api_sig` Last.fm : md5 hex de la concaténation `clé+valeur` de tous les
/// paramètres (sauf `format`/`callback`) triés par clé, suivie du secret.
pub fn api_sig(secret: &str, params: &BTreeMap<String, String>) -> String {
    use md5::{Digest, Md5};
    let mut h = Md5::new();
    for (k, v) in params {
        if k == "format" || k == "callback" {
            continue;
        }
        h.update(k.as_bytes());
        h.update(v.as_bytes());
    }
    h.update(secret.as_bytes());
    format!("{:x}", h.finalize())
}

/// Seuil de scrobble Last.fm (secondes) : min(durée/2, 240 s), et uniquement
/// si la piste dure plus de 30 s. `None` = ne jamais scrobbler.
pub fn scrobble_threshold(duration_secs: f64) -> Option<f64> {
    if duration_secs <= 30.0 {
        return None;
    }
    Some((duration_secs / 2.0).min(240.0))
}

// ---------- client Last.fm ----------

pub struct LastfmClient {
    http: reqwest::Client,
    api_url: String,
    cfg: LastfmSettings,
}

impl LastfmClient {
    pub fn new(cfg: LastfmSettings) -> Self {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(15))
            .build()
            .unwrap_or_else(|_| reqwest::Client::new());
        LastfmClient {
            http,
            api_url: cfg
                .api_url
                .clone()
                .filter(|u| !u.trim().is_empty())
                .unwrap_or_else(|| LASTFM_API_DEFAULT.into()),
            cfg,
        }
    }

    /// Appelle une méthode signée. `extra` = paramètres spécifiques (sans
    /// api_key/sk/format qui sont ajoutés ici). Retourne le JSON décodé ou
    /// le message d'erreur Last.fm (`{"error": n, "message": "…"}`).
    pub async fn call(
        &self,
        method: &str,
        extra: &[(&str, String)],
        with_session: bool,
    ) -> Result<Value, String> {
        let mut params = BTreeMap::new();
        params.insert("method".to_string(), method.to_string());
        params.insert("api_key".to_string(), self.cfg.api_key.clone());
        if with_session {
            params.insert("sk".to_string(), self.cfg.session_key.clone());
        }
        for (k, v) in extra {
            params.insert(k.to_string(), v.clone());
        }
        let sig = api_sig(&self.cfg.shared_secret, &params);
        let mut form: Vec<(String, String)> =
            params.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
        form.push(("api_sig".to_string(), sig));
        form.push(("format".to_string(), "json".to_string()));
        let resp = self
            .http
            .post(&self.api_url)
            .form(&form)
            .send()
            .await
            .map_err(|e| format!("lastfm http: {e}"))?;
        let status = resp.status();
        let body = resp.text().await.map_err(|e| format!("lastfm body: {e}"))?;
        let v: Value = serde_json::from_str(&body)
            .map_err(|_| format!("lastfm réponse non JSON (HTTP {status}): {body}"))?;
        if let (Some(err), Some(msg)) = (
            v.get("error").and_then(|e| e.as_i64()),
            v.get("message").and_then(|m| m.as_str()),
        ) {
            return Err(format!("lastfm erreur {err}: {msg}"));
        }
        Ok(v)
    }

    pub async fn now_playing(&self, t: &Track) -> Result<Value, String> {
        let mut extra: Vec<(&str, String)> =
            vec![("artist", t.artist.clone()), ("track", t.title.clone())];
        if !t.album.is_empty() && t.album != "Album inconnu" {
            extra.push(("album", t.album.clone()));
        }
        if let Some(d) = t.duration_secs {
            extra.push(("duration", (d.round() as i64).to_string()));
        }
        if let Some(n) = t.track_number {
            extra.push(("trackNumber", n.to_string()));
        }
        if let Some(m) = &t.mb_track_id {
            extra.push(("mbid", m.clone()));
        }
        self.call("track.updateNowPlaying", &extra, true).await
    }

    pub async fn scrobble(&self, t: &Track, timestamp: i64) -> Result<Value, String> {
        let mut extra: Vec<(&str, String)> = vec![
            ("artist", t.artist.clone()),
            ("track", t.title.clone()),
            ("timestamp", timestamp.to_string()),
        ];
        if !t.album.is_empty() && t.album != "Album inconnu" {
            extra.push(("album", t.album.clone()));
        }
        if let Some(d) = t.duration_secs {
            extra.push(("duration", (d.round() as i64).to_string()));
        }
        if let Some(n) = t.track_number {
            extra.push(("trackNumber", n.to_string()));
        }
        if let Some(m) = &t.mb_track_id {
            extra.push(("mbid", m.clone()));
        }
        self.call("track.scrobble", &extra, true).await
    }

    pub async fn user_info(&self) -> Result<Value, String> {
        self.call("user.getInfo", &[], true).await
    }

    /// `auth.getToken` — démarre le flux d'autorisation (pas besoin de session).
    pub async fn auth_get_token(&self) -> Result<String, String> {
        let v = self.call("auth.getToken", &[], false).await?;
        v.get("token")
            .and_then(|t| t.as_str())
            .map(str::to_string)
            .ok_or_else(|| "lastfm: pas de token dans la réponse".to_string())
    }

    /// `auth.getSession` — échange le token autorisé contre une session key.
    pub async fn auth_get_session(&self, token: &str) -> Result<(String, String), String> {
        let v = self
            .call("auth.getSession", &[("token", token.to_string())], false)
            .await?;
        let sess = v
            .get("session")
            .ok_or_else(|| "lastfm: pas de session dans la réponse".to_string())?;
        let key = sess
            .get("key")
            .and_then(|k| k.as_str())
            .ok_or_else(|| "lastfm: pas de clé de session".to_string())?
            .to_string();
        let name = sess
            .get("name")
            .and_then(|n| n.as_str())
            .unwrap_or("")
            .to_string();
        Ok((key, name))
    }

    /// URL d'autorisation web à ouvrir pour valider le token.
    pub fn auth_url(&self, token: &str) -> String {
        let base = if self.api_url.contains("audioscrobbler.com") {
            "https://www.last.fm/api/auth/".to_string()
        } else {
            // endpoint personnalisé (tests) : on colle /api/auth/ à côté
            self.api_url
                .trim_end_matches('/')
                .rsplit_once('/')
                .map(|(h, _)| format!("{h}/api/auth/"))
                .unwrap_or_else(|| self.api_url.clone())
        };
        format!(
            "{base}?api_key={}&token={}",
            urlencode(&self.cfg.api_key),
            urlencode(token)
        )
    }
}

pub fn urlencode(s: &str) -> String {
    let mut out = String::with_capacity(s.len() * 3);
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

// ---------- MusicBrainz ----------

pub struct MbClient {
    http: reqwest::Client,
    base: String,
    /// rate-limit 1 req/s (marge 1,1 s) partagé entre toutes les requêtes.
    last_req: Arc<Mutex<Instant>>,
}

impl MbClient {
    pub fn new(cfg: &MusicBrainzSettings) -> Self {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(15))
            .user_agent("auris/0.1 (https://auris.audio)")
            .build()
            .unwrap_or_else(|_| reqwest::Client::new());
        MbClient {
            http,
            base: cfg
                .api_url
                .clone()
                .filter(|u| !u.trim().is_empty())
                .unwrap_or_else(|| MB_API_DEFAULT.into())
                .trim_end_matches('/')
                .to_string(),
            last_req: Arc::new(Mutex::new(Instant::now() - MB_MIN_INTERVAL)),
        }
    }

    async fn get(&self, url: &str) -> Result<Value, String> {
        let wait = {
            let mut last = self.last_req.lock().unwrap();
            let wait = MB_MIN_INTERVAL.saturating_sub(last.elapsed());
            *last = Instant::now();
            wait
        };
        if wait > Duration::ZERO {
            tokio::time::sleep(wait).await;
        }
        let resp = self
            .http
            .get(url)
            .send()
            .await
            .map_err(|e| format!("musicbrainz http: {e}"))?;
        let status = resp.status();
        if status == reqwest::StatusCode::NOT_FOUND {
            return Ok(Value::Null);
        }
        let body = resp
            .text()
            .await
            .map_err(|e| format!("musicbrainz body: {e}"))?;
        serde_json::from_str(&body)
            .map_err(|_| format!("musicbrainz réponse non JSON (HTTP {status}): {body}"))
    }

    /// Fiche recording par MBID (avec artistes, release-group, tags, ISRC, note).
    pub async fn recording(&self, mbid: &str) -> Result<Value, String> {
        let url = format!(
            "{}/recording/{}?fmt=json&inc=artist-credits+release-groups+tags+isrcs+ratings",
            self.base,
            urlencode(mbid)
        );
        self.get(&url).await
    }

    /// Recherche recording par artiste + titre (fallback quand pas de MBID tagué).
    pub async fn search(&self, artist: &str, title: &str) -> Result<Value, String> {
        let q = format!(
            "artist:\"{}\" AND recording:\"{}\"",
            artist.replace('"', ""),
            title.replace('"', "")
        );
        let url = format!(
            "{}/recording/?fmt=json&limit=3&query={}",
            self.base,
            urlencode(&q)
        );
        self.get(&url).await
    }

    /// Recherche d'artiste par nom (page artiste #102) : `artists` + score.
    pub async fn artist_search(&self, name: &str) -> Result<Value, String> {
        let q = format!("artist:\"{}\"", name.replace('"', ""));
        let url = format!(
            "{}/artist/?fmt=json&limit=5&query={}",
            self.base,
            urlencode(&q)
        );
        self.get(&url).await
    }

    /// Fiche artiste par MBID : tags, note, relations, genres, aliases (#102).
    pub async fn artist(&self, mbid: &str) -> Result<Value, String> {
        let url = format!(
            "{}/artist/{}?fmt=json&inc=artist-rels+tags+ratings+genres+aliases",
            self.base,
            urlencode(mbid)
        );
        self.get(&url).await
    }

    /// Release-groups d'un artiste (discographie #102), albums uniquement.
    pub async fn release_groups(&self, mbid: &str) -> Result<Value, String> {
        let url = format!(
            "{}/release-group/?artist={}&fmt=json&limit=50&type=album",
            self.base,
            urlencode(mbid)
        );
        self.get(&url).await
    }

    /// Artistes partageant un tag (similaires #102) — le filtrage du nom
    /// demandé se fait côté appelant.
    pub async fn artists_by_tag(&self, tag: &str) -> Result<Value, String> {
        let q = format!("tag:\"{}\" AND type:group", tag.replace('"', ""));
        let url = format!(
            "{}/artist/?fmt=json&limit=12&query={}",
            self.base,
            urlencode(&q)
        );
        self.get(&url).await
    }

    /// Fiche « propre » pour l'UI, extraite de la réponse WS2 brute.
    pub fn summarize(v: &Value) -> Value {
        if v.is_null() {
            return Value::Null;
        }
        // soit un recording direct, soit le premier résultat d'une recherche
        let rec = if v.get("recordings").is_some() {
            v.get("recordings").and_then(|r| r.get(0)).cloned()
        } else {
            Some(v.clone())
        };
        let Some(r) = rec else { return Value::Null };
        let artist = r
            .get("artist-credit")
            .and_then(|c| c.get(0))
            .and_then(|a| a.get("name"))
            .and_then(|n| n.as_str())
            .unwrap_or("")
            .to_string();
        let rg = r.get("release-group");
        let release = rg
            .and_then(|g| g.get("title"))
            .and_then(|t| t.as_str())
            .unwrap_or("")
            .to_string();
        let date = rg
            .and_then(|g| g.get("first-release-date"))
            .and_then(|d| d.as_str())
            .unwrap_or("")
            .to_string();
        let label = rg
            .and_then(|g| g.get("label-info"))
            .and_then(|l| l.get(0))
            .and_then(|li| li.get("label"))
            .and_then(|l| l.get("name"))
            .and_then(|n| n.as_str())
            .unwrap_or("")
            .to_string();
        let tags: Vec<String> = r
            .get("tags")
            .and_then(|t| t.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|t| t.get("name").and_then(|n| n.as_str()))
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default();
        let isrcs: Vec<String> = r
            .get("isrcs")
            .and_then(|t| t.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|i| i.as_str())
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default();
        let rating = r.get("rating").and_then(|x| x.get("value")).cloned();
        let rating_count = r.get("rating-count").cloned();
        json!({
            "mbid": r.get("id").cloned().unwrap_or(Value::Null),
            "title": r.get("title").cloned().unwrap_or(Value::Null),
            "artist": artist,
            "release": release,
            "release_date": date,
            "label": label,
            "length_ms": r.get("length").cloned().unwrap_or(Value::Null),
            "tags": tags,
            "isrcs": isrcs,
            "rating": rating,
            "rating_count": rating_count,
            "url": r.get("id").and_then(|i| i.as_str()).map(|i| format!("https://musicbrainz.org/recording/{i}")),
        })
    }
}

/// Cache MusicBrainz (track_id → (ts, fiche)) + client partagé.
pub struct MbCache {
    pub client: MbClient,
    pub map: Mutex<std::collections::HashMap<String, (i64, Value)>>,
}

impl MbCache {
    pub fn new(cfg: &MusicBrainzSettings) -> Self {
        MbCache {
            client: MbClient::new(cfg),
            map: Mutex::new(std::collections::HashMap::new()),
        }
    }

    /// Fiche recording pour une piste : MBID tagué d'abord, sinon recherche
    /// artist+title, sinon `null`. Résultat mis en cache 10 min.
    pub async fn ficha(&self, t: &Track) -> Value {
        let now = chrono::Utc::now().timestamp();
        if let Some((ts, v)) = self.map.lock().unwrap().get(&t.id) {
            if now - *ts < MB_CACHE_TTL_SECS {
                return v.clone();
            }
        }
        let raw = if let Some(mbid) = &t.mb_track_id {
            match self.client.recording(mbid).await {
                Ok(v) if !v.is_null() => v,
                _ => match self.client.search(&t.artist, &t.title).await {
                    Ok(v) => v,
                    Err(e) => {
                        tracing::warn!("musicbrainz search: {e}");
                        Value::Null
                    }
                },
            }
        } else {
            match self.client.search(&t.artist, &t.title).await {
                Ok(v) => v,
                Err(e) => {
                    tracing::warn!("musicbrainz search: {e}");
                    Value::Null
                }
            }
        };
        let sum = MbClient::summarize(&raw);
        self.map
            .lock()
            .unwrap()
            .insert(t.id.clone(), (now, sum.clone()));
        sum
    }
}

// ---------- tâche de fond : now-playing + scrobble ----------

struct ScrobbleState {
    track_id: String,
    scrobbled: bool,
}

/// Tâche de fond : à chaque changement de piste, `track.updateNowPlaying` ;
/// quand la position dépasse le seuil, `track.scrobble` (une fois par piste).
pub fn spawn_scrobbler(state: AppState) {
    tokio::spawn(async move {
        let mut current: Option<ScrobbleState> = None;
        let mut ticker = tokio::time::interval(Duration::from_secs(5));
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            ticker.tick().await;
            let (lf_cfg, enabled) = {
                let s = state.settings.read().unwrap();
                (s.lastfm.clone(), s.lastfm.enabled)
            };
            if !enabled
                || lf_cfg.api_key.is_empty()
                || lf_cfg.shared_secret.is_empty()
                || lf_cfg.session_key.is_empty()
            {
                current = None;
                continue;
            }
            let st = state.engine.state();
            let cur = match &st.current {
                Some(t) if st.status == PlayStatus::Playing || st.status == PlayStatus::Paused => {
                    Some(t.clone())
                }
                _ => None,
            };
            let Some(t) = cur else {
                current = None;
                continue;
            };
            let client = LastfmClient::new(lf_cfg);
            match &current {
                Some(c) if c.track_id == t.id => {
                    // même piste : scrobble au seuil
                    if !c.scrobbled {
                        if let (Some(dur), Some(thr)) = (
                            t.duration_secs,
                            t.duration_secs.and_then(scrobble_threshold),
                        ) {
                            let _ = dur;
                            if st.position_secs >= thr {
                                let ts = chrono::Utc::now().timestamp()
                                    - st.position_secs.round() as i64;
                                match client.scrobble(&t, ts).await {
                                    Ok(_) => {
                                        tracing::info!(
                                            "lastfm scrobble: {} — {}",
                                            t.artist,
                                            t.title
                                        );
                                    }
                                    Err(e) => tracing::warn!("lastfm scrobble: {e}"),
                                }
                                if let Some(c) = &mut current {
                                    c.scrobbled = true;
                                }
                            }
                        }
                    }
                }
                _ => {
                    // nouvelle piste : now-playing
                    match client.now_playing(&t).await {
                        Ok(_) => tracing::info!("lastfm now-playing: {} — {}", t.artist, t.title),
                        Err(e) => tracing::warn!("lastfm now-playing: {e}"),
                    }
                    current = Some(ScrobbleState {
                        track_id: t.id.clone(),
                        scrobbled: false,
                    });
                }
            }
        }
    });
}

/// Extrait les réglages Last.fm courants (pour les handlers).
pub fn lastfm_cfg(s: &AppState) -> LastfmSettings {
    s.settings.read().unwrap().lastfm.clone()
}

/// MusicBrainz est-il activé ?
pub fn mb_enabled(s: &AppState) -> bool {
    s.settings.read().unwrap().musicbrainz.enabled
}

/// Config MusicBrainz courante.
pub fn mb_cfg(s: &AppState) -> MusicBrainzSettings {
    s.settings.read().unwrap().musicbrainz.clone()
}

/// Construit (ou met à jour) le cache MusicBrainz de l'état.
pub fn mb_cache(s: &AppState) -> Arc<MbCache> {
    let mut guard = s.mb_cache.lock().unwrap();
    if guard.is_none() {
        *guard = Some(Arc::new(MbCache::new(&mb_cfg(s))));
    }
    guard.clone().unwrap()
}

/// Invalide le cache MusicBrainz (ex. après changement d'URL dans les réglages).
pub fn mb_cache_reset(s: &AppState) {
    *s.mb_cache.lock().unwrap() = None;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn api_sig_reference() {
        // vecteur calculé à la main : md5("api_keyKmethodMsecretS")
        let mut p = BTreeMap::new();
        p.insert("method".to_string(), "M".to_string());
        p.insert("api_key".to_string(), "K".to_string());
        // `format` est exclu de la signature
        p.insert("format".to_string(), "json".to_string());
        let sig = api_sig("S", &p);
        use md5::{Digest, Md5};
        let mut h = Md5::new();
        h.update(b"api_keyK");
        h.update(b"methodM");
        h.update(b"S");
        assert_eq!(sig, format!("{:x}", h.finalize()));
        assert_eq!(sig.len(), 32);
    }

    #[test]
    fn seuil_scrobble_regles_lastfm() {
        assert_eq!(scrobble_threshold(20.0), None); // < 30 s : jamais
        assert_eq!(scrobble_threshold(30.0), None);
        assert_eq!(scrobble_threshold(60.0), Some(30.0)); // moitié
        assert_eq!(scrobble_threshold(600.0), Some(240.0)); // cap 4 min
        assert_eq!(scrobble_threshold(480.0), Some(240.0));
    }

    #[test]
    fn urlencode_basique() {
        assert_eq!(urlencode("a b&c=d"), "a%20b%26c%3Dd");
        assert_eq!(urlencode("abc-_.~"), "abc-_.~");
    }

    #[test]
    fn mb_summarize_recording_et_recherche() {
        let rec = json!({
            "id": "mbid-1",
            "title": "Airbag",
            "length": "287000",
            "artist-credit": [{"name": "Radiohead"}],
            "release-group": {
                "title": "OK Computer",
                "first-release-date": "1997-06-16",
                "label-info": [{"label": {"name": "Parlophone"}}]
            },
            "tags": [{"name": "rock"}, {"name": "alternative"}],
            "isrcs": ["GBAYE9700038"],
            "rating": {"value": 4.5},
            "rating-count": 1234
        });
        let s = MbClient::summarize(&rec);
        assert_eq!(s["artist"], "Radiohead");
        assert_eq!(s["release"], "OK Computer");
        assert_eq!(s["label"], "Parlophone");
        assert_eq!(s["isrcs"][0], "GBAYE9700038");
        assert_eq!(s["tags"].as_array().unwrap().len(), 2);
        // enveloppe de recherche : premier élément retenu
        let search = json!({"recordings": [rec.clone()]});
        let s2 = MbClient::summarize(&search);
        assert_eq!(s2["mbid"], "mbid-1");
        assert!(MbClient::summarize(&Value::Null).is_null());
    }
}
