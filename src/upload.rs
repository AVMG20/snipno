//! `snipno upload <file.png> [--labels "a,b"]`: send a screenshot to the uploader configured in
//! Settings and put the returned link on the clipboard. The overlay starts this as a detached
//! process, so the capture windows close right away while the upload runs.
//!
//! The uploader is ShareX-style and works with most self-hosted image hosts: a multipart POST
//! with the image in the `file` field, an optional bearer token, labels as `tags`, any extra form
//! fields, and a link template that picks the link out of the JSON response.
//!
//! The request itself is made by macOS's own `/usr/bin/curl`, fed its options on stdin so the
//! token never shows up in the process list.

use crate::config::{self, Upload};
use serde_json::Value;
use std::{
    error::Error,
    fs,
    io::Write,
    path::Path,
    process::{Command, Stdio},
};

pub fn run(file: &Path, labels: Option<String>) -> Result<(), Box<dyn Error>> {
    let cfg = config::load().upload;
    let labels = labels.unwrap_or_else(|| cfg.labels.clone());
    let result = send(&cfg, file, &labels);
    let _ = fs::remove_file(file);
    match result {
        Ok(link) => {
            arboard::Clipboard::new()?.set_text(link.clone())?;
            notify("Link copied", &link);
            Ok(())
        }
        Err(e) => {
            notify("Upload failed", &e);
            Err(e.into())
        }
    }
}

/// The name the image is uploaded as: the template with `{date}` (2026-11-12), `{time}`
/// (14-05-09) and `{timestamp}` (Unix seconds) filled in, made safe for a file name, plus `.png`.
pub fn file_name(template: &str, now: chrono::DateTime<chrono::Local>) -> String {
    let template = template.trim();
    let template = if template.is_empty() { "{date}-{time}" } else { template };
    let name = template
        .replace("{date}", &now.format("%Y-%m-%d").to_string())
        .replace("{time}", &now.format("%H-%M-%S").to_string())
        .replace("{timestamp}", &now.timestamp().to_string());
    // `/` and `:` mean folders to some servers; quotes and `;` would break the form header.
    let name: String = name
        .chars()
        .map(|c| if c.is_control() || "/\\:\";".contains(c) { '-' } else { c })
        .collect();
    let name = name.trim().trim_end_matches(".png").trim();
    format!("{}.png", if name.is_empty() { "screenshot" } else { name })
}

fn send(cfg: &Upload, file: &Path, labels: &str) -> Result<String, String> {
    if !cfg.enabled() {
        return Err("no upload URL set (menu bar icon → Settings → Upload)".into());
    }
    let mut child = Command::new("/usr/bin/curl")
        .args(["--silent", "--show-error", "--fail-with-body", "--max-time", "60", "--config", "-"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("could not run curl: {e}"))?;
    child
        .stdin
        .take()
        .expect("stdin is piped")
        .write_all(curl_config(cfg, file, labels, &file_name(&cfg.name, chrono::Local::now())).as_bytes())
        .map_err(|e| format!("could not run curl: {e}"))?;
    let out = child.wait_with_output().map_err(|e| format!("curl failed: {e}"))?;
    let body = String::from_utf8_lossy(&out.stdout);
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        let detail = [err.trim(), body.trim()].into_iter().filter(|s| !s.is_empty()).collect::<Vec<_>>().join(" ");
        return Err(shorten(detail.strip_prefix("curl: ").unwrap_or(&detail), 200));
    }
    link_from_response(&cfg.link, &body)
}

/// curl options, one per line, in `--config` syntax.
pub fn curl_config(cfg: &Upload, file: &Path, labels: &str, name: &str) -> String {
    let mut lines = vec![format!("url = {}", quote(cfg.url.trim()))];
    if !cfg.token.trim().is_empty() {
        lines.push(format!("header = {}", quote(&format!("Authorization: Bearer {}", cfg.token.trim()))));
    }
    // `form-string` sends the value as-is; with plain `form` a leading @ or < would read a file.
    for (key, value) in form_fields(&cfg.fields) {
        lines.push(format!("form-string = {}", quote(&format!("{key}={value}"))));
    }
    let labels = normalize_labels(labels);
    if !labels.is_empty() {
        lines.push(format!("form-string = {}", quote(&format!("tags={labels}"))));
    }
    lines.push(format!(
        "form = {}",
        quote(&format!("file=@\"{}\";filename=\"{name}\";type=image/png", file.display()))
    ));
    lines.join("\n") + "\n"
}

