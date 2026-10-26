//! A loopback fixture that exercises the embedding API in the browser suite.

use std::{env, io::Read, net::SocketAddr, path::PathBuf, time::Duration};

use semon_sessions::{
    AccountLink, AccountMenu, AccountWorkspace, LinkMethod, Options, SECURITY_HEADERS, ViewerCore,
};
use tiny_http::{Header, Method, Request, Response, Server, StatusCode};

const TOKEN: &str = "0123456789abcdef0123456789abcdef";

fn header(name: &str, value: &str) -> Header {
    Header::from_bytes(name, value).expect("static response header")
}

fn query_value<'a>(query: &'a str, key: &str) -> Option<&'a str> {
    query.split('&').find_map(|part| {
        let (name, value) = part.split_once('=')?;
        (name == key).then_some(value)
    })
}

fn request_header<'a>(request: &'a Request, name: &'static str) -> Option<&'a str> {
    request
        .headers()
        .iter()
        .find(|header| header.field.equiv(name))
        .map(|header| header.value.as_str())
}

fn same_origin_request(request: &Request, origin: &str) -> bool {
    if let Some(request_origin) = request_header(request, "Origin") {
        if request_origin == origin || request_origin.strip_suffix('/') == Some(origin) {
            return true;
        }
        if request_origin != "null" {
            return false;
        }
        return request_header(request, "Sec-Fetch-Site") == Some("same-origin");
    }
    if let Some(fetch_site) = request_header(request, "Sec-Fetch-Site") {
        return fetch_site == "same-origin";
    }
    request_header(request, "Referer").is_some_and(|referer| {
        referer == origin
            || referer
                .strip_prefix(origin)
                .is_some_and(|suffix| suffix.starts_with('/'))
    })
}

/// Sends a reply with `headers`: a core reply's own (`ViewerReply::headers`),
/// or [`SECURITY_HEADERS`] for this server's own answers.
fn answer(
    request: Request,
    status: u16,
    content_type: &str,
    body: Vec<u8>,
    etag: Option<&str>,
    headers: &[(&str, &str)],
    cookie: bool,
) {
    let mut response = Response::from_data(body).with_status_code(StatusCode(status));
    response.add_header(header("Content-Type", content_type));
    if let Some(etag) = etag {
        response.add_header(header("ETag", etag));
    }
    for (name, value) in headers {
        response.add_header(header(name, value));
    }
    if cookie {
        response.add_header(header(
            "Set-Cookie",
            &format!("semon_session={TOKEN}; HttpOnly; SameSite=Strict; Path=/"),
        ));
    }
    let _ = request.respond(response);
}

fn redirect_to_root(request: Request) {
    let mut response = Response::empty(StatusCode(303));
    response.add_header(header("Location", "/"));
    for (name, value) in SECURITY_HEADERS {
        response.add_header(header(name, value));
    }
    let _ = request.respond(response);
}

fn handle(core: &mut ViewerCore, mut request: Request, address: SocketAddr) {
    let url = request.url().to_owned();
    let (path, query) = url.split_once('?').unwrap_or((&url, ""));
    let expected_host = address.to_string();
    let valid_host = request
        .headers()
        .iter()
        .filter(|header| header.field.equiv("Host"))
        .count()
        == 1
        && request_header(&request, "Host") == Some(expected_host.as_str());
    let credentials = query
        .split('&')
        .filter(|part| {
            let name = part.split_once('=').map_or(*part, |(name, _)| name);
            semon_sessions::session_route::decode(name).ok().as_deref() == Some("t")
        })
        .count();
    let query_token = credentials == 1 && query_value(query, "t") == Some(TOKEN);
    let cookie_token = request_header(&request, "Cookie").is_some_and(|cookies| {
        cookies
            .split(';')
            .any(|part| part.trim().strip_prefix("semon_session=") == Some(TOKEN))
    });
    let is_get = request.method() == &Method::Get;
    let is_post = request.method() == &Method::Post;
    let post_action = matches!(
        path,
        "/workspaces/research" | "/workspaces/writing" | "/account/sign-out" | "/api/viewer/home"
    );
    let expected_origin = format!("http://{expected_host}");
    let valid_origin = same_origin_request(&request, &expected_origin);
    if is_post {
        eprintln!(
            "POST {path}: host={valid_host} session={} action={post_action} same_origin={valid_origin} origin_matches={} origin_null={} fetch_site_same_origin={} referer_matches={}",
            query_token || cookie_token,
            request_header(&request, "Origin") == Some(expected_origin.as_str()),
            request_header(&request, "Origin") == Some("null"),
            request_header(&request, "Sec-Fetch-Site") == Some("same-origin"),
            request_header(&request, "Referer").is_some_and(|referer| {
                referer == expected_origin
                    || referer
                        .strip_prefix(&expected_origin)
                        .is_some_and(|suffix| suffix.starts_with('/'))
            }),
        );
    }
    if !valid_host
        || (credentials > 0 && !query_token)
        || !(query_token || cookie_token)
        || (!is_get && !(is_post && post_action && valid_origin))
    {
        if is_get && path == "/" {
            eprintln!(
                "GET / rejected: host={valid_host} session={}",
                query_token || cookie_token
            );
        }
        answer(
            request,
            403,
            "text/plain; charset=utf-8",
            b"Forbidden".to_vec(),
            None,
            &SECURITY_HEADERS,
            false,
        );
        return;
    }
    if is_post && path != "/api/viewer/home" {
        redirect_to_root(request);
        return;
    }
    // The authenticated bootstrap is transport-only, including on direct links.
    let routing_query = query
        .split('&')
        .filter(|part| !part.starts_with("t="))
        .collect::<Vec<_>>()
        .join("&");
    let query = routing_query.as_str();
    let home_query;
    let query = if is_post {
        let mut bytes = Vec::new();
        let read = request
            .as_reader()
            .take(512 * 1024 + 1)
            .read_to_end(&mut bytes);
        let value = serde_json::from_slice::<serde_json::Value>(&bytes);
        if read.is_err()
            || bytes.len() > 512 * 1024
            || value
                .as_ref()
                .ok()
                .and_then(|value| value["seen"].as_array())
                .is_none()
        {
            answer(
                request,
                400,
                "text/plain; charset=utf-8",
                b"Invalid Home read arguments".to_vec(),
                None,
                &SECURITY_HEADERS,
                false,
            );
            return;
        }
        let seen = value.unwrap()["seen"].to_string();
        home_query = format!(
            "{}{}{}",
            query,
            if query.is_empty() { "" } else { "&" },
            semon_sessions::native_html::query(&[("seen", &seen)])
        );
        home_query.as_str()
    } else {
        query
    };
    let if_none_match = request_header(&request, "If-None-Match").map(str::to_owned);
    let reply = core.respond("GET", path, query, if_none_match.as_deref());
    let headers = reply.headers();
    answer(
        request,
        reply.status,
        reply.content_type,
        reply.body,
        reply.etag.as_deref(),
        headers,
        query_token,
    );
}

