//! Who the page refuses before anything is served: another Host, no token,
//! another origin, an oversized or dripping request, and the headers every
//! answer carries.

use super::*;

#[test]
fn a_foreign_host_is_refused_before_anything_is_served() {
    let dir = tempfile::tempdir().expect("tempdir");
    let page = Page::start(
        &dir.path().join("node"),
        dir.path(),
        Mode::Setup,
        &Seen::default(),
    );
    let port = page.port();
    for (host, why) in [
        (format!("evil.example:{port}"), "a rebinding name"),
        (
            format!("127.0.0.1:{}", port.wrapping_add(1)),
            "another port",
        ),
        ("127.0.0.1".to_owned(), "no port"),
        (
            format!("localhost.evil.example:{port}"),
            "a name that starts like localhost",
        ),
    ] {
        let reply = page.send("GET", "/", &[("Host", &host)], "");
        assert_eq!(
            reply.status, 403,
            "{why} ({host}) was served the page: a site that points its own name at 127.0.0.1 \
             would read this machine's setup as its own"
        );
    }
    assert_eq!(
        page.send("GET", "/", &[], "").status,
        403,
        "a request with no Host was served"
    );
    let ours = page.host();
    assert_eq!(
        page.send("GET", "/", &[("Host", &ours), ("Host", "evil.example")], "")
            .status,
        403,
        "two Host headers were served: a proxy and this server could each read a different one"
    );
    for host in [ours, format!("localhost:{port}")] {
        assert_eq!(
            page.send("GET", "/", &[("Host", &host)], "").status,
            200,
            "the page refused its own address {host}: the person would see an error, not setup"
        );
    }
}

#[test]
fn red_team_a_rebinding_host_with_the_right_token_is_refused() {
    let dir = tempfile::tempdir().expect("tempdir");
    let page = Page::start(
        &dir.path().join("node"),
        dir.path(),
        Mode::Setup,
        &Seen::default(),
    );
    let evil = format!("evil.example:{}", page.port());
    let token = page.ctx.token.clone();
    for (method, path) in [("GET", "/api/state"), ("POST", "/api/quit")] {
        let reply = page.send(
            method,
            path,
            &[("Host", &evil), ("X-Itsanas-Token", &token)],
            "",
        );
        assert_eq!(
            reply.status, 403,
            "{method} {path} under Host {evil} was answered although the token was right: a \
             token that leaked once (a screenshot, a shared terminal) would let any web page \
             that rebinds its name drive this machine's setup"
        );
    }
    assert_eq!(
        page.api("GET", "/api/state", "").status,
        200,
        "the refused quit stopped the server anyway"
    );
}

#[test]
fn the_api_refuses_a_missing_or_wrong_token() {
    let dir = tempfile::tempdir().expect("tempdir");
    let page = Page::start(
        &dir.path().join("node"),
        dir.path(),
        Mode::Setup,
        &Seen::default(),
    );
    let host = page.host();
    let right = page.ctx.token.clone();
    let mut wrong = right.clone().into_bytes();
    wrong[31] = if wrong[31] == b'0' { b'1' } else { b'0' };
    let wrong = String::from_utf8(wrong).expect("hex");
    for (headers, why) in [
        (vec![("Host", host.as_str())], "no token"),
        (
            vec![("Host", host.as_str()), ("X-Itsanas-Token", wrong.as_str())],
            "a token one digit off",
        ),
        (
            vec![
                ("Host", host.as_str()),
                ("X-Itsanas-Token", wrong.as_str()),
                ("X-Itsanas-Token", right.as_str()),
            ],
            "the right token beside a wrong one",
        ),
    ] {
        for (method, path) in [
            ("GET", "/api/state"),
            ("GET", "/api/plan"),
            ("POST", "/api/run"),
        ] {
            assert_eq!(
                page.send(method, path, &headers, "").status,
                403,
                "{method} {path} with {why} was answered: any local process or page could read \
                 this machine's state or start its setup"
            );
        }
    }
    assert_eq!(
        page.api("GET", "/api/state", "").status,
        200,
        "the right token was refused: the page could never work"
    );
}

#[test]
fn a_cross_origin_request_is_refused_and_no_cors_header_is_sent() {
    let dir = tempfile::tempdir().expect("tempdir");
    let page = Page::start(
        &dir.path().join("node"),
        dir.path(),
        Mode::Setup,
        &Seen::default(),
    );
    let host = page.host();
    let token = page.ctx.token.clone();
    let port = page.port();
    let other_port = format!("http://127.0.0.1:{}", port.wrapping_add(1));
    for (name, value, why) in [
        ("Origin", "http://evil.example", "another site"),
        ("Origin", "null", "a sandboxed frame or a file"),
        (
            "Origin",
            other_port.as_str(),
            "another port of this machine",
        ),
        (
            "Sec-Fetch-Site",
            "cross-site",
            "a fetch the browser marks cross-site",
        ),
    ] {
        let reply = page.send(
            "POST",
            "/api/quit",
            &[("Host", &host), ("X-Itsanas-Token", &token), (name, value)],
            "",
        );
        assert_eq!(
            reply.status, 403,
            "a POST from {why} ({name}: {value}) was obeyed: a page the person happens to have \
             open could change their settings or sign them out"
        );
    }
    let ours = format!("http://localhost:{port}");
    let reply = page.send(
        "GET",
        "/api/state",
        &[
            ("Host", &host),
            ("X-Itsanas-Token", &token),
            ("Origin", &ours),
        ],
        "",
    );
    assert_eq!(reply.status, 200, "the page's own origin was refused");
    assert!(
        !reply.head.to_ascii_lowercase().contains("access-control-"),
        "a CORS header was sent: it is the only way another origin could read an answer"
    );
}

