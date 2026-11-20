//! Image transport is available only through an explicit reader action. The HTML
//! frame itself has no network or IPC access.
use crate::{engine, providers::db, Store};
use base64::{engine::general_purpose::STANDARD, Engine};
use rusqlite::OptionalExtension;
use serde::Serialize;
use std::{
    net::{IpAddr, SocketAddr},
    time::Duration,
};

const MAX_IMAGE: usize = 1024 * 1024;
const MAX_TOTAL: usize = 4 * 1024 * 1024;
pub const MAX_IMAGES: usize = 12;
#[derive(Serialize)]
pub struct ImageResult {
    source: String,
    data_url: Option<String>,
}

fn public_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => {
            let [a, b, c, _] = ip.octets();
            !(a == 0
                || a == 10
                || a == 127
                || a >= 224
                || (a == 100 && (64..=127).contains(&b))
                || (a == 169 && b == 254)
                || (a == 172 && (16..=31).contains(&b))
                || (a == 192 && (b == 168 || b == 0 || (b == 88 && c == 99)))
                || (a == 198 && (b == 18 || b == 19 || (b == 51 && c == 100)))
                || (a == 203 && b == 0 && c == 113))
        }
        IpAddr::V6(ip) => {
            let s = ip.segments();
            (s[0] & 0xe000) == 0x2000
                && s[0] != 0x2002
                && !(s[0] == 0x2001 && (s[1] < 0x0200 || s[1] == 0x0db8))
                && !(s[0] == 0x3fff && s[1] < 0x1000)
        }
    }
}
fn remote_url(source: &str) -> Result<reqwest::Url, String> {
    if source.len() > 4096 || source.chars().any(char::is_control) {
        return Err("Invalid image URL".into());
    }
    let u = reqwest::Url::parse(source).map_err(|_| "Invalid image URL")?;
    if u.scheme() != "https"
        || !u.username().is_empty()
        || u.password().is_some()
        || u.port_or_known_default() != Some(443)
    {
        return Err("Only public HTTPS images are supported".into());
    }
    let host = u
        .host_str()
        .ok_or("Missing image host")?
        .trim_end_matches('.')
        .to_ascii_lowercase();
    if host == "localhost"
        || [
            ".localhost",
            ".local",
            ".internal",
            ".test",
            ".invalid",
            ".onion",
        ]
        .iter()
        .any(|s| host.ends_with(s))
    {
        return Err("Local image hosts are blocked".into());
    }
    if let Ok(ip) = host.trim_matches(['[', ']']).parse::<IpAddr>() {
        if !public_ip(ip) {
            return Err("Private image addresses are blocked".into());
        }
    }
    Ok(u)
}
fn image_data(bytes: &[u8]) -> Result<String, String> {
    if bytes.len() > MAX_IMAGE {
        return Err("Image exceeds size limit".into());
    }
    let mime = if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        "image/png"
    } else if bytes.starts_with(b"\xff\xd8\xff") {
        "image/jpeg"
    } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        "image/gif"
    } else if bytes.starts_with(b"RIFF") && bytes.get(8..12) == Some(b"WEBP") {
        "image/webp"
    } else {
        return Err("Unsupported image format".into());
    };
    let size = imagesize::blob_size(bytes).map_err(|_| "Invalid image")?;
    if size.width == 0
        || size.height == 0
        || size.width > 8192
        || size.height > 8192
        || size.width.saturating_mul(size.height) > 20_000_000
    {
        return Err("Image dimensions exceed limits".into());
    }
    Ok(format!("data:{mime};base64,{}", STANDARD.encode(bytes)))
}
async fn download(source: &str) -> Result<Vec<u8>, String> {
    let url = remote_url(source)?;
    let host = url
        .host_str()
        .ok_or("Missing image host")?
        .trim_matches(['[', ']']);
    let addresses: Vec<SocketAddr> =
        tokio::time::timeout(Duration::from_secs(3), tokio::net::lookup_host((host, 443)))
            .await
            .map_err(|_| "Image DNS lookup timed out")?
            .map_err(|_| "Image host unavailable")?
            .collect();
    if addresses.is_empty() || addresses.iter().any(|a| !public_ip(a.ip())) {
        return Err("Private image addresses are blocked".into());
    }
    // Pin the checked addresses to this request, avoiding DNS rebinding and proxies.
    let client = reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(Duration::from_secs(3))
        .timeout(Duration::from_secs(6))
        .resolve_to_addrs(host, &addresses)
        .build()
        .map_err(|_| "Image connection unavailable")?;
    let mut response = client
        .get(url)
        .header("Accept", "image/png,image/jpeg,image/gif,image/webp")
        .send()
        .await
        .map_err(|_| "Image download failed")?;
    if !response.status().is_success() {
        return Err("Image response rejected".into());
    }
    if response
        .content_length()
        .is_some_and(|n| n > MAX_IMAGE as u64)
    {
        return Err("Image exceeds size limit".into());
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| "Image download interrupted")?
    {
        if bytes.len() + chunk.len() > MAX_IMAGE {
            return Err("Image exceeds size limit".into());
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}
fn local_image(source: &str, parsed: Option<&mail_parser::Message<'_>>) -> Result<Vec<u8>, String> {
    if let Some(cid) = source.strip_prefix("cid:") {
        let cid = percent_encoding::percent_decode_str(cid)
            .decode_utf8()
            .map_err(|_| "Invalid content ID")?;
        let parsed = parsed.ok_or("Embedded image is no longer cached")?;
        let mut matches = parsed.attachments().filter(|p| {
            p.content_id()
                .is_some_and(|id| id.trim_matches(['<', '>']) == cid.trim_matches(['<', '>']))
        });
        let part = matches.next().ok_or("Embedded image not found")?;
        if matches.next().is_some() {
            return Err("Ambiguous embedded image".into());
        }
        let bytes = part.decoded();
        if bytes.len() > MAX_IMAGE {
            return Err("Image exceeds size limit".into());
        }
        return Ok(bytes.into_owned());
    }
    let (header, data) = source.split_once(',').ok_or("Invalid image source")?;
    if !matches!(
        header,
        "data:image/png;base64"
            | "data:image/jpeg;base64"
            | "data:image/gif;base64"
            | "data:image/webp;base64"
    ) {
        return Err("Unsupported image source".into());
    }
    STANDARD
        .decode(data)
        .map_err(|_| "Invalid image encoding".into())
}
pub async fn load_images_inner(
    store: &Store,
    id: &str,
    sources: Vec<String>,
) -> Result<Vec<ImageResult>, String> {
    if sources.len() > MAX_IMAGES
        || sources.iter().map(String::len).sum::<usize>() > 2 * engine::MAX_HTML
    {
        return Err("Too many images or image sources".into());
    }
    let raw = db(store, |d| {
        let m = engine::get(d, id)?;
        if !m.loaded || m.html_body.is_empty() {
            return Err("Open the formatted message before loading images".into());
        }
        if sources.iter().any(|s| s.starts_with("cid:")) {
            d.query_row("SELECT raw FROM mail_raw WHERE id=?1", [id], |r| {
                r.get::<_, Vec<u8>>(0)
            })
            .optional()
            .map_err(|e| e.to_string())
        } else {
            Ok(None)
        }
    })?;
    let work = async {
        let parsed = raw
            .as_deref()
            .and_then(|raw| mail_parser::MessageParser::default().parse(raw));
        let mut results = Vec::new();
        let mut total = 0;
        for source in sources {
            let bytes = if source.starts_with("https:") {
                download(&source).await
            } else {
                local_image(&source, parsed.as_ref())
            };
            let data_url = match bytes {
                Ok(bytes) if total + bytes.len() <= MAX_TOTAL => {
                    let data = image_data(&bytes).ok();
                    if data.is_some() {
                        total += bytes.len();
                    }
                    data
                }
                _ => None,
            };
            results.push(ImageResult { source, data_url });
        }
        results
    };
    tokio::time::timeout(Duration::from_secs(25), work)
        .await
        .map_err(|_| "Images took too long to load; try again".into())
}
#[tauri::command]
pub async fn load_message_images(
    store: tauri::State<'_, Store>,
    id: String,
    sources: Vec<String>,
) -> Result<Vec<ImageResult>, String> {
    load_images_inner(&store, &id, sources).await
}
fn message_link(url: &str) -> Result<reqwest::Url, String> {
    if url.len() > 4096
        || url.chars().any(char::is_control)
        || url.to_ascii_lowercase().contains("%0d")
        || url.to_ascii_lowercase().contains("%0a")
    {
        return Err("Invalid message link".into());
    }
    let u = reqwest::Url::parse(url).map_err(|_| "Invalid message link")?;
    if !u.username().is_empty() || u.password().is_some() {
        return Err("Links containing credentials are blocked".into());
    }
    match u.scheme() {
        "https" | "http" if u.host_str().is_some() => Ok(u),
        "mailto"
            if u.query_pairs()
                .all(|(k, _)| matches!(k.as_ref(), "subject" | "body" | "cc" | "bcc")) =>
        {
            Ok(u)
        }
        _ => Err("This link type is blocked".into()),
    }
}
#[tauri::command]
pub fn open_message_link(url: String) -> Result<(), String> {
    let url = message_link(&url)?;
    open::that(url.as_str()).map_err(|_| "Cannot open message link".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::{self, Mail};
    use std::sync::{Arc, Mutex};
    const PNG: &str="iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+a1XcAAAAASUVORK5CYII=";
    fn store() -> Store {
        let d = rusqlite::Connection::open_in_memory().unwrap();
        engine::migrate(&d).unwrap();
        Store(Arc::new(Mutex::new(d)))
    }
    fn mail(id: &str) -> Mail {
        Mail {
            id: id.into(),
            account_id: "a".into(),
            remote_id: id.into(),
            folder: "inbox".into(),
            ..Default::default()
        }
    }
    fn multipart() -> Vec<u8> {
        format!("MIME-Version: 1.0\r\nContent-Type: multipart/related; boundary=x\r\n\r\n--x\r\nContent-Type: text/html; charset=utf-8\r\n\r\n<h1>Welcome</h1><p>Hello <b>reader</b></p><img src=\"cid:logo\"><img src=\"https://example.com/pixel.png\">\r\n--x\r\nContent-Type: image/png\r\nContent-ID: <logo>\r\nContent-Transfer-Encoding: base64\r\n\r\n{PNG}\r\n--x--\r\n").into_bytes()
    }
    #[test]
    fn mime_html_survives_sync_and_stays_out_of_list_summaries() {
        let store = store();
        let d = store.0.lock().unwrap();
        let m = engine::parse_raw(&d, mail("html"), &multipart()).unwrap();
        assert!(m.html_body.contains("<h1>Welcome</h1>"));
        assert!(m.body.contains("Welcome"));
        assert!(!m.body.contains("<h1>"));
        assert_eq!(m.body_format_version, engine::BODY_FORMAT_VERSION);
        let mut metadata = mail("html");
        metadata.subject = "Updated subject".into();
        engine::put(&d, &metadata).unwrap();
        assert_eq!(engine::get(&d, "html").unwrap().html_body, m.html_body);
        assert!(engine::query_conversations(&d, "all", "", "", 0, None)
            .unwrap()
            .messages[0]
            .html_body
            .is_empty());
        assert!(engine::query(&d, "all", "", "", 0).unwrap().messages[0]
            .html_body
            .is_empty());
        assert!(engine::conversation(&d, "html").unwrap()[0]
            .html_body
            .is_empty());
        engine::trim_cache(&d, 0, "different").unwrap();
        let evicted = engine::get(&d, "html").unwrap();
        assert!(!evicted.loaded);
        assert!(evicted.html_body.is_empty());
    }
    #[tokio::test]
    async fn old_cached_html_is_upgraded_without_network_or_sync_gate() {
        let store = store();
        {
            let d = store.0.lock().unwrap();
            engine::parse_raw(&d, mail("old"), &multipart()).unwrap();
            d.execute("UPDATE mail SET payload=json_remove(payload,'$.html_body','$.body_format_version') WHERE id='old'",[]).unwrap();
        }
        let runtime = crate::live::Runtime::default();
        let _busy = runtime.gate.lock().await;
        let m = tokio::time::timeout(
            Duration::from_secs(1),
            crate::live::get_mail_inner(&store, &runtime, "old"),
        )
        .await
        .unwrap()
        .unwrap();
        assert!(m.html_body.contains("Welcome"));
        assert_eq!(m.body_format_version, engine::BODY_FORMAT_VERSION);
    }
    #[test]
    fn plain_text_and_oversized_html_keep_readable_text_fallbacks() {
        let store = store();
        let d = store.0.lock().unwrap();
        let m = engine::parse_raw(
            &d,
            mail("text"),
            b"Content-Type: text/plain\r\n\r\nHello <b>literal</b>",
        )
        .unwrap();
        assert!(m.html_body.is_empty());
        assert!(m.body.contains("<b>literal</b>"));
        let raw = format!(
            "Content-Type: text/html\r\n\r\n<p>{}</p>",
            "x".repeat(engine::MAX_HTML)
        );
        let m = engine::parse_raw(&d, mail("large"), raw.as_bytes()).unwrap();
        assert!(m.html_body.is_empty());
        assert!(!m.body.is_empty());
    }
    #[test]
    fn image_urls_cannot_target_private_or_non_https_endpoints() {
        for source in [
            "http://example.com/a.png",
            "file:///C:/secret",
            "data:image/png;base64,x",
            "https://u:p@example.com/x",
            "https://example.com:8443/x",
            "https://localhost/x",
            "https://LOCALHOST./x",
            "https://test.local/x",
            "https://127.0.0.1/x",
            "https://2130706433/x",
            "https://0x7f000001/x",
            "https://10.0.0.1/x",
            "https://192.168.1.1/x",
            "https://169.254.169.254/latest",
            "https://[::1]/x",
            "https://[::ffff:127.0.0.1]/x",
        ] {
            assert!(remote_url(source).is_err(), "{source}");
        }
        for ip in [
            "0.0.0.0",
            "100.64.0.1",
            "172.16.0.1",
            "192.0.2.1",
            "198.18.0.1",
            "198.51.100.1",
            "203.0.113.1",
            "224.0.0.1",
            "255.255.255.255",
            "fc00::1",
            "fe80::1",
            "2001:db8::1",
            "2002:7f00:1::1",
            "3fff::1",
        ] {
            assert!(!public_ip(ip.parse().unwrap()), "{ip}")
        }
        for ip in ["8.8.8.8", "1.1.1.1", "2606:4700:4700::1111"] {
            assert!(public_ip(ip.parse().unwrap()), "{ip}")
        }
        assert!(remote_url("https://images.example.com/photo.png?token=123").is_ok());
    }
    #[test]
    fn image_bytes_are_raster_and_dimension_bounded() {
        let png = STANDARD.decode(PNG).unwrap();
        assert!(image_data(&png)
            .unwrap()
            .starts_with("data:image/png;base64,"));
        for bytes in [
            b"<svg onload='alert(1)'/>".as_slice(),
            b"<html>not an image</html>",
            b"GIF89a",
        ] {
            assert!(image_data(bytes).is_err())
        }
        assert!(image_data(&vec![0; MAX_IMAGE + 1]).is_err());
        let mut huge = png.clone();
        huge[16..20].copy_from_slice(&100_000_u32.to_be_bytes());
        assert!(image_data(&huge).is_err());
    }
    #[tokio::test]
    async fn approved_images_are_scoped_to_the_selected_cached_message() {
        let store = store();
        {
            let d = store.0.lock().unwrap();
            engine::parse_raw(&d, mail("one"), &multipart()).unwrap();
            engine::parse_raw(
                &d,
                mail("two"),
                b"Content-Type: text/html\r\n\r\n<p>No embedded image</p>",
            )
            .unwrap();
        }
        let first = load_images_inner(&store, "one", vec!["cid:logo".into()])
            .await
            .unwrap();
        assert!(first[0].data_url.is_some());
        let other = load_images_inner(&store, "two", vec!["cid:logo".into()])
            .await
            .unwrap();
        assert!(other[0].data_url.is_none());
        assert!(load_images_inner(&store, "absent", vec![]).await.is_err());
        assert!(
            load_images_inner(&store, "one", vec!["cid:logo".into(); MAX_IMAGES + 1])
                .await
                .is_err()
        );
        let bad = load_images_inner(
            &store,
            "one",
            vec![
                "data:image/svg+xml;base64,PHN2Zy8+".into(),
                "https://127.0.0.1/tracker".into(),
            ],
        )
        .await
        .unwrap();
        assert!(bad.iter().all(|r| r.data_url.is_none()));
    }
    #[tokio::test]
    async fn private_image_url_opens_no_connection() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        assert!(download(&format!(
            "https://127.0.0.1:{}/x",
            listener.local_addr().unwrap().port()
        ))
        .await
        .is_err());
        assert!(
            tokio::time::timeout(Duration::from_millis(100), listener.accept())
                .await
                .is_err()
        );
    }
    #[test]
    fn message_links_reject_handlers_credentials_and_header_injection() {
        for url in [
            "javascript:alert(1)",
            "file:///C:/secret",
            "data:text/html,hello",
            "tauri://localhost",
            "https://u:p@example.com",
            "mailto:a@example.com?attach=C:/secret",
            "mailto:a@example.com%0abcc:b@example.com",
        ] {
            assert!(message_link(url).is_err(), "{url}")
        }
        for url in [
            "https://example.com/path",
            "http://example.com/path",
            "mailto:a@example.com?subject=Hello",
        ] {
            assert!(message_link(url).is_ok(), "{url}")
        }
    }
}
