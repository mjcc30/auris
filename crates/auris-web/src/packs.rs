//! Packs d'interface : thèmes CSS et packs de langue.
//!
//! Un pack = un simple fichier déposé par l'utilisateur ou la communauté :
//! - thèmes : `<data-dir>/themes/{id}.css` (id en `[a-z0-9-]`) ; les packs
//!   embarqués dans l'UI (`ui/dist/themes/*.css`) servent de repli.
//! - langues : `<data-dir>/locales/{lang}.json` (`{ "clé FR": "traduction" }`) ;
//!   repli sur les packs embarqués (`ui/dist/locales/*.json`).
//!
//! Listés via `GET /api/packs`, servis via `/api/packs/theme/{file}` et
//! `/api/packs/locale/{file}` (noms de fichiers assainis : pas de `..` ni de
//! séparateur de chemin).

use std::path::PathBuf;

use axum::{
    extract::{Path as AxumPath, State},
    http::{header, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use serde::Serialize;

use crate::{ApiResult, AppState, UiAssets};

/// Entrée de pack exposé à l'UI.
#[derive(Debug, Serialize)]
pub struct PackEntry {
    pub id: String,
    pub name: String,
    pub builtin: bool,
}

/// Réponse de `GET /api/packs`.
#[derive(Debug, Serialize)]
pub struct Packs {
    pub themes: Vec<PackEntry>,
    pub locales: Vec<PackEntry>,
}

/// Thèmes intégrés réservés (gérés par `data-theme`, pas par un fichier CSS).
const RESERVED_THEMES: &[(&str, &str)] = &[
    ("dark", "Sombre"),
    ("light", "Clair"),
    ("system", "Système"),
];

/// Langues embarquées dans l'UI (les packs de langue embarqués sont servis
/// depuis `ui/dist/locales/`).
const BUILTIN_LOCALES: &[(&str, &str)] = &[("fr", "Français"), ("en", "English")];

/// Assainit un nom de fichier de pack : `[a-z0-9-]` (+ majuscules si `upper`,
/// pour les codes langue type `pt-BR`) + extension, sans séparateur de chemin.
fn safe_stem(file: &str, ext: &str, upper: bool) -> Option<String> {
    let stem = file.strip_suffix(ext)?;
    if stem.is_empty()
        || stem.len() > 64
        || !stem.chars().all(|c| {
            c.is_ascii_lowercase()
                || c.is_ascii_digit()
                || c == '-'
                || (upper && c.is_ascii_uppercase())
        })
    {
        return None;
    }
    Some(stem.to_string())
}

fn list_custom(dir: PathBuf, ext: &str, upper: bool) -> Vec<(String, String)> {
    let mut out = Vec::new();
    if let Ok(rd) = std::fs::read_dir(&dir) {
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            if let Some(stem) = safe_stem(&name, ext, upper) {
                out.push((stem, name));
            }
        }
    }
    out.sort();
    out
}

/// `GET /api/packs` — liste les thèmes et langues disponibles (intégrés + communautaires).
pub async fn get_packs(State(s): State<AppState>) -> Json<Packs> {
    let themes_dir = s.data_dir.join("themes");
    let locales_dir = s.data_dir.join("locales");
    let (custom_themes, custom_locales) = crate::offload_io(move || {
        Ok::<_, auris_core::AurisError>((
            list_custom(themes_dir, ".css", false),
            list_custom(locales_dir, ".json", true),
        ))
    })
    .await
    .unwrap_or_default();

    // packs de thèmes embarqués dans l'UI (ui/dist/themes/*.css)
    let mut builtin_theme_files: Vec<String> = Vec::new();
    for f in UiAssets::iter() {
        if let Some(rest) = f.strip_prefix("themes/") {
            if let Some(stem) = safe_stem(rest, ".css", false) {
                builtin_theme_files.push(stem);
            }
        }
    }
    builtin_theme_files.sort();

    let mut themes: Vec<PackEntry> = RESERVED_THEMES
        .iter()
        .map(|(id, name)| PackEntry {
            id: id.to_string(),
            name: name.to_string(),
            builtin: true,
        })
        .collect();
    for stem in builtin_theme_files {
        themes.push(PackEntry {
            id: stem.clone(),
            name: stem,
            builtin: true,
        });
    }
    for (stem, _file) in custom_themes {
        themes.push(PackEntry {
            id: stem.clone(),
            name: stem,
            builtin: false,
        });
    }

    let mut locales: Vec<PackEntry> = BUILTIN_LOCALES
        .iter()
        .map(|(id, name)| PackEntry {
            id: id.to_string(),
            name: name.to_string(),
            builtin: true,
        })
        .collect();
    for (stem, _file) in custom_locales {
        locales.push(PackEntry {
            id: stem.clone(),
            name: stem,
            builtin: false,
        });
    }

    Json(Packs { themes, locales })
}

/// Sert un fichier de pack : d'abord `<data-dir>/<kind>/`, puis embarqué dans l'UI.
fn serve_pack(
    data_dir: &std::path::Path,
    kind: &str,
    file: &str,
    ext: &str,
    upper: bool,
    mime: &str,
) -> Response {
    if safe_stem(file, ext, upper).is_none() {
        return (StatusCode::BAD_REQUEST, "nom de pack invalide").into_response();
    }
    // pack communautaire du data-dir
    let p = data_dir.join(kind).join(file);
    if let Ok(bytes) = std::fs::read(&p) {
        return (StatusCode::OK, [(header::CONTENT_TYPE, mime)], bytes).into_response();
    }
    // pack embarqué dans le binaire (ui/dist/<kind>/<file>)
    let embedded = format!("{kind}/{file}");
    if let Some(f) = UiAssets::get(&embedded) {
        return (
            StatusCode::OK,
            [(header::CONTENT_TYPE, mime)],
            f.data.into_owned(),
        )
            .into_response();
    }
    (StatusCode::NOT_FOUND, "pack introuvable").into_response()
}

/// `GET /api/packs/theme/{file}.css` — thème CSS (communautaire ou embarqué).
pub async fn get_theme_css(
    State(s): State<AppState>,
    AxumPath(file): AxumPath<String>,
) -> Response {
    let file = if file.ends_with(".css") {
        file
    } else {
        format!("{file}.css")
    };
    serve_pack(
        &s.data_dir,
        "themes",
        &file,
        ".css",
        false,
        "text/css; charset=utf-8",
    )
}

/// `GET /api/packs/locale/{file}.json` — pack de langue JSON (communautaire ou embarqué).
pub async fn get_locale_json(
    State(s): State<AppState>,
    AxumPath(file): AxumPath<String>,
) -> Response {
    let file = if file.ends_with(".json") {
        file
    } else {
        format!("{file}.json")
    };
    serve_pack(
        &s.data_dir,
        "locales",
        &file,
        ".json",
        true,
        "application/json; charset=utf-8",
    )
}

// ---------- Création / suppression de packs communautaires (#41) ----------

/// Taille max d'un pack déposé : 256 Ko.
const MAX_PACK_BYTES: usize = 256 * 1024;

/// Valide un id de pack communautaire : `[a-z0-9-]` (majuscules autorisées pour
/// les codes langue), 1..=64 caractères, et pas un id réservé/embarqué.
fn valid_custom_id(id: &str, upper: bool, reserved: &[&str], builtin: &[String]) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id.chars().all(|c| {
            c.is_ascii_lowercase()
                || c.is_ascii_digit()
                || c == '-'
                || (upper && c.is_ascii_uppercase())
        })
        && !reserved.contains(&id)
        && !builtin.iter().any(|b| b == id)
}

