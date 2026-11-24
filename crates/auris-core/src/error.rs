use thiserror::Error;

#[derive(Debug, Error)]
pub enum AurisError {
    #[error("source introuvable: {0}")]
    SourceNotFound(String),
    #[error("URI invalide: {0}")]
    InvalidUri(String),
    #[error("format audio non supporte: {0}")]
    UnsupportedFormat(String),
    #[error("erreur de decodage: {0}")]
    Decode(String),
    #[error("erreur de sortie audio: {0}")]
    Output(String),
    #[error("aucun peripherique de sortie audio disponible")]
    NoOutputDevice,
    #[error("peripherique audio introuvable: {0}")]
    DeviceNotFound(String),
    #[error("piste introuvable: {0}")]
    TrackNotFound(String),
    #[error("album introuvable: {0}")]
    AlbumNotFound(String),
    #[error("artiste introuvable: {0}")]
    ArtistNotFound(String),
    #[error("erreur reseau: {0}")]
    Network(String),
    #[error("erreur de stockage: {0}")]
    Storage(String),
    #[error("erreur de mise a jour: {0}")]
    Update(String),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("json: {0}")]
    Json(#[from] serde_json::Error),
    #[error("autre: {0}")]
    Other(String),
}

pub type Result<T> = std::result::Result<T, AurisError>;

impl AurisError {
    pub fn other(msg: impl Into<String>) -> Self {
        AurisError::Other(msg.into())
    }
}
