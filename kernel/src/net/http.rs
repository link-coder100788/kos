//! Just enough HTTP/1.0: any method, with extra headers and a body.

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

#[derive(Clone, Debug)]
pub struct Url {
    /// https:// rather than http://.
    pub tls: bool,
    pub host: String,
    pub port: u16,
    /// Always starts with '/'.
    pub path: String,
}

impl Url {
    /// Parses `[http[s]://]host[:port][/path][?query]`. A `#fragment` is
    /// dropped: it names a spot in the page, and is never sent.
    pub fn parse(text: &str) -> Result<Url, String> {
        let text = text.trim();
        let text = text.split_once('#').map_or(text, |(before, _)| before);
        let (tls, rest) = match text.strip_prefix("https://") {
            Some(rest) => (true, rest),
            None => (false, text.strip_prefix("http://").unwrap_or(text)),
        };
        if rest.contains("://") {
            return Err("only http:// and https:// URLs are supported".to_string());
        }
        let (authority, path) = match rest.find(['/', '?']) {
            Some(i) if rest.as_bytes()[i] == b'?' => (&rest[..i], &format!("/{}", &rest[i..])[..]),
            Some(i) => (&rest[..i], &rest[i..]),
            None => (rest, "/"),
        };
        let (host, port) = match authority.rsplit_once(':') {
            Some((h, p)) => (h, p.parse::<u16>().map_err(|_| format!("bad port '{}'", p))?),
            None => (authority, if tls { 443 } else { 80 }),
        };
        if host.is_empty() {
            return Err("missing host name".to_string());
        }
        Ok(Url { tls, host: host.to_string(), port, path: path.to_string() })
    }

    /// Resolves a link (or a redirect's Location) against this URL, the
    /// way a browser does: `https://x/y` and `//x/y` name another site,
    /// `/y` another path here, `?q` another query here, and anything else
    /// is relative to this path's directory, with `.` and `..` worked out.
    pub fn join(&self, location: &str) -> Result<Url, String> {
        let location = location.trim();
        let location = location.split_once('#').map_or(location, |(before, _)| before);
        let scheme = if self.tls { "https:" } else { "http:" };
        let (dir, query) = match self.path.split_once('?') {
            Some((path, query)) => (path, Some(query)),
            None => (self.path.as_str(), None),
        };
        if let Some(other) = scheme_of(location) {
            return match other.to_ascii_lowercase().as_str() {
                "http" | "https" => Url::parse(&format!("{}{}", other.to_ascii_lowercase(), &location[other.len()..])),
                _ => Err(format!("can't open {}: links", other)),
            };
        }
        let path = if location.starts_with("//") {
            return Url::parse(&format!("{}{}", scheme, location));
        } else if location.is_empty() {
            match query {
                Some(q) => format!("{}?{}", dir, q),
                None => dir.to_string(),
            }
        } else if location.starts_with('?') {
            format!("{}{}", dir, location)
        } else if location.starts_with('/') {
            location.to_string()
        } else {
            let base = &dir[..dir.rfind('/').map_or(0, |i| i + 1)];
            format!("{}{}", base, location)
        };
        Ok(Url { path: normalize(&path), ..self.clone() })
    }

    pub fn to_string(&self) -> String {
        let (scheme, default_port) = if self.tls { ("https", 443) } else { ("http", 80) };
        if self.port == default_port {
            format!("{}://{}{}", scheme, self.host, self.path)
        } else {
            format!("{}://{}:{}{}", scheme, self.host, self.port, self.path)
        }
    }
}

/// The scheme of an absolute URL (`https` in `https://x`, `mailto` in
/// `mailto:a@b`), if it has one: letters, digits, `+-.`, then a colon.
fn scheme_of(s: &str) -> Option<&str> {
    let colon = s.find(':')?;
    let scheme = &s[..colon];
    let valid = scheme.chars().next().is_some_and(|c| c.is_ascii_alphabetic())
        && scheme.chars().all(|c| c.is_ascii_alphanumeric() || "+-.".contains(c));
    valid.then_some(scheme)
}

/// Removes `.` and `..` segments from an absolute path (keeping any query).
fn normalize(path: &str) -> String {
    let (path, query) = match path.split_once('?') {
        Some((p, q)) => (p, Some(q)),
        None => (path, None),
    };
    let mut out: Vec<&str> = Vec::new();
    let segments: Vec<&str> = path.split('/').skip(1).collect();
    for (i, segment) in segments.iter().enumerate() {
        let last = i + 1 == segments.len();
        match *segment {
            "." => {
                if last { out.push(""); }
            }
            ".." => {
                out.pop();
                if last { out.push(""); }
            }
            s => out.push(s),
        }
    }
    let mut result = format!("/{}", out.join("/"));
    if let Some(q) = query {
        result.push('?');
        result.push_str(q);
    }
    result
}

