//! Login handoff to a phone: the `qr` task.
//!
//! Typing an email with a gamepad is the worst part of `gamestore login`, so the
//! login can be finished on a phone instead. A one-page server is bound to the LAN
//! address only, on a path nobody can guess, and its address is shown as a QR
//! code. The phone opens that page, follows the GOG authorization URL from it, and
//! pastes back the address GOG lands on; the code→token exchange still happens
//! here, so the client secret never leaves the machine.
//!
//! The hop from the phone is plain HTTP on the LAN. What crosses it is a code that
//! is short-lived and single-use, which is the trade
//! `ROADMAP/2026-09-17/09-nice-to-haves.md` accepts. Nothing here logs the code or
//! the request body.
//!
//! The server is `std` only and handles one request at a time: it serves a single
//! person for a few minutes, and a dependency on an HTTP framework would cost more
//! than it saves.

use std::collections::hash_map::RandomState;
use std::hash::{BuildHasher, Hasher};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{IpAddr, SocketAddr, TcpListener, TcpStream, UdpSocket};
use std::sync::mpsc::{Receiver, TryRecvError};
use std::time::{Duration, Instant};

use percent_encoding::percent_decode_str;

use crate::{Error, Result};

/// Port the handoff listens on unless the configuration says otherwise. It is
/// fixed rather than ephemeral so a firewall can be opened for it once.
pub const DEFAULT_PORT: u16 = 8731;

/// How long the QR stays valid.
pub const DEFAULT_TTL: Duration = Duration::from_secs(10 * 60);

/// Largest request accepted, headers and body together. A pasted GOG address is a
/// few hundred bytes.
const MAX_REQUEST: usize = 16 * 1024;

/// How often the accept loop looks at the terminal and the clock.
const POLL: Duration = Duration::from_millis(100);

/// The address this machine uses to reach the LAN, found by asking the routing
/// table which source address a packet to a public address would leave from. No
/// packet is sent: connecting a UDP socket only picks a route.
pub fn lan_address() -> Option<IpAddr> {
    let socket = UdpSocket::bind(("0.0.0.0", 0)).ok()?;
    socket.connect(("192.0.2.1", 9)).ok()?;
    let ip = socket.local_addr().ok()?.ip();
    (!ip.is_unspecified() && !ip.is_loopback()).then_some(ip)
}

/// How a handoff ended.
#[derive(Debug, PartialEq, Eq)]
pub enum Outcome {
    /// The phone submitted an address and `on_paste` accepted it.
    Phone,
    /// Something was pasted in the terminal instead; the caller logs in with it.
    Terminal(String),
}

/// A bound, not yet serving, handoff page.
pub struct Handoff {
    listener: TcpListener,
    secret: String,
    url: String,
    ttl: Duration,
}

impl Handoff {
    /// Bind to `ip:port`. `ip` should be the LAN address, never `0.0.0.0`: the page
    /// has no business being reachable from anywhere else.
    pub fn bind(ip: IpAddr, port: u16, ttl: Duration) -> Result<Self> {
        let listener = TcpListener::bind(SocketAddr::new(ip, port))
            .map_err(|error| Error::io(format!("listening on {ip}:{port}"), error))?;
        listener
            .set_nonblocking(true)
            .map_err(|error| Error::io("configuring the handoff listener", error))?;
        let bound = listener
            .local_addr()
            .map_err(|error| Error::io("reading the handoff address", error))?;

        let secret = random_secret();
        let url = format!("http://{bound}/{secret}");
        Ok(Self {
            listener,
            secret,
            url,
            ttl,
        })
    }

    /// The address the QR encodes.
    pub fn url(&self) -> &str {
        &self.url
    }