/// Ids des thèmes embarqués dans l'UI (ui/dist/themes/*.css).
fn builtin_theme_ids() -> Vec<String> {
    UiAssets::iter()
        .filter_map(|f| {
            f.strip_prefix("themes/")
                .and_then(|r| safe_stem(r, ".css", false))
        })
        .collect()
}

/// `POST /api/packs/theme/{id}` — enregistre un thème CSS communautaire
/// (`<data-dir>/themes/{id}.css`). Body = CSS brut.
pub async fn post_theme(
    State(s): State<AppState>,
    AxumPath(id): AxumPath<String>,
    body: String,
) -> ApiResult<Json<PackEntry>> {
    let reserved: Vec<&str> = RESERVED_THEMES.iter().map(|(id, _)| *id).collect();
    if !valid_custom_id(&id, false, &reserved, &builtin_theme_ids()) {
        return Err(auris_core::AurisError::InvalidUri(format!(
            "id de thème invalide (attendu [a-z0-9-], non réservé) : {id}"
        ))
        .into());
    }
    if body.len() > MAX_PACK_BYTES {
        return Err(auris_core::AurisError::other(format!(
            "thème trop volumineux ({} o > {MAX_PACK_BYTES} o)",
            body.len()
        ))
        .into());
    }
    let dir = s.data_dir.join("themes");
    let file = format!("{id}.css");
    let name = file.clone();
    crate::offload_io(move || {
        std::fs::create_dir_all(&dir).map_err(|e| {
            auris_core::AurisError::other(format!("création de {}: {e}", dir.display()))
        })?;
        std::fs::write(dir.join(&file), &body)
            .map_err(|e| auris_core::AurisError::other(format!("écriture du thème {file}: {e}")))?;
        Ok(())
    })
    .await?;
    Ok(Json(PackEntry {
        id,
        name,
        builtin: false,
    }))
}

