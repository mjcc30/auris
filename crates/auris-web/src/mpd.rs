//! Serveur MPD (Music Player Daemon), sous-ensemble v1 (#80) : port TCP dédié
//! (config `mpd_port`, défaut 6600, 0 = désactivé), protocole ligne par ligne
//! façon MPD 0.24 — pour que les clients MPD tiers (Cantata, Mpdroid, mconnectHD,
//! MALP…) pilotent Auris.
//!
//! - Connexion : salutation `OK MPD 0.24.0`, puis une commande par ligne ;
//!   réponse = paires `clé: valeur` terminées par `OK`, ou `ACK [code@0] {cmd} msg`.
//! - Commandes : ping, close, commands, notcommands, tagtypes, urlhandlers, status,
//!   currentsong, stats, play [pos], playid, pause [0|1], stop, next, previous,
//!   seek, seekid, setvol, volume, repeat, random, single, consume, clear, add, addid,
//!   delete, deleteid, playlistinfo, plchanges, listplaylists, listplaylistinfo, load,
//!   lsinfo, list, find, search, outputs, enableoutput, disableoutput, idle, noidle.
//! - `idle` répond immédiatement `OK` en v1 (pas de push d'événements).

use auris_core::{OutputMode, PlayStatus, RepeatMode, Track};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpListener;

use crate::AppState;

const MPD_VERSION: &str = "0.24.0";

/// Démarre la boucle d'écoute MPD (bloquant jusqu'à l'arrêt du process).
pub async fn serve(state: AppState, port: u16) -> anyhow::Result<()> {
    let listener = TcpListener::bind(("0.0.0.0", port)).await?;
    tracing::info!("serveur MPD: 0.0.0.0:{port}");
    loop {
        let (sock, peer) = listener.accept().await?;
        let st = state.clone();
        tokio::spawn(async move {
            if let Err(e) = handle_conn(sock, st).await {
                tracing::debug!("mpd {peer}: {e}");
            }
        });
    }
}

async fn handle_conn(sock: tokio::net::TcpStream, state: AppState) -> anyhow::Result<()> {
    let (rd, mut wr) = sock.into_split();
    let mut lines = BufReader::new(rd).lines();
    wr.write_all(format!("OK MPD {MPD_VERSION}\n").as_bytes())
        .await?;
    while let Some(line) = lines.next_line().await? {
        let line = line.trim_end().to_string();
        if line.is_empty() {
            continue;
        }
        let (cmd, args) = parse_command(&line);
        let mut out = String::new();
        let close = matches!(cmd.as_str(), "close");
        match dispatch(&state, &cmd, &args, &mut out).await {
            Ok(()) => out.push_str("OK\n"),
            Err((code, msg)) => {
                out.push_str(&format!("ACK [{code}@0] {{{cmd}}} {msg}\n"));
            }
        }
        wr.write_all(out.as_bytes()).await?;
        if close {
            break;
        }
    }
    Ok(())
}

/// Découpe une ligne de commande MPD en (commande, args), avec gestion des
/// arguments entre guillemets (`play "ma piste.flac"`).
fn parse_command(line: &str) -> (String, Vec<String>) {
    let mut tokens = Vec::new();
    let mut cur = String::new();
    let mut in_quote = false;
    let mut has = false;
    for c in line.chars() {
        match c {
            '"' => {
                in_quote = !in_quote;
                has = true;
            }
            c if c.is_whitespace() && !in_quote => {
                if has {
                    tokens.push(std::mem::take(&mut cur));
                    has = false;
                }
            }
            c => {
                cur.push(c);
                has = true;
            }
        }
    }
    if has {
        tokens.push(cur);
    }
    let cmd = tokens.first().cloned().unwrap_or_default().to_lowercase();
    (cmd, tokens.into_iter().skip(1).collect())
}

type CmdResult = Result<(), (u32, String)>;

fn err(code: u32, msg: impl Into<String>) -> (u32, String) {
    (code, msg.into())
}

fn arg(args: &[String], i: usize) -> Result<&str, (u32, String)> {
    args.get(i)
        .map(String::as_str)
        .ok_or_else(|| err(2, "nombre d'arguments incorrect"))
}

fn arg_usize(args: &[String], i: usize) -> Result<usize, (u32, String)> {
    arg(args, i)?
        .parse()
        .map_err(|_| err(2, "argument entier attendu"))
}