#[test]
fn oversized_requests_are_refused_without_being_read_whole() {
    let dir = tempfile::tempdir().expect("tempdir");
    let page = Page::start(
        &dir.path().join("node"),
        dir.path(),
        Mode::Setup,
        &Seen::default(),
    );
    // A head one byte over the limit and never finished: only a server that
    // stops reading at the limit can answer at all.
    let mut head = format!("GET / HTTP/1.1\r\nHost: {}\r\nX-Pad: ", page.host()).into_bytes();
    head.resize(http::MAX_HEAD + 1, b'a');
    let started = Instant::now();
    let reply = exchange(page.port(), &head);
    assert_eq!(
        reply.status,
        431,
        "a head over {} bytes was not refused: a local process could make this server hold \
         as much memory as it sends",
        http::MAX_HEAD
    );
    assert!(
        started.elapsed() < super::READ_TIMEOUT,
        "the oversized head was answered only at the read timeout: the server waited for the \
         rest instead of refusing at the limit"
    );
    // A body announced at 10 MB and never sent: refused on the announcement.
    for length in [http::MAX_BODY + 1, 10_000_000] {
        let raw = format!(
            "POST /api/run HTTP/1.1\r\nHost: {}\r\nX-Itsanas-Token: {}\r\nContent-Length: \
             {length}\r\n\r\n",
            page.host(),
            page.ctx.token
        );
        let started = Instant::now();
        let reply = exchange(page.port(), raw.as_bytes());
        assert_eq!(
            reply.status, 413,
            "a body announced at {length} bytes was not refused on its announcement"
        );
        assert!(
            started.elapsed() < super::READ_TIMEOUT,
            "a body announced at {length} bytes was waited for before being refused"
        );
    }
}

#[test]
fn red_team_connections_dripping_bytes_cannot_starve_the_page() {
    use std::sync::atomic::{AtomicBool, Ordering};
    let dir = tempfile::tempdir().expect("tempdir");
    let page = Page::start(
        &dir.path().join("node"),
        dir.path(),
        Mode::Setup,
        &Seen::default(),
    );
    let stop = Arc::new(AtomicBool::new(false));
    let port = page.port();
    // Every slot taken by a client that sends one byte every 2 s: under the
    // 5 s per-read timeout, so only a total deadline can drop it.
    let drippers: Vec<_> = (0..super::MAX_CONNECTIONS)
        .map(|_| {
            let stop = Arc::clone(&stop);
            std::thread::spawn(move || {
                let Ok(mut stream) = TcpStream::connect(("127.0.0.1", port)) else {
                    return;
                };
                for byte in b"GET / HTTP/1.1\r\nX-Slow: aaaaaaaaaaaaaaaaaaaaaa"
                    .iter()
                    .cycle()
                {
                    if stop.load(Ordering::SeqCst) || stream.write_all(&[*byte]).is_err() {
                        return;
                    }
                    std::thread::sleep(Duration::from_secs(2));
                }
            })
        })
        .collect();
    std::thread::sleep(super::CONNECTION_DEADLINE + Duration::from_secs(2));
    let asked = Instant::now();
    let reply = page.api("GET", "/api/state", "");
    stop.store(true, Ordering::SeqCst);
    for dripper in drippers {
        let _ = dripper.join();
    }
    assert_eq!(
        reply.status, 200,
        "with every slot held by a client dripping bytes, the person's page got no answer: a \
         local process could stall setup until the idle exit"
    );
    assert!(
        asked.elapsed() < Duration::from_secs(5),
        "the page was answered only after {:?}",
        asked.elapsed()
    );
}

#[test]
fn every_response_says_no_store_and_forbids_framing() {
    let dir = tempfile::tempdir().expect("tempdir");
    let page = Page::start(
        &dir.path().join("node"),
        dir.path(),
        Mode::Setup,
        &Seen::default(),
    );
    let host = page.host();
    let mut oversized = format!("GET / HTTP/1.1\r\nHost: {host}\r\nX-Pad: ").into_bytes();
    oversized.resize(http::MAX_HEAD + 1, b'a');
    let replies = [
        page.send("GET", "/", &[("Host", &host)], ""),
        page.send("GET", "/app.js", &[("Host", &host)], ""),
        page.send("GET", "/app.css", &[("Host", &host)], ""),
        page.send("GET", "/nothing", &[("Host", &host)], ""),
        page.send("GET", "/", &[("Host", "evil.example")], ""),
        page.send("GET", "/api/state", &[("Host", &host)], ""),
        page.api("GET", "/api/state", ""),
        exchange(page.port(), &oversized),
    ];
    for reply in &replies {
        let says = |name: &str| reply.header(name).unwrap_or_default();
        assert_eq!(
            says("Cache-Control"),
            "no-store",
            "a {} answer may be cached: the state of a person's machine would stay on disk in \
             the browser's cache",
            reply.status
        );
        let csp = says("Content-Security-Policy");
        assert!(
            csp.contains("frame-ancestors 'none'") && csp.contains("script-src 'self'"),
            "a {} answer's CSP is {csp:?}: another page could frame this one, or a script it \
             did not serve could run in it",
            reply.status
        );
        assert_eq!(
            says("X-Frame-Options"),
            "DENY",
            "a {} answer may be framed",
            reply.status
        );
        assert_eq!(says("X-Content-Type-Options"), "nosniff");
        assert_eq!(says("Referrer-Policy"), "no-referrer");
        assert_eq!(says("Connection"), "close");
    }
}
