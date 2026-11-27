//! Tests du routage d'URI des sources (auris-sources).

use auris_sources::{
    http::is_playlist_url, http_raw::Url, is_http_uri, is_src_uri, make_uri, parse_src_uri,
    resolve_uri, UriRef,
};

#[test]
fn src_uri_roundtrip() {
    let uri = make_uri("nas", "/Rock/Album/track.flac");
    assert_eq!(uri, "src://nas/Rock/Album/track.flac");
    let (id, path) = parse_src_uri(&uri).unwrap();
    assert_eq!(id, "nas");
    assert_eq!(path, "Rock/Album/track.flac");
}

#[test]
fn src_uri_invalides() {
    assert!(parse_src_uri("src://").is_none());
    // `src://onlyid` (sans slash) = racine de la source, chemin vide — valide.
    let (id, path) = parse_src_uri("src://onlyid").unwrap();
    assert_eq!(id, "onlyid");
    assert_eq!(path, "");
    assert!(parse_src_uri("http://x/y.flac").is_none());
    assert!(parse_src_uri("file:///tmp/x").is_none());
}

#[test]
fn predicates_uri() {
    assert!(is_src_uri("src://a/b"));
    assert!(!is_src_uri("https://a/b"));
    assert!(is_http_uri("http://radio.fr/stream"));
    assert!(is_http_uri("https://nas.local:5005/x.flac"));
    assert!(!is_http_uri("src://a/b"));
}

#[test]
fn resolve_uri_dispatch() {
    match resolve_uri("src://nas/album/t.flac").unwrap() {
        UriRef::Src { source_id, path } => {
            assert_eq!(source_id, "nas");
            assert_eq!(path, "album/t.flac");
        }
        _ => panic!("attendu UriRef::Src"),
    }
    match resolve_uri("https://example.com/song.flac").unwrap() {
        UriRef::Http(u) => assert_eq!(u, "https://example.com/song.flac"),
        _ => panic!("attendu UriRef::Http"),
    }
    assert!(resolve_uri("ftp://x").is_err());
    assert!(resolve_uri("nimportequoi").is_err());
}

#[test]
fn url_parse_complet() {
    // mini client HTTP synchrone: http:// + port explicite, query conservee dans le path
    let u = Url::parse("http://nas.local:5005/music/a%20b.flac?token=1").unwrap();
    assert_eq!(u.host, "nas.local");
    assert_eq!(u.port, 5005);
    assert_eq!(u.path, "/music/a%20b.flac?token=1");
}

#[test]
fn url_parse_defauts() {
    let u = Url::parse("http://example.com").unwrap();
    assert_eq!(u.host, "example.com");
    assert_eq!(u.port, 80);
    assert_eq!(u.path, "/");
}

#[test]
fn url_parse_invalide() {
    assert!(Url::parse("pas une url").is_err());
    assert!(Url::parse("ftp://x/y").is_err());
    assert!(Url::parse("https://x/y").is_err()); // le mini client est http:// uniquement (https via reqwest)
    assert!(Url::parse("http://host:abc/x").is_err()); // port non numerique
}

#[test]
fn playlists_reconnues() {
    assert!(is_playlist_url("http://radio.fr:8000/stream.m3u"));
    assert!(is_playlist_url("https://x/y.M3U8"));
    assert!(!is_playlist_url("https://x/y.flac"));
    assert!(!is_playlist_url("https://x/y.pls")); // seul .m3u est detecte pour l'instant
}

#[test]
fn upnp_tag_content() {
    use auris_sources::upnp::tag_content;
    let xml = r#"<root><d:title>Ma piste</d:title><name>autre</name><empty></empty></root>"#;
    assert_eq!(tag_content(xml, "title").as_deref(), Some("Ma piste"));
    assert_eq!(tag_content(xml, "name").as_deref(), Some("autre"));
    assert!(tag_content(xml, "absent").is_none());
}

#[test]
fn upnp_uri_parse() {
    use auris_sources::upnp::parse_upnp_uri;
    // location sans scheme: split au premier '/'
    assert_eq!(
        parse_upnp_uri("upnp://192.168.1.5:9000/0/1/5"),
        Some(("192.168.1.5:9000".to_string(), "0/1/5".to_string()))
    );
    // location avec scheme: on ne coupe pas dans "http://"
    assert_eq!(
        parse_upnp_uri("upnp://http://192.168.1.5:9000/desc.xml"),
        Some((
            "http://192.168.1.5:9000".to_string(),
            "desc.xml".to_string()
        ))
    );
    assert!(parse_upnp_uri("src://a/b").is_none());
    assert!(parse_upnp_uri("upnp://sans-id").is_none());
}