    /// Serve the page until the phone completes a login, the terminal supplies an
    /// address, or the time runs out.
    ///
    /// `on_paste` gets what the phone submitted and does the exchange; an error
    /// from it is shown on the phone, which can try again. The path stops working
    /// as soon as one submission succeeds. Lines from `terminal` that are blank are
    /// ignored, and a closed `terminal` just leaves the phone as the only way in.
    pub fn serve<F>(
        self,
        authorization_url: &str,
        terminal: &Receiver<String>,
        mut on_paste: F,
    ) -> Result<Outcome>
    where
        F: FnMut(&str) -> Result<()>,
    {
        let deadline = Instant::now() + self.ttl;
        let mut terminal_open = true;

        loop {
            if terminal_open {
                match terminal.try_recv() {
                    Ok(line) if !line.trim().is_empty() => return Ok(Outcome::Terminal(line)),
                    Ok(_) => {}
                    Err(TryRecvError::Empty) => {}
                    Err(TryRecvError::Disconnected) => terminal_open = false,
                }
            }

            if Instant::now() >= deadline {
                return Err(Error::Auth(format!(
                    "nothing arrived within {} minutes; run `gamestore login` again for a new code",
                    self.ttl.as_secs() / 60
                )));
            }

            let stream = match self.listener.accept() {
                Ok((stream, _)) => stream,
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(POLL);
                    continue;
                }
                Err(error) => return Err(Error::io("accepting a handoff connection", error)),
            };

            // A client that misbehaves only loses its own request.
            if let Ok(true) = self.handle(stream, authorization_url, &mut on_paste) {
                return Ok(Outcome::Phone);
            }
        }
    }

    /// Answer one request. `Ok(true)` means a login went through.
    fn handle<F>(
        &self,
        mut stream: TcpStream,
        authorization_url: &str,
        on_paste: &mut F,
    ) -> std::io::Result<bool>
    where
        F: FnMut(&str) -> Result<()>,
    {
        stream.set_nonblocking(false)?;
        stream.set_read_timeout(Some(Duration::from_secs(10)))?;
        stream.set_write_timeout(Some(Duration::from_secs(10)))?;

        let request = match read_request(&mut stream) {
            Ok(request) => request,
            Err(_) => {
                return respond(
                    &mut stream,
                    "400 Bad Request",
                    &message_page("Bad request."),
                )
                .map(|_| false);
            }
        };

        // Anything but the exact secret path gets the same answer as a path that
        // does not exist, so the port reveals nothing to a scan.
        let path = request.target.split('?').next().unwrap_or_default();
        if path.strip_prefix('/') != Some(self.secret.as_str()) {
            respond(&mut stream, "404 Not Found", &message_page("Not found."))?;
            return Ok(false);
        }

        match request.method.as_str() {
            "GET" => {
                respond(&mut stream, "200 OK", &login_page(authorization_url, None))?;
                Ok(false)
            }
            "POST" => {
                let pasted = form_value(&request.body, "address").unwrap_or_default();
                match on_paste(&pasted) {
                    Ok(()) => {
                        respond(
                            &mut stream,
                            "200 OK",
                            &message_page("Logged in. You can close this page."),
                        )?;
                        Ok(true)
                    }
                    Err(error) => {
                        let page = login_page(authorization_url, Some(&error.to_string()));
                        respond(&mut stream, "200 OK", &page)?;
                        Ok(false)
                    }
                }
            }
            _ => {
                respond(
                    &mut stream,
                    "405 Method Not Allowed",
                    &message_page("Not allowed."),
                )?;
                Ok(false)
            }
        }
    }
}

/// 128 bits for the path, hex encoded. `RandomState` is seeded from the operating
/// system's random source, and two of them are two independent seeds; that is
/// plenty for a path that lives ten minutes on a LAN, without a dependency for it.
fn random_secret() -> String {
    let mut secret = String::with_capacity(32);
    for _ in 0..2 {
        let mut hasher = RandomState::new().build_hasher();
        hasher.write_u128(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|elapsed| elapsed.as_nanos())
                .unwrap_or_default(),
        );
        secret.push_str(&format!("{:016x}", hasher.finish()));
    }
    secret
}

#[derive(Debug)]
struct Request {
    method: String,
    target: String,
    body: String,
}