/// What to send besides the URL. `Default` is a plain GET.
#[derive(Clone, Debug)]
pub struct RequestOptions {
    pub method: String,
    /// Extra header lines ("Name: value"), one per line.
    pub headers: String,
    pub body: Vec<u8>,
}

impl Default for RequestOptions {
    fn default() -> Self {
        Self { method: "GET".to_string(), headers: String::new(), body: Vec::new() }
    }
}

impl RequestOptions {
    /// What a redirect turns this request into: 307 and 308 repeat it as
    /// it was, the others (301, 302, 303) become a plain GET, as browsers do.
    pub fn redirected(&self, status: u16) -> RequestOptions {
        if status == 307 || status == 308 {
            self.clone()
        } else {
            RequestOptions { headers: self.headers.clone(), ..RequestOptions::default() }
        }
    }
}

pub fn request_bytes(url: &Url, opts: &RequestOptions) -> Vec<u8> {
    let extra: Vec<&str> = opts.headers.lines().map(str::trim).filter(|l| l.contains(':')).collect();
    let has = |name: &str| extra.iter().any(|l| l.split(':').next().is_some_and(|n| n.trim().eq_ignore_ascii_case(name)));
    let mut head = format!("{} {} HTTP/1.0\r\nHost: {}\r\n", opts.method, url.path, url.host);
    if !has("user-agent") {
        head.push_str("User-Agent: KOS/0.1\r\n");
    }
    if !has("accept") {
        head.push_str("Accept: text/plain, */*\r\n");
    }
    for line in &extra {
        head.push_str(line);
        head.push_str("\r\n");
    }
    if !opts.body.is_empty() || !opts.method.eq_ignore_ascii_case("GET") {
        head.push_str(&format!("Content-Length: {}\r\n", opts.body.len()));
    }
    head.push_str("Connection: close\r\n\r\n");
    let mut bytes = head.into_bytes();
    bytes.extend_from_slice(&opts.body);
    bytes
}

pub struct HttpResponse {
    /// Where the response came from, after any redirects.
    pub url: String,
    pub status: u16,
    pub reason: String,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl HttpResponse {
    /// A header's value, by case-insensitive name.
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.iter().find(|(n, _)| n.eq_ignore_ascii_case(name)).map(|(_, v)| v.as_str())
    }
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w == needle)
}

pub fn parse_response(raw: &[u8]) -> Result<HttpResponse, String> {
    let header_end = find(raw, b"\r\n\r\n").ok_or("the server's response was cut off or isn't HTTP")?;
    let head = core::str::from_utf8(&raw[..header_end]).map_err(|_| "garbled response headers")?;
    let mut lines = head.split("\r\n");
    let status_line = lines.next().unwrap_or("");
    let mut parts = status_line.splitn(3, ' ');
    let version = parts.next().unwrap_or("");
    if !version.starts_with("HTTP/") {
        return Err(format!("not an HTTP response: {}", status_line));
    }
    let status = parts.next().and_then(|s| s.parse().ok()).ok_or("bad status line")?;
    let reason = parts.next().unwrap_or("").to_string();
    let headers: Vec<(String, String)> = lines
        .filter_map(|l| l.split_once(':'))
        .map(|(n, v)| (n.trim().to_string(), v.trim().to_string()))
        .collect();
    let mut response = HttpResponse { url: String::new(), status, reason, headers, body: raw[header_end + 4..].to_vec() };
    if response.header("transfer-encoding").is_some_and(|v| v.eq_ignore_ascii_case("chunked")) {
        response.body = dechunk(&response.body)?;
    }
    Ok(response)
}

/// Decodes a `Transfer-Encoding: chunked` body.
fn dechunk(mut data: &[u8]) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    loop {
        let line_end = find(data, b"\r\n").ok_or("cut-off chunked body")?;
        let size_text = core::str::from_utf8(&data[..line_end]).map_err(|_| "bad chunk size")?;
        let size_text = size_text.split(';').next().unwrap_or("").trim();
        let size = usize::from_str_radix(size_text, 16).map_err(|_| "bad chunk size")?;
        data = &data[line_end + 2..];
        if size == 0 {
            return Ok(out);
        }
        if data.len() < size {
            out.extend_from_slice(data);
            return Ok(out);
        }
        out.extend_from_slice(&data[..size]);
        data = data.get(size + 2..).unwrap_or(&[]);
    }
}