fn main() {
    let args: Vec<_> = env::args_os().collect();
    if args.len() != 6 {
        eprintln!("usage: ui-account-server LISTEN CLAUDE_HOME CODEX_HOME PROC_ROOT CACHE");
        std::process::exit(2);
    }
    let listen = args[1].to_string_lossy();
    let server = Server::http(listen.as_ref()).expect("bind fixture listener");
    let address = server
        .server_addr()
        .to_ip()
        .expect("fixture listener uses TCP");
    assert!(
        address.ip().is_loopback(),
        "fixture listener must be loopback"
    );
    let options = Options {
        source: semon_sessions::storage::HostSource::load_or_create(&PathBuf::from(&args[2]).join("source.json")).unwrap(),
        claude_home: PathBuf::from(&args[2]),
        claude_json: PathBuf::from(&args[2])
            .parent()
            .unwrap_or_else(|| std::path::Path::new("."))
            .join(".claude.json"),
        codex_home: PathBuf::from(&args[3]),
        copilot_home: PathBuf::from(&args[3]),
        proc_root: PathBuf::from(&args[4]),
        cache: PathBuf::from(&args[5]),
        all: true,
        since: Duration::from_secs(24 * 60 * 60),
        session: None,
        facts: None,
        scan_window: false,
    };
    let mut core = ViewerCore::new(options);
    core.set_account(Some(
        AccountMenu::new(
            "<img src=x onerror=\"window.__accountXss=1\">",
            "reader@example.invalid",
            "R",
            None,
            vec![
                AccountWorkspace {
                    name: "Research".into(),
                    role: "Owner".into(),
                    current: true,
                    switch_href: "/workspaces/research".into(),
                },
                AccountWorkspace {
                    name: "Writing".into(),
                    role: "Member".into(),
                    current: false,
                    switch_href: "/workspaces/writing".into(),
                },
            ],
            vec![
                AccountLink {
                    label: "Profile".into(),
                    href: "/account/profile".into(),
                    method: LinkMethod::default(),
                    danger: false,
                },
                AccountLink {
                    label: "Machines".into(),
                    href: "/machines".into(),
                    method: LinkMethod::default(),
                    danger: false,
                },
                AccountLink {
                    label: "Sign out".into(),
                    href: "/account/sign-out".into(),
                    method: LinkMethod::Post,
                    danger: true,
                },
            ],
        )
        .expect("valid account menu fixture"),
    ));
    assert!(core.set_nav_override("machines", "/account/workspaces"));
    // The printed URL means this synthetic fixture is ready for direct focused
    // API assertions, including attachmentApi's immediate selected range. Home
    // can return 200 while native publication is still partial; that response
    // alone never promised a selected transcript was ready.
    let started = std::time::Instant::now();
    loop {
        let reply = core.respond("GET", "/api/viewer/home", "", None);
        let home: serde_json::Value =
            serde_json::from_slice(&reply.body).expect("fixture Home publication is JSON");
        if reply.status == 200
            && home["observations"].as_array().is_some_and(Vec::is_empty)
            && home["recent"]
                .as_array()
                .is_some_and(|rows| !rows.is_empty())
        {
            break;
        }
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "fixture native publication did not become ready (Home status {})",
            reply.status
        );
        std::thread::sleep(Duration::from_millis(25));
    }
    eprintln!("Fixture native publication ready");
    println!("http://{address}/?t={TOKEN}");
    for request in server.incoming_requests() {
        handle(&mut core, request, address);
    }
}
