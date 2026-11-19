//! Viewer read authority can issue exactly the same one-time codes as the CLI.
use semon_push::{
    read_token,
    serve::{ServeOptions, Server},
};
use serde_json::{Value, json};
use std::{
    io::{Read, Write},
    net::{SocketAddr, TcpStream},
    sync::Arc,
    thread,
};

fn request(
    address: SocketAddr,
    method: &str,
    path: &str,
    headers: &str,
    body: &str,
) -> (u16, String, String) {
    let mut socket = TcpStream::connect(address).unwrap();
    socket
        .set_read_timeout(Some(std::time::Duration::from_secs(20)))
        .unwrap();
    write!(socket, "{method} {path} HTTP/1.1\r\nHost: {address}\r\nConnection: close\r\nContent-Length: {}\r\n{headers}\r\n{body}", body.len()).unwrap();
    let mut wire = String::new();
    socket.read_to_string(&mut wire).unwrap();
    let (head, body) = wire.split_once("\r\n\r\n").unwrap();
    (
        head.split_whitespace().nth(1).unwrap().parse().unwrap(),
        head.into(),
        body.into(),
    )
}

#[test]
fn viewer_pairing_requires_read_auth_and_same_origin_cookie_post() {
    let dir = std::env::temp_dir().join(format!("semon-viewer-pair-{}", std::process::id()));
    let mut options = ServeOptions::new(&dir);
    options.listen = "127.0.0.1:0".parse().unwrap();
    options.max_bytes = 4;
    let server = Server::bind(&options).unwrap();
    let address = server.local_addr().unwrap();
    let (token, _) = read_token::create(&dir, semon_push::simple_pair::now().unwrap()).unwrap();
    let sessions = semon_sessions::Options::default();
    let viewer = semon_sessions::Viewer::received(
        semon_sessions::ServeOptions {
            sessions: sessions.clone(),
            machines: vec![],
            received: Some(semon_sessions::ReceivedMachines::new(
                dir.clone(),
                &sessions,
            )),
            listen: address.to_string(),
        },
        address.port(),
        None,
        Arc::new(read_token::Access(dir.clone())),
    );
    // Only the last refusal per machine is kept; status ACKs cannot clear it.
    let machine = "machine-12345678abcdef01";
    viewer.record_push_storage(machine, 507, Some("receiver_storage_full"), true);
    viewer.record_push_storage(machine, 507, Some("receiver_storage_full"), true);
    viewer.record_push_storage(machine, 200, None, false);
    viewer.record_push_storage("recovered", 507, Some("machine_storage_full"), true);
    viewer.record_push_storage("recovered", 200, None, true);
    let server = server.with_viewer(viewer).unwrap();
    thread::spawn(move || server.run());
    assert_eq!(request(address, "POST", "/api/viewer/pair", "", "").0, 403);
    let bearer = format!("Authorization: Bearer {}\r\n", token.expose());
    let storage_path = "/api/viewer/receiver-storage";
    assert_eq!(request(address, "GET", storage_path, "", "").0, 403);
    assert_eq!(request(address, "POST", storage_path, &bearer, "").0, 405);
    let storage = request(address, "GET", storage_path, &bearer, "");
    assert_eq!(storage.0, 200);
    assert!(storage.1.to_lowercase().contains("cache-control: no-store"));
    let storage: Value = serde_json::from_str(&storage.2).unwrap();
    let refusals = storage["refusals"].as_array().unwrap();
    assert_eq!(refusals.len(), 1);
    assert_eq!(refusals[0]["machine_id"], machine);
    assert_eq!(refusals[0]["reason"], "receiver_storage_full");
    assert!(refusals[0]["time"].as_u64().unwrap() > 0);
    assert_eq!(refusals[0].as_object().unwrap().len(), 3, "metadata only");
    for path in [
        "/",
        "/viewer.js",
        "/viewer.css",
        "/shell.css",
        "/api/session-sources",
    ] {
        assert_eq!(request(address, "GET", path, &bearer, "").0, 200, "{path}");
    }
    assert_eq!(
        request(address, "GET", "/api/viewer/pair", &bearer, "").0,
        405
    );
    let login = request(address, "GET", &format!("/?t={}", token.expose()), "", "");
    assert_eq!(login.0, 303);
    let cookie = login
        .1
        .lines()
        .find_map(|line| {
            line.strip_prefix("Set-Cookie: ")
                .or_else(|| line.strip_prefix("set-cookie: "))
        })
        .unwrap()
        .split(';')
        .next()
        .unwrap();
    let cookie = format!("Cookie: {cookie}\r\n");
    assert_eq!(
        request(address, "POST", "/api/viewer/pair", &cookie, "").0,
        403
    );
    assert_eq!(
        request(
            address,
            "POST",
            "/api/viewer/pair",
            &format!("{cookie}Origin: https://other.example\r\n"),
            ""
        )
        .0,
        403
    );
    let issued = request(
        address,
        "POST",
        "/api/viewer/pair",
        &format!("{cookie}Origin: http://{address}\r\n"),
        "",
    );
    assert_eq!(issued.0, 200);
    assert!(issued.1.to_lowercase().contains("cache-control: no-store"));
    let issued: Value = serde_json::from_str(&issued.2).unwrap();
    assert_eq!(issued["code"].as_str().unwrap().len(), 11);
    assert!(issued["expires"].as_u64().unwrap() > semon_push::simple_pair::now().unwrap());
    let body = json!({"version":1,"code":issued["code"],"machine_id":"machine-12345678abcdef01","hostname":"synthetic-host"}).to_string();
    let paired = request(address, "POST", "/v1/pair", "", &body);
    assert_eq!(paired.0, 200);
    let paired: Value = serde_json::from_str(&paired.2).unwrap();
    assert_eq!(request(address, "POST", "/v1/pair", "", &body).0, 403);
    // Real push refusals reach the Viewer, and a successful write clears them.
    let push_auth = format!(
        "Authorization: Bearer {}\r\n",
        paired["token"].as_str().unwrap()
    );
    let append = |bytes: &[u8]| {
        serde_json::to_string(&semon_push::wire::Append {
            relocation: None,
            generation: None,
            root: "claude".into(),
            path: "projects/-synthetic/session.jsonl".into(),
            redactor_version: 1,
            offset: 0,
            head_sha256: semon_push::wire::head_sha256(bytes),
            bytes: semon_push::wire::base64_encode(bytes),
            replace: false,
        })
        .unwrap()
    };
    assert_eq!(
        request(
            address,
            "POST",
            "/v1/push/append",
            &push_auth,
            &append(b"12345")
        )
        .0,
        507
    );
    let storage: Value =
        serde_json::from_str(&request(address, "GET", storage_path, &bearer, "").2).unwrap();
    assert_eq!(storage["refusals"][0]["reason"], "machine_storage_full");
    assert_eq!(
        request(
            address,
            "POST",
            "/v1/push/append",
            &push_auth,
            &append(b"x\n")
        )
        .0,
        200
    );
    let storage: Value =
        serde_json::from_str(&request(address, "GET", storage_path, &bearer, "").2).unwrap();
    assert_eq!(storage["refusals"], json!([]));
    assert_eq!(request(address, "GET", storage_path, &push_auth, "").0, 403);
    // A push credential does not acquire Viewer pairing authority.
    assert_eq!(
        request(
            address,
            "POST",
            "/api/viewer/pair",
            &format!(
                "Authorization: Bearer {}\r\n",
                paired["token"].as_str().unwrap()
            ),
            ""
        )
        .0,
        403
    );
    let replacement = request(address, "POST", "/api/viewer/pair", &bearer, "");
    assert_eq!(replacement.0, 200);
    let _ = std::fs::remove_dir_all(dir);
}