/// `key=value&key=value` → pairs, skipping blanks and entries without a key.
pub fn form_fields(s: &str) -> Vec<(String, String)> {
    s.split('&')
        .filter_map(|pair| {
            let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
            let k = k.trim();
            (!k.is_empty()).then(|| (k.to_string(), v.trim().to_string()))
        })
        .collect()
}

/// `" blog, ,ui "` → `"blog,ui"`.
pub fn normalize_labels(s: &str) -> String {
    s.split(',').map(str::trim).filter(|l| !l.is_empty()).collect::<Vec<_>>().join(",")
}

/// The link to put on the clipboard. With a template, every `{json:path}` is filled in from the
/// response and `{response}` is the raw body. Without one, the usual `url`/`link` fields are
/// tried, then a plain-text body that is itself a URL.
pub fn link_from_response(template: &str, body: &str) -> Result<String, String> {
    let template = template.trim();
    let json: Option<Value> = serde_json::from_str(body).ok();
    if template.is_empty() {
        const COMMON: [&str; 8] =
            ["url", "link", "data.url", "data.link", "image.url", "files[0].url", "images[0].url", "result.url"];
        if let Some(json) = &json
            && let Some(link) = COMMON.iter().find_map(|p| json_at(json, p).and_then(scalar))
        {
            return Ok(link);
        }
        let text = body.trim();
        if text.starts_with("http") && !text.contains(char::is_whitespace) {
            return Ok(text.to_string());
        }
        return Err(format!(
            "no link in the response; set a link template in Settings. Response: {}",
            shorten(body.trim(), 120)
        ));
    }

    let mut out = String::new();
    let mut rest = template;
    while let Some(start) = rest.find('{') {
        out.push_str(&rest[..start]);
        let Some(len) = rest[start..].find('}') else {
            out.push_str(&rest[start..]);
            rest = "";
            break;
        };
        let token = &rest[start + 1..start + len];
        if token == "response" {
            out.push_str(body.trim());
        } else if let Some(path) = token.strip_prefix("json:") {
            let json = json.as_ref().ok_or_else(|| format!("response is not JSON: {}", shorten(body.trim(), 120)))?;
            let value = json_at(json, path)
                .and_then(scalar)
                .ok_or_else(|| format!("{path} is not in the response: {}", shorten(body.trim(), 120)))?;
            out.push_str(&value);
        } else {
            out.push_str(&rest[start..=start + len]); // not ours: keep it literally
        }
        rest = &rest[start + len + 1..];
    }
    out.push_str(rest);
    Ok(out)
}

/// `images[0].slug` → `json["images"][0]["slug"]`.
fn json_at<'a>(json: &'a Value, path: &str) -> Option<&'a Value> {
    let mut cur = json;
    for part in path.split('.') {
        let (key, mut indexes) = match part.find('[') {
            Some(i) => (&part[..i], &part[i..]),
            None => (part, ""),
        };
        if !key.is_empty() {
            cur = cur.get(key)?;
        }
        while let Some(rest) = indexes.strip_prefix('[') {
            let end = rest.find(']')?;
            cur = cur.get(rest[..end].trim().parse::<usize>().ok()?)?;
            indexes = &rest[end + 1..];
        }
    }
    Some(cur)
}

