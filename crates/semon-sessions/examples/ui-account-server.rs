//! A loopback fixture that exercises the embedding API in the browser suite.

use std::{env, net::SocketAddr, path::PathBuf, time::Duration};

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

fn answer(
    request: Request,
    status: u16,
    content_type: &str,
    body: Vec<u8>,
    etag: Option<&str>,
    cookie: bool,
) {
    let mut response = Response::from_data(body).with_status_code(StatusCode(status));
    response.add_header(header("Content-Type", content_type));
    if let Some(etag) = etag {
        response.add_header(header("ETag", etag));
    }
    for (name, value) in SECURITY_HEADERS {
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

fn handle(core: &mut ViewerCore, request: Request, address: SocketAddr) {
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
    let query_token = query_value(query, "t") == Some(TOKEN);
    let cookie_token = request_header(&request, "Cookie").is_some_and(|cookies| {
        cookies
            .split(';')
            .any(|part| part.trim().strip_prefix("semon_session=") == Some(TOKEN))
    });
    let is_get = request.method() == &Method::Get;
    let is_post = request.method() == &Method::Post;
    let post_action = matches!(
        path,
        "/workspaces/research" | "/workspaces/writing" | "/account/sign-out"
    );
    let expected_origin = format!("http://{expected_host}");
    let valid_origin = request_header(&request, "Origin") == Some(expected_origin.as_str());
    if !valid_host
        || !(query_token || cookie_token)
        || (!is_get && !(is_post && post_action && valid_origin))
    {
        answer(
            request,
            403,
            "text/plain; charset=utf-8",
            b"Forbidden".to_vec(),
            None,
            false,
        );
        return;
    }
    if is_post {
        redirect_to_root(request);
        return;
    }
    let if_none_match = request_header(&request, "If-None-Match").map(str::to_owned);
    let reply = core.respond("GET", path, query, if_none_match.as_deref());
    answer(
        request,
        reply.status,
        reply.content_type,
        reply.body,
        reply.etag.as_deref(),
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
        claude_home: PathBuf::from(&args[2]),
        codex_home: PathBuf::from(&args[3]),
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
    println!("http://{address}/?t={TOKEN}");
    for request in server.incoming_requests() {
        handle(&mut core, request, address);
    }
}