/// `DELETE /api/packs/theme/{id}` — supprime un thème communautaire
/// (les thèmes réservés/embarqués sont protégés).
pub async fn delete_theme(
    State(s): State<AppState>,
    AxumPath(id): AxumPath<String>,
) -> ApiResult<StatusCode> {
    let reserved: Vec<&str> = RESERVED_THEMES.iter().map(|(id, _)| *id).collect();
    if !valid_custom_id(&id, false, &reserved, &builtin_theme_ids()) {
        return Err(auris_core::AurisError::InvalidUri(format!(
            "id de thème invalide ou protégé : {id}"
        ))
        .into());
    }
    let dir = s.data_dir.join("themes");
    let file = format!("{id}.css");
    let existed = crate::offload_io(move || {
        let p = dir.join(&file);
        match std::fs::remove_file(&p) {
            Ok(()) => Ok(true),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(e) => Err(auris_core::AurisError::other(format!(
                "suppression de {}: {e}",
                p.display()
            ))),
        }
    })
    .await?;
    if existed {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(auris_core::AurisError::other(format!("thème communautaire introuvable : {id}")).into())
    }
}

/// `POST /api/packs/locale/{lang}` — enregistre un pack de langue communautaire
/// (`<data-dir>/locales/{lang}.json`). Body = JSON `{ "clé FR": "traduction" }`.
pub async fn post_locale(
    State(s): State<AppState>,
    AxumPath(lang): AxumPath<String>,
    body: String,
) -> ApiResult<Json<PackEntry>> {
    let builtin: Vec<String> = BUILTIN_LOCALES
        .iter()
        .map(|(id, _)| id.to_string())
        .collect();
    if !valid_custom_id(&lang, true, &[], &builtin) {
        return Err(auris_core::AurisError::InvalidUri(format!(
            "code langue invalide (attendu ex. en, pt-BR, non intégré) : {lang}"
        ))
        .into());
    }
    if body.len() > MAX_PACK_BYTES {
        return Err(auris_core::AurisError::other(format!(
            "pack de langue trop volumineux ({} o > {MAX_PACK_BYTES} o)",
            body.len()
        ))
        .into());
    }
    // validation : le body doit être un objet JSON string -> string
    let parsed: std::collections::BTreeMap<String, String> =
        serde_json::from_str(&body).map_err(|e| {
            auris_core::AurisError::InvalidUri(format!(
                "pack de langue invalide (JSON objet {{\"clé\": \"traduction\"}} attendu) : {e}"
            ))
        })?;
    if parsed.is_empty() {
        return Err(auris_core::AurisError::InvalidUri("pack de langue vide".into()).into());
    }
    let dir = s.data_dir.join("locales");
    let file = format!("{lang}.json");
    let name = file.clone();
    let pretty = serde_json::to_string_pretty(&parsed).unwrap_or(body);
    crate::offload_io(move || {
        std::fs::create_dir_all(&dir).map_err(|e| {
            auris_core::AurisError::other(format!("création de {}: {e}", dir.display()))
        })?;
        std::fs::write(dir.join(&file), pretty)
            .map_err(|e| auris_core::AurisError::other(format!("écriture de {file}: {e}")))?;
        Ok(())
    })
    .await?;
    Ok(Json(PackEntry {
        id: lang,
        name,
        builtin: false,
    }))
}

/// `DELETE /api/packs/locale/{lang}` — supprime un pack de langue communautaire
/// (les langues embarquées fr/en sont protégées).
pub async fn delete_locale(
    State(s): State<AppState>,
    AxumPath(lang): AxumPath<String>,
) -> ApiResult<StatusCode> {
    let builtin: Vec<String> = BUILTIN_LOCALES
        .iter()
        .map(|(id, _)| id.to_string())
        .collect();
    if !valid_custom_id(&lang, true, &[], &builtin) {
        return Err(auris_core::AurisError::InvalidUri(format!(
            "code langue invalide ou protégé : {lang}"
        ))
        .into());
    }
    let dir = s.data_dir.join("locales");
    let file = format!("{lang}.json");
    let existed = crate::offload_io(move || {
        let p = dir.join(&file);
        match std::fs::remove_file(&p) {
            Ok(()) => Ok(true),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(e) => Err(auris_core::AurisError::other(format!(
                "suppression de {}: {e}",
                p.display()
            ))),
        }
    })
    .await?;
    if existed {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(auris_core::AurisError::other(format!("pack de langue introuvable : {lang}")).into())
    }
}