/// Entrée « song » façon MPD pour une piste de la file.
fn song_lines(t: &Track, pos: usize, out: &mut String) {
    out.push_str(&format!("file: {}\n", esc(&t.uri)));
    if !t.title.is_empty() {
        out.push_str(&format!("Title: {}\n", esc(&t.title)));
    }
    if !t.artist.is_empty() {
        out.push_str(&format!("Artist: {}\n", esc(&t.artist)));
    }
    if !t.album.is_empty() {
        out.push_str(&format!("Album: {}\n", esc(&t.album)));
    }
    if let Some(y) = t.year {
        out.push_str(&format!("Date: {y}\n"));
    }
    if let Some(g) = &t.genre {
        out.push_str(&format!("Genre: {}\n", esc(g)));
    }
    if let Some(n) = t.track_number {
        out.push_str(&format!("Track: {n}\n"));
    }
    if let Some(d) = t.duration_secs {
        out.push_str(&format!("Time: {}\n", d.round() as u64));
        out.push_str(&format!("duration: {d:.3}\n"));
    }
    out.push_str(&format!("Pos: {pos}\nId: {}\n", t.id));
}

/// Échappement MPD minimal (les `"` et `\` sont échappés ; on reste en UTF-8).
fn esc(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

fn status_lines(state: &AppState, out: &mut String) {
    let st = state.engine.state();
    let repeat = match st.repeat {
        RepeatMode::Off => 0,
        RepeatMode::All => 1,
        RepeatMode::One => 0, // `single` porte le repeat-one
    };
    let single = match st.repeat {
        RepeatMode::One => 1,
        _ => 0,
    };
    out.push_str(&format!("volume: {}\n", st.volume));
    out.push_str(&format!("repeat: {repeat}\n"));
    out.push_str(&format!("random: {}\n", u8::from(st.shuffle)));
    out.push_str(&format!("single: {single}\n"));
    out.push_str("consume: 0\n");
    out.push_str(&format!("playlist: {}\n", st.queue.len()));
    out.push_str(&format!("playlistlength: {}\n", st.queue.len()));
    out.push_str("mixrampdb: 0.000000\n");
    let state_str = match st.status {
        PlayStatus::Playing => "play",
        PlayStatus::Paused => "pause",
        PlayStatus::Stopped => "stop",
    };
    out.push_str(&format!("state: {state_str}\n"));
    if let Some(i) = st.queue_index {
        out.push_str(&format!("song: {i}\n"));
        if let Some(t) = st.queue.get(i) {
            out.push_str(&format!("songid: {}\n", t.id));
        }
        let dur = st.duration_secs;
        out.push_str(&format!(
            "time: {}:{}\n",
            st.position_secs.round() as u64,
            dur.round() as u64
        ));
        out.push_str(&format!("elapsed: {:.3}\n", st.position_secs));
        out.push_str(&format!("duration: {dur:.3}\n"));
        if let Some(f) = &st.current_format {
            let (sr, bits, ch) = match f {
                auris_core::AudioFormat::Pcm {
                    sample_rate,
                    bits,
                    channels,
                } => (*sample_rate, *bits as u32, *channels as u32),
                auris_core::AudioFormat::Dsd {
                    sample_rate,
                    channels,
                } => (*sample_rate, 1, *channels as u32),
            };
            out.push_str(&format!("audio: {sr}:{bits}:{ch}\n"));
        }
        if let Some(n) = next_index_of(&st) {
            out.push_str(&format!("nextsong: {n}\n"));
            if let Some(t) = st.queue.get(n) {
                out.push_str(&format!("nextsongid: {}\n", t.id));
            }
        }
    }
}

fn next_index_of(st: &auris_core::PlayerState) -> Option<usize> {
    let i = st.queue_index?;
    if i + 1 < st.queue.len() {
        Some(i + 1)
    } else if st.repeat == RepeatMode::All && !st.queue.is_empty() {
        Some(0)
    } else {
        None
    }
}

async fn dispatch(state: &AppState, cmd: &str, args: &[String], out: &mut String) -> CmdResult {
    let eng = &state.engine;
    match cmd {
        "ping" | "noidle" => Ok(()),
        "close" => Ok(()),
        "commands" => {
            for c in COMMANDS {
                out.push_str(&format!("command: {c}\n"));
            }
            Ok(())
        }
        "notcommands" => Ok(()),
        "tagtypes" => {
            for t in ["Artist", "Album", "Title", "Track", "Genre", "Date"] {
                out.push_str(&format!("tagtype: {t}\n"));
            }
            Ok(())
        }
        "urlhandlers" => Ok(()),
        "status" => {
            status_lines(state, out);
            Ok(())
        }
        "currentsong" => {
            let st = eng.state();
            if let (Some(i), Some(t)) = (
                st.queue_index,
                st.current
                    .as_ref()
                    .or(st.queue.get(st.queue_index.unwrap_or(0))),
            ) {
                song_lines(t, i, out);
            }
            Ok(())
        }
        "stats" => {
            let lib = &state.library;
            let st = lib.stats();
            out.push_str(&format!("artists: {}\n", st.artists));
            out.push_str(&format!("albums: {}\n", st.albums));
            out.push_str(&format!("songs: {}\n", st.tracks));
            out.push_str(&format!("uptime: {}\n", uptime(state)));
            out.push_str(&format!("playtime: {}\n", st.playtime_secs.round() as u64));
            out.push_str(&format!(
                "db_playtime: {}\n",
                st.playtime_secs.round() as u64
            ));
            Ok(())
        }
        "play" => {
            let st = eng.state();
            match args.first() {
                Some(pos) => {
                    let i = pos
                        .parse::<usize>()
                        .map_err(|_| err(2, "position entière attendue"))?;
                    if i >= st.queue.len() {
                        return Err(err(50, "pas de chanson à cette position"));
                    }
                    eng.play_queue_index(i);
                }
                None => {
                    if st.status == PlayStatus::Paused {
                        eng.resume();
                    } else if let Some(i) = st.queue_index {
                        eng.play_queue_index(i);
                    }
                }
            }
            Ok(())
        }
        "playid" => {
            let id = arg(args, 0)?;
            let st = eng.state();
            let Some(i) = st.queue.iter().position(|t| t.id == id) else {
                return Err(err(50, "pas de chanson avec cet id"));
            };
            eng.play_queue_index(i);
            Ok(())
        }
        "pause" => {
            let st = eng.state();
            let on = match args.first().map(String::as_str) {
                None => st.status != PlayStatus::Paused, // bascule
                Some("1") | Some("true") => true,
                Some("0") | Some("false") => false,
                _ => return Err(err(2, "0 ou 1 attendu")),
            };
            if on {
                eng.pause();
            } else {
                eng.resume();
            }
            Ok(())
        }
        "stop" => {
            eng.stop();
            Ok(())
        }
        "next" => {
            eng.next();
            Ok(())
        }
        "previous" | "prev" => {
            eng.prev();
            Ok(())
        }
        "seek" => {
            let pos = arg_usize(args, 0)?;
            let secs: f64 = arg(args, 1)?
                .parse()
                .map_err(|_| err(2, "position en secondes attendue"))?;
            let st = eng.state();
            if pos >= st.queue.len() {
                return Err(err(50, "pas de chanson à cette position"));
            }
            eng.play_queue_index(pos);
            eng.seek(secs);
            Ok(())
        }
        "seekid" => {
            let id = arg(args, 0)?;
            let secs: f64 = arg(args, 1)?
                .parse()
                .map_err(|_| err(2, "position en secondes attendue"))?;
            let st = eng.state();
            let Some(i) = st.queue.iter().position(|t| t.id == id) else {
                return Err(err(50, "pas de chanson avec cet id"));
            };
            eng.play_queue_index(i);
            eng.seek(secs);
            Ok(())
        }
        "setvol" | "volume" => {
            let v: i64 = arg(args, 0)?
                .parse()
                .map_err(|_| err(2, "volume 0-100 attendu"))?;
            if !(0..=100).contains(&v) {
                return Err(err(2, "volume 0-100 attendu"));
            }
            eng.set_volume(v as u8);
            Ok(())
        }
        "repeat" => {
            eng.set_repeat(match arg(args, 0)? {
                "1" => RepeatMode::All,
                "0" => RepeatMode::Off,
                _ => return Err(err(2, "0 ou 1 attendu")),
            });
            Ok(())
        }
        "single" => {
            eng.set_repeat(match arg(args, 0)? {
                "1" => RepeatMode::One,
                "0" => RepeatMode::Off,
                _ => return Err(err(2, "0 ou 1 attendu")),
            });
            Ok(())
        }
        "random" => {
            eng.set_shuffle(match arg(args, 0)? {
                "1" => true,
                "0" => false,
                _ => return Err(err(2, "0 ou 1 attendu")),
            });
            Ok(())
        }
        "consume" => Ok(()), // mode non géré en v1, accepté
        "clear" => {
            eng.clear_queue();
            Ok(())
        }
        "add" | "addid" => {
            let uri = arg(args, 0)?;
            let src_id = uri
                .strip_prefix("src://")
                .and_then(|r| r.split('/').next())
                .unwrap_or("")
                .to_string();
            let t = Track::new(uri.to_string(), src_id);
            if cmd == "addid" {
                out.push_str(&format!("Id: {}\n", t.id));
            }
            eng.enqueue(t);
            Ok(())
        }
        "delete" => {
            let pos = arg_usize(args, 0)?;
            let st = eng.state();
            if pos >= st.queue.len() {
                return Err(err(50, "index hors file"));
            }
            eng.remove_queue_item(pos);
            Ok(())
        }
        "deleteid" => {
            let id = arg(args, 0)?;
            let st = eng.state();
            let Some(i) = st.queue.iter().position(|t| t.id == id) else {
                return Err(err(50, "id inconnu"));
            };
            eng.remove_queue_item(i);
            Ok(())
        }
        "playlistinfo" => {
            let st = eng.state();
            let range = args.first().map(String::as_str);
            for (i, t) in st.queue.iter().enumerate() {
                if let Some(r) = range {
                    // forme MPD « start:end » (end exclu, -1 = fin)
                    if let Some((a, b)) = r.split_once(':') {
                        let a: i64 = a.parse().unwrap_or(0);
                        let b: i64 = b.parse().unwrap_or(-1);
                        let (a, b) = (
                            a.max(0) as usize,
                            if b < 0 { st.queue.len() } else { b as usize },
                        );
                        if i < a || i >= b {
                            continue;
                        }
                    } else if let Ok(a) = r.parse::<usize>() {
                        if i != a {
                            continue;
                        }
                    }
                }
                song_lines(t, i, out);
            }
            Ok(())
        }
        "plchanges" | "plchangesposid" => {
            let st = eng.state();
            for (i, t) in st.queue.iter().enumerate() {
                song_lines(t, i, out);
            }
            Ok(())
        }
        "listplaylists" => {
            for p in state.library.playlists() {
                out.push_str(&format!("playlist: {}\n", esc(&p.name)));
            }
            Ok(())
        }
        "listplaylist" => {
            let name = arg(args, 0)?;
            let pls = state.library.playlists();
            let Some(p) = pls.into_iter().find(|p| p.name == name) else {
                return Err(err(50, "playlist inconnue"));
            };
            for id in &p.track_ids {
                if let Some(t) = state.library.track(id) {
                    out.push_str(&format!("file: {}\n", esc(&t.uri)));
                }
            }
            Ok(())
        }
        "listplaylistinfo" => {
            let name = arg(args, 0)?;
            let pls = state.library.playlists();
            let Some(p) = pls.into_iter().find(|p| p.name == name) else {
                return Err(err(50, "playlist inconnue"));
            };
            let mut pos = 0;
            for id in &p.track_ids {
                if let Some(t) = state.library.track(id) {
                    song_lines(&t, pos, out);
                    pos += 1;
                }
            }
            Ok(())
        }
        "load" => {
            let name = arg(args, 0)?;
            let pls = state.library.playlists();
            let Some(p) = pls.into_iter().find(|p| p.name == name) else {
                return Err(err(50, "playlist inconnue"));
            };
            let tracks: Vec<Track> = p
                .track_ids
                .iter()
                .filter_map(|id| state.library.track(id))
                .collect();
            if tracks.is_empty() {
                return Err(err(50, "playlist vide"));
            }
            eng.clear_queue();
            eng.play_queue(tracks, 0);
            Ok(())
        }
        "lsinfo" => {
            let uri = args.first().map(String::as_str).unwrap_or("");
            if uri.is_empty() {
                for s in state.sources.list() {
                    out.push_str(&format!("directory: {}\n", esc(&s.name)));
                    let _ = s;
                }
            } else {
                let entries = state
                    .sources
                    .browse(uri)
                    .await
                    .map_err(|e| err(50, format!("browse: {e}")))?;
                for e in entries {
                    if e.is_dir {
                        out.push_str(&format!("directory: {}\n", esc(&e.name)));
                    } else if e.is_audio {
                        let t = Track::new(e.uri.clone(), String::new());
                        song_lines(&t, 0, out);
                    }
                }
            }
            Ok(())
        }
        "list" => {
            let what = arg(args, 0)?.to_lowercase();
            match what.as_str() {
                "artist" => {
                    for a in state.library.artists() {
                        out.push_str(&format!("Artist: {}\n", esc(&a.name)));
                    }
                }
                "album" => {
                    for a in state.library.albums() {
                        out.push_str(&format!("Album: {}\n", esc(&a.title)));
                    }
                }
                "genre" => {
                    for g in state.library.genres() {
                        out.push_str(&format!("Genre: {}\n", esc(&g.name)));
                    }
                }
                _ => return Err(err(2, "type list non supporté (artist|album|genre)")),
            }
            Ok(())
        }
        "find" | "search" => {
            // find <type> <valeur…> ; search = correspondance partielle (comme find en v1)
            let what = arg(args, 0)?.to_lowercase();
            let query = args.get(1..).map(|a| a.join(" ")).unwrap_or_default();
            let res = state.library.search_grouped(&query, None);
            match what.as_str() {
                "artist" => {
                    for a in res.artists.items {
                        out.push_str(&format!("Artist: {}\n", esc(&a.name)));
                    }
                }
                "album" => {
                    for a in res.albums.items {
                        out.push_str(&format!("Album: {}\n", esc(&a.title)));
                    }
                }
                "title" | "filename" | "file" => {
                    for t in res.tracks.items {
                        song_lines(&t, 0, out);
                    }
                }
                _ => return Err(err(2, "type find non supporté (artist|album|title)")),
            }
            Ok(())
        }
        "outputs" => {
            let st = eng.state();
            out.push_str("outputid: 0\noutputname: DAC (local)\n");
            out.push_str(&format!(
                "outputenabled: {}\n",
                u8::from(st.output_mode == OutputMode::Local)
            ));
            out.push_str("outputid: 1\noutputname: Navigateur (Jouer ici)\n");
            out.push_str(&format!(
                "outputenabled: {}\n",
                u8::from(st.output_mode == OutputMode::Browser)
            ));
            Ok(())
        }
        "enableoutput" => {
            let id: u32 = arg(args, 0)?
                .parse()
                .map_err(|_| err(2, "outputid attendu"))?;
            eng.set_output_mode(match id {
                0 => OutputMode::Local,
                1 => OutputMode::Browser,
                _ => return Err(err(50, "outputid inconnu")),
            });
            Ok(())
        }
        "disableoutput" => Ok(()),
        "idle" => Ok(()), // v1 : pas de notification push
        other => Err(err(5, format!("commande inconnue \"{other}\""))),
    }
}

fn uptime(state: &AppState) -> u64 {
    let now = chrono::Utc::now().timestamp();
    now.saturating_sub(state.started_at).max(0) as u64
}

const COMMANDS: &[&str] = &[
    "ping",
    "close",
    "commands",
    "notcommands",
    "tagtypes",
    "urlhandlers",
    "status",
    "currentsong",
    "stats",
    "play",
    "playid",
    "pause",
    "stop",
    "next",
    "previous",
    "seek",
    "seekid",
    "setvol",
    "volume",
    "repeat",
    "random",
    "single",
    "consume",
    "clear",
    "add",
    "addid",
    "delete",
    "deleteid",
    "playlistinfo",
    "plchanges",
    "plchangesposid",
    "listplaylists",
    "listplaylist",
    "listplaylistinfo",
    "load",
    "lsinfo",
    "list",
    "find",
    "search",
    "outputs",
    "enableoutput",
    "disableoutput",
    "idle",
    "noidle",
];

/// Helper de test : exécute une commande hors connexion (sans préfixe OK/ACK).
pub async fn dispatch_for_test(state: &AppState, line: &str) -> (String, bool) {
    let (cmd, args) = parse_command(line);
    let mut out = String::new();
    let ok = dispatch(state, &cmd, &args, &mut out).await.is_ok();
    (out, ok)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_command_guillemets() {
        let (cmd, args) = parse_command("add \"Ma piste.flac\" extra");
        assert_eq!(cmd, "add");
        assert_eq!(args, vec!["Ma piste.flac".to_string(), "extra".to_string()]);
        let (cmd, args) = parse_command("PLAY 3");
        assert_eq!(cmd, "play");
        assert_eq!(args, vec!["3".to_string()]);
    }
}