fn read_request(stream: &mut TcpStream) -> std::io::Result<Request> {
    let invalid = || std::io::Error::from(std::io::ErrorKind::InvalidData);
    let mut reader = BufReader::new(stream.take(MAX_REQUEST as u64));

    let mut line = String::new();
    reader.read_line(&mut line)?;
    let mut parts = line.split_whitespace();
    let method = parts.next().ok_or_else(invalid)?.to_string();
    let target = parts.next().ok_or_else(invalid)?.to_string();

    let mut content_length = 0usize;
    loop {
        line.clear();
        if reader.read_line(&mut line)? == 0 {
            return Err(invalid());
        }
        let header = line.trim_end();
        if header.is_empty() {
            break;
        }
        if let Some((name, value)) = header.split_once(':')
            && name.trim().eq_ignore_ascii_case("content-length")
        {
            content_length = value.trim().parse().map_err(|_| invalid())?;
        }
    }
    if content_length > MAX_REQUEST {
        return Err(invalid());
    }

    let mut body = vec![0; content_length];
    reader.read_exact(&mut body)?;
    let body = String::from_utf8(body).map_err(|_| invalid())?;

    Ok(Request {
        method,
        target,
        body,
    })
}

/// One field of an `application/x-www-form-urlencoded` body.
fn form_value(body: &str, key: &str) -> Option<String> {
    body.split('&').find_map(|pair| {
        let (name, value) = pair.split_once('=')?;
        (name == key).then(|| {
            percent_decode_str(&value.replace('+', " "))
                .decode_utf8_lossy()
                .into_owned()
        })
    })
}

fn respond(stream: &mut TcpStream, status: &str, html: &str) -> std::io::Result<()> {
    write!(
        stream,
        "HTTP/1.1 {status}\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\n\
         Cache-Control: no-store\r\nReferrer-Policy: no-referrer\r\nConnection: close\r\n\r\n{html}",
        html.len()
    )?;
    stream.flush()
}

fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

const STYLE: &str = "body{font-family:system-ui,sans-serif;max-width:32rem;margin:0 auto;padding:1.5rem 1rem;\
line-height:1.5;background:#1b1b1f;color:#eee}a.button,button{display:block;width:100%;box-sizing:border-box;\
padding:.9rem;border:0;border-radius:.5rem;background:#8a2be2;color:#fff;font-size:1.05rem;text-align:center;\
text-decoration:none}textarea{width:100%;box-sizing:border-box;min-height:6rem;font-size:1rem;margin:.5rem 0}\
.error{background:#5a1d1d;padding:.75rem;border-radius:.5rem}";

fn login_page(authorization_url: &str, error: Option<&str>) -> String {
    let error = error
        .map(|error| format!("<p class=\"error\">{}</p>", escape(error)))
        .unwrap_or_default();
    format!(
        "<!doctype html><html><head><meta charset=\"utf-8\">\
         <meta name=\"viewport\" content=\"width=device-width,initial-scale=1\">\
         <title>gamestore login</title><style>{STYLE}</style></head><body>\
         <h1>gamestore login</h1>{error}\
         <p><b>1.</b> Log in to GOG. It opens in a new tab; keep this one.</p>\
         <p><a class=\"button\" href=\"{}\" target=\"_blank\" rel=\"noopener noreferrer\">Log in to GOG</a></p>\
         <p><b>2.</b> GOG ends on a mostly blank page. Copy its whole address \
         (it contains <code>code=</code>) and paste it here.</p>\
         <form method=\"post\"><textarea name=\"address\" required \
         placeholder=\"https://embed.gog.com/on_login_success?origin=client&amp;code=...\"></textarea>\
         <button type=\"submit\">Finish login</button></form></body></html>",
        escape(authorization_url)
    )
}