fn scalar(v: &Value) -> Option<String> {
    match v {
        Value::String(s) => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

/// A double-quoted curl config value.
fn quote(s: &str) -> String {
    let escaped: String = s
        .chars()
        .filter(|c| *c != '\n' && *c != '\r')
        .flat_map(|c| match c {
            '\\' => vec!['\\', '\\'],
            '"' => vec!['\\', '"'],
            c => vec![c],
        })
        .collect();
    format!("\"{escaped}\"")
}

fn shorten(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    s.chars().take(max).collect::<String>() + "…"
}

/// A macOS notification, so you know the link is ready (or why it isn't).
fn notify(title: &str, text: &str) {
    let esc = |s: &str| s.replace('\\', "\\\\").replace('"', "\\\"");
    let script = format!("display notification \"{}\" with title \"snipno\" subtitle \"{}\"", esc(text), esc(title));
    let _ = Command::new("/usr/bin/osascript").args(["-e", &script]).status();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn imgstore() -> Upload {
        Upload {
            hotkey: String::new(),
            url: "https://img.example.com/api/images".into(),
            token: "imgs_secret".into(),
            labels: "snipno".into(),
            fields: "isPublic=true".into(),
            link: "https://img.example.com/i/{json:images[0].slug}".into(),
            name: "shot".into(),
        }
    }

    fn at(s: &str) -> chrono::DateTime<chrono::Local> {
        use chrono::TimeZone;
        let naive = chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S").unwrap();
        chrono::Local.from_local_datetime(&naive).unwrap()
    }

    #[test]
    fn file_name_fills_in_the_date_and_time() {
        let now = at("2026-11-12 14:05:09");
        assert_eq!(file_name("screenshot-{date}-{time}", now), "screenshot-2026-11-12-14-05-09.png");
        assert_eq!(file_name("{timestamp}", now), format!("{}.png", now.timestamp()));
        assert_eq!(file_name("screenshot", now), "screenshot.png");
        assert_eq!(file_name("  ", now), "2026-11-12-14-05-09.png");
        assert_eq!(file_name("bug.png", now), "bug.png");
        assert_eq!(file_name("a/b: \"c\";d", now), "a-b- -c--d.png");
    }

    #[test]
    fn curl_config_carries_url_token_fields_labels_and_file() {
        let conf = curl_config(&imgstore(), Path::new("/tmp/snipno-1.png"), " ui , bug ,", "shot.png");
        assert_eq!(
            conf,
            "url = \"https://img.example.com/api/images\"\n\
             header = \"Authorization: Bearer imgs_secret\"\n\
             form-string = \"isPublic=true\"\n\
             form-string = \"tags=ui,bug\"\n\
             form = \"file=@\\\"/tmp/snipno-1.png\\\";filename=\\\"shot.png\\\";type=image/png\"\n"
        );
    }

    #[test]
    fn curl_config_skips_empty_token_and_labels_and_escapes_quotes() {
        let cfg = Upload { url: "https://x.test/up".into(), fields: "note=say \"hi\"\\".into(), ..Upload::default() };
        let conf = curl_config(&cfg, Path::new("/tmp/a.png"), "  ", "a.png");
        assert!(!conf.contains("header"));
        assert!(!conf.contains("tags="));
        assert!(conf.contains(r#"form-string = "note=say \"hi\"\\""#));
    }

    #[test]
    fn parses_form_fields() {
        assert_eq!(
            form_fields("isPublic=true& expiresAt = 2030-01-01 &&flag"),
            vec![
                ("isPublic".into(), "true".into()),
                ("expiresAt".into(), "2030-01-01".into()),
                ("flag".into(), String::new())
            ]
        );
        assert!(form_fields("").is_empty());
    }

    #[test]
    fn fills_the_link_template_from_imgstore_json() {
        let body = r#"{"images":[{"slug":"smTVDpaYFRgq","width":1024,"isPublic":true}]}"#;
        let link = link_from_response("https://img.example.com/i/{json:images[0].slug}.png", body);
        assert_eq!(link.unwrap(), "https://img.example.com/i/smTVDpaYFRgq.png");
        let err = link_from_response("https://x/{json:images[1].slug}", body).unwrap_err();
        assert!(err.contains("images[1].slug"), "{err}");
    }

    #[test]
    fn without_a_template_finds_common_link_fields_or_a_plain_url() {
        assert_eq!(link_from_response("", r#"{"data":{"link":"https://i.test/a.png"}}"#).unwrap(), "https://i.test/a.png");
        assert_eq!(link_from_response("", r#"{"url":"https://u.test/b"}"#).unwrap(), "https://u.test/b");
        assert_eq!(link_from_response("", "https://plain.test/c.png\n").unwrap(), "https://plain.test/c.png");
        assert!(link_from_response("", r#"{"images":[{"slug":"x"}]}"#).is_err());
    }

    #[test]
    fn keeps_unknown_braces_and_supports_raw_response() {
        assert_eq!(link_from_response("{response}?v={x}", "https://r.test/z").unwrap(), "https://r.test/z?v={x}");
    }
}