fn message_page(message: &str) -> String {
    format!(
        "<!doctype html><html><head><meta charset=\"utf-8\">\
         <meta name=\"viewport\" content=\"width=device-width,initial-scale=1\">\
         <title>gamestore login</title><style>{STYLE}</style></head><body><p>{}</p></body></html>",
        escape(message)
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::Ipv4Addr;
    use std::sync::mpsc;

    const LOCAL: IpAddr = IpAddr::V4(Ipv4Addr::LOCALHOST);
    const AUTH_URL: &str = "https://auth.gog.com/auth?client_id=1&response_type=code";

    fn send(url: &str, method: &str, path: &str, body: &str) -> String {
        let address = url.trim_start_matches("http://").split('/').next().unwrap();
        let mut stream = TcpStream::connect(address).unwrap();
        write!(
            stream,
            "{method} {path} HTTP/1.1\r\nHost: {address}\r\n\
             Content-Type: application/x-www-form-urlencoded\r\nContent-Length: {}\r\n\r\n{body}",
            body.len()
        )
        .unwrap();
        let mut response = String::new();
        stream.read_to_string(&mut response).unwrap();
        response
    }

    fn secret_path(url: &str) -> String {
        format!("/{}", url.rsplit('/').next().unwrap())
    }

    #[test]
    fn the_secret_is_128_bits_of_hex_and_differs_each_time() {
        let first = random_secret();
        assert_eq!(first.len(), 32);
        assert!(first.chars().all(|c| c.is_ascii_hexdigit()));
        assert_ne!(first, random_secret());
    }

    #[test]
    fn form_values_are_decoded() {
        let body = "x=1&address=https%3A%2F%2Fembed.gog.com%2F%3Fa%3Db%26code%3Dabc+def";
        assert_eq!(
            form_value(body, "address").as_deref(),
            Some("https://embed.gog.com/?a=b&code=abc def")
        );
        assert_eq!(form_value(body, "missing"), None);
    }

    #[test]
    fn the_page_escapes_the_authorization_url() {
        let page = login_page("https://x/?a=1&b=\"2\"", Some("<bad>"));
        assert!(
            page.contains("https://x/?a=1&amp;b=&quot;2&quot;"),
            "{page}"
        );
        assert!(page.contains("&lt;bad&gt;"), "{page}");
    }

    #[test]
    fn a_phone_login_goes_get_then_post_and_ends_the_handoff() {
        let handoff = Handoff::bind(LOCAL, 0, Duration::from_secs(30)).unwrap();
        let url = handoff.url().to_string();
        let (_keep, terminal) = mpsc::channel::<String>();

        let client = std::thread::spawn(move || {
            let path = secret_path(&url);
            let wrong = send(&url, "GET", "/not-the-secret", "");
            let page = send(&url, "GET", &path, "");
            let rejected = send(&url, "POST", &path, "address=nope");
            let accepted = send(
                &url,
                "POST",
                &path,
                "address=https%3A%2F%2Fx%2F%3Fcode%3Dgood",
            );
            (wrong, page, rejected, accepted)
        });

        let mut seen = Vec::new();
        let outcome = handoff
            .serve(AUTH_URL, &terminal, |pasted| {
                seen.push(pasted.to_string());
                if pasted.contains("code=good") {
                    Ok(())
                } else {
                    Err(Error::Auth("that was not it".to_string()))
                }
            })
            .unwrap();

        let (wrong, page, rejected, accepted) = client.join().unwrap();
        assert_eq!(outcome, Outcome::Phone);
        assert!(wrong.starts_with("HTTP/1.1 404"), "{wrong}");
        assert!(
            page.contains("auth.gog.com/auth?client_id=1&amp;response_type=code"),
            "{page}"
        );
        assert!(rejected.contains("that was not it"), "{rejected}");
        assert!(accepted.contains("Logged in"), "{accepted}");
        assert_eq!(seen, ["nope", "https://x/?code=good"]);
    }

    #[test]
    fn a_line_from_the_terminal_wins_and_blank_lines_are_ignored() {
        let handoff = Handoff::bind(LOCAL, 0, Duration::from_secs(30)).unwrap();
        let (sender, terminal) = mpsc::channel();
        sender.send("\n".to_string()).unwrap();
        sender.send("the-code\n".to_string()).unwrap();

        let outcome = handoff
            .serve(AUTH_URL, &terminal, |_| panic!("the phone was not used"))
            .unwrap();

        assert_eq!(outcome, Outcome::Terminal("the-code\n".to_string()));
    }

    #[test]
    fn it_gives_up_when_the_time_runs_out() {
        let handoff = Handoff::bind(LOCAL, 0, Duration::from_millis(200)).unwrap();
        let (sender, terminal) = mpsc::channel::<String>();
        drop(sender);

        let error = handoff.serve(AUTH_URL, &terminal, |_| Ok(())).unwrap_err();

        assert!(error.to_string().contains("gamestore login"), "{error}");
    }
}
